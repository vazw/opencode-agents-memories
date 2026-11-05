# 37 — W6.2 follow-up · Wire the production API services into the composition root

Status: ready-for-agent — 2026-11-04: AC4 + AC6 landed and verified (see the ## Comments entry of the same date; implementation commit c03338843b72b3b29f044ff9bb18c462a8a10b26), open pending docs/web-skeleton-smoke.md:422, :326-328 and :505-506 — three residual sites still describe the pre-wiring world: the D-13 row (:422) and the §F prose (:326-328) say the production wiring answers the sessions 501 "by design" today, and route-sheet rows 36–37 (:505-506) say `.with_sessions(episodic_session_port(...))` is "not wired"; all are falsified by this wave, the smoke doc is issue 65's file and was deliberately not edited, and issue 65's delivery items are scoped to the skills rows (:498-504) and docs/agents/testing.md, so this issue owns all three corrections and does not close on them.
AC6 = sessions wiring ONLY — the maintenance half moved to issue 39. Anchors are symbol-precise,
line numbers demoted to hints. See "2026-10-25 re-scope" below.)

## 2026-10-25 re-scope — four decisions from the grilling pass

Measured at `065ffdb` before deciding; two of this issue's premises were false.

1. **The `skills` 500 claim is stale and is withdrawn.** `src/main.rs` calls
   `.with_skills(skills_port)` on **both** the production and the degrade branch (landed with W6.4),
   so `/api/skills` is wired and answers from a real port. There is no skills conflation to record,
   and AC4 must not assert one. The conflation that *does* exist is **`/api/migration/*`**:
   `maintenance_failure` funnels `PortError::Unavailable` through `port_failure` →
   `internal_error()` = **500**, where `sessions` correctly answers **501**. That finding is carried
   to issue 39, not fixed here.
2. **AC4 becomes contract-only — no endpoint inventory.** `docs/bridge-schema.md` states the token
   shape and the `501` (service absent in this build) vs `500` (internal fault) *mechanism*, and
   explicitly does **not** enumerate per-endpoint availability, with one sentence saying why: that
   inventory tracks the build, so it belongs in release notes. Rationale: an inventory written now
   is falsified the moment issue 39 lands, and a doc asserting a volatile measurement is a gate that
   fails for the wrong reason.
3. **AC6 keeps the sessions half only; maintenance moves to issue 39.** `cleanup()` and
   `deduplicate()` take **no `State`** — they are unconditional `envelope::not_implemented(...)`
   stubs (D-15), so no `MaintenancePort` can reach them. Making all four maintenance routes real is
   two services (retention cleanup, memory deduplication), filed as issues 39 and 40.
4. **Anchors are symbol-precise; line numbers are hints.** Every line reference in the original AC6
   had drifted (`WebStoreDeps` 563→**593**, the fill site 853→**938**, `ComposedRuntime` is in
   `composition.rs` not `engine.rs`). Instructions now name the symbol and quote one context line;
   any line number is written `≈ N at HEAD 065ffdb`. **Start execution by re-locating each symbol
   and recording its actual line** — a stale hint must fail loudly, not point at the wrong code.

**Scheduling:** ABSORBED into W6.7 (issue 31), by user decision of 2026-10-21. The W6.7 Plan Gate
accepted the absorption in principle, conditioned on ten corrections, and the user confirmed it
directly (decision record `mem_1792565360736_79j2b80y6`).

**RE-SCOPED BY W6.7's DECISION 10 — this issue now carries AC4 and AC6.** W6.7's absorption
landed AC1, AC2, AC3 and the oracle criterion (listed as AC5) of this issue (the composition root
calls `ApiServices::production(...)`, the endpoints answer 200 against the live binary, the no-edge
guard holds, and the oracle runs the binary's own wiring path). What remains is **AC4**: the
`docs/bridge-schema.md` statement of the local-token header shape (`x-opencode-mem-token`, not
`Basic`, not `Bearer`) and the 500-vs-501 distinction, and the new **AC6**: the sessions and
maintenance port wiring. W6.7's verification sweep stays a PURE gate and does NOT fold the doc edit
in; both land here.

**W6.7 CLOSED 2026-10-21 — AC1, AC2, AC3 and the oracle criterion (listed as AC5) are LANDED and
ticked below.** W6.7 (issue 31) went through the fleet pipeline — two Plan Gate rounds (r0 rejected
on ten modifications, r1 approved), then a coder, then a reviewer who re-executed the whole test
plan independently and approved it. The live binary now serves the four store-backed endpoints 200
through the production wiring. **This issue stays open for AC4 and AC6.** Close it when the
`docs/bridge-schema.md` sentence exists **and** AC6 is ticked — do not close it on W6.7's commit, and
do not close it with AC4 or AC6 unticked.

**This issue stays open rather than closing with W6.7 for two reasons — AC4 and AC6.** Do not close
it on W6.7's commit; close it when the `docs/bridge-schema.md` sentence exists **and** AC6 is
ticked.

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
- [x] **AC4 (re-scoped 2026-10-25, contract-only)** — `docs/bridge-schema.md` states, as contract
      rather than inventory:
      1. the local-token header shape: `x-opencode-mem-token` (`crate::AUTH_HEADER`), and that
         neither `Authorization: Basic <token>` nor `Authorization: Bearer <token>` works — Bearer
         authenticates the *configured* `api_token`, which is `None` in standalone mode, so both
         shapes get 401 and only the custom header passes the gate. This is the fact a UI fetch seam
         author needs and cannot infer from the live server.
      2. the failure distinction: **501** = the endpoint's service is absent *in this build* (stable
         contract semantics, D-13/D-15), **500** = internal fault. A client must not conflate them.
      3. an explicit statement that per-endpoint availability is **deliberately not enumerated**
         here, because it tracks the build and belongs in release notes.
      **Do not write an endpoint status table.** An earlier draft of this AC would have recorded
      "skills answers 500", which is false since W6.4 wired the skills port — and any table written
      now is invalidated by issue 39. Verify the three statements against
      `crates/mem-web/src/api_auth.rs`, the gate order in `crates/mem-web/src/gate.rs`, and
      `crates/mem-web/src/api/maintenance.rs` before writing.

