# 61 — `mem-web` · a degraded build answers **500** where the contract says **501**, because `port_failure` ignores the error class

Status: ready-for-agent — 2026-10-27 (premise measured at HEAD `ffa4898`; 49 call sites, one mapping
function, and a reachable production degrade path — brief at the end of this file).

## PREMISE — measured, not inherited

`crates/mem-web/src/api/mod.rs`:

```rust
pub fn port_failure(endpoint: &'static str, error: ports::PortError) -> Response {
    tracing::error!(endpoint, error = %error, "api handler failed");
    envelope::internal_error()          // ← 500, for EVERY variant
}
```

`PortError` has four variants, and only one of them is an internal fault:

| variant | meaning (from its own doc comment) | `port_failure` answers |
|---|---|---|
| `Unavailable` | *"The endpoint's service is not wired in this build."* | **500** |
| `NotFound` | the addressed row does not exist | **500** if it reaches here |
| `Invalid(String)` | an upstream business-rule rejection, verbatim message | **500** if it reaches here |
| `Store(String)` | a store/port failure; detail logged, never returned | **500** — correct |

The module already defines the right constant for the absent-service case — `pub const STUB_STATUS:
StatusCode = StatusCode::NOT_IMPLEMENTED` (501) — but at the time of filing it had **zero call sites**: the
explicit stubs call `envelope::not_implemented`, which hardcodes the same 501. (Corrected 2026-10-27 by the
Plan Gate, which measured `grep -rn STUB_STATUS` over tracked sources: definition only, no consumers. An
earlier draft of this file claimed "the explicit stubs use it" — that was false.) The generic failure path
does not classify at all.

**Blast radius:** 49 call sites of `port_failure(...)` / `skill_failure(...)` across the handler modules
(`memories`, `maintenance`, and the rest); `memories` alone contributes 15 in its GET path.

**Reachable in production, not theoretical:** `src/main.rs` has

```rust
None => Arc::new(ApiServices::unavailable().with_skills(skills_port)),
```

so a build with no composed deps serves real routes whose ports answer `Unavailable`. Every one of those
endpoints then answers **500** with the internal-error envelope, which is precisely the conflation the
contract forbids: 501 means *"this service is absent in this build"* and 500 means *"something broke"*. A
client cannot tell "not supported here" from "broken here", and a log reader sees `tracing::error!` on a
perfectly ordinary degraded build.

## WHY IT MATTERS

- **A documented contract is false.** Issue 37's AC4 exists to write down the 501/500 distinction for UI
  authors; writing it down while the code answers 500 for every absent port would document the opposite of
  the behaviour.
- **It pollutes failure signal.** A degraded build logs `error`-level lines on every request, which is the
  same class of defect this repo already files under "a guard that cannot fire" — the alarm is always on.
- **It misleads the freshness checks.** Any smoke or parity table that asserts "the documented 501" cannot
  pass against a degraded build, so the row gets recorded as a failure that isn't one.

## WHAT A WAVE SHOULD PRODUCE

1. `port_failure` classifies: `Unavailable` → the 501 stub envelope the explicit stubs themselves answer (the
   status `STUB_STATUS` names) with the documented body; `Store` →
   500 with the detail logged and never returned. `NotFound` / `Invalid` must keep whatever behaviour they
   have today — they already have per-handler handling, and this issue is not a licence to redesign the
   error surface. **Measure the current mapping of those two before changing anything**, and change only
   `Unavailable` unless a measurement shows another variant is also wrong.
2. A test per variant on the shared path, not per handler: one for `Unavailable` → 501, one for `Store` →
   500 that also asserts the body carries no internal detail, and a control that the two are distinguishable
   by status alone.
3. A degraded-build oracle: build `ApiServices::unavailable()`, issue one store-backed request, and assert
   **501**, not 500. The inversion arm: mapping `Unavailable` back to 500 must fail that test.
4. No change to the explicit stubs' existing 501 behaviour, and no change to the auth gate ordering.

## SCOPE CEILING

`crates/mem-web/src/api/mod.rs` (the mapping function), the handler modules **only if** a call site bypasses
`port_failure` for `Unavailable` (measure first), and test files. **Not** `docs/bridge-schema.md` — the
contract statement is issue 37's AC4 and writing it here would collide with that wave. No production
behaviour change beyond the status code and its envelope; no new dependency; no manifest change.

