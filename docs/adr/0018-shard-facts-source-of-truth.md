# 0018 — Shard facts: the shard owns its count and identity; `metadata.db` is a cache

- **Status:** Proposed
- **Date:** 2026-10-28
- **Builds on:** `docs/adr/0011-hot-cold-memory-split.md` (tier movers),
  `docs/adr/0013-production-error-handling-panic-free.md` (no `unwrap`/`expect`; typed outcomes),
  `docs/adr/0021-schema-version-discipline.md` (the runner that makes a v3 file safe to exist).
- **Blueprint:** `bp-20261027-2353-issue43-w2-shard-v3-r4` (sha256 `8aceba1798e2`,
  `SYS-APPROVED 2026-10-28T00:01Z`).
- **Wave:** issue 43 W2, base `dc5bcad`.

## Context

Before W2 a shard's vector count lived **only** in the `metadata.db` registry, and a shard's embedding
identity lived in two loose `shard_metadata` keys (`embedding_dimensions`, `embedding_model`) that
`init_shard_db` rewrote with `INSERT OR REPLACE` on every open. Two consequences mattered:

1. **A model change with equal dimensions was invisible.** `MigrationService::detect_dimension_mismatch`
   returns `needs_migration: false` when dimensions match, so swapping `Xenova/nomic-embed-text-v1` for
   another 768-dimension model left every stored vector in a foreign latent space with a working,
   meaningless cosine distance. Nothing recorded which model produced a vector.
2. **A lost registry row could blind the only detector.** `create_shard` initializes whatever file
   `get_connection` opens, so a populated shard file whose `metadata.db` row was lost was routed through
   `init_shard_db`, which replaced its stored model with the configured one. Any later "fingerprint absent
   + model equal → not a mismatch" rule would then classify a foreign-model shard as matching.

W2 ships the **schema and the write**: a one-row `shard_stats` table inside each shard, an
`embedding_fingerprint` key in `shard_metadata`, the version-discipline refusal that makes a v3 file safe
to exist, and the consumption of both facts by the existing staged re-embed. W2 deliberately does **not**
change which model a read or a write trusts: `is_shard_valid` keeps comparing dimensions only,
`resolve_write_shard` keeps the registry's `vector_count` as the rotation input, and no search path learns
about fingerprints. That is W3.

## Grounds

### Grounds 1 — `shard_stats` is the shard's own truth, not a registry cache

The registry's `vector_count` is read on every `detect_dimension_mismatch` and was the rotation input. A
shard-local, absolute count lets W3 trust the file it is about to read without a registry round trip, and
gives W7 a retirement oracle that belongs to the file it deletes. The shape
`(id INTEGER PRIMARY KEY CHECK (id = 1), vector_count INTEGER NOT NULL CHECK (vector_count >= 0))` is
already pinned as an engine fact by W0's
`test_check_violation_inside_a_write_batch_is_an_error_and_rolls_back`.

### Grounds 2 — the two serde facts this ADR's design turns on (measured at `config/schema.rs`)

`EmbeddingPooling` has a **manual** `impl Deserialize` (`crates/mem-core/src/config/schema.rs:35`);
`LocalOnnxDtype` has **no `Deserialize` impl** at all (it offers `as_str` / `from_str`). **Neither**
implements `Serialize`. Deriving `Serialize` on either would be a config-module change for a storage
concern, so `EmbeddingIdentity::fingerprint()` serializes a **private** `FingerprintV1` view with borrowed
`str` fields instead. This is the corrected reading of the design's wording ("both derive `Deserialize`
only"): one is manual, the other absent, and neither is serializable.

### Grounds 3 — the JSON field order is part of the format

`serde_json::json!` builds a `Map` whose keys are sorted, so the fingerprint would be alphabetical and
undocumented. The private view declares `v, model, dims, pooling, dtype, task_prefixes, query_prefix,
document_prefix` in that order, no `skip_serializing_if` (an absent dtype renders `null`), and the bytes
are pinned by `crates/mem-core/tests/embedding_identity.rs`.

### Grounds 4 — the re-embed is the only transaction that may assert provenance

The only place a *false* claim is possible is the re-embed, where vectors are rewritten under a new
identity. There the claim is in the same `BatchMode::Write` batch as the inserts (claim first, count last),
so a crash between them is impossible. On the normal write path the claim is restricted to the **empty**
case, where a crash between claim and insert leaves a claim that is still true.

## Decisions

**D-18-1 — A shard owns its absolute count and its embedding identity; `metadata.db` is a cache.**
`shard_stats.vector_count` and `shard_metadata.embedding_fingerprint` are the shard's facts. The registry's
`vector_count` remains a **cache** that W2 keeps truthful on the write path (one observed count feeds both
writes, D-7) and W3 will validate before trusting. Nothing in W2 changes rotation's input.

