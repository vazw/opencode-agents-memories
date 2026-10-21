# 31 — W6.7 · Core memory screens (US 5.1)

Status: done — closed 2026-10-21 (W6.7, Plan Gate APPROVED, reviewer APPROVED on independent re-execution)

## What to build

The explorer's four core screens: **Memories** (browse/search the tier-1 project memories), **Tags** (the tag surface), **Profile** (the user-profile tier) and **Settings**. Today all four routes exist in `web-ui/src/routes.rs` carrying the owner label `Owner::W6_2`, and every one of them renders the "not yet implemented" notice — W6.2's approved blueprint explicitly created no UI file, so their owner label points at a wave that closed without building them.

This lane builds them against the W6.2 REST surface (already landed: the memories / tags / profile / config endpoint groups), and re-homes the four stale `Owner::W6_2` labels to this wave's owner **in the same change**, so the explorer's notices stop naming a closed wave. (The label re-homing may be carried by the W6.6 / issue-30 lane if it lands first — it owns `web-ui/src/routes.rs` anyway — but it must happen exactly once.)

**Ordered after the agent-facing loop** by the user's ruling of 2026-10-21 (build W5.3 → W5.4 first): the screens are buildable without the capture loop, but the explorer shows almost nothing until W5.3 puts host-driven rows into the episodic tier.

Wave detail: `.scratch/rust-port/roadmap.md` §W6.7.

## Acceptance criteria

- [x] `ProjectMemories`, `Tags`, `Profile` and `Settings` render real screens (not planned notices) against the W6.2 REST endpoints, using the existing same-origin `fetch` seam and its single Basic-forwarding `Authorization`.
      — **Done.** All four panels render live data through the existing `web-ui/src/api_request.rs` seam, unchanged. **Wording note:** the seam sends the local token in `x-opencode-mem-token` and sends NO `Authorization` header (it adds one only when a Basic credential is supplied); the issue's "single Basic-forwarding `Authorization`" names the SERVER GATE's Basic step (`crates/mem-web/src/gate.rs:74`), not the UI's token transport. The reviewer confirmed the live request carried `x-opencode-mem-token` and no `Authorization`. This is the blueprint's Finding 1, gate-verified.
- [x] The four route-table owner labels are re-homed from the closed `Owner::W6_2` to this wave's owner, and the nav/route tests are updated to match.
      — **Done.** `Owner::W6_7` added; the now-unowned `Owner::W6_2` variant AND its `label()` arm removed; `W6_3`/`W6_4` kept. `web-ui/tests/nav.rs` updated. The re-homing happened **exactly once** — issue 30 (W6.6) is not started.
- [x] Each screen follows the W6.5 house pattern: a pure (wasm-bindgen) test pinning the request it builds and the shape it renders, plus the browser smoke against the existing Chrome at `127.0.0.1:9222`.
      — **Done, on all three legs.** The gate caught that revision 0 had only two of them (request spec + decode, no rendered shape), so each screen also owns a pure `*_view` seam with loaded/empty/missing/error variants and a named RED mutation. 19/19 `core_screens` oracles, all pure (no `fetch`, no DOM, no `web_sys`). Browser smoke run against the existing Chrome.
