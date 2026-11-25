# 145 — the pre-turn `<memory_context>` block almost never reaches subagent sessions

Status: landed — sink shipped; M1 deferred pending the post-restart measurement

*(Related: issue 124, closed 2026-11-05 as "intermittent miss" — this issue supersedes its conclusion.)*

## The measurement

Ground truth is the query issue 124 established: a session received the block iff one of its stored
messages carries the literal `<memory_context>` marker (`session_message.data LIKE '%<memory_context>%'`).

**Today, 2026-11-18:**

| session | agent | messages | carrying `<memory_context>` |
|---|---|---|---|
| `ses_e9d5dd698ffe5lUV7W7s2KDpbh` | main (this session) | 18,948 | **180** (21 today) |
| `ses_e12b819a2ffeGsAVfmRbCN66Sg` | coder (issue 32) | 156 | **0** |
| `ses_e12c6c2f4ffe6LJN44vCpqC7Nb` | code-reviewer (141) | 80 | **0** |
| `ses_e12944535fferitJvVSfaFmSsa` | syseng (issue 28) | 79 | **0** |
| `ses_e12a283b2ffeui2vBlkdOnGrqN` | code-reviewer (32) | 62 | **0** |
| `ses_e12881697ffeSvAQxNoW5DEFah` | explore (throwaway probe) | 3 | **1** |

**Child sessions over all history in this database: 171 of 3,982 carry a block (4.3 %)**, and each of those
carries exactly **one**, at an arbitrary message position (`seq` 4, 41, 54, 84, 102, 126, 168, 183, 359, 368,
465, 2056) — never reliably on the spawn prompt, which is the turn that needs it.

### Correction, same day: the marker measures PERSISTENCE, and the debugger is a miss

The owner spotted the problem because the debugger child `ses_e130e6e2dffeyy1bGZz17qo9SM` (84 messages) does
**not** show `◈memory context` — and that session stores **0** markers, so UI and database agree there. An
earlier inject-delivery wave had recorded that ephemeral blocks are sometimes never saved in `session_message`,
so a stored marker is evidence of delivery but its absence is not proof against it; for this debugger both
signals say miss.

The decisive split is visible in the five misses and three hits spawned today, all of which attach at
**`seq 4`, the session's first user message**:

| session | agent | first user message (bytes) | block |
|---|---|---|---|
| `ses_e1283232cffeq1O3Glrm5REycu` | explore | 806 | **yes** |
| `ses_e1283232bffeqstrnlgeQ2bmHk` | explore | 806 | **yes** |
| `ses_e12881697ffeSvAQxNoW5DEFah` | explore | 878 | **yes** |
| `ses_e12b819a2ffeGsAVfmRbCN66Sg` | coder | 4,973 | no |
| `ses_e12944535fferitJvVSfaFmSsa` | syseng | 4,995 | no |
| `ses_e130e6e2dffeyy1bGZz17qo9SM` | debugger | 5,412 | no |
| `ses_e12c6c2f4ffe6LJN44vCpqC7Nb` | code-reviewer | 5,713 | no |
| `ses_e12a283b2ffeui2vBlkdOnGrqN` | code-reviewer | 6,322 | no |

Every fleet spawn prompt is 5–6 KB; every probe prompt is under 1 KB. **The size hypothesis is now dead:**
a padded `explore` probe given ~9,500 characters of inert filler plus the same four questions **received the
block** (`memory_context: yes`, 1 occurrence, profile and project facts present). Length is therefore not the
discriminator, and today's clean split was coincidence. Over all history the split never held either (children
with a ≥1 KB first prompt: 3,614 missed / 153 hit; under 1 KB: 167 missed / 12 hit).

The next single variable is **agent type** — and that is dead too: short-prompt probes on `coder` and `syseng`
(`ses_e12812aeaffe…`, `ses_e12812aeaffd…`) both answered `memory_context: yes` (1 occurrence, facts present,
~490 and ~540 prompt bytes). The **prompt shape** hypothesis is dead as well: a `coder` probe given the fleet's
exact preamble (`BLUEPRINT:`/`STAMP:`/`LEDGER:` + long task text, `ses_e12809c47ffe…`) also answered yes and
persisted 1 marker.

So five structural probes hit at 14:03–14:11 (explore ×4, coder, syseng; 3 messages each, 1–2 markers each),
and the four real spawns missed at 12:55–13:50. Every hit's spawn second has its `profile inject` line in the
engine log, and so does every miss (12:55:13, 13:11:14, 13:34:49, 13:50:22) — the engine answered in both
windows.

