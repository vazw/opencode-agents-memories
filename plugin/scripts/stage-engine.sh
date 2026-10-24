#!/usr/bin/env bash
# stage-engine.sh — build the engine binary and stage it into the npm package
# (W5.1 integration lane; M26/M30).
#
# - Builds the HOST triple by default (no cross-compile surprise); `--target`
#   is honored only when that toolchain exists (fails loudly otherwise).
# - Fails loudly when the binary is missing after the build.
# - chmod +x and smoke `--version`.
# - Output: plugin/engine/<platform>-<arch>/opencode-agents-memory PLUS the
#   packaged default candidate plugin/engine/<platform>-<arch>/web-ui/dist (the
#   built UI staged beside the binary) — GITIGNORED build artifacts
#   (plugin/.gitignore, M29); never committed.
# - Refuses (exit 5) when ../web-ui/dist/index.html is absent: an engine whose
#   packaged default cannot resolve must not be staged, and a missing bundle must
#   not spend a release build. Refuses (exit 7) when a bundle is already staged at
#   the destination — bundles are replaced, never merged (trunk hashes asset
#   names); the operator removes the stale one and re-runs. This script never
#   deletes anything.
set -euo pipefail

cd "$(dirname "$0")/.."   # plugin/

WEB_UI_SRC="../web-ui/dist"
if [ ! -f "$WEB_UI_SRC/index.html" ]; then
  echo "stage-engine: '$WEB_UI_SRC' has no index.html — build the web UI first (cd web-ui && trunk build); refusing to stage an engine whose packaged default cannot resolve (issue 36)" >&2
  exit 5
fi

declare -a CARGO_ARGS=()
TARGET=""
TARGET_SUBDIR=""
if [ "${1-}" = "--target" ]; then
  TARGET="${2-}"
  if [ -z "$TARGET" ]; then
    echo "stage-engine: --target requires an explicit triple" >&2
    exit 2
  fi
  if ! rustup target list --installed 2>/dev/null | grep -qx "$TARGET"; then
    echo "stage-engine: toolchain target '$TARGET' is not installed (M26: --target only when the toolchain exists)" >&2
    exit 2
  fi
  CARGO_ARGS=(--target "$TARGET")
  TARGET_SUBDIR="$TARGET/"
else
  TARGET="$(rustc -vV | sed -n 's/^host: //p')"
fi

cargo build --release "${CARGO_ARGS[@]+"${CARGO_ARGS[@]}"}" -p opencode-agents-memory

BUILD_DIR="${CARGO_TARGET_DIR:-target}"
BIN="$BUILD_DIR/$TARGET_SUBDIR/release/opencode-agents-memory"
if [ ! -x "$BIN" ]; then
  echo "stage-engine: built binary '$BIN' is missing/not executable — refusing to stage (loud failure, M26)" >&2
  exit 3
fi

case "$(uname -s)" in
  Darwin) PLATFORM=darwin ;;
  Linux)  PLATFORM=linux  ;;
  *) echo "stage-engine: unsupported host OS $(uname -s)" >&2; exit 4 ;;
esac
case "$(uname -m)" in
  x86_64)  ARCH=x64   ;;
  aarch64|arm64) ARCH=arm64 ;;
  *) echo "stage-engine: unsupported host arch $(uname -m)" >&2; exit 4 ;;
esac

OUT_DIR="engine/$PLATFORM-$ARCH"
WEB_UI_DEST="$OUT_DIR/web-ui/dist"
# Replace, never merge: trunk hashes asset names, so merging a new bundle into an
# old one leaves orphaned hashed assets. A staged bundle is the operator's to remove;
# this script never deletes. `-f …/index.html` (not `ls -A`) keeps shellcheck clean and
# is exact: the exit-6 check below guarantees every staged bundle has index.html.
if [ -f "$WEB_UI_DEST/index.html" ]; then
  echo "stage-engine: '$WEB_UI_DEST' already holds a staged bundle; bundles are replaced, never merged (trunk hashes asset names). Remove it yourself and re-run: rm -rf '$WEB_UI_DEST'" >&2
  exit 7
fi
mkdir -p "$OUT_DIR"
cp "$BIN" "$OUT_DIR/opencode-agents-memory"
chmod +x "$OUT_DIR/opencode-agents-memory"
mkdir -p "$WEB_UI_DEST"
cp -R "$WEB_UI_SRC/." "$WEB_UI_DEST/"
if [ ! -f "$WEB_UI_DEST/index.html" ]; then
  echo "stage-engine: staged bundle '$WEB_UI_DEST' has no index.html after copy — refusing to ship an unresolvable package" >&2
  exit 6
fi

# Smoke: the staged binary answers --version (M26).
"$OUT_DIR/opencode-agents-memory" --version >/dev/null

echo "staged: $OUT_DIR/opencode-agents-memory ($TARGET)"
echo "staged: $WEB_UI_DEST ($(find "$WEB_UI_DEST" -type f | wc -l) files)"
