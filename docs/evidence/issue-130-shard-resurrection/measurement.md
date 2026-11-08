# MEASUREMENT — issue 130: shard resurrection on open-first read

Wave: blueprint-20261108-issue-130-shard-resurrection (plan-gate round 3,
STAMP sha256=b45b29067f80). Worktree `.tmp/worktrees/issue-130`, branch
`fix/issue-130-shard-resurrection` (base 7a3fd89).
Date: 2026-11-08. Status: **PRE-REGISTERED EXPECTATIONS HELD — nothing
adjusted to the observation.**

## The reported defect (issue 130 matrix, rows 1–3)

turso create-on-open (`ConnectionManager::get_connection` → `open_database`,
crates/mem-store/src/connection.rs) makes every open-first READ lane silently
recreate a registered shard whose DB file was deleted while the engine runs —
the recreated file is schema-less, so the subsequent SQL fails and the lane
answers the D-9 500. Rows: `GET /api/tags` (all_scope_rows +
collect_distinct_tags walk), unfiltered `GET /api/memories`, tagged
`GET /api/memories?tag=`.

## Fix shape (what was measured, not designed here)

- `ProductionMemory::stat_shard_file` — read-only `std::fs::metadata` before
  any open on the four read lanes (`all_memories`,
  `list_memories_by_container_tag`, `distinct_tags`, `search_in_shard`);
  refusal carries the file name on the LOG channel only.
- `PortError::ShardFileMissing { file_name }`; `port_failure` answers
  **503** with the fixed body
  `{"success":false,"error":"shard storage is temporarily unavailable — the shard file is missing"}`;
  `response_status` maps the variant to 503 (single-sourced: the wire arm is
  built from `response_status`).
- TOCTOU ACCEPTED and DOCUMENTED (variant doc + guard doc): the stat→open
  window is not closed; narrowing it would put an existence refusal inside
  mem-store (a write-semantics change — deliberately not taken).

## Acceptance suite (Step 1 RED → Step 3 GREEN), MEASURED

`cargo test -p mem-web --test shard_resurrection` — 5 tests:

| row | pre-fix (RED run 16:55 UTC) | post-fix |
|---|---|---|
| A `…refuses_without_resurrecting_get_memories` | RED — "the read must never re-create a missing registered shard file" | PASS |
| B `…answers_503_get_tags` (+healthy control) | RED — 500 vs 503 (body half clean) | PASS |
| C `…answers_503_get_memories_tagged` (+healthy control, REGISTERED tag) | RED — 500 vs 503 | PASS |
| D `…present_registered_shard_read_answers_200` (GET shape only) | PASS | PASS |
| E `…stats_lane_unaffected_by_read_guard` | PASS | PASS |

MEASURED intermediate (Step 2, guard + interim `PortError::Store` mapping):
A/D/E PASS, B/C RED exactly on the 503 assertion with the fixed 500 envelope
body — the five-test truth the plan-gate pinned. Then Step 3 (typed variant
+ 3 arms) flipped B/C green with no test edited.

## Layer pins (Step 4), MEASURED

- 4a (`cargo test -p mem-web --lib`): `shard_file_guard_refuses_when_metadata_errors`
  (present half `Ok` + file untouched; deleted half typed refusal + file
  STAYS deleted), `shard_file_guard_carries_no_free_text` (payload is the
  file-name component only — no path, no errno), and the method-head scream
  `shard_file_guard_sits_at_the_method_head` (drives `all_memories` through
  the port seam; the generic-`Store` flavor answers in the MOVE mutant).
- 4b/4c (`cargo test -p mem-store --test shard_open_premise`): the
  create-on-open premise — `get_connection` / `open_shard` on a
  not-yet-existing path answer `Ok` and the file exists afterwards.
- Premise mutant M0 (deleted creation inside `open_database`): 4b + 4c RED
  ("create-on-open must answer Ok for a new path") — demonstrated once,
  reverted, `git status` clean (mutation-log.md).

## Mutation matrix (Step 5) — every kill observed, single-edit, reverted

Full record: `mutation-log.md` (same directory).