- [x] **AC6 (re-scoped 2026-10-25, sessions only)** — wire the sessions port at the composition root.
      Anchors are symbols; line numbers are hints at HEAD `065ffdb` and **must be re-located before
      use**.

      1. Add `episodic: Arc<EpisodicStore>` to `WebStoreDeps` (`crates/mem-bridge/src/engine.rs`,
         `pub struct WebStoreDeps` ≈ `:593`; the struct's existing fields are `shards`, `prompts`,
         `profile`, `embedder`). Fill it at the single construction site — the `composed.as_ref()
         .map(|runtime| WebStoreDeps { … })` expression in the same file ≈ `:938` — from
         `ComposedRuntime.store`, which already exists (`crates/mem-bridge/src/composition.rs`,
         `pub struct ComposedRuntime`, field `pub store: Arc<EpisodicStore>`).
      2. Call `.with_sessions(episodic_session_port(deps.episodic))` in the composition root
         (`src/main.rs`, the `ApiServices::production(ProductionDeps { … })` chain, ≈ `:159-166`;
         the builder is `crates/mem-web/src/api/store_adapter.rs`, `pub fn episodic_session_port`).
         `.with_skills(...)` is already called there — follow that shape, and keep the degrade branch
         (`ApiServices::unavailable()`) unchanged: sessions stay 501 when there is no composition.
      3. Add an oracle in `tests/production_api_wiring.rs` asserting the **two** session routes
         (`GET /api/projects/{project_id}/sessions`, `DELETE /api/projects/{project_id}/sessions/
         {session_id}`) answer **200** after seeding a row, and that they answer **501** on the
         degrade path. Nothing asserts today's 501, so without both directions the change lands
         unproven — mirror the existing file's token-acquisition pattern rather than inventing one.

      **Maintenance is NOT in this AC.** It moved to issue 39 because `cleanup()` and
      `deduplicate()` take no `State` and are unconditional 501 stubs (D-15), so a
      `MaintenancePort` wiring cannot reach them.

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
- **2026-10-25 — AC6's premise about `deduplicate()` is superseded (issue 51.2).** AC6 reads
  "`cleanup()` and `deduplicate()` take no `State` and are unconditional 501 stubs (D-15), so a
  `MaintenancePort` wiring cannot reach them". The `deduplicate` half stopped being true when issue 40
  landed as **`6bf7b28`**: that handler now takes `State` and answers through
  `MaintenancePort::deduplicate`. The `cleanup` half still holds (issue 39, PAUSED). **No AC changes:**
  AC6 stays sessions-only and maintenance stays out of it — but the dedup factory
  `production_deduplication_port` is pointed at this issue by its own doc comment and at issue 39 by
  this AC, so neither owns that wiring. The residual is recorded on issue 40's status line; assigning it
  is not this issue's work — and that assignment has since been made: the wiring is **owned by issue 54**
  (`54-dedup-port-wiring-unreachable`), filed 2026-10-25.
