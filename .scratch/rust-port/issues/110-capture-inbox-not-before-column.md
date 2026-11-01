# 110 — the capture inbox has no scheduling ("not before") column, so a long retry hint can only dead-letter

Status: needs-triage

Filed by issue 108's blueprint (stateless-capture wave) as a TRACKED deferral: the behaviour it names is
implemented and deliberate, but the missing column is a design gap, not a settled choice.

## Evidence

- `crates/mem-bridge/src/capture_prompt.rs` — `HOST_RETRY_AFTER_CEILING_MS = 30_000`.
- `crates/mem-bridge/src/capture.rs` — `record_failure` dead-letters with NO attempt bump when
  `failure.retry_after_ms > HOST_RETRY_AFTER_CEILING_MS` (`capture.rs`, branch 2). Deliberate: the drain
  path must not `sleep` behind one outage, and `EntryOutcome::Deferred` consumes no attempt and has no
  scheduling primitive.
- `crates/mem-store/src/episodic_sql.rs` — `SELECT_CAPTURE_INBOX_BATCH` selects `WHERE status = 'queued'`,
  so a dead row never re-enters the drain. Nothing in the tree requeues it.
- `DEADLETTER_CAPTURE_INBOX` is an `UPDATE … status='dead'`, so the row and its `payload` survive.

MEASURED by the wave's gate: `cargo test -p mem-bridge --test capture_failure_classes` →
`test_108_502_above_the_ceiling_dead_letters` asserts `dead_lettered == 1` and `attempts == 0` for a
`retryAfterMs: 120000` 502.

## Consequence

A host that answers `retryAfterMs > 30 s` (a rate limit or an overload lasting minutes) permanently removes
that capture from the retry path. The payload is retained, so a future requeue tool can recover it by hand —
but nothing requeues automatically, and the capture is invisible to the user.

## Shape of the fix (not decided here)

Add a nullable `not_before` (Unix ms or an ISO timestamp) to the capture inbox and make the drain select
`status = 'queued' AND (not_before IS NULL OR not_before <= now)`, then let a `502` with any
`retryAfterMs` set it instead of dead-lettering. That also retires the 30 s ceiling, which the owner should
adjudicate: it is a policy choice, not a measurement.

## Parent

Issue 108 (capture should use the host's stateless generate route), ADR-0029 D-29-10/D-29-11.
