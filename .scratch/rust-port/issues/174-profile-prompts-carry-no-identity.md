# 174 — user_prompts carries no identity, so the learner cannot select by user

Status: **needs-info** — the shape (a `user_id` column vs a read-time join) and whether a
multi-identity engine is in scope are owner rulings; proposed ADR-0034 below. `ready-for-agent` once
that ruling lands.
Filed: 2026-11-28 from the profile-learner review (this file, 175, 176, 177). No code changed.

## The defect

The profile learner is built **once**, with one composition-time identity
(`crates/mem-bridge/src/profile_learning.rs:133,150,338-386`; wired from the launch cwd at
`crates/mem-bridge/src/composition.rs:376-390`). Its ledger selects **every** unanalyzed prompt:

```rust
// crates/mem-profile/src/prompt.rs:700-715
pub async fn get_prompts_for_user_learning(&self, limit: i64) -> ... {
    "SELECT * FROM user_prompts
     WHERE user_learning_captured = 0
     ORDER BY created_at ASC LIMIT ?"
}
```

and `count_unanalyzed_for_user_learning` (`prompt.rs:687-697`) counts the same global set. Neither
takes an identity, and `user_prompts` has no column to filter on: the insert lists
`id, session_id, message_id, project_path, content, created_at, captured` (`prompt.rs:263`), the v2
additive columns are `capture_attempts, provider_id, model_id`
(`crates/mem-store/src/user_prompts_sql.rs:17-21`), and nothing stamps a user. The round then writes
to `ProfileStore` keyed by `self.user_id` (`crates/mem-profile/src/learning_service.rs:360,450,
478-486`) and marks the consumed ids globally (`learning_service.rs:490` →
`prompt.rs:718-732`).

So on any host that resolves more than one identity, identity A's learner folds identity B's prompt
text into A's profile and marks B's rows captured, so B never sees them.

**This is not hypothetical to the design.** `crates/mem-bridge/src/identity.rs:7-10` states the
inject path resolves the **per-call session directory** and that "a host serving several directories
may legitimately show two identities, and no gate may assert a single one." The learner is the one
consumer that asserts a single one.

## Impact

Mis-attribution, silent. Profiles are already keyed by user (`crates/mem-profile/src/profile_store.rs:
970-980`, `WHERE user_id = ? AND is_active = 1`), so no profile is destroyed — but one identity's
prompt content is analysed into another identity's profile, and the row is burnt
(`user_learning_captured = 1`) for its real owner. `project_path` is recorded but is **not** a usable
identity: `user_email_override` overrides it and a non-git directory yields `unknown`
(`identity.rs:44-54`).

## Already decided / not a defect

- **Project-level bleed is upstream behaviour.** The profile is user-global; `project_path` is
  provenance/display only, and upstream's `getPromptsForUserLearning` also selects all unanalyzed
  rows. This issue is about **identity**, not project partitioning; do not "fix" it by scoping to
  `project_path`.
- **Profiles need no migration.** They already key on `user_id`; the gap is the ledger (175 depends on
  this file) and the singleton learner.

## Options

**A — identity travels on the row.** Add `user_id TEXT` to `user_prompts` at migration v5
(`apply_user_prompts_schema`, `crates/mem-store/src/schema_migrations.rs:765-837`; column-set precedent
`user_prompts_sql.rs:17-21`), stamped in `save_prompt`/`persist_prompt` (`prompt.rs:250-280`) from the
capture's resolved identity — the same `resolve_user_id_with` precedence, not a second chain. Select
`WHERE user_id = ? AND user_learning_captured = 0`.

**B — read-time partition.** Keep the ledger global and join `session_id`/`project_path` to a new
scope table at read time. Rejected premise: identity is not derivable from `project_path` when
`user_email_override` is set or the directory has no git, so this reintroduces the resolver as SQL.

**Discriminating measurement.** Seed one unanalyzed prompt under identity A and one under identity B;
drive only A's round. Assert B's row is still `user_learning_captured = 0` and B's profile is absent or
unchanged, while A's profile analysed exactly A's prompt. Under A the row carries the identity and the
filter is one predicate. Under the current code both rows are marked and both texts land in A's
profile — the test reds. B cannot pass this test unless it re-resolves identity per row, which it
cannot do for an override store.

## Acceptance criteria

1. `user_prompts` gains a `user_id` column applied by `apply_user_prompts_schema` (v4 → v5 and fresh
   DB both end at v5 with the column present); the fresh-file DDL and the upgrade leg agree.
2. `ProfileLearning::record` persists the capture's **resolved** identity (the `resolve_user_id_with`
   precedence) — never the composition-time string.
3. `get_prompts_for_user_learning(limit, user_id)` and `count_unanalyzed_for_user_learning(user_id)`
   filter on it; the unfiltered arms are deleted, not kept beside the new ones.
4. The two-identity test above exists and is non-vacuous: **removing the `user_id = ?` predicate
   marks B's row and puts B's text in A's profile, and the test reds.** State that observation in the
   test comment.
5. A v4 fixture DB migrates to v5 with the column defaulting `NULL` for rows that predate it (176
   owns what happens to those rows).
6. `cargo test -p mem-profile -p mem-bridge -p mem-store` green; no count or duration pinned.

## Inverted world — what would close this as "not a defect"

`resolve_user_id_with` is provably constant per process (it is not: `user_email_override` can differ
per config and the inject path resolves per directory, `identity.rs:7-10`), **or** prompts are already
identity-stamped somewhere (`grep` over the `user_prompts` DDL and every INSERT finds no user field).
Neither holds.

## Dependencies

None. 175 depends on this; 176 backfills what this leaves `NULL`.

## Proposed ADR-0034 (outline only — do not write it)

**Title:** The prompt ledger carries the identity that produced each row.
**Context:** `identity.rs` permits several identities per host; the learner asserts one; the ledger has
no identity column.
**Decision:** (A) `user_prompts.user_id` at v5, stamped at record time from the resolved identity; the
learner selects and marks by identity. Existing profile keying is unchanged.
**Consequences:** one schema version (no downgrade, per the storage-format rule); a v4 store gains
`NULL` rows that 176 must disposition; the resolver stays the single authority (no SQL-side identity).
**Rejected:** read-time join (B); project-scoped selection (wrong axis, falsified above).

## Comments

- 2026-11-28 — filed from the learner review. `needs-info` because the A/B and the scope of
  multi-identity are the owner's call, and the ADR is not yet accepted. The evidence above is
  read-only; nothing in the live store was opened or written.
