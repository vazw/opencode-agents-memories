# 110 — the capture inbox has no scheduling ("not before") column, so a long retry hint can only dead-letter

Status: done — 2026-11-02 — landed in `11cfc0f` (blueprint `capture-inbox-not-before-r5-20261102-1620`,
sha `b7dd722d575e`): a nullable `capture_inbox.not_before` (episodic schema v7) with the drain selecting
`status = 'queued' AND (not_before IS NULL OR not_before <= now)`; any positive `retryAfterMs` schedules the
next attempt and consumes no attempt (a `0` hint is hintless and takes the ladder — review R-1), and
`HOST_RETRY_AFTER_CEILING_MS` is deleted (ADR-0029 D-29-12).
Review 1 REJECTED (R-1/R-2/R-3); the rework landed in `de6661c` — see `## Comments`. This line still claims
the landing, not an APPROVED verdict.

Filed by issue 108's blueprint (stateless-capture wave) as a TRACKED deferral: the behaviour it names is
implemented and deliberate, but the missing column is a design gap, not a settled choice.

## Ruling (owner, 2026-11-02)

Verbatim: *"Retire the ceiling, add not_before."* The rule below is the binding consequence.

The drain selects `status = 'queued' AND (not_before IS NULL OR not_before <= now)`. A failure that
carries a retry hint writes `not_before = now + retryAfterMs` and stays `queued`, **whatever the hint's
magnitude** — `HOST_RETRY_AFTER_CEILING_MS` stops being a dead-letter trigger. Dead-lettering remains
the outcome for failure classes that carry no retry hint (`record_failure`'s other branches), so a
permanent failure is still not retried forever.

Consequences the implementing wave must state and test:

- the ceiling constant loses its dead-letter role — say whether it is deleted or retained for another
  purpose, and do not leave it half-used;
- a `retryAfterMs: 120000` 502 flips from `dead` (attempts 0) to `queued` with `not_before` ≈ now+120 s —
  the existing `test_108_502_above_the_ceiling_dead_letters` pins the old behaviour and must be
  **replaced, not deleted**;
- clock source: the drain's `now` and the writer's `now + retryAfterMs` must come from one source, or a
  test using a fake clock will disagree with a row written by the real one.

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

## Comments

**2026-11-02 — review 1 rework (R-1, R-2, R-3) — commit `de6661c`.**

*R-1 (behavioural).* `record_failure` routed ANY `Some(ms)` to `schedule_retry`, and `not_before = now + 0` is
due at once, so a `retryAfterMs: 0` row consumed no attempt, never reached the five-attempt ladder, and was
retried once per drain for the life of the row (`attempts` pinned at 0, so nothing distinguished it from a
healthy row). The input is contract-reachable — `ProviderRequestErrorEncoded.retryAfterMs` declares no positive
minimum — and negatives were already dropped by the decoder (`Value::as_u64`), so `0` was the only non-positive
value that reached the scheduling branch.

Disposition: **reviewer option (i), a zero hint is treated as hintless.** MEASURED RED before the fix
(`cargo test -p mem-bridge --test capture_failure_classes`): `test_110_zero_hint_is_hintless_and_charges_the_
ladder` failed with `a zero hint orders no deferral, so the ladder charges the attempt / left: 0 / right: 1`
and, with the two assertions swapped for one observation, `a zero hint must never stamp a schedule / left:
Some(1000000) / right: None`; `retryable: 1` and `dead_lettered: 0` held on every drain. The fix is one
predicate (`retry_after_ms > 0` on branch 2); the decoder is untouched, so a `0` stays visible in the failure
payloads as the diagnostic for this class.

Measured blast radius: **`0` only.** `1`, `1500` and `120000` still schedule with `attempts` unchanged
(`capture_failure_classes` 8/8, `capture_partial_failure` 5/5, `capture_structured` 13/13,
`capture_wrapper_payload` 4/4). New rows: `test_110_zero_hint_is_hintless_and_charges_the_ladder` (the gate)
and `test_110_one_ms_hint_is_still_a_schedule` (the boundary guard against widening the guard into a floor
constant — a floor would be an arbitrary magnitude rule, the ceiling's mirror image, and the drain has no fixed
cadence to anchor one).

*R-2 (claim accuracy).* `crates/mem-episodic/src/inbox.rs` claimed a second time source "needs a visible
import". False in Rust — a fully-qualified `crate::ids::now_millis()` needs no `use`. The invariant holds and is
test-backed by the two fake-clock rows, but it is CONVENTIONAL, not compiler-enforced; the shipped comment now
says exactly that.

*R-3 (stale comment).* `crates/mem-bridge/tests/capture_wrapper_payload.rs` said the batch gates on
`status = 'queued'` alone. It now names the `not_before` arm and why that fixture is unaffected (its summarizer
always succeeds, so its only failures carry no hint and never stamp `not_before`).

*Blast radius of the wave as a whole:* no schema change, no new constant, no once-named-constant reappearance
(`grep -rn HOST_RETRY_AFTER_CEILING_MS crates/` empty; the `docs/adr` hit count stays exactly one, inside
D-29-12).
