# 105 — after a capture, the `shutdown` RPC is acked at 5–6 s against a 2000 ms wrapper timeout, so SIGTERM fires while the engine is still working

Status: needs-triage — 2026-11-01. Latencies are MEASURED (plan gate 1 for the issue-103 blueprint,
`.tmp/progress/shutdown-exit-bound.gate-1.md`); the **cause is not**. Filed from that gate, which was
asked to verify a planner's numbers and found the planner had measured the wrong interval.

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

- [ ] A post-capture teardown's `child_reaped`/`signal` pair is recorded from the live log, so
      "SIGTERM fires" is observation rather than inference.
- [ ] The cause is named with evidence: what the engine is doing during the 5–6 s, from its own log.
- [ ] Whichever side changes, the chosen behaviour is pinned: either a wrapper-budget test that fails at
      today's 2000 ms, or an engine row asserting `shutdown` is acked before capture work finishes.
- [ ] No claim of lost work appears in the fix unless a test shows an entry failing to replay.

## Parent

No slice parent. Found while scoping issue 103 (engine exit bound); different file, different fix, and
outside that wave's "tests and evidence only" scope — which is why it was not folded in.

## Comments

**How this was almost filed wrong.** The planner measured spawn→ack (3.07–3.33 s) and reported it as if it
bounded the wrapper's 2000 ms timeout. The plan gate re-measured from the shutdown **write** and got
5.05/5.91/6.14 s. Same subsystem, same shape of claim, an interval off — which is why the gate requires a
numeral to carry the command that produced it, and why this issue names its start point explicitly.
