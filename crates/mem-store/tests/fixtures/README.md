# mem-store test fixtures

Pure-data fixtures live here and are consumed read-only via
`CARGO_MANIFEST_DIR` (`common::diskann_fixture_path()` copies them into a
per-test temp storage root). python3 is NOT a build/test dependency — the
recipes below regenerate the artifacts after a clean checkout.

## `legacy_diskann_shard.db`

A byte-faithful copy of a libSQL-DiskANN-indexed memory shard (17-column
`memories`, `shard_metadata` with `embedding_dimensions=1024` /
`embedding_model=voyage-4-lite`, two rows with inert TEXT vector payloads,
and the two expression indexes `memories_vector_idx` /
`memories_tags_vector_idx` over `libsql_vector_idx(...)`).

**Why it exists:** the pure-turso 0.8.3 engine CANNOT open a DiskANN-indexed
file at all. Recorded probe evidence (2026-10-18, `/tmp/turso-probe` +
`/tmp/turso-legacy-probe`, turso 0.8.3 with both experimental flags, against
a copy of this fixture):

```
turso::Builder::new_local(...).build().await
  -> Parse error: Error: invalid expression in CREATE INDEX: libsql_vector_idx (vector)
```

The engine-migrator tests (W1.3 step 4, `tests/turso_engine_migrator.rs`)
copy this fixture into a temp storage and assert the loud refusal: no
marker, no `.pre-tursodb-` backup, byte-identical file (upstream converts
in-place via a libsql probe; this port REFUSES — recorded divergence, no
libsql dependency).

Vector payloads are inert TEXT on purpose: the engine refuses before any
read, so the payload format is never exercised.

**Regenerate:** `python3 make_legacy_diskann_shard.py` (stdlib `sqlite3`
with a registered `libsql_vector_idx` UDF — the proven recipe; the script
sanity-checks the two index rows).
