# 103 — the 6 s exit bound reds the workspace gate intermittently, and "0 failed" is a statement about machine load

Status: ready-for-agent — 2026-11-01. MEASURED by the batched review of `95857b7` + `39dadf3`
(`.tmp/progress/batch-review-1.md` §8 F1), not inherited. Pre-existing: neither commit touches the file.

## What is wrong

`tests/engine_shutdown.rs:538` `test_capture_stderr_has_no_transcript_canary` waits on
`wait_bounded(&mut child, Duration::from_secs(6))` (`:553`; the helper panics at `:138` with
"process did not exit within {bound:?}").

Measured exit latency is **~4–5 s against a 6 s bound** — a 20–30 % margin that load consumes:

```
4 failures in ~19 executions of this single test
  - first workspace run, while two scratch cargo builds were compiling
  - a full-suite rerun of engine_shutdown (same overlap)
  - once in three isolated runs on an otherwise idle machine (6.18 s vs passes at 4.35 / 4.46 s)
  - the final --no-fail-fast workspace run (49.17 s for that binary, no other cargo running)

attribution, scratch worktree at 95857b7 vs HEAD, interleaved on an idle machine, 5x each:
  A:    5.13  4.66  3.95  4.07  4.20 s   all pass
  HEAD: 4.39  4.27  4.01  3.90  5.09 s   all pass
```

Both trees sit at the same marginal latency, so the flake predates this wave and belongs to no commit.

## Why it matters beyond one red test

The consolidated wave gate's summary line becomes load-dependent: the coder reported
`289 suites / 2317 passed / 0 failed / 30 ignored`; the reviewer, on a quieter machine, twice got
`2316 passed / 1 failed` — same suites, same 30 ignored, the difference being exactly this test.
A wave can therefore be declared green or red by machine activity rather than by code. That is the same
defect class as a test that cannot fail, mirrored.

## What would settle it

A bound derived from the measured teardown, not a round number. Two candidate fixes; triage picks:

1. **Assert on the teardown report rather than wall-clock.** If the child writes a structured
   "shutdown complete" line, the test waits for that with a generous watchdog (e.g. 60 s) that only
   fires on a genuine hang. This removes load sensitivity entirely and keeps the canary assertion.
2. **Raise the bound to a measured multiple.** With ~4.4 s typical, 6 s is 1.36x. A 3x bound (~13 s)
   survives the worst observed (6.18 s) with margin; it does not fix the class, only the instance.

Prefer (1). Whichever is chosen, record the measured distribution and the chosen margin in the test's
doc comment, so the next reader knows the number is derived rather than taste.

## Acceptance criteria

- [ ] The test's wait is either event-based with a watchdog that only fires on a real hang, or a bound
      justified by the measured distribution recorded in its doc comment.
- [ ] A measured margin is stated, not a round number.
- [ ] Repeated runs under deliberate CPU load (e.g. concurrent `cargo build`) do not red it — evidence
      recorded under `docs/evidence/`.
- [ ] The canary assertion (`:559`) is unchanged: the transcript marker must still never reach stderr.
- [ ] `tests/engine_shutdown.rs`'s other `wait_bounded(…6)` call site (`:187`) is checked for the same
      margin; if it passes the same bound to the same work, it is fixed in the same change or the
      difference is stated.

## Parent

Slice issue 93 (capture-schema slice) sequencing is unrelated; this is a gate-integrity defect surfaced
by the batched review, and belongs to no slice.

## Comments

**The evidential lesson, not the timing one.** The coder's `0 failed` was true when it ran and false when
re-run — not because the code changed, but because the machine did. A consolidated single workspace run
invites a single summary line that reads as a property of the code. Any future wave gate that reports
`N passed / 0 failed` should record whether a second run was attempted, or the machine state, or simply
say "one run". This is why the reviewer's APPROVED verdict stands with this filed as follow-up rather
than a rejection: the wave did not cause it, and the property under review was independently re-derived.