**Live hypothesis: contention with auto-capture.** The four misses fall inside a capture-heavy window (engine
log captures at 13:05:43, 13:06:33, 13:07:06, 13:07:41, 13:09:57, 13:10:26, 13:11:42, 13:35:03, 13:35:23,
13:43:58 — the 13:11:14 and 13:34:49 spawns sit within ~30 s of a capture), while the five hits landed in a
quiet window. UNPROVEN and not to be acted on: it is the only hypothesis left, and the sampler cannot see
`inject.received {block_bytes, status}` because it goes to `process.stderr` with no sink on disk. Instrumenting
that is step 1 and it is not optional.

**The engine is not the failing end.** `~/.opencode-mem/opencode-mem.log` logs `profile inject` at the exact
spawn second of every child measured above — 13:11:14 (coder), 12:55:13 (141 reviewer), 13:34:49 (32 reviewer),
13:50:22 (syseng), 14:03:41 (probe) — and 75 times today in total, against 22 blocks that landed (21 parent,
1 probe). So the request arrives and the profile is rendered; the block is lost afterwards.

**Prompt size is not the cause:** comparing each child session's first stored message with whether it ever
received a block gives average 5,287 bytes (never blocked, n=3,811) versus 5,814 bytes (blocked, n=171) —
indistinguishable.

## Where the loss is

INFERRED, and it is the one thing this issue exists to settle by measurement: the attach is
`plugin/src/plugin.js:1402`, `output.message.parts.push({ type: "text", text: block, synthetic: true })`,
unconditional whenever the engine returned a non-empty block. Either (a) the engine's `block` is empty for
child turns despite the `profile inject` line, or (b) the host does not persist a synthetic part pushed onto
a child session's `chat.message` output, while it does for a root session. The wrapper already emits
`inject.received {session_id, block_bytes, handling_ms, status}` that would separate them — **but it goes to
`process.stderr` (`plugin/src/plugin.js:214-221`), and `~/.local/share/opencode/log/opencode.log` contains
zero `opencode-mem-plugin` lines**, so the telemetry does not exist on disk.

## What to build

1. **Instrument first, and cheaply:** `writeStructured` also appends its JSON line to a file under the
   engine's log directory (path from the plugin's existing config/env resolution — do not invent a second
   location scheme). No new format, no new event names. Then one spawn reproduces the miss and says which
   of (a) or (b) it is, with `block_bytes` and `status` in hand.
2. **Fix the winning arm**, not both:
   - if `status = "empty"` on child turns, the fault is in the engine's query construction for a child prompt
     (fix there, and say which input differed);
   - if `status = "ok"` with `block_bytes > 0` and no block in the session, the fault is the attach/persist
     path, and the fix must not depend on mutating a message object the host may replace.
3. **A test that would have caught it.** The regression test must assert a **child** session receives the
   block, not that the wrapper returns one: spawn a child in a fixture, then assert its stored messages carry
   the marker. A test of the wrapper's return value alone is the shape that let this pass in the first place.

## Acceptance criteria

- [ ] `inject.received` and `inject.requested` are readable on disk after a spawn (path recorded in the issue
      comment, with the verbatim lines from one child that missed and one that hit).
- [ ] The losing arm (a) or (b) is named with evidence, and only that arm is fixed.
- [ ] A test fails on the pre-fix tree for a **child** session and passes after, with the RED output pasted
      (`plugin/tests/*.test.mjs`, wasm-free, `node --test`).
- [ ] Post-fix measurement, same query as above: N children spawned, N blocks delivered, for N ≥ 3, in the
      issue comment.
- [ ] No behaviour change for root sessions: their block count per turn stays as it is (21 of today's 75
      injects were root-side, and those already work).

## Blocked by

Nothing. Touches `plugin/src/plugin.js` only; independent of issues 142/143 (web API) and of the issue 28
rehearsal.

## Evidence 2026-11-18 (issue 145, r29)

Wave status: OPEN. Step 0c item 2 read unreadable (`database is locked (5)` while the seed engine was alive); the gated items passed. The wave is not closed. Tracker status stays as it is.

