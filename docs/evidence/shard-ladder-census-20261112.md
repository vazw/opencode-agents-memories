# Shard ladder census — 34 consolidation sources

Measured 2026-11-12 against a copy of the frozen custody snapshot
`/home/opencode/.opencode-mem/recovery/consolidation-20261112T030050Z-452b0f5d`
(never the live store). Each shard was copied together with its `-wal` and
`-shm` sidecars before reading, because an uncheckpointed WAL holds the rows:
reading a bare `.db` reports 0 rows and is a false measurement.

## Method

```sh
S=/home/opencode/.opencode-mem/recovery/consolidation-20261112T030050Z-452b0f5d/data/projects
for h in $(cat .tmp/scratch/probe-ids/parent34.txt); do
  for f in $S/project_${h}_shard_0.db $S/project_${h}_shard_0.db-wal $S/project_${h}_shard_0.db-shm; do
    cp "$f" .tmp/scratch/probe-ladder/ 2>/dev/null
  done
done
# per shard: pragma user_version; count(*) from memories;
# pragma_table_info('memories') filtered for deleted_at / source_prompt_created_at
```

Census hash set: the 34 hashes pinned at
`blueprint-20261111-1304-lossless-shard-consolidation.md:55`.

## Result

| user_version | shards | rows |
|---|---|---|
| 2 | 12 | 911 |
| 3 | 22 | 4916 |
| 4 | 0 | 0 |

Total: **34 shards, 5827 rows**.

`memories.source_prompt_created_at` is present in **0** shards — no source has
reached the v4 rung.

`memories.deleted_at` is present in exactly **1** shard:
`project_e266c5689d45bee3_shard_0.db` (user_version 3, 1255 rows, 19 columns).

## The single deviation

`e266c5689d45bee3` carries a `deleted_at` column that no other shard has and
that has no counterpart in the in-tree DDL:

```sh
sqlite3 project_e266c5689d45bee3_shard_0.db \
  "select case when deleted_at is null then 'null' else 'set' end k, count(*) from memories group by k;"
# null|1255
```

**All 1255 rows carry `deleted_at IS NULL`.** The column is vestigial: no row
is soft-deleted, so it holds no information the consolidation would lose by
dropping it. Every other column matches the v3 shape.

## What this settles

The rev-11 §A ladder already covers user_version 2 and 3, and the
implementation reads both. The only shape outside the ladder is this one
vestigial column, so a rev-12 ladder entry needs no migration semantics for
soft-deleted rows — there are none.

The entry should still be written defensively: accept the column, carry every
row verbatim, drop `deleted_at` from the merged row because the target schema
has no such column, and **refuse exit 65 if any row carries a non-NULL
`deleted_at`** — so the all-NULL assumption is enforced by the code rather
than trusted from this measurement.

## What would refute this

A live store whose `e266` shard holds a non-NULL `deleted_at` at consolidation
time. The census above is a snapshot; the defensive refusal is what keeps the
conclusion safe if the store moves.
