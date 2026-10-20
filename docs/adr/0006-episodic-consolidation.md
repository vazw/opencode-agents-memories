# 0006 — Episodic consolidation: milestones, promotion and the retained source summaries

- **Status:** Accepted
- **Date:** 2026-10-20
- **Wave:** W4.3 (episodic consolidation & compaction; issue 17 — US 1.3 compaction half, US 1.5)
- **Amends:** nothing. **Builds on:** `docs/adr/0005-episodic-storage-model.md` (the `episodic.db` storage model, ADR-0005 D-1 … D-17).
- **Blueprint:** `20261020-1223-episodic-consolidation-r3` (sha256 `a5a92ae03233`, SYS-APPROVED 2026-10-20T12:36Z).

## Context

W4.1 landed the episodic `agent_sessions` table, W4.2 its soft-delete, orphan
tracking, timelines and recall. Nothing yet CONSOLIDATED those rows: the
episodic tier grew monotonically, every session kept its own embedding, and
the durable knowledge a session produced (its **decisions**) never reached the
permanent project memory tier.

W4.3 adds one age-based job that folds the sessions older than
`episodic.consolidationAgeDays` (config, default 30) into ONE milestone row
per `(project_id, agent_id)`, promotes the decisions those sessions carried
into the permanent memory tier, and removes the merged source rows in ONE
atomic write batch. The storage change is one additive migration (v3,
`episodic_milestones`); there is no new crate and no `mem-profile` edge.

Three questions were genuinely open and are recorded here: what happens to
the source rows, how the promoted facts are selected, and how the agent
attribution survives compaction.

## Decision

### D-18 — Milestones are per `(project_id, agent_id)`, and this wave ships no scheduler

A milestone is the surviving record of one consolidated
`(project_id, agent_id)` window. Per-agent (not per-project) so every
episodic row keeps exactly ONE agent id — the agent identity is the thing
compaction must preserve.

This wave ships NO timer and NO host hook: the composition root (W5.3) calls
`Consolidator::run`. The W3.3 daily cleanup/decay gate is profile-only and is
NOT reused; consolidation never touches profile tables, so the two lanes
operate on disjoint rows.

### D-19 — The skip/abort split is fixed

**Per-agent skip** (the agent is skipped, NOTHING is written for it, and the
run CONTINUES with the next agent): summarizer error, empty summary,
oversize summary, zero fitting transcript rows, dimension mismatch (milestone
or promotion embedding), metadata decode failure, contributors JSON over its
bound.

**Run abort** (the whole run returns `Err`; no further agents are processed;
no episodic batch is attempted): store errors, sink errors, batch errors.

Phases (a)–(d) of the per-agent pipeline perform ZERO writes when they fail.
The sink is never called before the summary is accepted, and the
contributors-JSON bound is checked BEFORE the first promotion. Promotions
always precede the episodic batch.

### D-20 — The promotion rule: every decision text, exact-deduped

Every decision recorded in a consolidated session is a candidate. There is NO
LLM-based fact extraction in this wave. The selection is implemented in
D-28.

### D-21 — The promoted memory's `memory_type` is the free-form string `"decision"`

The column is a plain TEXT column in the `memories` table and an
`Option<String>` in `MemoryInsert`. There is NO enum, NO registry and NO
"house vocabulary" for it anywhere in this repo (existing tests write
`"note"` and `"project"`). `"decision"` is therefore simply an unconstrained
string chosen for this feature: nothing validates it and nothing else depends
on it.

### D-22 — The summarizer is fail-closed

There is no deterministic template fallback. A summarizer failure, an empty
summary and an oversize summary are all per-agent skips: the agent's rows
stay untouched and the run continues. A milestone is never written from a
summary that was not accepted.

### D-23 — Decision 1: the source rows are HARD-deleted (USER-APPROVED 2026-10-20)

**USER-APPROVED 2026-10-20 — "(c) Hard delete + keep source summaries in the milestone".**

The source rows are hard-deleted, irreversibly, in the SAME atomic batch as
the milestone INSERT; the approved form retains each source session's own
privacy-stripped `summary` verbatim in the milestone's contributors JSON, so
the raw per-session text stays recoverable from the milestone after the row is
gone.

