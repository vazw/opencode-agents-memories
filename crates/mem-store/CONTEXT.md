# mem-store

**Purpose:** storage engine on the `turso` crate: shards, connection lifecycle, migrations, vector search, encryption.

**Ubiquitous language:**

- **Shard** — one SQLite db per (scope, hash, index) at `{storage}/{scope}s/{scope}_{hash}_shard_{n}.db`; registry (`metadata.db`) authoritative for index/activity; rotation at `max_vectors_per_shard`; incompatible shards raise `ShardIncompatible` ("left untouched"), never quarantined.
- **Shard facts** — the shard owns its absolute vector count (`shard_stats`, one row `id = 1`, written only from `COUNT(*)`, never a placeholder or a delta) and its embedding identity (`shard_metadata.embedding_fingerprint`). `metadata.db` is a **cache** of the count (ADR-0018). `withheld non-empty` means a populated un-fingerprinted shard: W2 records a count and no fingerprint; `reembed_scope` is the supported way out.
- **Embedding fingerprint** — canonical JSON `{"v":1,…}` from `mem_core::embedding::EmbeddingIdentity::fingerprint()` (fallible: an encoding error writes nothing, `WithheldEncoding`). No credential, no API URL, no NUL bytes; field order is declaration order. Claimed only when the shard is empty and the key is absent; a present key is never overwritten.
- **Write-once identity keys** — `embedding_dimensions`/`embedding_model` are `INSERT OR IGNORE` in `init_shard_db` (D-16, a deliberate divergence from upstream's `INSERT OR REPLACE`): a populated file whose registry row was lost keeps its own identity, so the model detector cannot be blinded by `create_shard`.
- **Version discipline** — every `apply_schema_migrations` call passes a per-set `max_known`; a file whose `user_version` is higher raises `SchemaNewerThanBuild` before the pending check (`current`, never `target`). W2 raises only the shard set (`MEMORY_SHARD_MAX_KNOWN = 3`); prompts v2 is W5's half (ADR-0021).

- **Golden SQL** — upstream statements byte-for-byte in `src/sql.rs`, pinned by `tests/golden_sql.rs` (D2 blocking rule); `is_read_query` is the batch read/write classifier.
- **Ready gate** — injected legacy/encryption steps + swap recovery + shard warmup; deduped, failed runs re-run all steps, `reset()` re-arms.
- **Operation lock** — `.turso-operation.lock`/`.turso-migrate.lock`; live PID blocks writes (`writes are temporarily blocked`); corrupt/dead = stale; release PID-guarded.
- **Cross-process operation lock** — the cross-process mutex the migrators and W3.1 profile learning rely on: PID-file lock (`create_new`), live holder never stolen, stale recovery by PID liveness (`/proc/<pid>`); the `examples/lock_holder.rs` helper drives the cross-process tests.
- **Short-write hardening** — `acquire_turso_operation_lock` `sync_all`s the lock body after writing; `read_live_lock` treats an empty/partial/unparseable lock file as stale (best-effort unlink) so a crash mid-write can never wedge writes.
- **Encryption at rest** — shards/metadata can be libSQL-encrypted (`aes256gcm` default) via turso `experimental_encryption`; an encrypted file is not a standard SQLite file (a plaintext tool cannot read it).
- **Key provisioning** — `resolve_or_create_database_encryption_key`: explicit hex / `env://` / `file://` / bare path / auto-generated; the default key file `~/.config/opencode/opencode-mem-db.key` holds 64 hex chars (32 random bytes from `/dev/urandom`), mode 0600 (parent 0700), never logged or carried in an error; key generation is fail-closed.
- **Encryption marker** — `.tursodb-encrypted-v1` at the storage root: its presence means encrypted DBs exist (blocks new-key generation) and it is written after a successful plaintext→encrypted migration.
- **Encryption migrator** — `encryption_migrator`: table-copies each plaintext candidate into a staged encrypted db, rename-atomic swap (backup kept), verifies the encrypted open, then writes the marker; wired into the ready gate between legacy migration and swap recovery (`build_ready_gate_with_encryption`).
- **Swap journal** — `.path-migrate-swap.json`, written before any rename; recovery validates containment and restores exactly, removing the journal only on full success.
- **Exact-scan fallback** — no ANN index ever; `ORDER BY vector_distance_cos(...) LIMIT k`; hybrid blend formulas.
- **Engine marker** — `.tursodb-engine-v1`; DiskANN detection; loud refusal, never a marker on refusal.
- **Legacy sidecar/marker/backup** — `.turso-migrate.json`/`.turso-migrated`/`.legacy.bak`; legacy dims preserved; abort on unreadable vectors.
- **Re-embed swap** — `.reembed-{pid}-{millis}.tmp`, `.pre-reembed-{nonce}.bak`, `.reembed-swap.json`, recovery before engine conversion.
- **ReEmbedder seam** — mem-store never depends on mem-embed.
- **MigrationService** — dims preflight + fresh-start/re-embed strategies, scope write lock.
- **Sidecars** — `-wal`/`-shm`/`-tshm` companions move with their db; Windows lock retries use the upstream delay table. Any deletion needs the exclusive root plus a `Free`/`Missing` probe before the first unlink; a foreign multiprocess holder is the documented residual (issue 46).
- **Exclusive root** — `delete_guard::ExclusiveRoot`: a live proof that this process holds the storage root's `EngineLock`; published by the lock's process registry, matched by the **exact** root path (never a prefix), and withdrawn by the last lock drop. `ConnectionManager::exclusive_root` is the lookup.
- **Delete claim** — `delete_guard::FileClaim`, minted only by `ExclusiveRoot::claim`; a remove/rename needs one per end, a copy only for its destination (the source is a documented snapshot). The probe runs **inside** the mutating function before its first unlink; `Held`/`Unknown` refuse with `ExclusivityRefused`, and a needed-but-absent proof refuses with `NoExclusiveRoot`.
- **Drop-based release** — `TursoDb` owns `Database` + `Connection`; no close API — last handle drop releases; manager close = remove + drop.
- **Db seam** — boxed-future `Db` (`execute`/`query`/`batch`); `BatchMode::Write` = transactional `Immediate`; vector ops are free fns over `&dyn Db`.

Cross-links: the bullets above are the pinned source of truth for every shard, ready gate, swap journal, sidecar, golden SQL and operation lock term used by later waves.