- Baseline on the working tree (not clean HEAD), `plugin/`, full suite in place: `rc=0`, `ℹ pass 239`, `ℹ fail 0` (`.tmp/progress/145-r29-baseline.txt`).
- Sink: `plugin/tests/log-sink.test.mjs` `ℹ pass 6`, `ℹ fail 0`, rc 0 after the retry rewrite. Mutant M1a (`logSink = null` on failure) killed on `the sink never recovered`. Mutant M1c (warn-once latch removed) killed on `warns.length === 1` with actual `6`, not the predicted `3`: each failing event writes two structured lines (`inject.requested`, `inject.received`), so three events give six.
- Toast: `plugin/tests/failure-toast-145.test.mjs` `ℹ pass 7`, `ℹ fail 0`. Cases 1–4 RED: toast assertion (`toast assertion`) failing under the removed-toast and single-mutant builds. Cases 5–7 regression guards, killed: `empty-toast-restored` on `toast assertion` (case 5), `capture-toast-on-inject-path` on `toast assertion` (case 6), `toast-appends-part` on the deep-equal assertion (case 7). Empty-block negative control: `block_bytes: 0`, `toastCalls 0`, `settled true`. `grep -n inject_empty plugin/src/plugin.js plugin/dist/plugin.js`: no match, rc 1.
- Step 0 item 8: `awk 'NR>=1395 && NR<=1430' plugin/src/plugin.js | grep -c 'isChildSession\|parentID'` prints `0`, awk rc 0. `isChildSession` at `:37, :1444, :1543, :1672, :1718`.
- Step 0 item 11 anchor scope: `git show HEAD:plugin/src/plugin.js | grep -n 'inject.received'` gives `:1392`, `:1394`; working tree gives `:1420`, `:1422` (the 24 line sink shift in the plan).
- Post-change full suite: `rc=0`, `ℹ pass 247`, `ℹ fail 0` (baseline + 8).
- Probe guard amendment: `ℹ pass 2`, `ℹ fail 0`. Mutants killed on `code=0 stderr=`: reap guard and root guard.
- Step 0c seed: item 4 post-exit COUNT `1`; item 6 nonce COUNT `1`. Item 2 immediate read unreadable, recorded, not a seed failure. Drain path not labelled.
- Step 8 probe: three of three children carry the nonce (`nonce_count=1`), history heading `## Recent sessions`, block 328 bytes, `status:"ok"`, `host_contract:"setup"`. Block-level delivery on a prompted session only. Probe heading regex corrected from a line-anchored form, which could not match the engine's one-line heading. First run `rc=7` kept in `.tmp/progress/145-r29-probe.txt`.
- Shell half unmet: crates/mem-bridge/src/inject.rs carries no background-shell section, so a child cannot be told which shell it left running; filed as issue 146 (146-inject-block-lacks-shell-section.md).
- M1 gap deferred. The arm (a)/(b) question is unresolved on current evidence. Runbook: `docs/evidence/issue-145/README.md`.

## Evidence 2026-11-18 (issue 145, r32)

Blueprint `20261118-0715-issue-145-inject-sink-r32` (sha `b6dd8346e6dc`), third execution attempt. The tree is HEAD `89edfe6`
plus the reviewed working-tree change; nothing was reverted. Scratch only; live pids 4719 and 20156 untouched; `<web-port>` 3750.

