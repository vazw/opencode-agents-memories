#!/usr/bin/env bash
# stage-engine.sh — build the engine binary and stage it into the npm package
# (W5.1 integration lane; M26/M30).
#
# - Builds the HOST triple by default (no cross-compile surprise); `--target`
#   is honored only when that toolchain exists (fails loudly otherwise).
# - Fails loudly when the binary is missing after the build.
# - chmod +x and smoke `--version`.
# - Output: plugin/engine/<platform>-<arch>/opencode-agents-memory — a
#   GITIGNORED build artifact (plugin/.gitignore, M29); never committed.
set -euo pipefail

cd "$(dirname "$0")/.."   # plugin/

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
mkdir -p "$OUT_DIR"
cp "$BIN" "$OUT_DIR/opencode-agents-memory"
chmod +x "$OUT_DIR/opencode-agents-memory"

# Smoke: the staged binary answers --version (M26).
"$OUT_DIR/opencode-agents-memory" --version >/dev/null

echo "staged: $OUT_DIR/opencode-agents-memory ($TARGET)"
