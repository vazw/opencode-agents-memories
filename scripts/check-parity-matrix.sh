#!/usr/bin/env bash
# Parity-matrix gate — one dispositioned row per pinned upstream test file.
#
# Pin: example/opencode-mem v2.39.0 @ 9086c65 (140 `tests/*.test.ts` files).
# docs/agents/upstream-tests-9086c65.txt is the recorded pin, derived from the
# clone by this command (run from the repo root):
#
#   git -C example/opencode-mem ls-tree -r --name-only 9086c65 -- tests |
#     grep -E '^tests/[^/]+\.test\.ts$' | LC_ALL=C sort \
#     > docs/agents/upstream-tests-9086c65.txt
#
# When the vendored clone is present its `git ls-tree` must agree byte-for-byte.
# The clone is read-only: this gate never writes to it.
#
# Checks (each violation prints one FAIL line):
#   1. manifest: PINNED_COUNT entries, LC_ALL=C sorted, every line shaped
#      tests/<name>.test.ts, no duplicates
#   2. manifest == `git ls-tree 9086c65 -- tests`; a clone-less checkout anchors
#      the manifest with PINNED_COUNT, and a present-but-pinless clone is a FAIL
#   3. testing.md: one row per manifest entry, and no row outside the manifest
#   4. every row carries a disposition in {port, covered-by, N-A}
#   5. every row has a non-empty counterpart cell (a bare em dash is empty) and
#      non-empty notes; `TBD` / `TODO` / `N/A` are rejected outright
#   6. every repo path named in the counterpart cell exists (no invented ports)
#   7. every N-A row cites a decision (#NN, ADR-NNNN, D-NN, issue NN) or a
#      scope ruling, and every cited ruling capability occurs in OVERVIEW.md
#
# Usage: bash scripts/check-parity-matrix.sh           # exit 0 = matrix complete
#        PARITY_MATRIX_ROOT=<dir> bash ...            # read-only audit of a copy
set -uo pipefail

ROOT="${PARITY_MATRIX_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
MATRIX="$ROOT/docs/agents/testing.md"
MANIFEST="$ROOT/docs/agents/upstream-tests-9086c65.txt"
UPSTREAM="$ROOT/example/opencode-mem"
PIN=9086c65
# Content anchor for the clone-less branch: the pinned list's line count as
# derived from 9086c65 by the command in this header. Update with the manifest.
# Re-pin: regenerate the manifest with the header command, set PIN/MANIFEST,
# then PINNED_COUNT=$(wc -l <"$MANIFEST" | tr -d ' '), and check both directions:
#   comm -23 <(git -C example/opencode-mem ls-tree -r --name-only "$PIN" -- tests |
#     grep -E '^tests/[^/]+\.test\.ts$' | LC_ALL=C sort) \
#     <(grep -oE '^\| `tests/[^`]+`' docs/agents/testing.md |
#       sed 's/^| `//; s/`$//' | LC_ALL=C sort)
#   comm -13 <(git -C example/opencode-mem ls-tree -r --name-only "$PIN" -- tests |
#     grep -E '^tests/[^/]+\.test\.ts$' | LC_ALL=C sort) \
#     <(grep -oE '^\| `tests/[^`]+`' docs/agents/testing.md |
#       sed 's/^| `//; s/`$//' | LC_ALL=C sort)
#   both directions must print nothing.
PINNED_COUNT=140

fail=0
bad() { printf 'FAIL: %s\n' "$*" >&2; fail=1; }

if [ ! -f "$MATRIX" ]; then
    printf 'FAIL: %s does not exist\n' "$MATRIX" >&2
    printf 'parity-matrix: FAIL\n' >&2
    exit 1
fi
if [ ! -f "$MANIFEST" ]; then
    printf 'FAIL: %s does not exist\n' "$MANIFEST" >&2
    printf 'parity-matrix: FAIL\n' >&2
    exit 1
fi

