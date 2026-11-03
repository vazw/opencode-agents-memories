# 117 — the engine's structured stderr goes into a pipe the wrapper never drains


Status: done — 2026-11-03 — landed 0bfdcf1: engine stderr is drained into ~/.opencode-mem/opencode-mem.log in the log's own [ts] message: {json} shape, bounded by a 256-line queue with a counted drop summary; wrapper appends only, the engine keeps rotation (ADR-0013 amendment 2026-11-03).

*(Renumbered from 102 on 2026-11-03: the rust-port range already occupied 100–114, so the four issues filed by the 2026-11-03 waves are 115–118. Blueprints written before the renumber cite the old path; the blueprint for this issue is `.tmp/blueprints/blueprint-20261103-0952-issue-102-engine-stderr-drain-r2.md`.)*

## Observation (MEASURED 2026-11-03)

The wrapper spawns the engine with three pipes and attaches **no stderr listener**:

```
plugin/src/plugin.js:568-570
  const childProcess = spawn(execPath, engineArguments(), {
    stdio: ["pipe", "pipe", "pipe"],
    env: childEnvironment(),
  });
```

`stdout` is consumed (`childProcess.stdout.on("data", …)` → `handleWireLine`). `stderr` is written by
the engine and read by nobody.

The engine writes its NDJSON structured events there — `structured_stderr` (`engine.rs:104-122`) emits
`{"component","event","level",…}` lines to stderr for things like `consolidation.startup_tick`,
`consolidation.startup_failed`, `degraded`, and the capture retry/dead-letter events.

The dated log file (`~/.opencode-mem/opencode-mem.log`) does **not** carry them: it is written by
`mem_core::logging::log`, a different path (that is why `embedding.input_cap_resolved` appears there
and `consolidation.*` does not). Measured across every dated log:

```
grep -c 'consolidat' ~/.opencode-mem/opencode-mem.log        → 0
grep -c 'consolidat' ~/.opencode-mem/opencode-mem-2026-11-*.log → 0
```

## Two consequences

1. **Observability.** A diagnosis of "is the hourly consolidation pass running?" cannot be answered
   from the logs. During this session that question had to be settled by reading `agent_sessions` and
   the age window instead — the events that answer it were being written into a pipe.
2. **A latent write-block (INFERRED).** An unread pipe holds ~64 KiB; once full, the engine blocks on
   the next `write(2)` to stderr. No stall has been observed in this store (the events are rare), so
   this is reasoned, not measured — but it is the kind of failure that arrives as a hang with no log
   line, which is exactly the state this issue is about.

## Options

1. **Drain into the engine log** — parse each NDJSON line and append it in the log's own
   `[ts] event: {…}` shape. Keeps one place to look; costs a line-parser on the hot path (bounded by
   volume, which is low).
2. **Drain to a second file** (`opencode-mem.stderr.log`) — no parsing, but two logs to correlate.
3. **`stdio: ["pipe","pipe","ignore"]`** — deliberate silence, and the write-block disappears with
   it. Cheapest; loses the events entirely.

Whichever is chosen, it should be stated *why* the other two were rejected, because "we stopped
writing diagnostics somewhere" is a decision that outlives the code.

## Acceptance criteria

- The wrapper either consumes the engine's stderr or explicitly discards it, with the choice recorded.
- If consumed: a test that a `structured_stderr` line reaches the chosen destination, and that the
  wrapper survives a child that writes a large volume to stderr without blocking.
- `consolidation.startup_tick` (or whatever replaces it) is retrievable from disk after an engine
  start, so the next "is the pass running?" question is answerable without reading the store.

## Comments

**Landing evidence (2026-11-03, `0bfdcf1`).** Blueprint
`.tmp/blueprints/blueprint-20261103-1020-issue-117-engine-stderr-drain-r4.md` (stamp `c06e30393cae`),
ledger `.tmp/progress/blueprint-20261103-1020-issue-117-engine-stderr-drain-r4.ledger.md`.

**Suite:** `cd plugin && node --test tests/*.test.mjs` → **207** tests, **207** pass, **0** fail,
37 219 ms — the `194` re-derived at baseline `e9cfde9` plus this file's **13** new cases in
`plugin/tests/engine-stderr-drain.test.mjs`.

