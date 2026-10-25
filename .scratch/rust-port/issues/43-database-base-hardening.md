# 43 — W9.0 · Database base hardening (review findings)

Status: needs-triage — 2026-10-25. **Blocks issue 39 (retention cleanup) and issue 41 (scheduler).**

## RE-VERIFICATION 2026-10-25 (architecture pass) — read this before the items below

An architecture review re-derived every claim from the tree and from turso probes. **Four claims in this
issue do not survive** — three of them mine. The items below are retained as history; the rulings live in
`.tmp/drafts/20261025-database-base-hardening-design.md` and in the wave table there.

| claim as filed | verdict |
|---|---|
| 43.3 "`memories` records no embedding model" | **True of the row, false of the shard.** `shard_metadata.embedding_model` exists (`sql.rs:428-431`, written at `shard_manager.rs:585-608`). The real defect: **nothing reads it**, and `INSERT OR REPLACE` **overwrites** it. Fix = verify, don't just record. |
| 43.2 "`vector_count` drift makes rotation early or late" | **False.** `resolve_write_shard` → `sync_shard_vector_count` → `COUNT(*)` → absolute `SET` on **every write** (`shard_manager.rs:681-702, 808`). Drift self-heals on the write path; the residue is that `shard_path_migration.rs:618` makes a decision from the count without reconciling. |
| 43.6 "`user-prompts.db` has no schema version" | **False as stated.** `prompt.rs:147-161` runs the shared runner and `schema_migrations.rs:143-157` stamps `user_version = 1`. The real gap: `ensure_user_prompt_columns` runs three `ALTER`s on every open **outside** the version mechanism and swallows every non-duplicate error. |
| 43.4 "`secure_delete = ON` fixes the WAL residue" | **The pragma does not exist in this engine.** Probe: set returns `Ok`, read returns **zero rows**, and the canary survives in the main file after DELETE + `wal_checkpoint(TRUNCATE)`, with or without it. Recovery is `wal_checkpoint(TRUNCATE)` + `VACUUM` under `experimental_vacuum(true)` (without the flag `VACUUM` is a **parse error** — so the existing best-effort VACUUM can never have succeeded). |

**Two things this issue understated:** 43.1 has **two** deterministic-id writers (`promotion.rs:344` as well
as `capture.rs:212`); and the review-prescribed "id index in `metadata.db`" would **fail open** — a lost index
row *is* the duplicate it exists to prevent — so the ruling is a scope scan on the write path, measured at
53 µs (k=1) / 131 µs (k=5) / 453 µs (k=20) with cached handles.

**Measured, and it kills a proposed re-layout:** 3 000 rows × 768 dims, two vectors/row — inline 24.7 MB /
26.8 ms cold / 12.3 ms warm; narrow companion table **28.8 MB** / 20.1 ms / 11.8 ms, +1.76 ms hydrate. Warm
delta **4 %**, and the narrow file is **larger**. Both run at ~4 µs/row: **compute-bound on
`vector_distance_cos`**, which a table split cannot fix. Real root: 19 shards, 2 341 rows. The cheaper lever
is `max_vectors_per_shard` (currently 50 000 ⇒ ~400 ms per query before rotation).

**Wave sequence (design §2):** W0 engine-capability tests → W1 id resolution → W2 shard v3 (`shard_stats` +
fingerprint) → W3 count/model authority → W4 one-active index → W5 prompts v2 → W6 erasure → W7 retired-shard
reaping → W8 benchmark gate. W4 and W5 are parallel-safe; W2→W3 is a strict schema-then-authority boundary.

**ADRs to write:** 0017 scope-wide id resolution · 0018 the shard is the source of truth for its own facts ·
0019 erasure posture · 0020 cross-file backup ordering · 0021 schema version discipline · 0022 one active
shard is a constraint · 0023 retired-shard reaping · 0024 brute-force scan cost, measured.

---

## OWNER DECISIONS 2026-10-25 (bind the wave sequence)

