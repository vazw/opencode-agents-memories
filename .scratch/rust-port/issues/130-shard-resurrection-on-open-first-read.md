# 130 — shard resurrection on open-first read

Status: needs-triage

Filed from: issue 129 residuals r9, Step 4 route matrix (diagnosis STOP; syseng ruling
Q2=ACCEPT-STOP, 2026-11-06). Matrix: `docs/evidence/issue129-residuals/partial-store-route-matrix.md`.

## Problem

A registered shard file deleted while the engine runs is silently RESURRECTED by the open-first
read paths, and the deletion-detecting routes answer a generic 500:

- Matrix rows 1–3 (MEASURED, fresh child per route): `GET /api/tags`,
  `GET /api/memories` (unfiltered), `GET /api/memories?tag=<scope>` all reach `all_scope_rows`/
  `collect_memories` → `all_memories`/`list_memories_by_container_tag` → `open`, which **creates**
  the missing file; the request answers 500 `{"success":false,"error":"Internal Server Error"}` and
  the file is back on disk afterwards.
- Matrix rows 5–8: the `find_memory` routes (`DELETE/PUT /api/memories/{id}`,
  `POST /api/memories/{id}/pin`, `POST /api/memories/bulk-delete`) refuse without resurrecting but
  flatten the condition to the same generic 500 (`PortError::Store(_)` → `internal_error()` at
  `crates/mem-web/src/api/mod.rs:636-640`).
- Row 4: `/api/stats` already reports per-shard `fileExists:false` with HTTP 200 (pinned by
  `test_stats_missing_shard_file_sets_file_name_only_error_and_succeeds`) — no change there.

## Why not fixed in 129

1. A structured 503 for the `find_memory` family needs the `port_failure` arm in
   `crates/mem-web/src/api/mod.rs` — outside shard A's exclusive file list — and re-baselines the
   frozen 500 pin in `crates/mem-web/tests/scope_wide_delete.rs:373`. Both are fresh-scope decisions
   the r9 plan does not authorize.
2. The open-first read routes cannot carry a 503 at all today: their detection IS the recreation.
   Fixing them means a stat-before-open on the read paths — that changes storage semantics (when a
   read may create a shard file) and must be gated on its own.

## Solution sketch (TO BE GATED — not a commitment)

Stat-before-open on the open-first read paths: before `open`, check the registered shard's path
(the same existence check `collect_shard_stats` already makes) and answer a structured unavailable
response instead of opening, so a deleted shard is neither resurrected nor flattened to 500. Must
define: which reads keep create-on-open semantics for a fresh root vs a registered-but-missing
shard; the response contract per route; and the TOCTOU window the 129 plan warned about.

## Out of scope (stays as decided in 129)

- Any 503 remap of `PortError::Store` in `crates/mem-web/src/api/mod.rs::port_failure` (its own
  gate decision; would also need the frozen `scope_wide_delete.rs:373` 500 re-baselined).
- The frozen `scope_wide_delete` guard test itself (byte-identical by the r9 rules).