---

## Agent Brief

**Category:** bug
**Summary:** a build whose services are absent answers 500 instead of the documented 501, because the shared
failure helper ignores the error class.

**Current behavior (verified at HEAD `ffa4898`):**
`port_failure` maps every `PortError` variant to the internal-error envelope. `PortError::Unavailable` means
"the endpoint's service is not wired in this build", and `STUB_STATUS` (501) already exists for exactly that
case, used by the explicit stubs. 49 call sites funnel through the shared helper, and `src/main.rs` has a
reachable `ApiServices::unavailable()` degrade path, so a build with no composed deps serves 500 on every
store-backed route and logs an `error` line per request.

**Desired behavior:**
`Unavailable` answers **501** with the documented envelope, and a store failure still answers **500** with no
internal detail in the body. The two are distinguishable by status alone. `NotFound` and `Invalid` keep their
current behaviour unless a measurement shows they are also mis-mapped — this issue fixes the class confusion
for the absent-service case, not the whole error surface.

**Key interfaces:**
- `PortError` — the four variants and their documented meanings.
- `port_failure(endpoint, error)` — the single shared mapping; `skill_failure` delegates to it for everything
  except `Invalid`.
- `STUB_STATUS` — the existing 501 constant. It has no call sites today (the stubs hardcode via
  `envelope::not_implemented`), so bind the tests to it rather than adding another literal, and do not assume
  the stubs already depend on it.
- The envelope helpers (`json`, `error`, `internal_error`) — the degraded response must use the same success
  shape as the stubs, not a new one.

**Acceptance criteria:**
- [ ] `PortError::Unavailable` through the shared path answers **501** with the documented envelope.
- [ ] `PortError::Store` answers **500**, and the body carries no internal detail.
- [ ] A test asserts the two are distinguishable by status, and one control asserts the mapping is in the
      shared helper rather than duplicated per handler.
- [ ] A degraded-build oracle (`ApiServices::unavailable()`) issues one store-backed request and observes 501;
      the inversion arm (mapping `Unavailable` back to 500) fails that test.
- [ ] `NotFound` and `Invalid` behaviour is unchanged, with the measurement that justifies leaving them
      recorded.
- [ ] No behaviour change to the explicit stubs, the auth gate, or any route's payload.

**Out of scope:**
- `docs/bridge-schema.md` — the 501/500 contract statement is issue 37's AC4.
- Redesigning the error surface, or adding variants to `PortError`.
- The composition-root wiring itself (issue 37 · AC6).

## Comments

- **2026-10-27 — Plan Gate REJECTED (revision 1 of 2).** Blueprint
  `20261026-2223-issue-61-degraded-build-status` (STAMP `70c6877c54db`) was rejected for five mechanical
  reasons, all correctable without redesign — the gate called the design, the six oracles and the scope
  boundary sound:

  1. The Step-2 inventory command matched only one of the four `Unavailable` arms, because three are written
     `Err(PortError::Unavailable) =>`; the expectation must be four lines, named.
  2. The Step-6 gate used `git diff --stat`, which cannot see the untracked new test file; it must be
     `git status --porcelain` or `git add -N` first.
  3. Step 7's "passed count equals N + 6" is wrong: `--list` counts an `#[ignore]`d test that never passes,
     so the passed sum is N + 5 while the delta is +6.
  4. Step 1 claimed `git worktree list` would still show the issue-59 reviewer scratch tree; at `b46bcce` it
     shows only the main worktree (that path is an unregistered leftover directory, already gone).
  5. D5's rationale mis-cited D-11: the two skills catalog `Unavailable` arms use `envelope::internal_error()`,
     which is *inside* the envelope. What D-11 governs is the catalog token posture. The arms stay 500 because
     they mirror upstream, not because they sit outside the envelope.

  This file was also corrected: the gate measured `grep -rn STUB_STATUS` over tracked sources and found the
  **definition only, zero call sites** — the explicit stubs call `envelope::not_implemented`, which hardcodes
  501. The earlier claim that "the explicit stubs use it" was false and is now replaced with the measurement.
