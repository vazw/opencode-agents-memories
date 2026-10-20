# 0005 — Episodic memory uses a dedicated `episodic.db` table, not a memory shard or `ai-sessions.db`

- **Status:** Accepted
- **Date:** 2026-10-19
- **Wave:** W4.1 (episodic schema & scoping; US 1.1 storage half)
- **Supersedes:** nothing. **Amends:** the PRD's storage-topology wording (see "PRD contradiction" below).

## Context

`mem-episodic` must persist one record per agent session, bound to the
non-optional scope triple `(project_id, session_id, agent_id)`. The PRD left the
topology open between three candidates:

1. a memory shard (`project_{hash16}_shard_{N}.db`, the W1.x/W2.x layout),
2. an `ai-sessions.db`-style shared auxiliary database,
3. a dedicated `episodic.db`.

The record's dedup semantics matter: `UNIQUE(project_id, session_id, agent_id)`
must be enforceable in one place, and W4.2's timeline reconstruction must read
one project's sessions without fan-out.

## Decision

Episodic sessions live in a **dedicated `episodic.db`**, in the
`agent_sessions` table created by migration v1 (label `episodic-v1`,
`SchemaMigration.description = "episodic-v1: Baseline agent_sessions with
embedding"`).

`EpisodicStore` takes `storage_path` + `StoreConfig` explicitly (the W2.x/W3.1
managers-arg pattern) and opens the file through `ConnectionManager`, so the
existing `PRAGMA foreign_keys = ON` (connection.rs), encryption resolution and
storage-path containment apply unchanged. The `Episodic` config section is not
consumed by the store in W4.1.

## Grounds

### Why not a memory shard

Memory shards rotate: files are named `project_{hash16}_shard_{N}.db`, existing
shards are treated as read-only and new writes go to the next index
(`shard_manager.rs` module doc). "Store sessions like memories" would therefore
split one project's sessions across shard files as rotation proceeds, which:

- makes `UNIQUE(project_id, session_id, agent_id)` unenforceable — a unique
  index only constrains rows inside one database file;
- forces cross-shard fan-out for W4.2 timeline reads, plus merge/sort logic for
  data that is naturally ordered by `created_at`;
- couples the episodic lifecycle to shard rotation, a memory-specific concern.

### Why not `ai-sessions.db`

The `ai-session` tables in the upstream reference are a *capture bookkeeping*
table (which sessions have been analyzed), not an episodic record store with
scope binding, embeddings and parent linkage. Sharing the file would mix two
different lifecycles and give the episodic schema a dependency on a table it
does not own. The dedicated-file shape also has a direct precedent in this port:
W3.1's gate accepted a dedicated `user-profiles.db` for the same reasons.

### PRD contradiction

The PRD listed the memory-shard topology as an option. This ADR records the
divergence and its grounds; the acceptance criteria (not the shard layout)
remain the contract, and the PRD's D-V7 dependency-edge constraint is still
satisfied because SQL-side `uuid7_str()` adds no crate edge (see D-1 below).

## Ratified divergences recorded here

- **D-1 — PK strategy (PRD ratified).** Row ids are minted by turso's built-in
  `SELECT uuid7_str()`; no `uuid`/`rand` crate, no hand-rolled generator.
  `crates/mem-episodic/src/ids.rs` therefore keeps only `now_millis()`. Because
  the id is minted in SQL, the PRD §Phase 3 D-V7 constraint (which governs
  dependency edges) is untouched. A `with_id_factory` seam exists for
  deterministic tests.
- **D-3 — Column layout (draft ratified).** The record keeps scope-binding
  columns (`id`, `project_id`, `session_id`, `agent_id`, `parent_session_id`,
  `summary`, `created_at`, `updated_at`) plus one JSON payload column
  (`metadata`) and one embedding column (`embedding F32_BLOB(dims)`). The PRD's
  "normalized" list fields (`files_touched`, `completed_work`, `decisions`,
  `open_threads`) can only be TEXT-JSON columns in SQLite anyway: normalizing
  them would duplicate the metadata blob, create dual-write drift, and buy no
  indexable access path. W4.2's required filters are already served by
  `created_at` and `agent_id`; `touched_files` remains a W4.2 join-table
  decision.
- **D-4 — Embedding content (PRD ratified).** The embedded document is the
  locked template `"Session {session_id} by {agent_id} on {branch}: {summary}.
  Completed: {completed_work}. Decisions: {decisions}."`, where `{session_id}`
  is the *scope-triple* session id from the record (not the minted row id),
  `{branch}` absent → the pinned literal `unknown branch`, list entries join
  with `"; "`, and `apply_task_prefix(..., EmbedTask::Document,
  &PrefixOptions::default())` is applied before the provider sees the text. The
  raw `summary` remains the stored column. A FakeEmbedder test pins the exact
  string.
- **D-5 — Parent linkage (PRD ratified).** `parent_session_id` is a self-FK with
  `ON DELETE SET NULL`; deleting a parent orphans children rather than
  cascading. `PRAGMA foreign_keys = ON` already runs on every open.
- **D-6 — Dedup (PRD ratified).** `UNIQUE(project_id, session_id, agent_id)` is
  part of v1. W4.1 has no upsert: a duplicate surfaces as
  `EpisodicError::Conflict`.

