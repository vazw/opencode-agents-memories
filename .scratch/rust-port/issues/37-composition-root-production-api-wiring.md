# 37 — W6.2 follow-up · Wire the production API services into the composition root

Status: open — re-scoped to AC4 only (2026-10-21)

**Scheduling:** ABSORBED into W6.7 (issue 31), by user decision of 2026-10-21. The W6.7 Plan Gate
accepted the absorption in principle, conditioned on ten corrections, and the user confirmed it
directly (decision record `mem_1792565360736_79j2b80y6`).

**RE-SCOPED BY W6.7's DECISION 10 — this issue now carries AC4 ONLY.** W6.7's absorption satisfies
AC1, AC2, AC3 and AC5 of this issue (the composition root calls `ApiServices::production(...)`, the
endpoints answer 200 against the live binary, the no-edge guard holds, and the oracle runs the
binary's own wiring path). What remains is **AC4**: the `docs/bridge-schema.md` statement of the
local-token header shape (`x-opencode-mem-token`, not `Basic`, not `Bearer`) and the 500-vs-501
distinction. W6.7's verification sweep stays a PURE gate and does NOT fold that doc edit in — so
AC4 lands here, in its own small lane, once W6.7 closes.

**W6.7 CLOSED 2026-10-21 — AC1, AC2, AC3 and AC5 are LANDED and ticked below.** W6.7 (issue 31)
went through the fleet pipeline — two Plan Gate rounds (r0 rejected on ten modifications, r1
approved), then a coder, then a reviewer who re-executed the whole test plan independently and
approved it. The live binary now serves the four store-backed endpoints 200 through the production
wiring. **This issue therefore stays open for AC4 alone, and it is now the only thing standing
between it and closure.** Close it when the `docs/bridge-schema.md` sentence exists — do not close
it on W6.7's commit, and do not close it with AC4 still unticked.

**The re-scope is why this issue stays open rather than closing with W6.7.** Do not close it on
W6.7's commit; close it when the `docs/bridge-schema.md` sentence exists.

Named here because the W6.7 planner's live probe found it on 2026-10-21, and at the time it was
owned by nobody: issue 24 is marked closed, and issue 31 lists it as a closed dependency.

**Two corrections the Plan Gate forced, which change this issue's cost.** (1) `mem-bridge` may never
depend on `mem-web` — `crates/mem-bridge/Cargo.toml` has no such dependency, `engine.rs:599` says so
explicitly, and `docs/adr/0007-bridge-strategy.md:584` forbids a new `mem-*` dependency edge (which
issue 31's AC4 also forbids). The ROOT crate becomes the composition root for the mem-web surface.
(2) `UserPromptManager` and `ProfileStore` must be constructed for the FIRST TIME — nothing in
`mem-bridge` or `src` opens them today, so there is no duplicate-open bug, but there is a real cost:
two new stores per engine start.

**Why this exists.** The W6.2 REST surface (issue 24) is complete and tested at the crate level:
`crates/mem-web/src/api/` has the routers, the DTOs, the store adapter and a production
implementation. But the binary never asks for the production implementation. The composition root
still hands mem-web the stub:

- `src/main.rs:118` — `api: Arc::new(mem_web::api::ports::ApiServices::unavailable())`.
- `crates/mem-web/src/api/ports.rs:538` — `unavailable()` gives every port an `UnavailablePort`, so
  every port answers `PortError::Unavailable`.
- `crates/mem-web/src/api/ports.rs:564` — `ApiServices::production(deps: ProductionDeps)` is the real
  wiring, and its own doc comment calls it *"the W5.x composition root's entry point"*. That
  obligation was recorded in a doc comment and never discharged.
- The ONLY call to `ApiServices::production` in the whole tree is mem-web's own integration test,
  `crates/mem-web/tests/turso_api_scope.rs:82`. No code path from `src/main.rs` reaches it.

**Consequence — every store-backed REST endpoint 500s in the live engine.** Observed on
2026-10-21 against a standalone engine started with `--web-dist` pointing at a built `web-ui/dist`
(port 41823, local token read from the storage root's `.auth-token`):

| request | result |
|---|---|
| `GET /api/health` | 200 `{"success":true,"status":"ok","authEnabled":false}` |
| `GET /` | 200 `text/html`, the Trunk shell (`web-ui-647f64e8736b1358.js`) |
| `GET /api/memories` | 500 `{"success":false,"error":"Internal Server Error"}` |
| `GET /api/tags` | 500 same |
| `GET /api/user-profile` | 500 same |
| `GET /api/stats` | 500 same |
| `GET /api/skills` | 500 same |
| `GET /api/search` (no query) | 200 `{"success":false,"error":"query parameter required"}` |

`/api/search` and `/api/health` reach their handlers; every endpoint that touches the store does
not. The crate-level tests pass because they build `ApiServices::production(...)` themselves, so the
gap is invisible to `cargo test`.

**Consequence for the waves behind it.** W6.7 (issue 31) has an acceptance criterion that all four
screens render LIVE data against the Rust server. With the stub wired, none of them can: their
three data endpoints and the config endpoint all 500. W6.4 (issue 26) has the same problem, and
additionally: `ApiServices::production` deliberately leaves `skills`, `maintenance` and `sessions` on
`UnavailablePort` — skills until W6.4, maintenance until the W5.x readiness gate supplies a
`MigrationService`, sessions until the composition root calls
`.with_sessions(episodic_session_port(store))`. The `with_skills` / `with_sessions` /
`with_maintenance` builders are the injection points for those.

**Auth shape, for whoever builds the UI fetch seam** (recorded here because it is easy to get wrong
and the live server gives no hint): the LOCAL token travels in the `x-opencode-mem-token` header
(`crate::AUTH_HEADER`), NOT as `Authorization: Basic …` and NOT as `Authorization: Bearer …` —
Bearer authenticates the CONFIGURED `api_token`, which is `None` in standalone mode. Sending
`Basic <raw-token>` gets 401; sending `Bearer <raw-token>` gets 401; sending
`x-opencode-mem-token: <raw-token>` passes the gate and reaches the handler. Source:
`crates/mem-web/src/api_auth.rs:46-87` and the gate order in `crates/mem-web/src/gate.rs:30-105`.

## What to build

1. **Construct the production deps in the composition root.** Build `ProductionDeps` from the
   `ShardManager` the engine already opens, plus `UserPromptManager` and `ProfileStore`, which the
   engine does **not** open today and which this work must construct for the FIRST TIME, and pass
   the engine's `EmbeddingService` as the embed port.
   **CORRECTED 2026-10-21 — an earlier draft of this issue said the engine "already opens" all
   three.** It does not: `UserPromptManager::new` (`crates/mem-profile/src/prompt.rs:67`) and
   `ProfileStore::new` (`crates/mem-profile/src/profile_store.rs:373`) occur only in mem-profile's
   and mem-web's tests. So there is **no** duplicate-open bug to reconcile — but there IS a real
   cost: two new stores per engine start, each with its own database file in the storage root.
2. **Call `ApiServices::production(deps)`** instead of `unavailable()`, then apply the injection
   builders the wave needs: `.with_skills(...)` for W6.4's skill surface, `.with_sessions(...)` with
   `episodic_session_port(store)` for the episodic tier, `.with_maintenance(...)` once the readiness
   gate supplies a `MigrationService`.
3. **Cover the wiring.** A test that starts the real composition root and asserts a store-backed
   endpoint answers 200 rather than 500 — the crate-level test cannot see this gap, so the oracle
   has to run the binary's own wiring path.

## Acceptance criteria

- [x] `src/main.rs` no longer calls `ApiServices::unavailable()`; the composition root builds
      `ApiServices::production(...)` over the real stores.
      — **LANDED with W6.7 (issue 31), 2026-10-21.** The root crate is the mem-web composition root:
      it builds `EmbeddingService` + `ApiServices::production(ProductionDeps{..})` from a
      mem-bridge-owned `WebStoreDeps`. The degrade path is "no deps → `unavailable()`".
- [x] `GET /api/memories`, `/api/tags` and `/api/user-profile` answer 200 against a live standalone
      engine with the local token in the `x-opencode-mem-token` header.
      — **LANDED with W6.7, 2026-10-21.** Measured live: token-less 401 (which lazily creates
      `.auth-token`), then 200 for `/api/memories`, `/api/tags`, `/api/user-profile`, `/api/stats`.
- [x] The endpoints that are unavailable BY DESIGN answer the documented 501 and not a 500 — the two
      are different failures and must not be conflated.
      **CORRECTED 2026-10-21 — the endpoint set in this criterion was wrong.** Measured against the
      live binary with the production wiring in place (W6.7): `sessions` answers 501
      (`crates/mem-web/src/api/sessions.rs:34`) and cleanup/deduplicate answer 501
      (`crates/mem-web/src/api/maintenance.rs:221/229`), but **`skills` answers 500** — `skill_failure`
      funnels `PortError::Unavailable` through `port_failure` into `internal_error`
      (`crates/mem-web/src/api/skills.rs`). So skills is a *conflation* defect in mem-web, not a
      by-design 501. The criterion stands for `sessions` and `maintenance`; skills needs either a
      mem-web fix (out of this issue's scope) or an explicit, documented exception.
      — **LANDED with W6.7 for `sessions` and `maintenance` (both measured 501).** The skills case is
      carried into AC4 below, because a `docs/bridge-schema.md` that says "skills is 501" would be
      false, and because a doc statement is the cheapest place to record the measured truth.
- [x] An oracle exists that runs the composition root's own wiring path and fails on the
      `unavailable()` stub, so the gap cannot silently return.
      — **LANDED with W6.7, 2026-10-21.** `tests/production_api_wiring.rs` spawns the real binary,
      seeds one row through a real `ShardManager` (own runtime, dropped before the spawn), acquires
      the token the way a client does (token-less 401 → bounded poll → token-bearing 200) and asserts
      the seeded row. It went RED first on the stub's `500 {"success":false,"error":"Internal Server
      Error"}`. A second oracle, `test_no_mem_bridge_to_mem_web_edge`, keeps the forbidden
      `mem-bridge`→`mem-web` edge absent (ADR-0007:584).
- [ ] `docs/bridge-schema.md` states the local-token header shape (`x-opencode-mem-token`) and the
      500-vs-501 distinction — **with the skills case stated as measured above, not as the old
      "skills/maintenance/sessions are all 501" claim.** The W6.7 blueprint's Finding 4 and the
      W6.7 Plan Gate's "verified sound" list both carried that wrong claim; it was measured, not
      inferred, and it is the reason this AC is the last thing standing. **This is the whole of
      what remains in issue 37.** The sentence must also say which endpoints answer 501 by design
      (`sessions`, `maintenance`) and which answer 500 (`skills`), and either name the skills
      handler's conflation as a mem-web defect to fix or record the exception explicitly.

## Blocked by

- 24 · W6.2 (the REST surface and the production adapter this wires) — closed as a crate, but the
  composition-root half was never done.
- 29 · W6.5 (the shell whose fetch seam needs the token shape) — closed.

## Comments

- Discovered 2026-10-21 by the W6.7 planner's live probe, which died mid-probe on an upstream
  endpoint failure and left the engine running; the orphan was killed and the observations above
  were taken from the live server before the cleanup.
- Not a W6.8 regression: W6.8's composition-root wire is correct and provably so — it resolved the
  dist, the server bound, `/` served the built shell and `/api/health` answered 200. The gap is
  orthogonal and one layer down, in the API services the same composition root hands to mem-web.
