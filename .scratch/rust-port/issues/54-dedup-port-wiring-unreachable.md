# 54 — Composition root · `POST /api/deduplicate` is implemented, tested and unreachable in the shipped binary

Status: ready-for-agent — 2026-10-25. Filed on the owner's ruling ("file issue 54, narrow scope") after issue 51's
sweep found the residual unowned. **The work will add nothing to `crates/`**: the symbols it needs already exist —
`ApiServices::with_maintenance` (`crates/mem-web/src/api/ports.rs:622`), `production_deduplication_port`
(`crates/mem-web/src/api/deduplication/mod.rs:264`), and the two resolved config fields
`deduplication_enabled` / `deduplication_similarity_threshold` (`crates/mem-core/src/config/mod.rs:487-492`).
The new artifact is one `.with_maintenance(..)` call in `src/main.rs`'s `Some(deps)` arm, plus the tests that
prove it is read.

## The defect, measured at `8a39e01`

Issue 40 landed a real handler: `pub async fn deduplicate(State(state): State<Arc<AppState>>)`
(`crates/mem-web/src/api/maintenance.rs:245`, body `:246-253`), a real `MaintenancePort::deduplicate`
(`deduplication/mod.rs:249-251`), and the factory that mints it. **Nothing in the shipped composition root
calls the factory**, so the handler's `Err(PortError::Unavailable)` arm is the only reachable one:

| what | where it stands (re-derived 2026-10-25, not inherited) |
|---|---|
| the factory | `crates/mem-web/src/api/deduplication/mod.rs:264` — `production_deduplication_port(shards: ShardManager, enabled: bool, threshold: f64) -> Arc<dyn MaintenancePort>` |
| its only caller | `crates/mem-web/tests/dedup_endpoint.rs:33` — a **test**. `grep -rn 'production_deduplication_port' --include=*.rs .` returns the definition, one doc comment (`dto.rs:493`), and that test |
| the port slot | `crates/mem-web/src/api/ports.rs:622` — `with_maintenance(mut self, maintenance: Arc<dyn MaintenancePort>)`, never called by `src/main.rs` |
| the composition root | `src/main.rs:150-172` — the `Some(deps)` arm chains `.with_skills(skills_port)` **only**; the `None` arm chains `.with_skills(..)` on `ApiServices::unavailable()`. No `.with_maintenance(..)`, no `.with_sessions(..)` |
| the shipped answer | `POST /api/deduplicate` ⇒ `Err(PortError::Unavailable)` ⇒ `envelope::not_implemented(DEDUPLICATE_UNAVAILABLE)` ⇒ **501**, while `crates/mem-web/src/api/mod.rs:185-187` documents exactly that: *"The handler is real: the 501 comes from the unavailable port the default (and fake) wiring injects, not from a leftover stub."* |
| the config keys | resolved at `mem-core/src/config/mod.rs:487-492` from `schema.rs:380-381`, defaults `true` / `0.9` (`schema.rs:576-577`) — **zero production readers** today, the same parsed-but-unread shape as issue 49 |

Two sources of truth disagree in the tree, and that is the whole defect: the factory's doc says *"The
composition-root injection point (issue 37 calls this; this wave does not)"* (`deduplication/mod.rs:257-258`),
while issue 37's AC6 disclaims maintenance "to issue 39" and issue 39 is **PAUSED** and scoped to
`/api/cleanup`. Neither owns it. This issue does.

## Scope — exactly one seam and its reader proof

**In scope**

1. **54.1 — the wiring.** In `src/main.rs`'s `Some(deps)` arm, chain
   `.with_maintenance(production_deduplication_port(<shards>, <enabled>, <threshold>))` after `.with_skills(..)`,
   with the two config values taken from the **resolved** `core_config`
   (`deduplication_enabled`, `deduplication_similarity_threshold`) — never hardcoded, never a second resolution
   path. The `None` arm keeps `ApiServices::unavailable()` unchanged: with no composed stores there are no
   shards to deduplicate, so its 501 is the documented degrade, not a gap.
2. **54.2 — the reader test (issue 49's rule).** A key that is parsed and never read is a review failure. Prove
   both values reach the service: config `deduplicationEnabled: false` ⇒ the documented
   `PortError::Invalid` business message (200 `{"success":false,"error":"Deduplication is disabled in config"}`,
   the arm at `maintenance.rs:247`); config absent ⇒ the default `true` runs the service. One test may drive
   `DedupService` directly for the threshold; the *wiring* test is 54.3.
3. **54.3 — the anti-regression.** A test over the built runtime (composition root or the narrowest seam that
   constructs it) asserting the API's maintenance port is **not** `Unavailable` — i.e. `deduplicate()` returns
   something other than `Err(PortError::Unavailable)`. This is the test that would have caught issue 40's
   landing: "the port exists but production forgot to inject it" is exactly the class 48's `with_eraser` test
   catches on its side.
4. **54.4 — verify the route table is *not* part of this.** `ROUTE_TABLE`'s `POST /api/deduplicate` row stays
   `RouteExpectation::Stub` (`mod.rs:187`) **if** the contract suite drives the default/fake wiring rather than
   the composition root — re-derive that before editing either file; flipping the row would make
   `api_endpoint_contract` a 200 that never ran, which is worse than a documented 501. If the re-derivation
   shows the suite drives production wiring, the row changes in the same commit and the doc comment with it.

**Out of scope**

- `/api/cleanup` (issue 39, PAUSED), the migration methods on `MaintenancePort` (37/39 — every one of them
  keeps returning `Unavailable`, `deduplication/mod.rs:220-247`), the sessions port (37's AC6).
- `DedupService::run()`'s algorithm, storage effects or report shape. This issue wires an existing operation; it
  does not change what the operation does.
- No new config key, no schema change, no new dependency.

## Flagged consequence — this makes a destructive maintenance op reachable

Until this lands, `POST /api/deduplicate` cannot mutate anything from the shipped binary. After it lands, a
request from any authenticated client runs `DedupService::run()` over the live shard vectors, gated **only** by
`deduplicationEnabled` (default `true`). That is the intended upstream behaviour and the owner asked for it, but
the commit must say so in its message and the status line must record it, because "made an endpoint live" is a
different kind of change from "added a field". Issue 40's status already flags that both dedup and cleanup
mutate shard vectors; this is the moment that becomes user-reachable, and issue 39's pause does not cover it.

## Resume path

No blueprint exists for this issue. When taken up: the wave is small enough for a direct planner → gate → coder
→ reviewer pass, and the acceptance criteria above are the test plan. Priority is below the three in-flight waves
(45, 48, 51) and above nothing else; it depends on none of them and blocks none of them.

## Why this is worth an issue rather than a note

Three separate status lines currently point at someone else for this wiring — the factory's doc at issue 37,
issue 37's AC6 at issue 39, and issue 40's status at "unowned" — and issue 51's rule (now being implemented)
forbids `done` on a deliverable carrying a residual with no named owner. Recording the gap without an owner is
what allowed it to survive issue 40's wave, its review, and the audit that found everything around it.