**RED → GREEN per row** (row 14 is the suite command, not a function):

| Row | RED (observed) | GREEN |
| --- | --- | --- |
| 1-3 | `SyntaxError: The requested module '../src/plugin.js' does not provide an export named 'engineLogPath'` | 3/3 |
| 4-5 | same missing export at HEAD; green with the shape's non-object fold | pass |
| 6 | `actual: 'Auto-capture memory persisted', expected: 'engine.stderr'` | pass |
| 7 | `bytes … actual: undefined, expected: 5000` | pass |
| 8 | `the engine's log file must exist` after 6 172 ms (no stderr listener ⇒ `logText === null`) | pass, 439 ms |
| 9 | `the handshake must settle — the engine blocked for 2008 ms` (settled=false, total 8 053 ms, logText null) | settled=true; 512 lines written, 1 summary |
| 10 | `conservation: 0 written + 0 counted` | written 512 + dropped 16 488 = 17 000; second run 768 + 16 232 |
| 11 | `the drain must create <home>/.opencode-mem/opencode-mem.log itself` | pass, no warning |
| 12 | `(a) the failure is named once` | a/b survive with one warning each; (c) `recoveryLinePresent=true`, `dropCounts=[1]`, exactly 1 warning |
| 13 | `the line lands in the engine's own log file` | pass; `elsewhere.log` absent |

Rows 9-13's RED was measured against `git show HEAD:plugin/src/plugin.js` staged as `dist/plugin.js`
(the driver imports `dist`), then the mirror was restored byte-identical.

**Mutant proofs.** `flushEngineLog`'s synthesis deleted → `conservation: 768 written + 0 counted`.
One summary per dropped line → `the summary must not be per-line spam, got 16232 summaries`.
Naive `writeFileSync(..., {flag:"a"})` inline in the stderr handler → `(a) the host must survive a
mkdir failure: node:fs:621` (the driver died inside the stream handler). r3's arrangement (clear
`stderrWriting` only in the append success callback) → `(c) the drain must resume after the repair`
(record: `recoveryLinePresent=false`, `dropCounts=[]`).

**Errnos, as they appear:** `mkdir(<regular file>, {recursive:true})` → `EEXIST: file already exists`;
`appendFile(<directory>)` → `EISDIR: illegal operation on a directory`; `appendFile(<regular
file>/…)` → `ENOTDIR: not a directory`.

**Two `testable: no` decisions, stated rather than papered over:** the teardown listener removal, the
queue bound being checked before rendering, the `ENGINE_STDERR_MAX_BUFFER` force-out branch, and the
seven-field re-establishment on a reused `bridge.v1` record are verified by reading the diff — none of
them is observable to this suite.

**Out-of-wave tree state, recorded not absorbed:** `Cargo.lock` (zstd `0.13.3` → `0.13.4`) was modified
at 17:35 +07:00 by a concurrent Rust-lane `cargo test --workspace` in the same worktree. It is not this
wave's change, was not staged, and is left as found.

**Tracker post-`Status:` block** (`docs/agents/issue-tracker.md`, run from the repository root):

```sh
for f in .scratch/*/issues/*.md; do
  n=$(basename "$f" | cut -d- -f1)
  grep -q '^Status:.*ready-for-agent' "$f" || continue
  c=$(git log --format='%h %s' -F --grep="(issue $n)" -1)
  [ -n "$c" ] && printf 'STALE-CLAIM %s: %s\n' "$f" "$c"
done
```

Observed output, verbatim (one line, **not** the expected silence):

```
STALE-CLAIM .scratch/rust-port/issues/102-persist-decay-sweep-stamp.md: 39dadf3 feat(bridge): run the profile confidence decay sweep from the engine lifecycle
```

It names a DIFFERENT issue — `102-persist-decay-sweep-stamp.md`, whose `Status:` line is still
`ready-for-agent — 2026-11-01` while commit `39dadf3` claims `(issue 102)` in its body. That is a real
stale claim in the tracker and it is outside this wave's scope (this wave owns issue 117 only); it is
recorded here rather than silently fixed because the Status line of another issue is another wave's
claim. Issue 117's own line is `done`, so the loop skips it by construction — silence about 117 is a
filter, not proof that its line is true.
