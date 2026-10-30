# 0020 — Cross-file backup ordering: reverse commit order, one lock, one invariant registry

- **Status:** Accepted (owner acceptance, 2026-10-30).
- **Date:** 2026-10-30
- **Builds on:** `docs/adr/0013-production-error-handling-panic-free.md` (typed refusals, no panic),
  `docs/adr/0021-schema-version-discipline.md` (per-set maxima; the runner's proof-gated per-file
  `.schema-v{n}-{millis}.bak` is the precedent this rule extends).
- **Wave:** issue 43 W5. 43.5 is a **record-only** deliverable of this wave: it is written alongside W5's
  planning, not as a wave of its own (confirmed here rather than inventing a wave for it). No code ships
  with this ADR.

## Context

The storage root is a set of files, not one database: `user-prompts.db`, `metadata.db`, `facts.db`, the
memory shards, `vault.db`, `skills.db` and the episodic database are opened, committed and closed
independently. No cross-file transaction exists, so a backup that copies them one by one can capture a
state that never existed as a whole (a prompt row whose shard write is not yet in the shards, or the
reverse).

Today there is **no user-facing backup path** — no operator-invoked copy/export over the storage root.
The `.bak` writers in the tree are all single-file, operation-scoped artifacts of the operation that
creates them:

| writer | artifact | scope |
|---|---|---|
| migration runner (`schema_migrations.rs::backup_before_destructive`) | `<db>.schema-v{from_version}-{millis}.bak` | one file, one destructive upgrade, proof-gated |
| encryption migrator | `<db>.pre-encrypt-{millis}.bak` | one file |
| re-embed service | `<db>.pre-reembed-{nonce}.bak` | one file |
| legacy migrator | `<db>.legacy.bak` | one file |

The migration runner's copy is the only **precedent** for the rule below: it is per-file, named with the
version it came from, and refused without the caller's exclusivity proof (`ExclusiveRoot`, ADR-0021 /
issue 46). It is not itself a multi-file backup and this ADR does not govern it.

43.5 asks for the multi-file rule **before** a user-facing backup path exists, so the first implementation
does not invent its own ordering.

## Grounds

| claim | verdict |
|---|---|
| a user-facing multi-file backup path exists today | **FALSE** (READ: no entry point over the storage root in `src/` or `crates/*/src`; the `.bak` writers are the four operation-scoped artifacts above). |
| the files can be committed atomically as a set | **FALSE** (READ: each database is its own file with its own connection; no cross-file transaction exists). |
| the runner names its copy with the from-version and a timestamp | **TRUE** (READ: `crates/mem-store/src/schema_migrations.rs`, `format!("{}.schema-v{}-{}.bak", …)`). |
| the runner copies without a proof | **FALSE** (READ: it returns `StoreError::NoExclusiveRoot` **before** the copy when a backup is needed and no proof is supplied). |
| the prompts row is erased before the shards for retention | **Rule of the implementing wave** (W6). Stated here so the backup order and the erase order are read from one place; not measured in this wave. |

## Decisions

**D-20-1 — Copy in reverse commit order.**
A multi-file backup copies the files in the reverse of the order their contents are committed: the
file written last is copied first — **`user-prompts.db` before the shards** for retention. A backup that
stops mid-way then holds the most recent state first, never a stale suffix presented as a complete set.

**D-20-2 — Hold the engine lock for the whole copy window.**
The copy runs inside the same root-exclusivity window the migration runner already requires: no copy may
start without the caller's `ExclusiveRoot`, and the lock is held across every file in the set, not taken
and released per file. A copy that cannot prove exclusivity refuses before it writes the first byte.

**D-20-3 — The cross-file invariant list lives in ONE registry.**
Which files must be mutually consistent, and the order between them, is declared once in a single
registry. Call sites may not carry their own ad-hoc ordering: a second list is a second authority for the
same invariant, and the non-atomic window is exactly where the two drift apart.

## Scope note

This ADR records the rule only. It adds no backup path, changes no schema, and takes no copy. The wave
that introduces a user-facing backup path owns the registry's location and the first implementation, and
must implement the erase ordering — erase the prompts row before touching shards for retention — under
these three decisions.

## Residuals

- **R-20-1** — nothing enforces D-20-1/D-20-2/D-20-3 until a multi-file copy exists; this is a declaration.
- **R-20-2** — the four operation-scoped `.bak` writers stay out of scope; they are single-file artifacts
  of the operation that creates them, and the runner's copy is already proof-gated.
- **R-20-3** — the registry's home is not chosen here. The implementing wave must choose it before the
  first call site, or D-20-3 collapses back into per-site ordering.