| # | decision | consequence |
|---|---|---|
| 1 | **43.3 — include a re-embed path now** | W2/W3 grow: the fingerprint check must ship *with* a supported way to re-embed a scope, not with a bare refusal. This is the largest scope addition in the set. |
| 2 | **43.8 — typed refusal, hard error** | A root written by a newer build refuses to open (`SchemaNewerThanBuild`). A rollback bricks the root until the newer build returns — intended, loud, and a release-process constraint. |
| 3 | **Never shared with upstream.** *"we are building something new, the upstream is just ref"* | No compatibility contract. Shard v3 and prompts v2 are **ours alone**; W2 and W5 need no downgrade note, and upstream's TypeScript build is a behavioural reference only, never a co-tenant of the storage root. |
| 4 | **43.10 — 100 ms p95 budget** | `2 × cap × 4 µs ≤ 100 ms` ⇒ `max_vectors_per_shard ≈ 12 500` (from 50 000). One config line, no schema change; shard count grows. Record the arithmetic in ADR-0024 so the number is auditable. |
| 5 | **43.11 — reaping allowed, no undo** | W7 may delete retired shard files under its safety conditions (inactive, own count zero, opens cleanly, not the highest index), registry row first. The only wave that destroys user data. |

Decision 3 is the cheapest: it deletes a whole compatibility dimension from W2 and W5.


## W0 LANDED 2026-10-25 — `af01156` (engine facts pinned)

