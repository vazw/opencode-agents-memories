# mem-store

**Purpose:** storage engine on the `turso` crate: shards, connection lifecycle, migrations, vector search, encryption.

**Ubiquitous language:**

- **Shard** — one SQLite db per (scope, hash, index) at `{storage}/{scope}s/{scope}_{hash}_shard_{n}.db`; registry (`metadata.db`) authoritative for index/activity; rotation at `max_vectors_per_shard`; incompatible shards raise `ShardIncompatible` ("left untouched"), never quarantined.
- **Golden SQL** — upstream statements byte-for-byte in `src/sql.rs`, pinned by `tests/golden_sql.rs` (D2 blocking rule); `is_read_query` is the batch read/write classifier.
- **Ready gate** — injected legacy/encryption steps + swap recovery + shard warmup; deduped, failed runs re-run all steps, `reset()` re-arms.
- **Operation lock** — `.turso-operation.lock`/`.turso-migrate.lock`; live PID blocks writes (`writes are temporarily blocked`); corrupt/dead = stale; release PID-guarded.
- **Swap journal** — `.path-migrate-swap.json`, written before any rename; recovery validates containment and restores exactly, removing the journal only on full success.
- **Sidecars** — `-wal`/`-shm`/`-tshm` companions move with their db; Windows lock retries use the upstream delay table.
- **Drop-based release** — `TursoDb` owns `Database` + `Connection`; no close API — last handle drop releases; manager close = remove + drop.
- **Db seam** — boxed-future `Db` (`execute`/`query`/`batch`); `BatchMode::Write` = transactional `Immediate`; vector ops are free fns over `&dyn Db`.

Cross-links: the bullets above are the pinned source of truth for every shard, ready gate, swap journal, sidecar, golden SQL and operation lock term used by later waves.