- [x] No new REST endpoint, no schema change, no new dependency; §0b D5 holds — no Leptos server function may shim a REST handler.
      — **Done.** No route-table row, no `paths.rs` entry, no DTO in mem-web, no crate. `crates/mem-web/**` untouched. §0b D5 holds. **Manifest guard, resolved by mtime:** the root `Cargo.toml` and `Cargo.lock` differ from HEAD (+1 line `exclude = ["web-ui"]`; +18 lock lines, one added package `signal-hook-registry`) but every one of those files was last written **2026-10-20 17:55Z / 19:24Z — over 14 hours before the wave began** (the `exclude` is W6.5's need, the lock delta a stale-lock refresh), and `crates/mem-bridge/Cargo.toml` is byte-identical to HEAD. The wave's own file set is exactly `src/main.rs`, `crates/mem-bridge/src/{engine,composition}.rs`, `tests/production_api_wiring.rs`, `web-ui/src/*`, `web-ui/tests/*`. A guard oracle (`test_no_mem_bridge_to_mem_web_edge`) asserts the forbidden `mem-bridge`→`mem-web` edge is absent (ADR-0007:584).
- [x] Manual smoke: all four screens render live data against the Rust server.
      — **Done.** The composition-root wiring that AC5 depends on was **absorbed into this wave** (see below). Against a live standalone engine: token-less `GET /api/memories` → 401 (which lazily creates `.auth-token`), then token-bearing → 200 for `/api/memories`, `/api/tags`, `/api/user-profile`, `/api/stats`, plus a bare 200 from `/api/health`. All four screens rendered their real panel with **no planned-notice**; nav badges read `PLANNED` only on Export/Import and Skills; 0 console errors; all network same-origin.

## Blocked by

- 24 · W6.2 (the REST surface these screens consume) — closed
- 29 · W6.5 (the shell, route table, nav model and fetch seam) — closed
- 21 · W5.3 (the capture loop) — **ordered before this lane** by the user's ruling of 2026-10-21, not a hard dependency
- 35 · W6.8 (the composition-root wire that points mem-web at the built UI — required for this lane's browser smoke against the live server)
- 37 · W6.2 follow-up (the composition-root **API-services** wire) — **unowned and unstarted** when this wave started, which made AC5 unpassable; **absorbed into this wave** by user decision of 2026-10-21. This issue's absorption satisfies issue 37's AC1–AC3 and AC5; **issue 37 remains open for its AC4 only** (the `docs/bridge-schema.md` token-header / 500-vs-501 sentence) and is re-scoped accordingly.

## Out of scope

- `ExportImport` and `SkillLibrary` screens (W6.4 / issue 26).
- The `SessionHistory` screen (W6.3 / issue 25) and the `Agents` contribution view (W6.6 / issue 30).
- Any change to the W6.2 REST contract.
- **Issue 36's lane.** A plugin-launched engine still loses the entire axum surface and the dist forwarding. W6.7 does **not** repair that, and its smoke launches the engine directly in `--mode=standalone` with an explicit `--web-dist`. No handoff may read as if the plugin path were repaired.
- **Issue 37's AC4 doc edit.** The verification sweep stayed a PURE gate; `docs/bridge-schema.md` was not edited.

## Evidence

Blueprint `tmp/blueprints/blueprint-20261021-0655-w67-core-memory-screens.md` (body `649e7acc3fbd`, `approval: SYS-APPROVED 2026-10-21T07:54Z`). Two Plan Gate rounds: r0 REJECTED on ten required modifications, r1 APPROVED. Ledger `tmp/progress/20261021-0655-w67-core-memory-screens.ledger.md`: 9 rows, 9 `- [x]`, 0 `- [ ]`, 0 `- [~]`.

**Reviewer's own re-execution (not the implementer's word):** `cargo fmt --all --check` (root + web-ui) exit 0; `cargo clippy --workspace --all-targets -- -D warnings` exit 0, 0 diagnostics; wasm clippy exit 0; `cargo test --workspace` **1391 passed / 0 failed**; `production_api_wiring` **2/2**; wasm suite **81 passed / 0 failed** (`core_screens` 19/19); `trunk build` reproduced the same hashed assets the implementer smoked, so the smoke exercised current source; orphan probe empty before and after every run. All 27 required test names present exactly once. Security gate: **10/10 Mandates PASS** (authentication, authorization/IDOR, input validation, secrets, data protection, privacy, error handling, logging, dependencies, secure config).

**The composition-root wiring, and why it was in scope.** AC5 is unpassable while the composition root hands mem-web `ApiServices::unavailable()` — every store-backed endpoint answers 500 in the live binary while the crate suite stays green, because the crate tests assemble the production wiring themselves. `ApiServices::production(...)` was called from exactly one place in the whole tree: mem-web's own integration test. The Plan Gate accepted the absorption in principle, conditioned on ten modifications, and the **user confirmed it by direct decision on 2026-10-21** (record `mem_1792565360736_79j2b80y6`). The wiring shape the gate verified: `mem-bridge` gained a mem-bridge-owned `WebStoreDeps { shards, prompts, profile, embedder }` and **no** `mem-web` edge; the **root crate** (`src/main.rs`) is the mem-web composition root and alone builds `EmbeddingService` + `ApiServices::production(...)`; `WebConfig` drops `Debug` and carries `deps: Option<WebStoreDeps>`; the degrade path is "no deps → `unavailable()`"; the launch order is compose-before-launch with the lock still first, and exactly one `ShardManager` exists over the storage root.

**The oracle runs the binary's own wiring path** (memory `mem_1792561576490_y40u5vlk8`): it spawns `env!("CARGO_BIN_EXE_opencode-agents-memory")`, seeds one row through a real `ShardManager` in its own dropped runtime with a `_project_`-tagged container and a `create_shard`-registered shard, acquires the token the way a client does (token-less 401 → bounded 20 ms/5 s poll → token-bearing 200), and asserts the seeded row's id/content/tags. It went **RED first** on the stub's `500 {"success":false,"error":"Internal Server Error"}` — the exact mutation the gate specified.

## Carry-forward (non-blocking, raised by the reviewer — no action required, recorded so they are not lost)

- **`ScreenBody`'s `Live` catch-all arm** (`web-ui/src/screens.rs:88`) would silently render the sessions panel for any future unnamed `Live` route. A future wave that adds a `Live` route without touching that arm gets the wrong screen with no error.
- **`web-ui/src/screens.rs:6-7` module doc** still describes `Live` as "today … the W6.3 session-history timeline" — stale since this wave added four live routes.
- **`web-ui` ships no favicon**, so the browser's default `/favicon.ico` probe 404s on a first load. `web-ui/index.html` is W6.5-owned and out of this wave's scope; the reviewer's fresh-tab run produced no favicon request and 0 console errors.
- **The ledger's per-step timestamps were backfilled**: rows 2–8 share one `DONE (recorded) 2026-10-21T08:33:01Z` stamp with no `STARTED` marks, so the ledger's temporal record for those steps is one batch timestamp rather than a per-step observation. The observable gate still holds (9 rows, zero `- [ ]`, no `- [~]`), and the reviewer judged it non-blocking — but the ledger's value is its per-step temporal authority, so the next wave's executor should stamp each step as it happens.
- **`/api/skills` answers 500, not 501** — the blueprint's Finding 4 and the gate's "verified sound" list both claimed 501. Measured: sessions 501, cleanup/deduplicate 501, skills 500 (`skill_failure` funnels `PortError::Unavailable` through `port_failure` → `internal_error`). This is a blueprint-document defect, owned by issue 37's AC4 lane; recorded as `mem_1792572205625_7w2wmh0bu` because W6.4 builds the skills surface and will meet it.

## Comments

- 2026-10-21: planned through the fleet pipeline — planner → syseng Plan Gate (2 rounds) → coder → code-reviewer. The reviewer's first leg died on an upstream outage after 16 minutes with no output; it was respawned on the code-reviewer's own designated fallback rung with mitigations (emit text early, order the work cheapest-first) so a death could not return nothing.
- The wave made **no git writes** of any kind. Its files sit in the working tree only; the commit is the owner's act.
- Six lessons from this wave are filed in the vault at `Feedback/2026-10-21-w67-gate-lessons.md`.
