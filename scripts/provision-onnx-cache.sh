#!/bin/sh
# Provision the ONNX model cache the port's tool embedder reads.
#
# Why this exists: the engine resolves the model at
# `$TMPDIR/.opencode-mem-onnx/<model>` with downloads disabled
# (`crates/mem-bridge/src/composition.rs`, `embedder_from_config`, `hub: None`),
# NOT at `{storagePath}/.cache` where the upstream plugin keeps it. And the
# `tokenizers` 1.0.0-rc.2 reader the port uses rejects Hugging Face's legacy
# `"version": "1.0"` tokenizer.json, so the cache needs the canonical v2.0 form.
# Without both halves every `search`/`add` answers JSON-RPC `-32603` while the
# rest of the store reads fine.
#
# usage: provision-onnx-cache.sh [--source DIR] [--cache-root DIR] [--model ID]
#
#   --source      model directory to copy (read-only); default
#                 $HOME/.opencode-mem/data/.cache/Xenova/nomic-embed-text-v1
#                 (the upstream plugin's cache in the live store)
#   --cache-root  the engine's cache root; default ${TMPDIR:-/tmp}/.opencode-mem-onnx
#   --model       model id as it appears in `embeddingModel`; default
#                 Xenova/nomic-embed-text-v1
#
# The destination is <cache-root>/<model>. Nothing under <source> is written;
# the tokenizer is copied first and only the copy is canonicalized. Re-running
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
CACHE_ROOT=${TMPDIR:-/tmp}/.opencode-mem-onnx
SOURCE=

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

[ -d "$SOURCE" ] || {
	printf 'source model dir not found: %s\n' "$SOURCE" >&2
	exit 1
}
[ -f "$SOURCE/tokenizer.json" ] || {
	printf 'source has no tokenizer.json: %s\n' "$SOURCE" >&2
	exit 1
}
[ -f "$SOURCE/onnx/model.onnx" ] || {
	printf 'source has no onnx/model.onnx: %s\n' "$SOURCE" >&2
	exit 1
}
[ "$(cd "$SOURCE" && pwd -P)" != "$(cd "$DEST" 2>/dev/null && pwd -P || printf '%s' "$DEST")" ] || {
	printf 'source and destination are the same directory: %s\n' "$DEST" >&2
	exit 1
}

mkdir -p "$DEST"
cp -a "$SOURCE/." "$DEST/"
# Upstream leaves partial downloads (*.tmp.<pid>.<rand>) beside the weights.
find "$DEST" -type f -name '*.tmp.*' -delete

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
	"$build_dir/target/release/provision-onnx-canon" "$DEST/tokenizer.json"
	version=$(tokenizer_version "$DEST/tokenizer.json")
	[ "$version" = "2.0" ] || {
		printf 'canonicalization did not produce version 2.0 (got %s)\n' "${version:-none}" >&2
		exit 1
	}
fi

printf 'cache_root=%s\n' "$CACHE_ROOT"
printf 'model=%s\n' "$MODEL"
printf 'model_dir=%s\n' "$DEST"
printf 'tokenizer_version=%s\n' "$version"
printf 'model_onnx_bytes=%s\n' "$(wc -c <"$DEST/onnx/model.onnx")"
printf 'model_onnx_sha256=%s\n' "$(sha256sum "$DEST/onnx/model.onnx" | cut -d' ' -f1)"