**Grounds (honestly restated).** The issue file states acceptance criteria
only — it does NOT state a growth goal; the roadmap §W4.3 wording is *"merge
episodic logs older than N days into milestone summaries"*, and the hard
delete is what performs that merge: the source rows are replaced by their
milestone. **The growth claim is WEAKENED, stated explicitly:** the hard
delete bounds the LIVE-row population **only for agents that consolidate
successfully**. An agent that is permanently skipped — an over-bound
contributors JSON (the bound depends only on the source text, so it never
changes between runs), a corrupt `metadata` row, or any other per-agent skip —
keeps its live rows forever, so this wave does NOT bound the live-row
population for those agents. The soft-deleted population is never reclaimed at
all (see "Documented limitations"). The retention refinement is what makes the
irreversible delete acceptable: the per-session text survives inside the
milestone, so nothing the user wrote is lost — only the row's storage and its
embedding are.

The rejected alternatives were (a) hard delete alone (loses the raw session
text irreversibly; only the LLM summary survives) and (b) soft delete
(reversible, but soft-deleted rows are never purged in this wave, so DB growth
stays unbounded and a second purge policy would be needed later).

### D-24 — The in-process gate is a process-wide registry keyed by the canonical storage path

`crates/mem-episodic/src/gate.rs` holds
`static REGISTRY: OnceLock<Mutex<HashMap<PathBuf, Arc<AtomicBool>>>>`.

**Std only** (`std::sync::{OnceLock, Mutex, atomic::AtomicBool}`,
`std::collections::HashMap`, `std::path::{Path, PathBuf}`): the gate is a
TRY-acquire (busy ⇒ skip), so an async mutex would add a dependency for no
benefit. `mem-episodic` therefore carries NO tokio edge.

The key is the CANONICAL storage path: `std::fs::canonicalize(path)`; if that
fails (the directory does not exist yet), the nearest existing ancestor is
canonicalized and the missing tail re-appended; if that also fails, the raw
path is used. `try_acquire` gets (or creates) the `Arc<AtomicBool>` for the
key — the registry mutex is held only long enough to clone the `Arc` and is
NEVER held across an await — and then `compare_exchange(false, true, SeqCst,
SeqCst)`. `GateGuard` owns the flag and stores `false` on drop, so every path
releases the gate.

### D-25 — The busy mapping, and both locks before the first read

`Consolidator::run` acquires the in-process gate FIRST, then the cross-process
operation lock
`acquire_turso_operation_lock(store.storage_path(), deps.probe.clone(), "episodic-consolidation")`.

- `Err(StoreError::MigrationLocked { .. })` ⇒ **busy**.
- `Err(StoreError::Io { kind: Some(ErrorKind::AlreadyExists), .. })` ⇒ **busy**.
- ANY other lock error ⇒ **run abort**.

Busy is NOT an error: `Ok(ConsolidationReport { busy: true, ..default })` with
ZERO database reads after the lock attempt, zero summarizer calls, zero sink
calls and zero batch writes. BOTH locks are acquired BEFORE the first read of
the run (before `eligible_agents`). The operation lock is released explicitly
at the end of the run (`TursoOperationLock::release()`), with `Drop` as the
fallback; the gate guard is released by `Drop` on every path.

The cross-process case belongs to mem-store's operation lock — stale-lock
recovery is NOT hand-rolled here.

### D-26 — The shard `vector_count` drift after a failed increment is accepted, bounded and reconciled

If the increment fails, `promote` returns `Err`: the memory row IS written
and the shard's `vector_count` is NOT incremented (a drift of exactly one
row). The sink MUST NOT bump on the `AlreadyPresent` path, so a re-run after a
failed bump never double-counts. The drift is reconciled automatically: the
NEXT `get_write_shard` call for that scope re-syncs `vector_count` from the
shard's `memories` table (`ShardManager::sync_shard_vector_count`), which
corrects the count to the true number of rows before the next insert + bump.

### D-27 — Source-summary retention and the contributors bound

Each source session's privacy-stripped `summary` is retained verbatim in the
milestone's contributors JSON. The `contributors` column is already
`TEXT NOT NULL` and already holds that JSON, so the retention needs NO schema
change (no extra column, no v4 migration).

The bound is
`max_contributors_bytes(row_count) = MAX_TRANSCRIPT_BYTES + 1_536 × row_count`,
measured on the **ASSEMBLED JSON** (`encode_contributors(...).len()`).

