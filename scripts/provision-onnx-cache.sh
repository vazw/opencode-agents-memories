#!/bin/sh
# Provision the ONNX model cache the port's tool embedder reads.
#
# Why this exists: the engine resolves the model at `{storagePath}/.cache/<model>`
# with downloads disabled (`crates/mem-bridge/src/composition.rs`,
# `local_cache_root` + `embedder_from_config`, `hub: None`) — the same tree the
# upstream plugin uses (`embedding.ts:232`). And the `tokenizers` 1.0.0-rc.2
# reader the port uses rejects Hugging Face's legacy `"version": "1.0"`
# tokenizer.json, so the cache needs the canonical v2.0 form. Without both
# halves every `search`/`add` answers JSON-RPC `-32603` while the rest of the
# store reads fine.
#
# usage: provision-onnx-cache.sh [--source DIR] [--cache-root DIR] [--model ID]
#                                [--weights FILE] [--variant DTYPE]
#
#   --source      model directory to provision from; default
#                 $HOME/.opencode-mem/data/.cache/Xenova/nomic-embed-text-v1
#                 (the plugin cache inside the live store)
#   --cache-root  the engine's cache root; default
#                 $HOME/.opencode-mem/data/.cache (i.e. <storagePath>/.cache)
#   --model       model id as it appears in `embeddingModel`; default
#                 Xenova/nomic-embed-text-v1
#   --weights     the cache-relative weights file to require and report;
#                 default onnx/model.onnx. A weights file with a sibling
#                 `<weights>_data` is refused: the engine's three-file loader
#                 cannot read external-data ONNX (bge-m3 fp32 is a 607,298 B
#                 stub plus a 2.27 GB data file).
#   --variant     shorthand for the dtype suffix the engine's
#                 ONNX_DTYPE_VARIANTS table uses: q8 -> _quantized, fp16 ->
#                 _fp16, int8 -> _int8, q4 -> _q4 (e.g. --variant q8 is
#                 onnx/model_quantized.onnx). An unknown variant is refused;
#                 so is a --weights that disagrees with --variant.
#
# The destination is <cache-root>/<model>. When --source and the destination
# are the same directory (the default invocation: the model is already in the
# store) nothing is copied — the cache is verified in place and only a legacy
# tokenizer is rewritten, through a temporary file in the same directory.
# Otherwise the source is copied and only the copy is canonicalized. Re-running
# is safe and produces the same bytes: the destination is refreshed from the
# source, so a legacy tokenizer arrives again and the helper is rebuilt
# (~35 s cold, measured).
#
# Canonicalizing needs the cargo registry cache that holds tokenizers
# 1.0.0-rc.2 + tk-{encode,convert,serialize} 0.1.0-rc.2 (already present on a
# machine that has built this workspace). The build is --offline on purpose: a
# provisioning step must not fetch code.
set -eu

MODEL=Xenova/nomic-embed-text-v1
CACHE_ROOT=$HOME/.opencode-mem/data/.cache
SOURCE=
WEIGHTS=onnx/model.onnx
WEIGHTS_SET=0
VARIANT=

usage() {
	sed -n 's/^# \{0,1\}//p' "$0" | sed -n '/^usage:/,/^Canonicalizing/p' | sed '$d'
}

while [ $# -gt 0 ]; do
	case $1 in
	--source)
		SOURCE=${2:?--source needs a directory}
		shift 2
		;;
	--cache-root)
		CACHE_ROOT=${2:?--cache-root needs a directory}
		shift 2
		;;
	--model)
		MODEL=${2:?--model needs an id}
		shift 2
		;;
	--weights)
		WEIGHTS=${2:?--weights needs a cache-relative file}
		WEIGHTS_SET=1
		shift 2
		;;
	--variant)
		VARIANT=${2:?--variant needs a dtype (q8, fp16, int8 or q4)}
		shift 2
		;;
	-h | --help)
		usage
		exit 0
		;;
	*)
		printf 'unknown argument: %s\n' "$1" >&2
		exit 2
		;;
	esac
done

[ -n "$SOURCE" ] || SOURCE="$HOME/.opencode-mem/data/.cache/$MODEL"
DEST=$CACHE_ROOT/$MODEL

# The suffix map is the engine's own ONNX_DTYPE_VARIANTS table
# (crates/mem-embed/src/local/model_cache.rs): a dtype the engine resolves must
# name the same file this script provisions, or search/add answers -32603.
if [ -n "$VARIANT" ]; then
	case $VARIANT in
	q8) suffix=_quantized ;;
	fp16) suffix=_fp16 ;;
	int8) suffix=_int8 ;;
	q4) suffix=_q4 ;;
	*)
		printf 'unknown --variant: %s (expected q8, fp16, int8 or q4)\n' "$VARIANT" >&2
		exit 2
		;;
	esac
	variant_weights=onnx/model$suffix.onnx
	if [ "$WEIGHTS_SET" -eq 1 ] && [ "$WEIGHTS" != "$variant_weights" ]; then
		printf -- '--weights %s disagrees with --variant %s (%s)\n' \
			"$WEIGHTS" "$VARIANT" "$variant_weights" >&2
		exit 2
	fi
	WEIGHTS=$variant_weights
fi