# --- 1. the manifest is the pin -------------------------------------------
manifest_count=$(wc -l <"$MANIFEST" | tr -d ' ')
LC_ALL=C sort -c "$MANIFEST" 2>/dev/null ||
    bad "manifest is not sorted under LC_ALL=C"
while IFS= read -r dup; do
    bad "manifest lists $dup more than once"
done < <(LC_ALL=C sort "$MANIFEST" | uniq -d)
while IFS= read -r junk; do
    [ -n "$junk" ] || continue
    bad "manifest lines must match tests/<name>.test.ts: $junk"
done < <(grep -vE '^tests/[^/]+\.test\.ts$' "$MANIFEST" || true)

# --- 2. the clone, when present, agrees with the manifest ------------------
if ! git -C "$UPSTREAM" rev-parse --git-dir >/dev/null 2>&1; then
    [ "$manifest_count" -eq "$PINNED_COUNT" ] ||
        bad "clone absent: manifest has $manifest_count entries, expected PINNED_COUNT=$PINNED_COUNT"
    printf 'NOTE: %s is not a checkout — the manifest and PINNED_COUNT are the sole pin\n' "$UPSTREAM" >&2
elif ! git -C "$UPSTREAM" cat-file -e "$PIN^{commit}" 2>/dev/null; then
    [ "$manifest_count" -eq "$PINNED_COUNT" ] ||
        bad "manifest has $manifest_count entries, expected PINNED_COUNT=$PINNED_COUNT"
    bad "clone is present but pin commit $PIN is missing — the manifest cannot be verified against it"
else
    live=$(git -C "$UPSTREAM" ls-tree -r --name-only "$PIN" -- tests |
        grep -E '^tests/[^/]+\.test\.ts$' | LC_ALL=C sort)
    if ! diff -q <(printf '%s\n' "$live") "$MANIFEST" >/dev/null; then
        bad "manifest disagrees with git ls-tree $PIN (regenerate it with the command in this header)"
        diff <(printf '%s\n' "$live") "$MANIFEST" | sed 's/^/     /' >&2
    fi
fi

# --- 3-7. the registry rows ------------------------------------------------
row_names=""
n_rows=0
n_port=0
n_covered=0
n_na=0
rows=$(grep -nE '^\| `tests/[^`]+` \|' "$MATRIX" || true)
if [ -z "$rows" ]; then
    bad "no registry rows found in $MATRIX"
