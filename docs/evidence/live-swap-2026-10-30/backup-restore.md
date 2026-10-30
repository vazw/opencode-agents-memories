# Backup + restore rehearsal — executed 2026-10-30

Executed at 2026-10-30T17:16:53Z (local 2026-10-31 00:16 +07) against **copies only**. Engine
`72d7f64d…4d5e653`. Scratch root: `.tmp/scratch/issue80-20261031/` (gitignored); paths below are relative
to it. The live store `~/.opencode-mem/data` was read by `cp -a` and never written.

Deviation, stated up front: the runbook stops OpenCode and checkpoints the live DBs. The rehearsal may
not stop OpenCode, so the `cp -a` came from the running store and the `PRAGMA wal_checkpoint(TRUNCATE)`
step ran on that copy. From there the procedure is the runbook's: dated `cp -a` backup → manifest →
`sha256sum -c` → per-DB `integrity_check` → restore into a scratch directory → open with the engine.

## 1. Read-only copy + checkpoint (on the copy)

```sh
cp -a ~/.opencode-mem snapshot/                # 148 files, 654,720,843 B, identical to the source
# sidecars present in the copy before checkpointing (top level):
#   metadata.db-wal 576,832 B · user-profiles.db-wal 1,112,432 B · user-prompts.db-wal 1,145,392 B
#   (five empty -wal files + 6 × -tshm + 1 × -shm)
sqlite3 -readonly "file:snapshot/data/metadata.db?immutable=1" \
  "SELECT count(*), sum(vector_count) FROM shards;"          # 22 | 3052   (base file only)
mapfile -t dbs < <(find snapshot/data -type f -name '*.db' | sort)          # 29
for db in "${dbs[@]}"; do sqlite3 "$db" 'PRAGMA wal_checkpoint(TRUNCATE);'; done
find snapshot/data -type f \( -name '*-shm' -o -name '*-tshm' \) -delete
```

Output: 29 lines, every one `0|0|0` (busy|log|checkpointed — SQLite applies the WAL when it opens the
file, so a drained WAL reports zeros), `checkpoint_failures=0`. The proof the frames landed:

```
metadata_base shards=22 vectors=3052
post-checkpoint  shards=22 vectors=3104
```

then `integrity_checked=29 failures=0` over every checkpointed DB (read with `immutable=1`, valid because
the WAL is now empty).

## 2. Dated backup of the root + both config files

```sh
TS=20261030T171653Z
cp -a snapshot "backup-$TS"
mkdir -p "backup-$TS/config"
cp -a ~/.config/opencode/opencode.jsonc     "backup-$TS/config/opencode.jsonc"
cp -a ~/.config/opencode/opencode-mem.jsonc "backup-$TS/config/opencode-mem.jsonc"
```

