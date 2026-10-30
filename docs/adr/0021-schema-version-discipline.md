# 0021 — Schema-version discipline: one runner, a mandatory `max_known`, refuse before pending

- **Status:** Accepted (owner acceptance, 2026-10-30).
- **Date:** 2026-10-28
- **Builds on:** `docs/adr/0013-production-error-handling-panic-free.md` (typed refusals, no panic),
  `docs/adr/0018-shard-facts-source-of-truth.md` (the first set W2 raises).
- **Blueprint:** `bp-20261027-2353-issue43-w2-shard-v3-r4` (sha256 `8aceba1798e2`,
  `SYS-APPROVED 2026-10-28T00:01Z`).
- **Wave:** issue 43 W2, base `dc5bcad`.

## Context

The on-disk format is ours alone: no upstream counterpart, no downgrade path. Before W2 the runner
(`crates/mem-store/src/schema_migrations.rs::apply_schema_migrations`) read `PRAGMA user_version`, filtered
pending migrations, and returned `Ok(current)` when nothing was pending. A file written by a **newer**
build therefore opened silently: it had no pending migrations under an older migration set, so the old
build proceeded to read and write a format it did not know. The only version-shaped guard was
`ensure_user_prompt_columns`, which adds columns and **matches error messages** ("duplicate column") to
tolerate an already-applied `ALTER` — an anti-pattern this ADR retires by direction.

W2 is the first wave to write a version a smaller-max build must refuse (shard v3). This ADR records the
mechanism; **W5 owns the prompts half** (the `user_version = 2` bump, the probe-then-`ALTER` replacement of
`ensure_user_prompt_columns`, and the deletion of the error-message matching). The split is deliberate: W2
ships the discipline every set uses, W5 migrates the one set that still needs it.

## Decisions

**D-21-1 — One runner, one check.**
Every set goes through `apply_schema_migrations` — metadata, shards, prompts, profiles, facts, vault, and
the episodic v1..v5 fold. The version refusal lives there and nowhere else. `apply_episodic_schema` applies
v6 itself but reuses the runner for v1..v5, so it inherits the same check.

**D-21-2 — `max_known` is mandatory and per-set.**
`MigrationOptions` gains `max_known: i64` and **drops `Default`**: a permissive default (`i64::MAX`) would
be fail-open on exactly the file the check exists to refuse, so every call site must name its set's
maximum. The maximum is a constant **beside the set**, not derived from the migration slice:

| set | constant | value |
|---|---|---|
| `metadata.db` | `METADATA_MAX_KNOWN` | 2 (W4) |
| memory shards | `MEMORY_SHARD_MAX_KNOWN` | 3 (W2's v3) |
| `user_prompts` | `USER_PROMPTS_MAX_KNOWN` | 2 (W5) |
| `user_profiles` | `USER_PROFILES_MAX_KNOWN` | 1 |
| episodic | `EPISODIC_MAX_KNOWN` | 6 |
| `facts.db` | `FACTS_MAX_KNOWN` | 1 |
| `vault.db` | `VAULT_MAX_KNOWN` | 1 |

Deriving the maximum from the slice is wrong: `apply_episodic_schema` applies v6 while
`episodic_migrations()` returns v1..v5, so a derived maximum would refuse every v6 database on the second
open. `EPISODIC_MAX_KNOWN` is therefore 6, not 5.

**D-21-3 — Refuse before the pending check, on `current`, never on `target`.**
`apply_schema_migrations` reads `user_version` and returns
`StoreError::SchemaNewerThanBuild { label, found, supported }` when `current > max_known`, **before** the
`pending.is_empty()` early return. A newer file has no pending migrations, so an in-loop check would never
run. The comparison is on the file's `current`, never on the in-memory set's highest version: a fresh
database (`current = 0`) must still apply v1..vN under a build that knows N, returning `Ok(N)`. The test
pins both arms on the **same file**: apply v1..v3 with `max_known: 3` → `Ok(3)`, reopen with
`max_known: 2` → `Err(found: 3, supported: 2)` even though nothing is pending.

**D-21-4 — The refusal is typed and sized.**
`SchemaNewerThanBuild` is 32 bytes inline (`&'static str` + two `i64`), far under the `StoreError`
high-water mark (112, `ScopeShardUnreadable`); the clippy gate fails at 128, so the variant is shrunk
rather than allowed. The check is write-path-only in W2: `is_shard_valid` and every search read open the
shard file directly and never call the runner, so a v4 shard is refused on write but still searched.
Closing that is W5's open-path work or W3's authority check (residual R-1).

**D-21-5 — Convergence over pragma atomicity.**
The runner applies each migration as one `BatchMode::Write` batch, then sets `PRAGMA user_version`
separately. A crash between the two leaves DDL committed with an old version; the next apply must
**converge**, not fail. Shard v3 is `CREATE TABLE IF NOT EXISTS` (pinned by
`test_a_v3_shaped_file_stamped_v2_converges`); the episodic v6 path measures whether the pragma landed in
the batch and falls back to a separate `execute`. No `.bak` is added to `ensure_shard_schema`: the step is
additive and idempotent (ADR-0018 R-7).

**D-21-6 — No error-message matching for version state.**
Version decisions are rows (`PRAGMA user_version`), never string matches on an error message. W5 replaced
the `ALTER` attempts in `ensure_user_prompt_columns` with a `PRAGMA table_info` probe and deleted the
function.

## Scope note

Making `max_known` mandatory at all 24 `MigrationOptions` literals (18 files) is **W5's cost pulled
forward**: W5's 43.6/43.7 work inherits a runner that already refuses a newer file. W2 keeps the
mechanism, W5 keeps the prompts-v2 half and the schema-discipline cleanup.

## Residuals

- **R-1** — the refusal is reached only through `apply_schema_migrations`; a pre-W2 binary cannot refuse a
  v3 file by construction, and read paths do not run the check yet.
- A same-major format written by a newer build inside `max_known` is still accepted; the maximum is a
  deliberate declaration, not a signature.
