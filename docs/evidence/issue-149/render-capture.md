# Issue 149 — render capture (owner-run)

This procedure produces the only evidence that a rendered block carries a child's own pre-compaction actions. The owner runs
it, never a plan step. A passing test suite says nothing about it, and it writes nothing under `~/.opencode-mem/`. It works
on a copy of the storage root, under `.tmp/`, and removes its own snapshot on exit.

<!-- criterion-23:begin -->
Criterion 23 (issue 149) — `testable: no`; satisfied by an owner-run procedure, never by a plan step.

Purpose. Prove two things, both required, neither sufficient alone: (a) that after a compaction the wrapper received a second block for the session, and (b) that a block rendered for that session's own row carries a `## Recent actions` section built from the child's pre-compaction action lines.

Observations.
1. Delivery, from the wrapper's log, which carries event names, timestamps, `block_bytes` and `status` and never the block. Pass: at least one `inject.received` with `status:"ok"` and `block_bytes > 0`, timestamped after the compaction boundary recorded for that child.
2. Render, from the engine's own answer for that session, on a copy of the root. Pass: the extracted block contains a `## Recent actions` section whose lines are the action strings stored on that row.

The reading. `recent-action-delivered` requires BOTH observations. Neither alone satisfies it: a render without a post-boundary delivery is `rendered-not-delivered`, and a delivery without the section is `render-missing`. Record the resolved `chatMessage.injectOn` with every reading.

What the plan may not claim. A passing test suite, a green gate and a merged change are not evidence of either observation. Only a run of `docs/evidence/issue-149/render-capture.md` produces them, and only the owner runs it.
<!-- criterion-23:end -->

## Procedure

```sh
set -u
ROOT="${ENGINE_ROOT:?set to the live storage root}"
ENGINE_PID="${ENGINE_PID:?set to the engine pid, read live}"
CHILD="${CHILD_SESSION:?set to the child session id}"
PROJECT="${PROJECT_ID:?set to the project id the live run sent, or omit per R14}"
BOUNDARY="${BOUNDARY_TS:?set to the compaction boundary, ISO 8601}"
ART="$PWD/.tmp/render-capture-artifacts"
mkdir -p "$ART"
exe=$(readlink "/proc/$ENGINE_PID/exe"); echo "exe=$exe rc=$?"
sha256sum "$exe" | tee "$ART/binary.sha256"
before=$(du -sb "$ROOT" | cut -f1); echo "live bytes=$before"
SNAP=$(mktemp -d "$PWD/.tmp/rc-snap.XXXXXX")
trap 'find "$SNAP" -mindepth 1 -delete; rmdir "$SNAP"' EXIT
case "$(realpath -e "$SNAP")" in "$(realpath -e "$PWD/.tmp")"/?*) ;; *) echo "refused: snapshot outside .tmp"; exit 1 ;; esac
tar -C "$ROOT" --exclude=.engine.lock --exclude=status --exclude='*.bak*' -cf - . | tar -C "$SNAP" -xf -; echo "copy rc=$?"
copied=$(du -sb "$SNAP" | cut -f1); echo "copied bytes=$copied"
[ "$copied" -le $((before * 2)) ] || { echo "uncalibrated (copy exceeds ceiling)"; exit 1; }
[ -s "$SNAP/opencode-mem.log" ] || echo "log empty or redirect not in effect; record the cause, do not read it as proof"
sed -n 's/^\[\([^]]*\)\] inject\.received: \(.*\)$/\1 \2/p' "$SNAP/opencode-mem.log" > "$ART/received.txt"; echo "received rc=$?"
awk -v b="$BOUNDARY" '$1 > b' "$ART/received.txt" | grep -F '"status":"ok"' > "$ART/post-boundary.txt"
echo "post-boundary ok lines=$(wc -l < "$ART/post-boundary.txt")"
err="$ART/sqlite.err"
row=$(sqlite3 "$SNAP/episodic.db" "SELECT created_at || '|' || id FROM agent_sessions WHERE session_id='$CHILD' AND deleted_at IS NULL" 2>"$err"); rc=$?
if [ "$rc" -ne 0 ]; then echo "uncalibrated (sqlite3 refused: $(head -c 200 "$err"))"; exit 1; fi
[ -n "$row" ] || { echo "uncalibrated (child row absent in snapshot)"; exit 1; }
IFS='|' read -r c_at c_id <<<"$row"
newer=$(sqlite3 "$SNAP/episodic.db" "SELECT COUNT(*) FROM agent_sessions WHERE project_id='$PROJECT' AND deleted_at IS NULL AND (created_at, id) > ($c_at, '$c_id')" 2>"$err"); rc=$?
if [ "$rc" -ne 0 ]; then echo "uncalibrated (sqlite3 refused: $(head -c 200 "$err"))"; exit 1; fi
echo "newer project rows=$newer"
# render: the engine inject call, with exclude_session and project_id as the live run sent them (R13, R14), is not scripted here.
```

## Requirements