**The 1_536-byte-per-row framing allowance does NOT cover the JSON escaping
expansion.** A `"` or `\` becomes 2 bytes and any other control character
becomes the 6-byte `\u00XX` form, so the ESCAPED text can be up to 6× the raw
text and is NOT bounded by `MAX_TRANSCRIPT_BYTES`. The RAW retained text IS a
subset of the transcript bytes (it is the very text the one stripping path
puts on the transcript's `Summary:` line, and the transcript of the
consolidated rows is bounded by `MAX_TRANSCRIPT_BYTES` through `fit_prefix`),
but the ESCAPED text is not. What makes the check airtight is that the
measured quantity is the exact byte length of the string that gets bound into
the `contributors` column — so no escaping expansion can slip past it. The
allowance covers the per-row JSON framing on top of the text
(`session_row_id` ≤ 256, `session_id` ≤ 256, `parent_session_id` ≤ 256,
`agent_id` ≤ 128, `agent_name` ≤ 128, the `created_at` digits, the JSON keys
and the punctuation) and scales with the row count.

An over-bound contributors JSON is a **per-agent skip**
(`SkipReason::ContributorsTooLarge`): nothing is written for that agent, its
rows stay untouched, and the run continues. A retained summary is NEVER
truncated and a contributor is NEVER dropped to fit.

### D-28 — The promotion selection algorithm (singletons included)

`select_promotable_facts(project_id, rows)` is PURE (no I/O, no clock, no
randomness) and deterministic:

1. the candidate list is built in FIRST-APPEARANCE order — for each row
   (input order) and each decision of that row (decision order), the content
   is privacy-stripped and trimmed, a blank content is SKIPPED, and the
   candidate id is `"{row_id}#{decision_index}"` (the row id rides IN the
   candidate id, so `source_row_ids` is recoverable with NO out-of-band
   index→row map);
2. the candidates are grouped by normalized key (lowercased, whitespace runs
   collapsed), preserving the FIRST-APPEARANCE order of the keys;
3. EVERY group yields exactly one fact. A **single-member** group is promoted
   directly, because `mem_core::dedup::dedup_groups` skips groups with fewer
   than two members (`crates/mem-core/src/dedup.rs:99`; the module doc at
   `dedup.rs:74` states exact groups are *"emitted only when it has ≥ 2
   members"*) and would therefore drop the normal case — a decision stated in
   exactly ONE session. A **multi-member** group is handed to
   `dedup_groups` as `DedupCandidate { id, key, embedding: None }`: all
   members share one key and carry no embedding, so the call returns exactly
   ONE `DedupKind::Exact` group whose member ids are the group's member ids
   in first-appearance order;
4. the fact's `content` is the FIRST member's content and its
   `source_row_ids` are the DISTINCT row ids parsed from the member ids
   (two identical decisions inside the SAME session therefore list that
   session's row id once). The candidate-id parse assumes the source row id
   contains NO `#`: episodic row ids are UUIDv7 values minted by the store,
   so they never do;
5. the fact id is `"episodic-decision-"` plus the first 32 hex characters of
   `sha256(project_id + "\u{1f}" + key)` — deterministic, so promotion is
   idempotent across runs.

The selection sees the CONSOLIDATED rows (`rows[..k]`) — never the whole
claimed set.

### D-29 — The agent-attribution precedence matches upstream exactly

`resolve_session_agent` is a pure port of upstream `resolveSessionAgent`
(`example/opencode-mem/src/index.ts` L75-140):

1. **the session agent wins outright** — a non-blank `session_agent` is
   returned TRIMMED, with no message scanned;
2. otherwise **ONE newest→oldest scan** over the messages, where
   - a **user** message contributes its `agent` (trimmed) when non-blank;
   - an **assistant** message is SKIPPED when `summary === true` or its mode
     is `"compaction"`; otherwise it contributes `agent ?? mode` (upstream's
     v2 assistant `agent` is preferred over `mode`);
   - an assistant message with NEITHER a non-blank `agent` NOR a non-blank
     `mode` contributes nothing and the scan CONTINUES;
   - every other role is skipped;
3. otherwise `None`.

The single scan is load-bearing: upstream runs ONE loop, so a NEWER assistant
message outranks an OLDER user message (a two-pass implementation would
return the older user agent and would lose the v2 assistant `agent` — exactly
the identity this feature preserves). No divergence is recorded, because the
upstream file is the behavioral contract for the port.