**D-18-2 — `shard_stats` is written only from `COUNT(*)`, absolute, never a delta, never a placeholder.**
The v3 migration creates the table and writes **no row**: a placeholder `0` before the count is known would
be a W7 reaping trigger (a retired shard whose own count is zero is deleted). The row exists only with a
value measured by `COUNT(*)` in the statement that writes it. Every writer writes the measured total, so a
lost or skipped write is repaired by the next write; `increment_vector_count`/`decrement_vector_count` are
untouched (a delta split across a wave boundary is an untestable drift window).

**D-18-3 — The fingerprint is a typed, versioned, credential-free JSON document.**
`{"v":1,"model":…,"dims":…,"pooling":…,"dtype":…,"task_prefixes":…,"query_prefix":…,"document_prefix":…}`.
`v` is the format: a future change produces a different string, hence a mismatch, hence a re-embed — the
fail-closed direction. No credential and no API URL is a field: a URL can carry a token in its query
string, and the string is written into a plaintext shard that backups copy (residual R-3). No NUL bytes:
the existing `mem_embed::EmbeddingService::fingerprint()` is a **cache key** with its own pinned bytes
(residual R-4), not this identity.

**D-18-4 — The claim rule: absent AND zero rows.**
`prepare_active_shard` runs `ensure_v3 → read_fingerprint → sync_count` and then decides:

1. key **present** → `AlreadyPresent`, even when the shard is non-empty (the read precedes every claim
   attempt);
2. key absent + rows > 0 → `WithheldNonEmpty { rows }` — provenance is unprovable, so nothing is recorded;
3. key absent + rows = 0 → `claimable_fingerprint` → `claim_identity`. A claim's `changes = 0` after a
   clean read is only a lost race, reported as `AlreadyPresent`.

The claim itself is `INSERT OR IGNORE … SELECT … WHERE NOT EXISTS (key) AND NOT EXISTS (memories)`: `OR
IGNORE` turns a lost race into a no-op (the primary key is `shard_metadata.key`), the first `NOT EXISTS`
makes a present key immutable, and the second restricts the claim to the empty shard.

**D-18-5 — D-16: the two legacy identity keys are write-once (`INSERT OR IGNORE`).**
`init_shard_db` writes `embedding_dimensions` and `embedding_model` with
`INSERT OR IGNORE` — a **deliberate divergence from upstream's `INSERT OR REPLACE`**
(`example/opencode-mem/src/services/turso/shard-manager.ts`, frozen strings in
`crates/mem-store/src/sql.rs` and `tests/golden_sql.rs` moved together). A populated file whose registry
row was lost therefore keeps its stored model and dimensions when `create_shard` re-registers it, and
`detect_embedding_mismatch` sees the foreign model instead of a rewritten match. The staged rebuild runs on
a **fresh** temp file and still stamps the target identity. D-16 makes no comparison and refuses nothing:
W3 still owns every read/write refusal.

**D-18-6 — The supported way out of a model change is `reembed_scope`.**
`detect_embedding_mismatch` judges one shard with three cases: a present fingerprint mismatches iff it
differs from the configured one; an absent fingerprint with a different stored model is a mismatch; an
absent fingerprint with an absent or equal model is **not** (legacy, assumed matching — not proof).
`reembed_scope(scope, scope_hash)` reuses the guard chain (`is_running`, operation lock, per-shard scope
write lock, staged rebuild/verify/swap) and returns the same typed `MigrationResult`. It short-circuits
**before staging** only when a shard's fingerprint is **present and equal** — never on
`detect_embedding_mismatch().is_empty()`, because a populated pre-W2 shard has no fingerprint and an equal
model and must still be rewritten. `migrate_to_new_model(ReEmbed)` unions dimension mismatches with
identity mismatches; `FreshStart` stays dimension-only, so a model rename can never silently archive a
scope.

**D-18-7 — The narrowing (R-12).**
The design's W2 observable ("a v2 shard opened by the writer gains a correct `shard_stats` row **and the
fingerprint**") was too strong. The honest observable is: **every written shard gets a correct
`shard_stats` row; the fingerprint is claimed only where it is provable (empty shard).** A populated
pre-v3 shard is reported as `WithheldNonEmpty { rows }` until `reembed_scope` rewrites it. W2 records;
W3 verifies; the gate ruled this justified, not a scope reduction.

**D-18-8 — Hot-cold rider (ADR-0011).**
Any future tier mover that copies rows between shards maintains **both** facts on the destination: it
re-syncs `shard_stats` from `COUNT(*)` and, if it moves vectors across identities, never copies a
fingerprint it did not prove — the destination starts unclaimed unless the mover rewrites the vectors under
the configured identity.

**D-18-9 — Model authority on reads: selection-time, typed, no partials.**
`MemoryPort::verify_shards(&[ShardRef])` is REQUIRED (no default). `memories::search` calls it once per
shard list, in both branches, immediately after `shards(…)` and before any `search_in_shard` or prompt
query. `ProductionMemory::verify_shards` reads each shard's `shard_metadata` facts, judges them, collects
**every** mismatched shard of the list, and refuses with one `PortError::ModelMismatch`, answered **409**
with the typed Display (ids, model names, the command). A shard whose facts cannot be read is logged and
skipped — today's per-shard tolerance, not a mismatch. No partial answer is ever returned.
**Numbering note:** the ruling's "D-18-7…D-18-10" was written before W2 landed and is superseded by the
recorded D-18-7/D-18-8; W3's decisions are D-18-9…D-18-12.

