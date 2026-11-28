# 175 — the profile learner is a singleton, so it can only ever analyze one identity

Status: **needs-info** — blocked on 174's schema ruling and the same multi-identity scope question.
`ready-for-agent` once 174 lands and the registry shape (A/B below) is chosen.
Filed: 2026-11-28, with 174/176/177. No code changed.

## The defect

There is exactly one learner. `build_profile_learning` returns one `ProfileLearning` holding one
round and one `user_id` (`crates/mem-bridge/src/profile_learning.rs:338-386`), stored once at
`ComposedRuntime.learning` (`crates/mem-bridge/src/composition.rs:376-390,413`) and driven by one tick
(`crates/mem-bridge/src/engine.rs:1548-1575`). The round fetches and writes the profile under
`self.user_id` (`crates/mem-profile/src/learning_service.rs:360,450,478-486`).

After 174, the ledger can filter by identity — but a single learner still runs every round as the
launch-cwd identity. The due path cannot route: `arm` is keyed by `session_id` alone
(`profile_learning.rs:232-235`), and `run_due` explicitly drops it —
`for (_session_id, jobs) in due` (`profile_learning.rs:256`). So an identity-scoped select on a
singleton learner would starve every identity except the one it was built for.

The record seam already knows the answer and throws it away: `record` receives the capture's
`session_id` (`profile_learning.rs:171-226`) but never associates a session with an identity.

## Impact

With 174 alone: prompts from identity B are either filtered out forever (if the filter is applied to
the singleton, whose identity is A) or still pooled into A (if not). Either way the multi-identity host
`identity.rs:7-10` describes is not served. On a genuinely single-identity store this is a no-op — the
defect is latent until a second identity exists.

## Options

**A — one learner per identity, built lazily on first `record`.** A registry
`HashMap<String, Arc<ProfileLearning>>` (or `Arc<Mutex<…>>`) on the runtime; each learner owns its own
round and `IdleScheduler`. `arm` records `session_id → identity` so `run_due` can be driven per
learner. Invariant: identity is a constructor input, never re-derived mid-round.

**B — one learner, identity resolved per due session at round start.** Cheaper, but identity is not on
the scheduler's due tuple today, so it must be threaded through `arm`/`run_due` anyway — and a
mid-round identity decision is the same seam that produced this bug.

**Discriminating measurement.** Record one prompt under identity A and one under B; arm both so they
fall due in the same tick. Each profile's `total_prompts_analyzed` must reflect **only** its own
prompt, and both rounds must report `Completed`. Under A two learners run their own rounds. Under B,
or the current singleton, one profile absorbs both or the second identity never runs — the assertion
reds.

## Acceptance criteria

1. `ComposedRuntime` exposes the learner registry (or the chosen per-identity equivalent), and the tick
   drives every due identity, not one.
2. `record`/`arm` associate the session with the identity resolved for that capture — the same
   `resolve_user_id_with` precedence, not `com.current_dir()` re-derived per tick.
3. The two-identity-same-tick test above exists and is non-vacuous: **delete the session→identity
   routing and one identity's prompt is attributed to the other (or zero rounds complete), and the
   test reds.** State that in the test comment.
4. Single-identity behaviour is unchanged: `test_composition_passes_an_explicit_identity`
   (`crates/mem-bridge/tests/profile_learning_wiring.rs:605`) and
   `test_unknown_identity_builds_no_learner` (`:543`) stay green (adjust signatures only as 174 forces).
5. `test_learning_canary_never_enters_any_log_channel` (`profile_learning_wiring.rs:772`) stays green:
   no prompt text or identity-bearing id enters a log channel, including the new per-identity events.
6. `cargo test -p mem-bridge -p mem-profile` green; no count or duration pinned.

## Inverted world — what would close this as "not a defect"

The engine is provably single-identity: `identity.rs:7-10`'s "two identities" sentence is struck and a
gate asserts one identity per process (`resolve_user_id_with` constant across all call sites and
directories). Neither holds today; `user_email_override` and per-directory resolution both break it.

## Dependencies

174 (the ledger must carry identity before a round can be scoped to one).

## Out of scope

Redesigning the idle scheduler; per-project profiles; the web surface's own identity resolution.

## Comments

- 2026-11-28 — filed from the learner review. `needs-info`: the registry shape (A/B) and whether
  multi-identity is in scope are the owner's ruling, and 174 gates it. Read-only evidence; no live
  store touched.
