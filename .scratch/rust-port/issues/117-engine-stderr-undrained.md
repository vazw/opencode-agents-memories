# 117 — the engine's structured stderr goes into a pipe the wrapper never drains


*(Renumbered from 102 on 2026-11-03: the rust-port range already occupied 100–114, so the four issues filed by the 2026-11-03 waves are 115–118. Blueprints written before the renumber cite the old path; the blueprint for this issue is `.tmp/blueprints/blueprint-20261103-0952-issue-102-engine-stderr-drain-r2.md`.)*
Status: ready-for-agent — the fix is a few lines in the wrapper, but the destination of those lines is
a decision (append to the engine log, or a separate file, or drop them deliberately).

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
