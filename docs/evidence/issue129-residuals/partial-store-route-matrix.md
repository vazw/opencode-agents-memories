# Issue 129 — Step 4: partial-store route matrix (diagnosis, STOP)

Status: **STOPPED, measurement only.** No handler changed. `test_partial_storage_root_answers_structured_unavailable`
is NOT authored — see the two blockers. The `/api/stats` decision stands unchanged (200 + per-shard
`fileExists:false` + `Shard DB file missing: <fileName>`, pinned by
`test_stats_missing_shard_file_sets_file_name_only_error_and_succeeds`).

> File-ownership note: the FAN-OUT table lists three evidence files for shard A and none for Step 4.
> This file is the tracked diagnosis artifact for the STOP and needs integrator acknowledgement.

Tree: `.tmp/worktrees/issue-129`, base `eda2efd` + the shard's uncommitted audit change.
Every row below is **MEASURED** with the real child engine, a real registered project shard
(`storage/projects/project_a1b2c3d4e5f67890_shard_0.db`, one seeded row), the file deleted after the
child booted, absent-before asserted immediately before each request, and a FRESH child process per row
(an earlier single-process run was invalid: a cached connection masked the recreation).
Raw: `.tmp/progress/r9-shard-A/step4-measure.log`. Healthy comparison in the same run:
`GET /api/memories` → `200`, `total: 1`, the seeded row listed.

| # | method + route | branch that detects it | opens / creates? | pre-fix status | body | file after request | 503 feasible? |
|---|---|---|---|---|---|---|---|
| 1 | `GET /api/tags` | `all_scope_rows` → `all_memories` → `open` | **creates** the file | 500 | `{"success":false,"error":"Internal Server Error"}` | **recreated** | no — a 503 would have to stop an open that currently recreates; that changes storage semantics |
| 2 | `GET /api/memories` (unfiltered) | `collect_memories` → `all_memories` → `open` | **creates** the file | 500 | same generic body | **recreated** | no — same reason |
| 3 | `GET /api/memories?tag=<scope>` (filtered) | `collect_memories` → `list_memories_by_container_tag` → `open` | **creates** the file | 500 | same generic body | **recreated** | no — same reason |
| 4 | `GET /api/stats` | `collect_shard_stats` (existence check) | no | 200 | per-shard `fileExists:false` + `Shard DB file missing: <fileName>` | absent | decided: stays 200 (pinned test) |
| 5 | `DELETE /api/memories/{id}` | `find_memory` → `memory_by_id` stat-before-open | no | **500** | `{"success":false,"error":"Internal Server Error"}` | absent | blocked — see (a)+(b) |
| 6 | `PUT /api/memories/{id}` | same (`find_memory`) | no | 500 | same generic body | absent | blocked |
| 7 | `POST /api/memories/{id}/pin` | same (`find_memory`) | no | 500 | same generic body | absent | blocked |
| 8 | `POST /api/memories/bulk-delete` | same (`find_memory`, per id) | no | 500 | same generic body | absent | blocked |
| 9 | `GET /api/user-profile` | unrelated store | no | 200 | `exists:false` | absent | n/a (must NOT be 503: lazy store, not partial) |

So: the deletion-detecting branch EXISTS at four routes (5–8, one shared `find_memory` seam), it refuses
WITHOUT resurrecting the file, and the refusal is currently flattened to the generic D-9 500.

## Blockers (why no 503 landed)

**(a) The mapping lives in a file shard A does not own.** `PortError::Store(_)` → `500` is one arm of
`crates/mem-web/src/api/mod.rs::port_failure`, an exhaustive match (no wildcard arm). A distinct typed
signal is impossible without either editing that file or adding a `PortError` variant — and adding a
variant breaks the same exhaustive match at compile time. Both are outside the shard's exclusive file
list, and the plan's rule is "stop and request fresh ownership/gate; do not silently expand".

**(b) A preexisting test pins the 500 byte-for-byte on exactly the verified route.**
`crates/mem-web/tests/scope_wide_delete.rs:340-395`
(`test_delete_endpoint_refuses_a_missing_shard_without_resurrecting_it`) asserts
`response.status == 500`, `error == "Internal Server Error"`, and `!db_path.exists()`. It is a
preexisting, frozen test ("Preserve all preexisting tests unchanged"), so answering 503 on `DELETE
/api/memories/{id}` contradicts it: the 503 needs a syseng decision that re-baselines that test, which
this plan does not authorize.

Alternatives considered and rejected as unsafe or out of scope:
- mapping every `PortError::Store` to 503 — explicitly forbidden (conflates unrelated I/O with missing
  storage);
- a handler-level `ShardRef.db_path` stat in `memories.rs` (in A's files) returning 503 — still breaks
  blocker (b), duplicates the adapter's existing stat, and adds a TOCTOU window the plan warns about;
- leaving the DELETE family at 500 and mapping something else — no other route has a distinct typed
  missing-store signal (`/api/tags`, `/api/memories` detect it only by *creating* the file first, which
  is the very hazard that must not be enshrined behind a 503).

## What this needs

1. syseng: a decision on the preexisting 500 contract — re-baseline `scope_wide_delete`'s status assertion
   for missing storage, or drop the 503 claim for the DELETE family (diagnosis, not deliverable).
2. ownership: `crates/mem-web/src/api/mod.rs` (the `port_failure` arm) plus, if the routes 1–3 recreation
   is to be fixed, a separate design for a stat-before-open on the open-first read paths — that one DOES
   change storage semantics (today those reads resurrect a deleted shard) and must be gated on its own.

`/api/stats` behavior is untouched and still passes
`test_stats_missing_shard_file_sets_file_name_only_error_and_succeeds`.

Decision (syseng ruling Q2=ACCEPT-STOP, 2026-11-06, session `ses_e4eb3541bffezzOuDRv7X3YBW4`):
Step 4 closes as **diagnosis** per the syseng ruling Q2=ACCEPT-STOP 2026-11-06 —
`test_partial_storage_root_answers_structured_unavailable` is not authored and no Step 4 code
changes. The route matrix above is the measurement the criterion closes on. Inverted world: a route
with a distinct typed missing-store signal that requires neither touching `port_failure`
(`crates/mem-web/src/api/mod.rs:636-640`) nor re-baselining the frozen 500
(`crates/mem-web/tests/scope_wide_delete.rs:373`) reopens Step 4 as measurement. The stat-before-open
follow-up is filed as `.scratch/rust-port/issues/130-shard-resurrection-on-open-first-read.md`
(Status: needs-triage, solution sketch TO BE GATED — out of scope for 129). Ownership grant
(review-1 LOW-2 settled, one sentence): this matrix file is granted as shard A's fourth evidence
file for the r9 wave. Durable mirror: `docs/evidence/issue129-residuals/scope-amendments.md`
(Amendment 2).