| item | value |
|---|---|
| backup dir | `backup-20261030T171653Z/` — 617 MB, **85 files**, **29 DBs** |
| `config/opencode.jsonc` | sha256 `8e5cebe3dbf55cec65ddf5a6502f9dcf83e55ab308a3916958e1abddf999fc9a` |
| `config/opencode-mem.jsonc` | sha256 `94888c5c36ce168f3e6a89dfdc8403d26cfddfe251b885d21b9bfe6d669a0058` |
| `data/metadata.db` | sha256 `7d3ae4443449654ef217ea99ed5d8073ddbc0dc429f4e4fd50b8ea43c173cbba` |
| `data/user-prompts.db` | sha256 `ebf4ee6ce30e7998477f09d055e583fd7ae396faf517fe729e5ced05b262951d` |
| `data/user-profiles.db` | sha256 `97ca048d2642987573d0119666b0e6478652c49b08628d968a471808520e9fe1` |
| `data/projects/project_4182…_shard_0.db` | sha256 `17edeaf4ecb77e17b7494712bae9481bb99f2ff2f7bc984c34df65bcf78c3cb2` |
| `data/.cache/…/model.onnx` | sha256 `347440e93b5ec979fdcf6041b72721aade7b9680c16022e2830db7115ff6fd9f`, 547,552,426 B |
| `data/.cache/…/tokenizer.json` | sha256 `d241a60d5e8f04cc1b2b3e9ef7a4921b27bf526d9f6050ab90f9267a1f9e5c66`, still `"version": "1.0"` |
| `~/.opencode-mem/.auth-token` (upstream's, at the root) | included, 64 B |

State of the backup (each DB read with `immutable=1`):

| DB | `user_version` | rows |
|---|---:|---|
| `metadata.db` | 1 | 22 shards, **3,104** vectors, 22 active |
| `user-prompts.db` | 1 | **1,745** |
| `user-profiles.db` | 1 | **2** profiles, **10** changelogs |
| `skills.db` | 1 | 0 |
| `episodic.db` | 6 | 0 sessions |
| `ai-sessions.db` | 1 | 0 |
| `projects/*.db` | **2** × 22 | 3,104 memories total, 768 dims everywhere |

## 3. Manifest + verify + integrity

```sh
( cd "backup-$TS" && find . -type f ! -name MANIFEST.sha256 -print0 | sort -z | xargs -0 sha256sum > MANIFEST.sha256 )
wc -l < MANIFEST.sha256                 # 85
sha256sum -c MANIFEST.sha256 --quiet; echo "manifest_rc=$?"
```

```
manifest_lines=85
verified_ok=85 failed=0
manifest_rc=0
```

The manifest is committed verbatim as `backup-manifest-20261030T171653Z.txt` (sha256
`712d6cee3ba08db65627a9c1d12c67c11472ed32c25c84ed10d2b56fc4023c92`); the `sha256sum -c` output as
`backup-verification-20261030T171653Z.txt` (sha256 `df713a01c057ca2bbefaee611b16363a5319847e7deaef8dcc0de6cdc51b972c`).

```sh
for db in $(find "backup-$TS/data" -type f -name '*.db' | sort); do
  sqlite3 -readonly "file:$db?immutable=1" 'PRAGMA integrity_check;'
done | sort | uniq -c
```

```
     29 ok
```

Timings, measured separately on the same files (timed with `time`): 29-DB checkpoint **0.18 s**;
manifest of 85 files **3.5 s**; `integrity_check` over the 29 DBs **0.6 s**; `cp -a` of the 617 MB
backup **0.09 s** (warm page cache, fast NVMe — expect seconds cold).

## 4. Restore rehearsal

Runbook command shape, target absent (the rehearsal cannot replace the live `data/`):

```sh
mv  ~/.opencode-mem/data ~/.opencode-mem-backups/data.port-$TS     # live form; here the target was absent
cp -a "backup-$TS/data" restore/data
```

Byte comparison of the restored tree against the backup:

```
non-db files identical: 31
db sha diffs:            0
```

State comparison against the backup's pre-state (`store-state.sh`, path line ignored):

```
$ diff <(tail -n +2 pre-state.txt) <(tail -n +2 state-0-restored.txt)
$ echo $?
0
```

i.e. 22 shards, 3,104 memories, 1,745 prompts, 2 profiles + 10 changelogs, every per-shard content/tags
byte sum and vector count identical.

### Open #1 — first store touch (migration)

Isolated `HOME` (own `opencode-mem.jsonc` + `.gitconfig`), `TMPDIR` = the provisioned cache, engine
`--storage-root restore/data`, web on 4799.

```
hello:ok 63ms · health:ok 1ms · list-shards:ok 242ms · list:ok 26ms · list:ok 128ms
search:ok 2811ms · search:ok 62ms · profile:ok 39ms · skills:ok 14ms
web: health 200 · memories+prompts 401 · profile 200 · stats 200 · engine_exit {code:0}
```

`401` is the lazy token mint (the file did not exist yet; the first gated request creates it — see
`provisioning.md` §4), not a failure. After the run, checkpoint the copy and compare:

```
$ diff <(tail -n +2 state-0-restored.txt) <(tail -n +2 state-1b-open1-checkpointed.txt)
2c2
< db=metadata.db user_version=1 sha256=7d3ae4443449654ef217ea99ed5d8073ddbc0dc429f4e4fd50b8ea43c173cbba
---
> db=metadata.db user_version=2 sha256=358cab3d09c37549755fe6c4f351775218042bb3613abe233f4ecc1856e6e1bd
```

Nothing else changed: same 22 shards, same 3,104 memories, same per-shard byte sums, prompts still v1,
profiles still v1, all shards still v2. `metadata.db` 24,576 → 28,672 B.

### Open #2 — fresh engine, prompts migration

```
hello:ok 16ms · health:ok 4ms · list-shards:ok 201ms · list:ok 70ms · list:ok 126ms
search:ok 3006ms · search:ok 62ms · profile:ok 19ms · skills:ok 14ms
web: health 200 · memories+prompts 200 · profile 200 · stats 200 · engine_exit {code:0}
```

```
$ diff <(tail -n +2 state-1b-open1-checkpointed.txt) <(tail -n +2 state-2-open2.txt)
6c6
< db=user-prompts.db user_version=1 sha256=ebf4ee6ce30e7998477f09d055e583fd7ae396faf517fe729e5ced05b262951d
---
> db=user-prompts.db user_version=2 sha256=c9e74044bd752f8cef77dd9b73016c31d8fab4bfeee73491d2e8c2cee7b848e5
```

Again nothing else. File size unchanged (1,540,096 B): the stamp moved, the rows did not.

### Open #2 doubles as the reopen check

It is a fresh engine process started after open #1's ordered `shutdown` (`{"ok":true}`, exit 0). It
answered `hello`/`health`/`list-shards`/`list` on the first try (201 ms / 70 ms / 126 ms). This is the
positive control for the runbook's reopen step. The `-32603`-after-`SIGTERM` anomaly and its
`*.db-shm`/`*.db-tshm` recovery are transcribed in the export/import report §6; the recovery was not
re-triggered here (it did not reproduce on a cleanly shut-down store).

### Post-restore integrity + final state

```
restore_integrity_checked=29 failures=0
bak_files=0
metadata      user_version=2  shards=22  vectors=3104  active=22
user-prompts  user_version=2  rows=1745
user-profiles user_version=1  profiles=2  changelogs=10
shard_versions: 22 × v2
```

## 5. The lazy shard migration, reproduced

The `add` in `provisioning.md` wrote to a separate copy (`prov/data`). With its WAL applied, that
project's shard is exactly what the dry run measured:

```
$ sqlite3 <copy-with-wal>/project_4182…_shard_0.db \
    "PRAGMA user_version; SELECT count(*) FROM memories; SELECT * FROM shard_stats;"
3
932
1|932
```

`shard_stats` exists, one row `1|932`; the other 21 shards have no non-empty WAL (never written) and stay
`user_version = 2`. Read the base file alone and it still says v2 — the migration lives in the WAL until
the next checkpoint. That is the reading trap the runbook warns about.

## 6. Reproduction

```sh
ROOT=/home/vaz/Code/Personal/Rust/opencode-agents-memory
S=$ROOT/.tmp/scratch/issue80-20261031
B=$S/backup-20261030T171653Z
# 1 copy + checkpoint        2 backup + manifest       3 restore + compare
cp -a ~/.opencode-mem "$S/snapshot"
# (checkpoint the copy as in §1)
cp -a "$S/snapshot" "$B"
( cd "$B" && find . -type f ! -name MANIFEST.sha256 -print0 | sort -z | xargs -0 sha256sum > MANIFEST.sha256 )
( cd "$B" && sha256sum -c MANIFEST.sha256 --quiet ); echo $?
mkdir -p "$S/restore" && cp -a "$B/data" "$S/restore/data"
$ROOT/.tmp/scratch/swap-dryrun/store-state.sh "$S/restore/data"
```
