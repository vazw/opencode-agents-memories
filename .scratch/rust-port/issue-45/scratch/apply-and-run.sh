#!/bin/sh
# Reproduce the issue-45 A/B measurement (2026-10-25).
# The harness is throwaway: it is NOT a deliverable and must be removed after.
#
#   1. copies the prototype test into the tree (no Cargo.toml/Cargo.lock change:
#      fcntl is declared directly, so no libc dependency is added);
#   2. builds + runs the five arms;
#   3. removes the test file.
#
# Run from the repo root.
set -eu
REPO="$(cd "$(dirname "$0")/../../.." && pwd)"
cd "$REPO"
cp .tmp/scratch/issue-45-ab/issue45_ab_measure.rs crates/mem-store/tests/issue45_ab_measure.rs
trap 'rm -f crates/mem-store/tests/issue45_ab_measure.rs' EXIT
sh .tmp/scripts/cargo-serialized.sh cargo test -p mem-store --test issue45_ab_measure -- --nocapture --test-threads=1