**D-18-10 — Model authority on writes: every vector producer, at the write seam; deletes stay open.**
`shard_facts::prepare_write_shard` reads the facts once, judges, and on a positive mismatch returns
`StoreError::ShardModelMismatch` **before** W2's claim/count body runs (a refused write mutates nothing).
`resolve_write_shard` uses it, so capture, import, API insert and episodic promotion inherit the refusal.
`ProductionMemory::update_memory` (the `PUT /api/memories/{id}` hole) verifies at the port before its
delete+insert batch; it is total and fail-closed (no registry id or an unreadable shard → the same typed
refusal, never a silent write). `delete_memory`/`pin`/`unpin` are **not** refused: erasure must not depend
on a re-embed. The handler embeds before calling the port, so a refused PUT spends one embedding call —
recorded, accepted.

**D-18-11 — An absent fingerprint falls back to model equality, and equality is not proof.**
`judge_model_facts` is pure and total: a present fingerprint wins over the model; with no configured
identity the model is still compared (a build without a fingerprint check is not blind to a named foreign
model); an absent fingerprint with an absent or equal model is `ModelEquality` — allowed, warned, never
called provenance. `legacy-unknown` is a named foreign model, not an absent one, so an upgraded install is
refused until re-embedded. The warning is observable without a dev-dependency
(`tracing::subscriber::set_default`) and is pinned by the capture tests in both crates, so AC6b's
`testable: no` is withdrawn.

**D-18-12 — The count is measured at commit; the registry adopts it; the deltas are gone.**
Every row-count-changing `BatchMode::Write` batch ends with `SYNC_COUNT_SQL` (no binds — the statement
measures `COUNT(*)` itself), and the caller adopts the committed value into the registry cache
(`ShardManager::adopt_stored_count`). `delete_memories(ids)` is one transaction of N deletes plus the sync
and returns the read-back; an empty list is a read-only no-op. `increment_vector_count` /
`decrement_vector_count` and their SQL constants are deleted; promotion's insert stays a single `execute`
and resyncs with the next statement (R-4).

**D-18-13 — The identity-facts read is cached per open connection.**
The authority used to issue one `SELECT key, value FROM shard_metadata …` per shard per
`GET /api/search`; measured at 105–142 µs that was 9–12 % of a recall, over R-13's pre-registered 5 %.
`ModelFactsCache` serves a hit only to the EXACT handle it was read through (`Weak<dyn Db>` +
`Arc::ptr_eq`), so a close+reopen of the same path can never serve the previous file's facts — even if a
slow reader re-caches after the close, its entry carries the old handle. The five invalidation points:
(1) a successful open and (2) a close (`ConnectionManager::open_database`/`close_one`); (3)
`close_all`/`ShardManager::reset` clear the map; (4) the explicit writers of `shard_metadata` invalidate
after commit — `create_shard` after `init_shard_db` and again after a `Claimed`, the write seam's claim in
`prepare_write_shard`, and the two `legacy_migrator` writes (the re-embed swap is covered by its closes);
(5) the writer rule is documented on `init_shard_db` and `claim_identity`: **any writer of
`shard_metadata` invalidates the path.** Verified by three tests (serve-until-invalidated, claim
invalidation, reopened connection) with their named mutations, each RED. Measured (release, 500 rows,
768 dims): cached `0.07 µs`/shard vs uncached `104.66 µs`; `delta_pct = 0.01 %` (gate < 5 %),
`uncached_delta_pct = 11.08 %` (the control still measures the same quantity).

## Residuals

- **R-3** — a same-name remote endpoint or ONNX-file swap is not caught: the authority refuses only on
  positive evidence of a difference (a differing fingerprint, or a differing model name). An equal model
  name is not proof. A build without an identity compares the model and warns.
- **R-5** — the only remaining gap is promotion's single-execute insert: its resync is the next statement,
  outside that execute's window (R-4); every delete path writes the absolute count in its own batch.
- **R-6** — unchanged: retired shards may carry no `shard_stats` row until opened; the model authority
  reads `shard_metadata`, never `shard_stats`, so search is unaffected. `delete_memories` is unbounded and
  issue 72 must chunk.
- **R-7** — no `.bak` for shard migrations; v3 is additive and convergent, and a v2 binary ignores the
  extra table.
- **R-12** — an upgraded install (`legacy-unknown`, or any foreign model) is refused, by design; the way
  out is the operator command the error names: `opencode-agents-memory --mode=reembed-scope --scope
  <container tag>`.
- **R-13 — resolved by D-18-13.** Six pre-fix runs landed at 9.14–11.51 %; the per-connection cache
  measures `delta_pct` 0.01 % with the uncached control at 11.08 % (release, 500 rows, 768 dims). The
  pre-registered "move the read to the shard-open path" was realised as the per-connection cache at the
  same read seam, so no signature outside `prepare_write_shard` changed.
