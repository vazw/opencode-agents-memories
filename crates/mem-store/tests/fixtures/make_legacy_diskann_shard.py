#!/usr/bin/env python3
"""Regenerate tests/fixtures/legacy_diskann_shard.db (blueprint W1.3 step 2).

The fixture's purpose is the libSQL DiskANN INDEX EXPRESSION in
sqlite_master: the pure-turso engine cannot open a file whose schema
contains `CREATE INDEX ... (libsql_vector_idx(vector))` — the recorded
probe (2026-10-18, /tmp/turso-probe + /tmp/turso-legacy-probe, turso
0.8.3 with both experimental flags, against a copy of this fixture):

    turso::Builder::new_local(...).build().await
      -> Parse error: Error: invalid expression in CREATE INDEX:
         libsql_vector_idx(vector)

so the engine migrator tests (W1.3 step 4) ship a byte-faithful copy of a
DiskANN-indexed shard without needing libsql at test time. python3 is NOT a
build/test dependency; the produced `.db` is checked in like any other
pure-data fixture and consumed read-only via CARGO_MANIFEST_DIR.

Vector payloads are inert TEXT: the engine refuses before any read, so the
payload format is never exercised (recorded divergence — upstream's libsql
probe fallback in `needsEngineRewrite` is N-A: no libsql dependency).

Recipe (proven, stdlib only): register `libsql_vector_idx` as a UDF so
sqlite3 accepts the expression index DDL.
"""

import os
import sqlite3
import sys

MEMORIES_DDL = """
CREATE TABLE memories (
  id TEXT PRIMARY KEY,
  content TEXT NOT NULL,
  vector F32_BLOB(1024) NOT NULL,
  tags_vector F32_BLOB(1024),
  container_tag TEXT NOT NULL,
  tags TEXT,
  type TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  metadata TEXT,
  display_name TEXT,
  user_name TEXT,
  user_email TEXT,
  project_path TEXT,
  project_name TEXT,
  git_repo_url TEXT,
  is_pinned INTEGER DEFAULT 0
)
"""

SHARD_METADATA_DDL = """
CREATE TABLE shard_metadata (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
)
"""

INDEX_DDL = [
    "CREATE INDEX memories_vector_idx ON memories (libsql_vector_idx(vector))",
    "CREATE INDEX memories_tags_vector_idx ON memories (libsql_vector_idx(tags_vector))",
]


def main() -> int:
    out_path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "legacy_diskann_shard.db")
    for suffix in ("", "-wal", "-shm", "-journal"):
        stale = out_path + suffix
        if os.path.exists(stale):
            os.remove(stale)

    db = sqlite3.connect(out_path)
    try:
        # The registered UDF makes the expression-index DDL parseable by
        # sqlite3; its return value is irrelevant (inert TEXT payloads).
        db.create_function("libsql_vector_idx", 1, lambda value: value, deterministic=True)
        db.execute(MEMORIES_DDL)
        db.execute(SHARD_METADATA_DDL)
        db.execute("INSERT INTO shard_metadata (key, value) VALUES ('embedding_dimensions', '1024')")
        db.execute("INSERT INTO shard_metadata (key, value) VALUES ('embedding_model', 'voyage-4-lite')")

        rows = [
            # tagged row (tags_vector payload inert TEXT)
            (
                "mem_diskann_1",
                "diskann tagged memory",
                "[1,0,0]",
                "[0,1,0]",
                "opencode_project_a1b2c3d4e5f67890",
                "alpha,beta",
                1700000000000,
                1700000000000,
            ),
            # untagged row (NULL tags_vector)
            (
                "mem_diskann_2",
                "diskann untagged memory",
                "[0,0,1]",
                None,
                "opencode_project_a1b2c3d4e5f67890",
                None,
                1700000000001,
                1700000000001,
            ),
        ]
        db.executemany(
            "INSERT INTO memories (id, content, vector, tags_vector, container_tag, tags, created_at, updated_at)"
            " VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            rows,
        )
        for ddl in INDEX_DDL:
            db.execute(ddl)
        db.commit()

        # Sanity: the file opens with sqlite3 and contains the two index rows.
        index_rows = db.execute(
            "SELECT name FROM sqlite_master WHERE type='index' AND name LIKE 'memories%vector_idx'"
        ).fetchall()
        assert len(index_rows) == 2, f"expected 2 DiskANN index rows, got {index_rows}"
        count = db.execute("SELECT COUNT(*) FROM memories").fetchone()[0]
        assert count == 2, f"expected 2 memories, got {count}"
    finally:
        db.close()
    print(f"wrote {out_path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
