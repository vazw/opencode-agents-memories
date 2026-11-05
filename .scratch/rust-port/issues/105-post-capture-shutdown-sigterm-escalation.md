# 105 — after a capture, the `shutdown` RPC is acked at 5–6 s against a 2000 ms wrapper timeout, so SIGTERM fires while the engine is still working

Status: done — 2026-11-04. Fixed by calibrating `SHUTDOWN_RESPONSE_TIMEOUT_MS = 10000` in `plugin/src/plugin.js` (and synced to `plugin/dist/plugin.js`). Evidence recorded in `docs/evidence/issue-105-shutdown-escalation/README.md`.

## What is measured

After a capture, the engine's `shutdown` RPC is acknowledged **late** — the ack arrives but well past the
wrapper's budget:

```
write -> ack:      5.05 s   5.91 s   6.14 s      (ack always {"ok":true}; late, never refused)
wrapper timeouts:  SHUTDOWN_RESPONSE_TIMEOUT_MS = 2000
                   SHUTDOWN_TERM_AFTER_MS       = 2000      (plugin/src/plugin.js:52,54)
```

So `runShutdown` escalates and SIGTERM lands roughly **4.1 s after the capture**, while the engine is
still doing capture work. The engine's own shutdown pipeline reports `elapsed_ms` of 128–814 ms against a
4–6 s wall time, i.e. most of the delay is spent *before* the pipeline it times.

**Correction worth keeping:** an earlier measurement reported 3.07–3.33 s, because it timed from process
SPAWN rather than from the `shutdown` write. Spawn→ack is not the quantity `SHUTDOWN_RESPONSE_TIMEOUT_MS`
bounds. Any future figure must name its start point.

## What is NOT claimed

- **Not data loss.** `crates/mem-bridge/src/engine.rs:1627-1630` leaves undrained entries for the next
  start's replay, so a SIGTERM during capture work defers work rather than dropping it.
- **Not a cause.** Capture work occupying a single-threaded runtime is a hypothesis, not a measurement.
- Not that the flake in issue 103 shares this cause — 103 is a test-timing defect about a shutdown with
  no capture; this is a teardown that follows one. They were found together; they may or may not be related.

## What would settle it

The wrapper already writes the answer: `plugin/src/plugin.js:910-914` records `child_reaped` with
`signal: "SIGTERM"`. One post-capture teardown showing that pair closes "the escalation fires"; a
teardown showing `signal: null` refutes it. Then the engine-side log for the same window tells you what
it was doing when the RPC arrived.

## What lands (once triaged)

Either the wrapper's budget matches a measured post-capture shutdown, or the engine acks `shutdown`
promptly and defers the work — but which is correct depends on the cause, so no fix is prescribed here.

## Acceptance criteria

- [x] A post-capture teardown's `child_reaped`/`signal` pair is recorded from the live log, so
      "SIGTERM fires" is observation rather than inference.
- [x] The cause is named with evidence: what the engine is doing during the 5–6 s, from its own log.
- [x] Whichever side changes, the chosen behaviour is pinned: either a wrapper-budget test that fails at
      today's 2000 ms, or an engine row asserting `shutdown` is acked before capture work finishes.
- [x] No claim of lost work appears in the fix unless a test shows an entry failing to replay.

## Parent

No slice parent. Found while scoping issue 103 (engine exit bound); different file, different fix, and
outside that wave's "tests and evidence only" scope — which is why it was not folded in.

## Comments

**Closure — 2026-11-04:**
- AC1: Live reproduction log recorded in `docs/evidence/issue-105-shutdown-escalation/README.md` showing uncalibrated wrapper logging `child_reaped` with `signal: "SIGTERM"` at ~4.0 s (4018 ms).
- AC2: Synchronous ONNX model loading (`LocalOnnxEmbedder::load()`, blocking `std::fs::read` + `LoadedSession::load` + `load_tokenizer`) blocks the single-threaded Tokio runtime for 4.19–5.5 s, delaying stdio `shutdown` RPC read until model loading completes (total write→ack 5.05–6.14 s).
- AC3: Calibrated `SHUTDOWN_RESPONSE_TIMEOUT_MS = 10000` in `plugin/src/plugin.js` and synced to `plugin/dist/plugin.js` (verified via `node --test plugin/tests/dist-sync.test.mjs`). New test `test_post_capture_shutdown_budget_allows_graceful_exit` in `plugin/tests/shutdown.test.mjs` fails RED under 2000 ms timeout and passes GREEN under 10000 ms timeout.
- AC4: Verified that SQLite `capture_inbox` preserves queued records across teardown (dequeuing is non-destructive; deletion occurs only on `mark_done` after embedding), replayed on next engine startup via `startup_drain` (verified with `cargo test -p mem-bridge --test startup_drain`). Zero work lost.

**How this was almost filed wrong.** The planner measured spawn→ack (3.07–3.33 s) and reported it as if it
bounded the wrapper's 2000 ms timeout. The plan gate re-measured from the shutdown **write** and got
5.05/5.91/6.14 s. Same subsystem, same shape of claim, an interval off — which is why the gate requires a
numeral to carry the command that produced it, and why this issue names its start point explicitly.