fi
while IFS= read -r line; do
    [ -n "$line" ] || continue
    lineno=${line%%:*}
    body=${line#*:}
    upstream=$(printf '%s' "$body" | cut -d'|' -f2 |
        sed -E 's/^[[:space:]]*`//; s/`[[:space:]]*$//; s/[[:space:]]+$//')
    disp=$(printf '%s' "$body" | cut -d'|' -f3 |
        sed -E 's/^[[:space:]]+//; s/[[:space:]]+$//')
    counterpart=$(printf '%s' "$body" | cut -d'|' -f4 |
        sed -E 's/^[[:space:]]+//; s/[[:space:]]+$//')
    notes=$(printf '%s' "$body" | cut -d'|' -f5 |
        sed -E 's/^[[:space:]]+//; s/[[:space:]]+$//')

    n_rows=$((n_rows + 1))
    row_names="$row_names$upstream
"

    cells=$(printf '%s' "$body" | awk -F'|' '{print NF - 2}')
    [ "$cells" -eq 4 ] ||
        bad "line $lineno ($upstream): $cells cells, expected 4 (upstream | disposition | counterpart | notes)"

    case "$disp" in
        port) n_port=$((n_port + 1)) ;;
        covered-by) n_covered=$((n_covered + 1)) ;;
        N-A) n_na=$((n_na + 1)) ;;
        "") bad "line $lineno ($upstream): empty disposition" ;;
        *) bad "line $lineno ($upstream): unknown disposition '$disp'" ;;
    esac

    case "$counterpart" in
        "" | "—" | "-") bad "line $lineno ($upstream): empty counterpart/evidence cell" ;;
    esac
    [ -n "$notes" ] || bad "line $lineno ($upstream): empty notes cell"
    if printf '%s %s %s' "$disp" "$counterpart" "$notes" | grep -qE 'TBD|TODO|N/A'; then
        bad "line $lineno ($upstream): placeholder text (TBD/TODO/N/A)"
    fi

    while IFS= read -r token; do
        [ -n "$token" ] || continue
        path=${token%%:*}
        case "$path" in
            crates/* | plugin/* | tests/* | src/* | web-ui/* | docs/*)
                case "$path" in
                    *..*) bad "line $lineno ($upstream): refusing '..' in evidence path: $path" ;;
                    *)
                        [ -e "$ROOT/$path" ] ||
                            bad "line $lineno ($upstream): evidence path does not exist: $path"
                        ;;
                esac
                ;;
        esac
    done < <(printf '%s' "$counterpart" | grep -oE '`[^`]+`' | tr -d '`')

    if [ "$disp" = "N-A" ] &&
        ! printf '%s %s' "$counterpart" "$notes" |
            grep -qE '#[0-9]+|ADR-[0-9]+|D[0-9]+|issue [0-9]+|Scope ruling 2026-11-26'; then
        bad "line $lineno ($upstream): N-A does not cite a decision (#NN, ADR-NNNN, D-NN, issue NN) or a scope ruling"
    fi

    while IFS= read -r cap; do
        [ -n "$cap" ] || continue
        grep -qF -- "$cap" "$ROOT/OVERVIEW.md" ||
            bad "line $lineno ($upstream): ruling capability '$cap' does not occur in OVERVIEW.md"
    done < <(printf '%s' "$notes" |
        grep -oE 'Scope ruling 2026-11-26 — `[^`]+`' |
        sed -E 's/.*`([^`]+)`$/\1/')
    if printf '%s' "$notes" | grep -q 'Scope ruling 2026-11-26' &&
        ! printf '%s' "$notes" | grep -qE 'Scope ruling 2026-11-26 — `[^`]+`'; then
        bad "line $lineno ($upstream): ruling citation must name one backticked OVERVIEW capability after the em dash"
    fi
done < <(printf '%s\n' "$rows")

[ "$n_rows" -eq "$manifest_count" ] ||
    bad "$n_rows registry rows, expected $manifest_count (the manifest count)"
while IFS= read -r duprow; do
    [ -n "$duprow" ] || continue
    bad "duplicate registry row for $duprow"
done < <(printf '%s' "$row_names" | LC_ALL=C sort | uniq -d)

missing=$(LC_ALL=C comm -23 <(LC_ALL=C sort "$MANIFEST") <(printf '%s' "$row_names" | LC_ALL=C sort -u))
unknown=$(LC_ALL=C comm -13 <(LC_ALL=C sort "$MANIFEST") <(printf '%s' "$row_names" | LC_ALL=C sort -u))
n_missing=0
while IFS= read -r name; do
    [ -n "$name" ] || continue
    n_missing=$((n_missing + 1))
    bad "no registry row for $name"
done < <(printf '%s\n' "$missing")
n_unknown=0
while IFS= read -r name; do
    [ -n "$name" ] || continue
    n_unknown=$((n_unknown + 1))
    bad "registry row is not a pinned upstream file: $name"
done < <(printf '%s\n' "$unknown")

printf 'parity-matrix: %d/%d rows (port %d, covered-by %d, N-A %d); %d missing, %d unknown\n' \
    "$n_rows" "$manifest_count" "$n_port" "$n_covered" "$n_na" "$n_missing" "$n_unknown"
if [ "$fail" -ne 0 ]; then
    printf 'parity-matrix: FAIL\n' >&2
    exit 1
fi
printf 'parity-matrix: OK\n'
exit 0