- **2026-10-27 — AC4 + AC6 landed (implementation commit `c03338843b72b3b29f044ff9bb18c462a8a10b26`).**
  Wiring: `WebStoreDeps` gained `pub episodic: Arc<EpisodicStore>`, filled once at its single
  construction site from `runtime.store.clone()`, and the root composition root calls
  `.with_sessions(episodic_session_port(deps.episodic.clone()))` after `.with_skills(skills_port)`;
  the `None` arm is unchanged. Verified by three new root oracles plus one gate pin:
  `test_production_wiring_serves_the_seeded_session_history` (both routes `200`; alpha lists exactly
  its two seeded rows, beta exactly its one row with the `AC2-FOREIGN-PROJECT-SESSION` marker, an
  unseeded project `data: []`), `test_production_wiring_session_delete_is_project_scoped` (scoped
  delete `200 {"success":true}` and the row disappears; a cross-project delete `404
  {"success":false,"error":"Session not found"}` and the foreign row survives),
  `test_sessions_stay_on_the_documented_stub_when_composition_fails` (poisoned `storage/episodic.db`
  → `engine.composition_failed` on stderr, health `200`, memories `501`, skills `200`, both session
  routes `501`), and `test_the_local_token_is_accepted_only_on_the_custom_header` (custom header
  admitted; Bearer and well-formed Basic both `401`). Mutation proof: deleting the `.with_sessions(..)`
  line turned both session oracles RED with the documented `501 {"success":false,"error":"Session
  history endpoints land with W6.3"}` body; restored file hash matched the pre-mutation hash and the
  oracles re-passed after recompilation. AC4: `docs/bridge-schema.md` §11 states the three contract
  statements (custom-header-only local token, 501-vs-500, no endpoint inventory) and names the pinning
  oracles; `ports.rs` and `sessions.rs` doc comments corrected accordingly. Full battery: `cargo fmt
  --all --check`, both clippy gates and `cargo test --workspace` all exit 0 (`test result: ok. 9
  passed; 0 failed` for `tests/production_api_wiring.rs`; 235 `test result: ok` suite lines, 0
  failures across the workspace). **Residuals, owned here:** three sites in `docs/web-skeleton-smoke.md`
  still describe the pre-wiring world and are falsified by this wave — the §F prose at `:326-328` and
  the D-13 row at `:422` say the production wiring answers the sessions `501` "by design" today, and
  route-sheet rows 36–37 at `:505-506` say `.with_sessions(episodic_session_port(...))` is "not wired".
  The smoke doc is issue 65's file and was deliberately not edited; issue 65's delivery items are
  scoped to the skills rows (`:498-504`) and `docs/agents/testing.md`, so it does not cover these —
  hence this issue stays open on all three.

## Agent Brief

**Category:** enhancement
**Summary:** two leftovers from the composition-root wiring — wire the sessions port, and state the auth/failure
contract where a UI author will find it.

**Current behavior (verified at HEAD `206cf9f`):**
AC1–AC3 and the oracle criterion landed with the composition root: the root builds real `ApiServices` over the
real stores, the degrade path is "no deps → unavailable", and the live oracle seeds a row through a real shard
manager and asserts it. Two criteria remain open. **AC6:** `ApiServices::with_sessions` exists as a builder with
documentation, but no production caller invokes it, and `WebStoreDeps` has no `episodic` field, so the sessions
port cannot be attached — every session-backed endpoint stays on the unavailable path. **AC4:** the bridge
schema says nothing about the local-token header or about the 501-versus-500 distinction, so a UI author has to
discover both by experiment.

**Desired behavior:**
The composition root passes the episodic store into the web deps and attaches the sessions port at its single
construction site, so session-backed endpoints answer real data instead of the documented stub. The bridge
schema gains the three contract statements AC4 names: the custom token header and the fact that neither Basic
nor Bearer works in standalone mode; that 501 means "this service is absent in this build" while 500 means an
internal fault, and a client must not conflate them; and that per-endpoint availability is deliberately **not**
enumerated, because it tracks the build.

**Key interfaces:**
- `WebStoreDeps` — the struct that must gain `episodic: Arc<EpisodicStore>`, filled from the composed runtime's
  existing store field at the same place the other deps are filled.
- `ApiServices::with_sessions(episodic_session_port(store))` — the call with no caller today.
- `docs/bridge-schema.md` — the contract surface AC4 edits.
- The auth gate's header constant and its gate order — verify the statements against the code before writing
  them.

**Acceptance criteria:**
- [ ] The sessions port is attached at the composition root through a single construction site; the deps struct
      carries the episodic store.
- [ ] A session-backed endpoint answers real data against a live engine, and a project-scoped call never
      returns another project's sessions.
- [ ] The bridge schema states the token header shape, the Basic/Bearer failure, and the 501-versus-500
      distinction — each verified against the auth gate and the maintenance handler, not recalled.
- [ ] **No endpoint status table is written.** Per-endpoint availability is explicitly declared out of scope in
      the document.
- [ ] The `unavailable()` degrade path still exists and is still tested.

**Out of scope:**
- The maintenance/cleanup port — that half moved to issue 39.
- Fixing the port-failure conflation in `mem-web` (a `PortError::Unavailable` still funnels to 500 through the
  generic failure helper); AC4 records the contract, it does not change the handler.
- Any endpoint beyond sessions.