Each constraint below is part of the recipe above. Each cites the code or measurement it rests on.

- **R1** The wrapper and engine log is `[<ts>] <event>: {json}` text, not JSON lines (`crates/mem-core/src/logging.rs:317`), so `jq` exits 5 on it. Extract the payload with a `sed` step and record its exit code. A rotated file is `opencode-mem-<date>.log` (`logging.rs:250`); read the archive too when the current file starts after the boundary.
- **R2** Never `CMD || VAR=`. Capture the exit code and stderr separately. Classify a non-zero exit as `uncalibrated (sqlite3 refused: …)` and an empty result as `uncalibrated (child row absent in snapshot)`. These are different failures; never read one as the other.
- **R3** A newer-row count filters `deleted_at IS NULL` and compares `(created_at, id)`, matching `LIST_SESSIONS` (`crates/mem-store/src/episodic_sql.rs:315`) and the timeline's `ORDER BY created_at DESC, id DESC` (`:401`). A count on `created_at` alone disagrees with the window it checks.
- **R4** A `stat` over a glob that matches nothing leaves an empty file, and two empty files compare equal. Require `[ -s … ]` on each side, cover `.db`, `-wal`, `-shm` and `-tshm`, and use `%.9Y`, since `%Y` has one-second granularity and hides a concurrent write.
- **R5** A containment guard built from an always-matching pattern is a tautology. Compare `realpath -e "$SNAP"` against `realpath -e "$ROOT/.tmp"` with a non-empty suffix, and demonstrate that a bad value is refused (`uncalibrated` or `refused`, never silence).
- **R6** Bound the copy with a run-time ceiling derived from the live root's size (`du -sb` taken twice), and print the copied total to stdout. No pinned literal.
- **R7** Write every artifact outside the disposable directory before the trap runs, or the procedure destroys its own evidence. Cleanup is `find … -mindepth 1 -delete`, then `rmdir`.
- **R8** Copy the tree minus `.engine.lock`, `status` and `*.bak*`, as `crates/mem-bridge/tests/input_cap_corpus_audit.rs:13-17` does. A per-stem loop that omits `projects/`, `.wal-mode` and `.turso-migrated` is a different object: `docs/evidence/shard-consolidation-measurement/ab-draft.md:176-186` lists `.wal-mode`, `.turso-migrated` and `.auth-token` among the files a copy must carry. Copy them, or record the omission as an accepted difference.
- **R9** The embedder cache is `{storagePath}/.cache/{model}` (`crates/mem-embed/src/local/mod.rs:281`). A copy without it may fail to open. Classify that failure; never read it as a render defect.
- **R10** The binary must be the one the child runs. `target/release/opencode-agents-memory` (83308328 B, 2026-11-18 04:56) and `plugin/engine/linux-x64/opencode-agents-memory` (83203448 B, 2026-11-18 00:51) differ. Record `sha256sum`, size and `%.9Y` mtime for both, and resolve the live one with `readlink /proc/<pid>/exe`.
- **R11** A live-log `stat` falsifier cannot attribute cause, because any concurrent wrapper write trips it. Assert that the redirect took effect with `[ -s "$SNAP/opencode-mem.log" ]`, and record the cause of any live-log change instead of treating it as proof of a leak.
- **R12** `jq -r` appends a newline and `jq -j` does not, so a `wc -c` comparison against `block_bytes` mismatches by construction unless `-j` is used. Handle the newline explicitly.
- **R13** `exclude_session` removes the requesting session's own row from the episodic candidates (`crates/mem-bridge/src/inject.rs:480-484`). It does not remove the actions section. Record whether the live run sent it; do not assume.
- **R14** `project_id` is omitted when `hostProjectId` is null (`plugin/src/plugin.js:1617`), and the engine then returns `block: null`. A render that always sends a project can produce a block the child never received. Reproduce the omission whenever the live run had one.

## Open items no read-only pass can settle

These are open, not passed. Each is `uncalibrated` until it is met.

- Whether the `sqlite3` CLI can read the Turso-format file at all. The `episodic.db` path in the recipe is also unconfirmed against the live layout.
- Whether the engine opens the snapshot.
- Whether `inject` answers at all on a copy.
- Whether the copy's block is equivalent to the one delivered live.

## Classification

- `recent-action-delivered`: requires BOTH a post-boundary `inject.received` with `status:"ok"` and `block_bytes > 0`, AND a rendered block for that session's row carrying a `## Recent actions` section whose lines are the stored action strings.
- `rendered-not-delivered`: the render carries the section, but there is no post-boundary delivery pair. This is the partial outcome.
- `render-missing`: a delivery without the section.
- `uncalibrated`: an unmet precondition, with the cause in parentheses (R2, R5, R9, R10).
- `window-evicted`: ten or more newer project rows exist after the child's row, so the child fell out of the read window (criterion 19's ceiling).
- `contaminated`: a child answer that does not appear verbatim in the captured section.

Record the resolved `chatMessage.injectOn` with every reading.
