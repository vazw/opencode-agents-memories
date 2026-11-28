#!/usr/bin/env bash
# Non-vacuity check for the shard-14003 write refusal.
# Compares the build's fingerprint for a given config against the fingerprint
# actually stored in the (copied) shard.
#
# Usage: check_identity.sh <config-home> <copied-shard.db>
# Exit 0 = equal (writes would pass), 1 = unequal (refusal), 2 = setup error.
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../.." && pwd)

home=${1:?config home}
shard=${2:?copied shard db}

rlib=$(ls -t "$repo"/target/debug/deps/libmem_core-*.rlib 2>/dev/null | head -1)
[ -n "$rlib" ] || { echo "no libmem_core rlib; run cargo build -p mem-core first" >&2; exit 2; }

rustc --edition 2024 -L "$repo/target/debug/deps" --extern mem_core="$rlib" \
  -o "$here/identity_probe" "$here/identity_probe.rs" || exit 2

configured=$(CONFIG_HOME="$home" "$here/identity_probe" | sed -n 's/^FINGERPRINT=//p')
stored=$(sqlite3 "file:$shard" "SELECT value FROM shard_metadata WHERE key='embedding_fingerprint';")

printf 'configured: %s\n' "$configured"
printf 'stored    : %s\n' "$stored"
if [ "$configured" = "$stored" ]; then
  echo "MATCH"
  exit 0
fi
echo "MISMATCH"
exit 1