`crates/mem-store/tests/turso_capabilities.rs`, 22 tests, `mem-store` 194 -> 216. Test-only; no
production file, no manifest, `sql.rs`/`golden_sql.rs` untouched. Reviewer APPROVED after five
independent inversions of its own; the two previously-rejected vacuous controls (AC6's fixture,
AC3/AC4's missing distinct `shard_index`) are verifiably gone.

**Two facts that bind the later waves:**

1. `Connection::execute` compiles only the **first** statement of a multi-statement string — it
   returns `Ok` and applies nothing after it. Any multi-statement sequence must go through
   `Db::batch(BatchMode::Write)`. Pinned by AC2.
2. A constraint raised **inside** `BatchMode::Write` surfaces as `BatchStatementFailed`, which
   `StoreError::from` does not classify, so `is_constraint == false`. W4's rider is sound on
   `execute` paths; **W2's `CHECK (vector_count >= 0)` fires inside a batch and must not expect
   the classifier.** Pinned by AC6/AC7/AC8.

**OPEN OWNER DECISION (blocks W6's erasure posture, not W0):**

`VACUUM` cannot run in the configuration production builds. With `experimental_vacuum(true)` **and**
multiprocess WAL enabled, the engine refuses it at translation
(`VACUUM is incompatible with experimental multiprocess WAL`); production enables multiprocess WAL
on every non-Windows platform. Measured, pinned by AC15/AC17.

Consequence: the design's W6 remedy ("enable `experimental_vacuum(true)` in the connect factory —
one flag") **does not work**. Every sweep would report `PendingErase` against an operation that can
never succeed. W6 must first decide whether multiprocess WAL is dropped for the prompts database —
a production trade, not a test-wave choice. This is recorded, not decided: the design document was
deliberately left unamended so the evidence that the design was wrong survives.

---

## W6 ERASURE — PRIOR ART, AND A CONTRADICTION IN THE DESIGN (2026-10-25, vault research)

Before deciding W6, know that the owner's own ADR already ruled on this — and that the design
contradicts itself.

**ADR-0012 D-14 (2026-10-23) refuses exactly the remedy the design proposes** (`docs/adr/0012-vault-key-custody.md:539`):

> "**No `VACUUM` and no `secure_delete` in the purge path.** `VACUUM` rewrites the whole database: it
> is expensive, it needs the key, and it can fail on a busy database, which would make erasure *less
> reliable* than leaving the page images alone."

Its stated posture is **logical** deletion plus named residuals, and its argument works because the vault
database is **encrypted**, so what survives in old pages is **ciphertext**, and destroying a key generation
is the real defence.

**That reasoning does not transfer to `user-prompts.db` for free, because that database is not encrypted.**
`crates/mem-core/src/config/schema.rs:602` sets `database_encryption_enabled: false` as the default.

The design draft (`.tmp/drafts/20261025-database-base-hardening-design.md:144`) says:

> "Encryption at rest stays the real posture (default is `false`, `schema.rs:602`); VACUUM is
> belt-and-braces on a plaintext root."

**Those two clauses cannot both hold.** If encryption is off by default there is no ciphertext, so the
residue is plaintext and "encryption is the real posture" is a position nobody has taken a decision for.
Also note ADR-0012's reason for refusing `VACUUM` is **reliability** ("it can fail on a busy database"),
not absence — and the engine measurement supersedes its parenthetical: `secure_delete` does not exist at
all in turso 0.8.3, so "it does not scrub the WAL" is moot.

**Consequence for the owner's decision.** W6 is not only "drop multiprocess WAL so `VACUUM` can run?".
It is at least three options, and the third was not on the table before:

1. drop multiprocess WAL for `user-prompts.db` and `VACUUM` on sweep;
2. keep multiprocess WAL and accept that deleted prompt text is never erasable — which, with
   `database_encryption_enabled = false`, means **plaintext residue on disk**;
3. turn encryption on for that database (which changes the key-custody question, ADR-0012's subject) and
   treat the residue as ciphertext, exactly as the vault does.

The vault's own privacy page (`[[open-problem-9-privacy-model-agent-memory]]`) lists "**Right to forget**:
users can delete their data" among its privacy-by-design requirements — so option 2 is in tension with a
stated product position, not merely with a preference.

**Still not decided, and not decidable by a wave. Recorded so the decision arrives with its history.**

---

## Why this exists

Two independent reviews of `.tmp/drafts/20261025-database-design-handover.md` (2026-10-25) found defects in
the **existing** storage base that make the retention feature unsound to build on. The owner's decision:
*"Pause everything, fix the base first."* Every claim below was re-verified against the tree before filing;
two review claims were refuted and are recorded as such so they are not re-litigated.

**Two independent reviewers converged on the same conclusion about issue 39:** as designed, retention
**does nothing today** — `linked_memory_id` has zero writers, so Phase 1 reads an empty set — and both
proposed recording the prompt's creation time on the memory **at capture time**, which removes the
cross-file phases entirely. That is a separate finding, recorded in issue 39's pause note.

## P0 — correctness (a duplicate or a wrong count breaks data)

### 43.1 `memories.id` is not unique across shards

**Verified.** `memories.id` is `TEXT PRIMARY KEY`, which enforces uniqueness **per file**. Capture writes to
`get_write_shard(...)` (the active shard) and its existence check is
`SELECT id, container_tag, session_id FROM memories WHERE id = ?` **on that one shard's connection**
(`crates/mem-bridge/src/capture.rs`, the "Existence check: existing id = replay candidate" block). After
rotation the original row lives in an inactive shard, the check on the new active shard finds nothing, and the
plain `INSERT` inserts the **same deterministic id again** — the ids are
`auto-capture-<64 hex>` over `("capture-summary-v1", container_tag, capture_id)`, so re-capturing is normal
operation.

Consequences, both verified: `inject.rs` reads **all** shards
(`get_all_shards` → `open_shard` → `list_memories_by_container_tag`), so a duplicate surfaces as two hits;
and a delete by id removes one copy and leaves the other.

**Fix direction:** one of — (a) make the collision check span all shards of the scope (a scope-wide id index
in `metadata.db`, or a scan), (b) accept duplicates and make every read path deduplicate by id, or (c) close
it at the rotation boundary by refusing to rotate when an id is live in the retired shard. Decide before
retention lands, because retention deletes by id and would remove one copy.

**Note:** this refutes a claim in the handover doc ("an id is not unique over time, only at any instant").
Ids are not unique across shards at any instant either.

### 43.2 `vector_count` is a denormalised counter that can only drift

`metadata.db.shards.vector_count` counts rows living in another file. Increment on insert, decrement on
delete — and the retention design's own decrement sits *outside* the shard's write batch because it is a
different database, so it cannot be atomic with the delete it counts. The current implementation logs a
failed decrement and continues, which keeps the report honest but leaves the count permanently wrong; the
write path then rotates early or late.

**Fix direction (both reviewers agree):** reconcile instead of accumulating deltas. `SELECT COUNT(*)` inside
the shard whenever it is already open, and write the absolute value; do the same on the write path when it
opens the active shard. Optionally move the authoritative count into a one-row table inside the shard so it
is updated in the same batch as the rows it counts, leaving `metadata.db` a cache.

## P1 — silent corruption / data retention

### 43.3 The embedding model is not recorded

**Verified.** `memories` has `vector F32_BLOB(dims)` and `tags_vector`, and no column naming the model that
produced them. `provider_id` / `model_id` exist only on `user_prompts`. If the model changes while the
dimension count stays the same, every stored vector is in a different latent space and every distance is
meaningless — with **no error**, because brute-force search happily compares them.

**Fix direction:** record model id + dimensions per shard (a `shard_info` row is the natural home, which also
makes a shard self-describing and lets `metadata.db` be rebuildable), and refuse to insert or search when the
configured model does not match.

### 43.4 Deleted prompt text survives deletion

**Verified.** `secure_delete` appears **0** times in the tree. In WAL mode a `DELETE` frees space in the
B-tree; the raw text remains readable in the database file and in `-wal` until checkpoint and vacuum. The
best-effort `VACUUM` that cleanup performs can fail silently ("DB busy"), and `user-prompts.db` holds up to
~4 KB per row of verbatim user input — the most sensitive table in the system.
`PRAGMA wal_checkpoint(TRUNCATE)` already exists in the migration path as a precedent.

**Fix direction:** `PRAGMA secure_delete = ON` on the prompts database, a `wal_checkpoint(TRUNCATE)` after a
delete sweep, and a canary test: insert a unique string, run retention, grep the `.db` and `-wal` bytes.

### 43.5 Cross-file ordering invariants are not applied to backups

A file-by-file backup can produce a state no crash could. Once a cross-file ordering invariant exists
(issue 39's "stamps before prompt deletion"), a backup that copies shards first and the prompts DB afterwards
captures exactly the unsafe pairing. **Rule:** copy downstream-before-upstream (the prompts DB first, then
the shards), and write it down where the next backup path will read it.

## P2 — schema hygiene

### 43.6 `user-prompts.db` has no schema version

Two mechanisms in one storage root: shards use `PRAGMA user_version` + an ordered migration list;
`user-prompts.db` converges through idempotent DDL on open and swallows duplicate-column errors.

**Fix direction (both reviewers, same shape, ~25 lines):** run the existing untouched DDL, then if
`user_version == 0` set it to `1`. Version 1 means "whatever upstream's DDL produces". Subsequent changes
become numbered migrations in a `user_prompts_sql.rs`. `sql.rs` and the golden tests are untouched.

### 43.7 Duplicate-column detection by error message

If the swallow is keyed on the error text, a wording change in turso either breaks startup or starts
swallowing unrelated errors (including I/O and corruption). Check `PRAGMA table_info` before each `ALTER`
instead of matching an error string.

### 43.8 Higher schema versions than this build knows

Refuse to open a file whose `user_version` is above what the build understands, for shards and prompts alike.
A downgrade should fail loudly rather than silently mis-read newer data.

### 43.9 One active shard per scope is not enforced

`shards` has `UNIQUE(scope, scope_hash, shard_index)` but nothing prevents two rows with `is_active = 1` for
one scope if rotation is interrupted. **Fix direction:** a partial unique index
`CREATE UNIQUE INDEX ... ON shards(scope, scope_hash) WHERE is_active = 1` — **verify turso supports partial
indexes first**; if not, enforce in code and test the interleaving.

## P3 — performance (measure before acting)

### 43.10 Brute-force search cost is not bounded by shard size

Capping shards bounds one file's size; a scope's query still scans every shard of that scope. With `content`,
`metadata`, `vector` and `tags_vector` inline in one row, a scan pages in text it does not need. The reviews
disagree on urgency (one says it fails first on page-cache I/O at 25–50 k memories; the other is silent), and
neither measured it here — **this needs a benchmark at 10× and 100× current data before any re-layout.**

Possible fix if the benchmark justifies it: move `vector`/`tags_vector` into a narrow companion table so a
scan reads sequential vector pages and joins back only for top-K.

### 43.11 Retired, empty shards are never removed

Retention empties old shards but nothing deletes them, and every scope-wide search and retention run still
opens them — each open also sets up encryption. Cheap win: delete empty inactive shards (or record why not).

## Refuted review claims (do not re-file)

- **"`user_prompts` has no secondary indexes."** FALSE. It has **6**, including
  `idx_user_prompts_created ON user_prompts(created_at DESC)` and
  `idx_user_prompts_linked ON user_prompts(linked_memory_id)` — the two the review prescribed as missing.

## Ordering

43.1 and 43.2 first: retention deletes by id and counts by registry, so both are load-bearing for it. 43.3
and 43.4 are independent of retention and can run in parallel. 43.6–43.8 are one small wave. 43.10 waits for
a benchmark.

## Comments

- Filed 2026-10-25 from two concurrent reviews of the database design handover. Every TRUE claim above was
  re-verified in the tree; the two refuted ones are listed so a future reader does not re-open them.
- Issue 39 (retention cleanup) is paused pending 43.1 and 43.2, and is itself re-opening its storage design
  on the reviewers' convergent finding.
