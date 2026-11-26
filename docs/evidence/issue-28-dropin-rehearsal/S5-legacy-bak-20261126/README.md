# S5 `.legacy.bak` — observed 2026-11-26

Cell satisfied by **observation**, per the owner's choice of option B in
`.scratch/rust-port/issues/28-parity-matrix-rehearsal.md`. Nothing here is a substitute: the
staged binary renamed a real shard, rebuilt it, and rewrote the marker.

Labels: **MEASURED** = executed and output captured; **READ** = read from source.

## 1. Why the natural store cannot produce this cell (READ)

`crates/mem-store/src/legacy_migrator.rs`:

| Gate | Line | Effect |
|---|---|---|
| Marker + every shard complete | `:933` | whole run returns early |
| Per-shard sidecar complete | `:380`, `:386` | that shard returns early |
| **Shard already vector-ready** | `:405-416` | writes a *fresh sidecar*, returns — **no rename** |
| Rename to `.legacy.bak` | `:505-521` | conversion path only |
| Production trigger | `ready.rs:142`, `reap.rs:142`, `reembed.rs:174` | `ensure_ready()` — **not called on standalone boot** |

The brief in issue 28 proposed "delete one shard's sidecar". **That would not have worked** — a
deleted sidecar on an already-converted shard falls into `:405`, which writes a new sidecar and
returns. The shard has to be **not vector-ready** for the rename to be reachable.

The legacy shape is precisely "a `memories` table with no `shard_metadata`":
`crates/mem-store/tests/turso_legacy_migrator.rs:22-42` (`LEGACY_SCHEMA`, "NO shard_metadata
table"). Readiness (`:171-210`) requires `shard_metadata['embedding_dimensions']` and a
`vector_extract` length match.

## 2. What was done (MEASURED)

Source: `.tmp/rehearsal-28/20261125T062331Z/store` — 36 shards, 1 sidecar, 0 `.legacy.bak`,
**5,463 rows** total.

1. `cp -a --reflink=auto` into `.tmp/probe-28-s5/store` (0.345 s, reflink).
2. One real shard reduced to the legacy shape:
   `projects/project_fd2bfb63318dfd29_shard_0.db`, **2,959 rows**, `sqlite3 … 'DROP TABLE shard_metadata;'`.
   Rows, vectors and indexes untouched; only the identity table that makes it "ready" is gone.
3. **First attempt — standalone engine, spare port 3751, 81 s**: nothing happened. No sidecar, no
   rename, marker mtime unchanged. `ensure_ready()` (and therefore the legacy step) is never called
   on boot. This is a finding in its own right: **a restarted engine does not migrate a legacy
   store**; only the migration service does.
4. **Actual trigger**: `--mode=reap-shards` (dry run), empty `HOME` so no config file is read.
   `reap.rs:142` calls `ensure_ready()` before the reap predicate. `rc=0`.

## 3. Result (MEASURED)

```
BEFORE: bak=0  sidecars=1
AFTER : bak=1  sidecars=36
```

| artifact | value |
|---|---|
| `projects/project_fd2bfb63318dfd29_shard_0.db.legacy.bak` | 27,181,056 B, **2,959 rows** |
| fresh `…shard_0.db` | **2,959 rows**, `embedding_dimensions=768`, `embedding_model=Xenova/nomic-embed-text-v1` |
| `…shard_0.db.turso-migrate.json` | `sourceCount 2959 / expectedCount 2959 / importedCount 2959 / skippedCount 0 / status complete` |
| `.turso-migrated` | rewritten: `completedAt 2026-11-26T08:29:02.809Z`, 36 shard paths under the probe root |
| store row total after | **5,463** = 5,463 before (backup excluded) |
| other 35 shards | already vector-ready → sidecar written, **no rename** (`:405` branch, observed in the same run) |

`run.txt` is the reap invocation's stdout verbatim (`reaped: []`, `dry_run: true`, all 36 shards
`skipped: active`).

## 4. Caveats, stated plainly

- **The legacy shape was constructed** by dropping `shard_metadata`, not inherited from a store that
  was never migrated. What is real: the staged binary, the 2,959 real rows with their real 768-dim
  vectors, the rename, the rebuild, the marker rewrite. What is synthetic: the trigger that makes
  that shard non-ready.
- The trigger was `reap-shards`, not engine boot, because boot has no path to `ensure_ready()`.
- The probe ran entirely under `.tmp/probe-28-s5/`. Live root verified untouched afterwards:
  pid 15903 alive at `/api/health` 200, marker mtime still `Nov 16 18:12`, 0 probe paths in it,
  0 `.legacy.bak` under `~/.opencode-mem/data/projects`.

## 5. Reproduce

```sh
cd /home/opencode/Code/Personal/Rust/opencode-agents-memory
mkdir -p .tmp/probe-28-s5/home .tmp/probe-28-s5/logs
cp -a --reflink=auto .tmp/rehearsal-28/20261125T062331Z/store .tmp/probe-28-s5/store
sqlite3 .tmp/probe-28-s5/store/projects/project_fd2bfb63318dfd29_shard_0.db 'DROP TABLE shard_metadata;'
HOME=$PWD/.tmp/probe-28-s5/home ./plugin/engine/linux-x64/opencode-agents-memory \
  --mode=reap-shards --storage-root $PWD/.tmp/probe-28-s5/store
find .tmp/probe-28-s5/store -name '*.legacy.bak'        # expect one, 2,959 rows
```

Expect one `.legacy.bak` whose count equals the shard's row count, and every other shard gaining a
`status: complete` sidecar and nothing else.