## Documented limitations

- **Milestones have no retention or pruning in this wave.** Nothing here
  deletes, ages out or compacts a milestone row, so `episodic_milestones`
  grows monotonically with the number of consolidations. The retained source
  summaries make each milestone row larger than a summary-only milestone would
  be; the contributors-JSON bound caps that growth per milestone, but the
  NUMBER of milestones is unbounded in this wave.
- **Soft-deleted rows are never reclaimed by this wave.** A row whose
  `deleted_at` is set is invisible to the eligibility query, so it is never
  claimed, never summarized and never purged. The growth bound applies to
  LIVE rows only.
- **An over-bound contributors JSON is a PERMANENT per-run skip.** The bound
  depends only on the source rows' own text, which does not change between
  runs, so the agent is skipped on EVERY run: its live rows are never claimed
  away, never summarized into a milestone and never deleted. The operator must
  shorten the offending source summaries (or raise the bound).
- **A permanently corrupt `metadata` row blocks its agent's consolidation**
  for as long as it exists (the agent is skipped every run). Repair or remove
  the row; this wave adds no quarantine path.
- **The promoted rows live in the permanent memory tier**, whose retention is
  not this wave's concern.

## Port-level deviations (recorded, not silently taken)

- **`futures` is a direct dependency of `mem-episodic`.** The
  `AiProvider::complete` seam returns a `futures::Stream`; neither
  `core::future::Stream` nor `std::future::Stream` is nameable on stable
  Rust (verified by compiling both on rustc 1.98.0) and no crate in this
  workspace re-exports the trait, so draining the stream needs the `futures`
  crate. It is already a workspace dependency (`mem-ai` uses it), so no new
  crate enters the build graph; the ADR-0001 mem-* edge graph is unchanged
  (`tests/workspace_deps.rs` stays green) and the crate still carries NO
  tokio edge and NO `mem-profile` edge.
- **`ShardManager::get_write_shard_allowing_own_lock` (new, additive).**
  `get_write_shard` refuses to resolve a shard while ANY live operation lock
  exists — including one the CURRENT process holds. The consolidation run
  holds that lock for its whole duration (D-25) and its promotion writes land
  in a shard, so without this seam the consolidation could never promote
  anything: its own marker would block it. The new method tolerates only a
  lock held by the current process; a lock held by ANOTHER process still
  blocks the write, and `get_write_shard` itself is unchanged (so the
  existing "a lock blocks even its own holder" contract stays green). The
  promotion sink uses `get_write_shard` first and falls back to the tolerant
  path ONLY when the lock is its own.
  - **Ownership is decided by PID ALONE.** `LockState` carries no host
    identity, so "is this lock ours?" can only compare the recorded pid with
    `std::process::id()`. Two residual cases follow from that, and they are
    recorded here rather than left implicit:
    1. **Stale-pid reuse.** A lock file written by a dead process whose pid
       was later recycled to ours reads as "live" (the probe asks the
       host-local `/proc`). This is benign here: the run's own lock
       acquisition treats that file as busy, and the existing staleness path
       unlinks it.
    2. **Same pid on a HOST-SHARED storage root.** A live foreign process on
       another host sharing the same filesystem and carrying the same pid
       would be mistaken for our own lock — a fail-open. This is out of the
       supported deployment (one storage root per host). The fix that would
       close it — a per-run token in `LockState` instead of a bare pid — is
       DEFERRED: it changes the on-disk lock format, which is shared with the
       TypeScript upstream.
- **The eligible-sessions statement is a builder function**
  (`mem_store::episodic_sql::eligible_sessions_for_agent_sql()`) because its
  projection is the shared `SESSION_COLUMNS` list; the file's other
  projection-dependent statements (`timeline_sessions_sql`,
  `semantic_timeline_sessions_sql`) are builders for the same reason.
- **An embedder failure aborts the run.** The error-class matrix has no
  embedder row; an embedder failure is an infrastructure failure, so it is
  classified with the store/sink/batch class (run abort) rather than as a
  per-agent skip.
- **The contributors JSON is decoded on the read path.** `Milestone.contributors`
  is a `Vec<Contributor>`; the JSON is encoded before the INSERT and decoded
  in `list_milestones`. A row whose contributors JSON cannot be decoded is
  skipped with a warning — the list path fails soft, like `list_sessions`.