- Tree state, re-measured: `git status --short` 13 entries (7 modified, 6 untracked), verbatim in `docs/evidence/issue-145/r32-status.txt`; `git diff --stat` `7 files changed, 112 insertions(+), 27 deletions(-)`, verbatim in `docs/evidence/issue-145/r32-diffstat.txt`. No tracked-file surprise and no untracked surprise, so the surprise rule did not fire.
- Step 0 item 11 anchor scope: `git show HEAD:plugin/src/plugin.js | grep -n 'inject.received'` gives `1392:` and `1394:`; `grep -n 'inject.received' plugin/src/plugin.js` gives `1420:` and `1422:` — a 28-line shift. A HEAD-anchored read lands 28 lines off.
- Step 0 item 8 session-kind count, re-derived: `awk 'NR>=1399 && NR<=1435' plugin/src/plugin.js | grep -c 'isChildSession\|parentID'` prints `0`, pipeline rc `1` (rc `1` accompanies a zero count). Handler bounds `:1399` `"chat.message": async (hookInput, output) => {` and `:1435` `},`. `grep -n 'isChildSession' plugin/src/plugin.js` → `37:`, `1448:`, `1547:`, `1676:`, `1722:`; none inside the range. So the empty branch at `:1420` is one shared branch for roots and children, and the negative control covers both.
- Step 0 item 7 shell gap, re-read: `grep -c -i 'shell' crates/mem-bridge/src/inject.rs` → `0`, rc `1`; `grep -n '^## ' crates/mem-bridge/src/inject.rs` → no output, rc `1`.
- Step 0b model: provisioner rc=0, `tokenizer_version=2.0`, `model_onnx_sha256=347440e93b5ec979fdcf6041b72721aade7b9680c16022e2830db7115ff6fd9f`; sizes equal (onnx 547552426, tokenizer.json 710878, config.json 1636); no `*.tmp.*`.
- Step 0 baseline, in place, `plugin/`: `rc=0`, `ℹ tests 247 ℹ pass 247 ℹ fail 0` (`.tmp/progress/145-r32-baseline.txt`).
- Step 6 post-change, in place: `rc=0`, `ℹ tests 247 ℹ pass 247 ℹ fail 0` (`.tmp/progress/145-r32-post.txt`). Delta 0, as the two new test files are already in the reviewed tree.
- Sink, Step 1: state machine on the tree — `218: let logSink = "dir";`, `219: let logSinkWarned = false;`, `232: if (logSink === "dir") mkdirSync(...)`, `234: logSink = "ready";`, `237-238: if (logSinkWarned) return; logSinkWarned = true;`. GREEN `tests/log-sink.test.mjs`: `rc=0`, `ℹ tests 6 ℹ pass 6 ℹ fail 0`. Mutant M1a (`logSink = null` on failure, dist-only) `rc=1`, `ℹ fail 1`, `AssertionError [ERR_ASSERTION]: the sink never recovered` — the retry case dies on its own named assertion, before the `inject.received` assertion. Mutant M1c (warn-once latch removed) `rc=1`, `ℹ fail 1`, `assert.equal(warns.length, 1)` at `tests/log-sink.test.mjs:97` with actual `6 !== 1` (each failing event writes two structured lines; the amended three-event driver makes the count discriminate).
- Toast, Step 1b: GREEN `tests/failure-toast-145.test.mjs`: `rc=0`, `ℹ tests 7 ℹ pass 7 ℹ fail 0`. RED for cases 1-4 re-produced with dist-only mutants, each failing on the toast assertion after `settled`, the wire count and the answer marker had already passed — so the fixture reached the wire and the toast is the missing behaviour:
  - `refusal-check-removed`: `✖ test_capture_refusal_toasts_once`, `✖ test_same_reason_different_sessions_one_toast`, `✖ test_two_reasons_two_toasts_with_their_own_texts`; `AssertionError [ERR_ASSERTION]: toast assertion`, `0 !== 1`.
  - `throw-toast-removed`: `✖ test_capture_throw_toasts_once`; `toast assertion`, `0 !== 1`.
  - `text-param-ignored`: cases 1, 2, 4; deep-equal body failure and `AssertionError: capture body`.
  Cases 5-7 are regression guards, GREEN before the slice, proven by mutant kills: `empty-toast-restored` → `✖ test_inject_empty_block_does_not_toast`, `toast assertion`, `1 !== 0`; `capture-toast-on-inject` → `✖ test_inject_error_keeps_single_toast` and `✖ test_toast_failure_does_not_change_the_hook_output`, `toast assertion`, `2 !== 1`; `toast-appends-part` → `✖ test_toast_failure_does_not_change_the_hook_output`, deep-equal parts failure. Dedupe mutants: `dedupe-miss-always-distinct-key` → case 3 `toast assertion`, `2 !== 1`; `shared-key` → case 4 `toast assertion`, `1 !== 2`.