## Follow-ups

- **W5.3 recapture/upsert:** recapture needs an update path
  (`UPDATEd ... WHERE project_id/session_id/agent_id`) or an explicit
  conflict-resolution policy; W4.1 deliberately exposes no update method, so a
  scope triple stays immutable.
- **W4.2 timelines:** parent linkage is already in place; orphan handling for
  dangling references that predate the FK belongs to W4.2.
- **W4.3 / Phase 7 dimension drift:** `F32_BLOB(dims)` is fixed at first
  migration; changing dimensions requires a re-embed path, not an in-place
  schema change.
- **W5.3 privacy:** summaries are stored verbatim in W4.1. Capture must run
  `mem_ai::privacy` stripping before constructing a `NewAgentSession`.

---

## Addendum — 2026-10-19 (W4.2: soft delete, orphan tracking, timeline queries)

Migration **v2** extends the same `agent_sessions` table: the v1 DDL and index
statements are byte-unchanged (the PRD's "never rewrite v1 in place"), and v2
adds exactly two columns plus one index.

- **Columns.** `deleted_at INTEGER` (nullable) and `orphaned INTEGER NOT NULL
  DEFAULT 0`, appended by `ALTER TABLE ... ADD COLUMN` in that order. The
  `ADD COLUMN ... NOT NULL DEFAULT 0` form is supported by turso_core 0.8.3
  (`translate/alter.rs` rejects only a NOT-NULL column whose *effective*
  default is NULL), and the port already exercises the ALTER path in
  `ensure_user_prompt_columns`.
- **Index.** `idx_agent_sessions_project_deleted_created` on
  `(project_id, deleted_at, created_at)` serves the visibility predicate every
  timeline statement opens with.
- **Migration label (D-10 delta).** `EpisodicStore::EPISODIC_MIGRATION_LABEL`
  becomes the set-level `"episodic"` (`"episodic-v1"` → `"episodic"`): the
  `MigrationOptions.label` is tracing metadata only, and per-version identity
  stays in each `SchemaMigration.description` (`episodic-v1:` /
  `episodic-v2:`). This is a deliberate divergence from W4.1's D-7 constant,
  not a silent one.
- **Visibility rule (D-11).** A row with `deleted_at IS NOT NULL` is invisible
  to `get_session`, `session_detail`, `list_sessions`, `search_sessions`, both
  timelines, `count_sessions(Some(project))` and both endpoints of
  `link_parent`/`unlink_parent`. It stays visible to exactly two raw seams,
  documented as such: `count_sessions(None)` (global diagnostics) and
  `embedding_for` (diagnostic read-back). One definition — not two — is what
  removes the "orphaned timeline entry" class this wave exists to eliminate.
- **Orphan flag (D-12).** `orphaned` is a persisted *write-time* flag meaning
  "this record's parent link was severed by a delete of that parent". It is set
  by both delete paths on every child of the deleted row, cleared by
  `link_parent`/`unlink_parent`, and false for a record that never had a
  parent. Read paths never drop an entry because a link is broken — they
  surface the flag (the ported `memory-timeline-orphan` intent). Invariant,
  asserted by the tests: for every summary in a timeline,
  `parent_session_id.is_none() || orphaned == true || get_session(parent).is_some()`.
  Both delete paths write `MARK_CHILDREN_ORPHANED` + the parent delete as one
  `BatchMode::Write` (transactional `IMMEDIATE`) batch, so a crash cannot leave
  children visible-but-unflagged; `soft_delete_session` is additionally
  idempotent and returns `false` without writing for an unknown or
  wrong-project id.
- **`touched_files` predicate: no join table (D-14, the W4.2 answer to W4.1's
  D-3 open question).** The filter is evaluated in Rust against the decoded
  `metadata.files_touched` (ANY-match, case-sensitive, exact string equality),
  preserving D-3's "no second normalized copy, no dual-write drift".
  Consequence, accepted and bounded: the SQL candidate statement carries no
  `LIMIT` (because `last_n_sessions` must apply *after* every filter), so one
  project's matching rows are read before the Rust-side filter; the *returned*
  page stays bounded by `last_n_sessions` / `DEFAULT_TIMELINE_LIMIT` (≤
  `MAX_TIMELINE_LIMIT = 500`), and per-project episodic volume is bounded by
  W4.3 compaction. `json_each` was rejected as the alternative (unverified in
  turso_core 0.8.3, and it would couple filters to payload JSON).
- **Semantic path (D-15).** `semantic_timeline` embeds
  `apply_task_prefix(query, EmbedTask::Query, &PrefixOptions::default())` — the
  same `PrefixOptions` as W4.1's document path, differing only in task — and
  ranks by the exact `vector_distance_cos` scan (no vector index exists in this
  tier; W1.3 exact-fallback semantics). `compose_embedded_document` /
  `embedded_text` are byte-unchanged (D-4 re-embedding stability).
- **Tool schema (D-16).** `recall_project_sessions`'s args deny unknown fields
  (an input contract, unlike the forward-compatible `SessionMetadata.extra`),
  derive `mode` when absent, and pin `to_json_schema()` to upstream's
  `{ type, properties, required }` parameter shape. Registration is deferred to
  W5.4.