[ -d "$SOURCE" ] || {
	printf 'source model dir not found: %s\n' "$SOURCE" >&2
	exit 1
}
[ -f "$SOURCE/tokenizer.json" ] || {
	printf 'source has no tokenizer.json: %s\n' "$SOURCE" >&2
	exit 1
}
[ -f "$SOURCE/$WEIGHTS" ] || {
	printf 'source has no %s: %s\n' "$WEIGHTS" "$SOURCE" >&2
	exit 1
}
# External-data ONNX ships a small graph file plus a huge `<file>_data` blob;
# the engine reads exactly three files (weights, tokenizer, config) and has no
# external-data support, so provisioning such a pair yields an unloadable
# model. Refuse before anything is copied.
weights_data=${WEIGHTS}_data
[ ! -f "$SOURCE/$weights_data" ] || {
	printf 'source weights %s are a stub with a sibling %s: the engine cannot read external-data ONNX\n' \
		"$WEIGHTS" "$weights_data" >&2
	exit 1
}
IN_PLACE=0
if [ "$(cd "$SOURCE" && pwd -P)" = "$(cd "$DEST" 2>/dev/null && pwd -P || printf '%s' "$DEST")" ]; then
	# The default invocation: the model already lives in the store's cache.
	# Copying a directory onto itself is a no-op with a footgun (cp would
	# rewrite every file); verify in place instead.
	IN_PLACE=1
	printf 'source and destination are the same directory: verifying in place (%s)\n' "$DEST" >&2
else
	mkdir -p "$DEST"
	cp -a "$SOURCE/." "$DEST/"
	# Upstream leaves partial downloads (*.tmp.<pid>.<rand>) beside the weights.
	find "$DEST" -type f -name '*.tmp.*' -delete
fi

# The only string-valued "version" in the file is the top-level tokenizer one.
tokenizer_version() {
	sed -n 's/.*"version": *"\([0-9][0-9.]*\)".*/\1/p' "$1" | head -1
}

version=$(tokenizer_version "$DEST/tokenizer.json")
if [ "$version" != "2.0" ]; then
	printf 'canonicalizing tokenizer %s (version %s -> 2.0)\n' "$DEST/tokenizer.json" "${version:-unknown}"
	build_dir=$(mktemp -d "${TMPDIR:-/tmp}/provision-onnx-canon.XXXXXX")
	cleanup() { find "$build_dir" -depth -delete; }
	trap cleanup EXIT INT TERM
	mkdir -p "$build_dir/src"
	# tokenizers 1.0.0-rc.2 re-exports tk_encode::utils::ProgressFormat, which
	# tk-encode 0.1.0-rc.3 dropped; pin the rc.2 family the workspace locks.
	cat >"$build_dir/Cargo.toml" <<'TOML'
[package]
name = "provision-onnx-canon"
version = "0.0.0"
edition = "2021"

[workspace]

[dependencies]
tokenizers = { version = "=1.0.0-rc.2", default-features = false }
tk-encode = "=0.1.0-rc.2"
tk-convert = "=0.1.0-rc.2"
tk-serialize = "=0.1.0-rc.2"
TOML
	cat >"$build_dir/src/main.rs" <<'RS'
fn main() {
	let path = std::env::args().nth(1).expect("usage: provision-onnx-canon <tokenizer.json>");
	let canonical = tokenizers::canonicalize_file(&path).expect("canonicalize_file");
	std::fs::write(&path, canonical).expect("write");
	tokenizers::from_json_file(&path).expect("rc.2 reader accepts canonical file");
	println!("canonicalized {path}");
}
RS
	CARGO_TARGET_DIR="$build_dir/target" cargo build --release --offline --quiet \
		--manifest-path "$build_dir/Cargo.toml" >&2
	# In place, the live cache's tokenizer is replaced atomically: canonicalise
	# a sibling temp file, verify it, then mv it over the original. A crash
	# mid-write therefore leaves the old tokenizer, never a truncated one.
	if [ "$IN_PLACE" -eq 1 ]; then
		canon_target=$DEST/tokenizer.json.new.$$
		cp -a "$DEST/tokenizer.json" "$canon_target"
		trap 'find "$canon_target" -delete 2>/dev/null || true; cleanup' EXIT INT TERM
	else
		canon_target=$DEST/tokenizer.json
	fi
	"$build_dir/target/release/provision-onnx-canon" "$canon_target"
	version=$(tokenizer_version "$canon_target")
	[ "$version" = "2.0" ] || {
		printf 'canonicalization did not produce version 2.0 (got %s)\n' "${version:-none}" >&2
		exit 1
	}
	if [ "$IN_PLACE" -eq 1 ]; then
		mv -f "$canon_target" "$DEST/tokenizer.json" || {
			printf 'cannot replace %s with the canonical tokenizer\n' "$DEST/tokenizer.json" >&2
			exit 1
		}
	fi
	version=$(tokenizer_version "$DEST/tokenizer.json")
fi

printf 'cache_root=%s\n' "$CACHE_ROOT"
printf 'model=%s\n' "$MODEL"
printf 'model_dir=%s\n' "$DEST"
printf 'tokenizer_version=%s\n' "$version"
printf 'model_onnx_bytes=%s\n' "$(wc -c <"$DEST/$WEIGHTS")"
printf 'model_onnx_sha256=%s\n' "$(sha256sum "$DEST/$WEIGHTS" | cut -d' ' -f1)"