- Negative control (owner ruling: an empty block never toasts): `injectMode:"success"` → `block_bytes: 0`, `status:"empty"`, `settled true`, `toastCalls 0`. `grep -n 'inject_empty' plugin/src/plugin.js plugin/dist/plugin.js` → no output, rc `1`.
- `cmp plugin/dist/plugin.js plugin/src/plugin.js` rc 0 and `cmp plugin/dist/v2/plugin.js plugin/src/v2/plugin.js` rc 0 after every mutant run.
- Step 0c seed, scratch root, post-exit reads only. No read of `.tmp/root-145/episodic.db` ran while the seed engine was alive, because the engine holds `locking_mode=EXCLUSIVE` for the connection's lifetime — controlled reproduction in `docs/evidence/issue-145/scratch-db-lock.md` (an open WAL write transaction does not block a reader; `locking_mode=EXCLUSIVE` does, and a writable reader fails identically). No `OPEN — inbox state unreadable` was produced. Ledger lines verbatim: `- Step 0c seed-ack 2026-11-18T17:36:01Z capture response: {"id":2,"jsonrpc":"2.0","result":{"accepted":true}}`; `- Step 0c close_stamp=1795023366390 close_utc=2026-11-18T17:36:06Z (epoch ms, recorded immediately before child.stdin.end(); hold 5000 ms from seed-ack)`; `exit 7127 2026-11-18T17:36:06Z`. The hold is `await new Promise((r) => setTimeout(r, 5000))` in the Node driver. Classification (a reading, never a gate, and the plan carries no threshold): `- Step 0c item 4.6 classification: created_ms=1795023361287 close_ms=1795023366390 comparison=1795023361287 -lt 1795023366390 => capture-time label=capture-time`. Item 4.5 post-exit COUNT `1`; item 6 nonce COUNT `1`; item 4.7 `capture_inbox` counts empty (no rows).
- Step 8, scratch block delivery: probe rc 0 `PROBE OK`; seed COUNT `1` (rc 0, read post-exit). Per child `nonce_count=1`, heading `## Recent sessions`, 328 bytes, `block_sha256=8d0625a58822a7c2a5e047033cafcf64b97a3e99c6481aafa02c000116634790` (identical for all three). Block-level delivery on a prompted session only; stored-message delivery is the R3 runbook's job. Scratch sink lines carry `"host_contract":"setup"`, verbatim: `[2026-11-19T00:40:07.749+07:00] inject.received: {"block_bytes":328,"component":"opencode-mem-plugin","handling_ms":15,"host_contract":"setup","level":"info","session_id":"ses_probe_child_1","status":"ok"}`.
- Step 7 / 8b probe guard: `grep -n 'plugin/undefined' plugin/tests/probe-145-guard.test.mjs` → no output, rc `1`; marker count `5` rc 0; guard GREEN `ℹ tests 2 ℹ pass 2 ℹ fail 0` rc 0. Mutant `reap-guard-deleted` → `✖ test_probe_refuses_without_seed_exit_line`, `AssertionError [ERR_ASSERTION]: code=0 stderr=`; mutant `root-guard-deleted` → `✖ test_probe_refuses_without_driver_storage_root`, `AssertionError [ERR_ASSERTION]: code=0 stderr=`. Probe header cites `startBridge` at `plugin/src/plugin.js:898`; the tree measures `909` (`async function startBridge(state) {`). A comment, not a gate.
- Shell half unmet: crates/mem-bridge/src/inject.rs carries no background-shell section, so a child cannot be told which shell it left running; filed as issue 146 (146-inject-block-lacks-shell-section.md).
- M1 gap deferred. The arm (a)/(b) question is unresolved on current evidence. Runbook: `docs/evidence/issue-145/README.md`.

## Preflight 2026-11-25 — the post-restart run is unblocked except for the owner's R2

Host restarted 2026-11-25T04:21:39Z (`args=["service","restart"]`), which this runbook's header waits for. Read-only; nothing under `~/.opencode-mem/` written, no signal sent. Window 04:50:45Z.

- **S0 schema: PASS.** `session_message` lists `id, session_id, type, seq, time_created, time_updated, data`; `session_v2` lists `id, project_id, workspace_id, parent_id, …`. No drift, so Q0 may run.
- **R1 host graph: PASS.** `grep 'host_contract' ~/.opencode-mem/opencode-mem.log | tail -3` shows `"host_contract":"setup"` on live inject lines, newest `2026-11-25T11:43:02.701+07:00` (`status:"ok"`, `block_bytes:2871`). The V2 host graph is running; no further restart is needed for R1.
- **Q1 calibration: PASS, under a corrected literal.** `grep -c '"host_contract"'` = 462. The runbook's original `tail -5 | grep 'inject.received'` returns rc 1 while the engine is healthy, because a live host appends engine stderr to the same file and the last five lines are usually not inject lines — MEASURED here. The line is now `grep 'inject.received' … | tail -1`, which returns the 11:43:02 line with `"level":"info"`. Fixed in `docs/evidence/issue-145/README.md`.
- **Processes:** host pid 8878 (`opencode serve --service`), engine pid 8953 (`plugin/engine/linux-x64/opencode-agents-memory --storage-root /home/opencode/.opencode-mem/data`).

**Still blocked on the owner, and only on this:** R2 — five fresh children from a different directory, each starting one named background shell plus one named action, then the owner triggers compaction in each child and stops. R3/Q6/Q7 then need one follow-up turn per child with a fresh `PROBE-NONCE-<8 hex>` written after the boundary. Nothing in R0/R1/Q1 can be advanced further without that.

Q8 remains not measured by construction (no tool hook; issue 146 closes as covered by 149).