| mutant | victims (MEASURED) |
|---|---|
| 1: guard deleted, `all_memories` | A RED (resurrection assert); others green |
| 2: guard deleted, `list_memories_by_container_tag` | C RED; others green |
| 2b: guard deleted, `distinct_tags` | B RED (walk order tags.rs:114 before :122); A/C/D/E green |
| 3a: ADD refusal in `get_connection` | 4b+4c premise RED; mem-web suite 5/5 SETUP-ABORT (fixture panic, real_store.rs:111 — recorded mechanism); 4a lib green 17/17 |
| 3b: MOVE guards into `open_shard` | 4c RED + **4b GREEN**; 4a method-head pin RED; B/C RED 503→500; A **green-with-reason** (no status pin — sanctioned by blueprint Step 5 line 170) |
| 4(i): arm returns `internal_error()` | B/C RED (body = 500 envelope); re-observed on final arm |
| 4(ii): variant folded into `response_status` Store arm | **SURVIVED the first build** — literal arm built the wire status independently of `response_status`; diagnosed (doc line "names the status the client received" = single-source linkage), arm now derives status from `response_status` (deviation recorded); re-observed: B/C RED on status with 503 body intact |

## Workspace gates (Step 6), MEASURED

- `cargo test --workspace`: **2493 passed, 0 failed** (0 suites FAILED).
- `cargo fmt --all --check`: clean.
- Clippy gate 1 (`--workspace --lib -D clippy::unwrap_used -D clippy::expect_used`): exit 0.
- Clippy gate 2 (`--workspace --all-targets -D warnings -A await_holding_lock -A assertions_on_constants -A doc_lazy_continuation`): exit 0. One real finding fixed during the gate:
  `shard_path.exists() == false` in test E → `!shard_path.exists()` (`clippy::bool_comparison`).
- web-ui lanes: `cargo clippy --workspace --lib` exit 0, `--all-targets` exit 0,
  `cargo test --target wasm32-unknown-unknown` — all suites ok (238 wasm tests).
- `cargo build` (engine binary): OK.

## LIVE engine verification (Step 6) — the real observation

Script: `.tmp/scratch/issue-130-live.sh` (also mirrored in the transcript
`.tmp/scratch/issue-130-live/<ts>/transcript.txt`), standalone child engine
per `tests/engine_manual_launch.rs` conventions (scratch storage root,
minimal dist shell, bounded health poll 200, token-less request mints
`.auth-token`, `x-opencode-mem-token` header), engine started 2026-11-08
17:48 UTC, PID 23969, port 46369 — isolated storage root
`.tmp/scratch/issue-130-live/20261108T174810Z/root/`.

1. Seed: `POST /api/memories` (real local ONNX embedder, cache symlinked to
   the provisioned live-store cache; engine never downloads) → **200**
   `{"success":true,…}`.
2. Healthy control: `GET /api/tags` → **200** with
   `{"tag":"opencode_project_13000000000000d1"}`.
3. Deleted the registered shard file
   `projects/project_13000000000000d1_shard_0.db` under the running engine.
4. `GET /api/tags` → **503**, body exactly the fixed 503 envelope, file name
   NOT echoed; file after the read: **absent** (no resurrection).
5. `GET /api/memories?includePrompts=false` → **503**, fixed body; file
   still **absent**.
6. `GET /api/stats` → **200**, `fileExists:false`, error text
   `Shard DB file missing: project_13000000000000d1_shard_0.db`
   (fileName-only, no path) — the stats lane is untouched by the guard.
7. Engine structured log: both refusals emit
   `{"event":"api.handler_failed","status":503,"variant":"ShardFileMissing"}`
   with the endpoint named — the wire never sees the file name, the log does.

**Every observation matched its pre-registered value; no mismatch, no
STOP-trigger.**

### Live-run hygiene note

The live script's `HOME` was NOT sandboxed (one env omission): the engine
resolved its config and file-log paths in the real home, so the stats
response's `daily` rows parse the real engine's log files and the engine may
have appended to the real file-log channel. The storage root was fully
isolated (scratch dir; `storage.root_audit` shows episodic.db/metadata.db
created under the scratch root) and no live-store file was opened for write
by this run. Approval of the fix does not depend on the stats numbers; only
rows 4–7 above carry the decision.

## Predicates / inverted world

- Read lane observed recreating a deleted registered shard file → reopens the
  decision as a defect (TOCTOU window stays documented).
- Deleted-file fixture becoming green without the typed arm → the guard is
  not at the method head.
- `verify_shards` residual (the FILTERED-search branch) is NOT covered by
  this wave and stays named in the issue digest.
