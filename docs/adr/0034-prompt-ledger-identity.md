# 0034 — The prompt ledger carries the identity that produced each row

- **Status:** Accepted (owner ruling, 2026-11-28)
- **Date:** 2026-11-28
- **Builds on:** ADR-0007 (bridge strategy); the identity resolver `crates/mem-bridge/src/identity.rs`,
  whose module doc already permits several identities per host (“a host serving several directories may
  legitimately show two identities, and no gate may assert a single one”).
- **Decides:** issue 174 (schema and write path), gates 175 (the learner), and sets the input shape for
  176 (migration disposition).
- **Evidence:** `.scratch/rust-port/issues/174-profile-prompts-carry-no-identity.md` (`## Rulings`,
  2026-11-28) and the read-only measurement recorded there: the live `user-profiles.db` holds two
  identities, `user-prompts.db` holds 184 rows at `user_version = 4` with 15 unanalyzed that all
  resolve to one of them.
- **Wave:** issues 174–176 — profile learner multi-identity.

## Context

`user_prompts` has no identity column. The profile learner is built **once**, with one composition-time
`user_id`, and its ledger select is global:

```rust
// crates/mem-profile/src/prompt.rs
SELECT * FROM user_prompts WHERE user_learning_captured = 0 ORDER BY created_at ASC LIMIT ?
```

The round writes to `ProfileStore` keyed by that one `user_id` and marks the consumed ids globally.
`user_profiles` already keys on `user_id` (emails), and `identity.rs` already treats several identities
per host as legitimate — the learner is the lone consumer that asserts a single one. On a host that
resolves more than one identity (this one does), one identity's prompt text is folded into another
identity's profile and burnt (`user_learning_captured = 1`) for its real owner.

## Decision

1. **`user_prompts.user_id`** — a nullable `TEXT` column added at schema v5, carrying the **resolved
   email** (`resolve_user_id_with` precedence: `userEmailOverride`, else the git `user.email`, else
   `"unknown"`). Emails are used because `user_profiles.user_id` already holds emails; the names agree
   and no translation layer is introduced.
2. **Stamped at record time from a single resolution.** The capture's scope is resolved **once** on the
   accept path and the resulting email is reused by the prompt row and the memory row, so the two rows
   for one capture can never name different identities. A second resolution inside `record` is rejected
   — it is the same “two resolutions drift” class that ADR/issue 177 closed for the embedding dtype.
3. **The learner selects and marks by identity.** `LearningRound::run` takes the identity per call;
   the tick enumerates `DISTINCT user_id` with unanalyzed prompts and runs one round per identity.
   A row that never resolved to an email stays `NULL` and is selected by no learner.

Existing profile keying is unchanged; profiles need no migration.

## Consequences

- One schema version (v4 → v5, no downgrade — the storage-format rule). A v4 store gains `NULL` rows;
  issue 176's backfill resolves each row's `project_path` to an email and stamps the resolvable ones,
  leaving `unknown`/deleted-directory rows `NULL`. Nothing is deleted; no profile content is rewritten.
- The resolver stays the single authority for identity — no SQL-side identity derivation.
- The old singleton may already have folded cross-identity text into both profiles. That is un-foldable
  without re-deriving from prompts, which is a separate, destructive, opt-in step; it is recorded as a
  caveat, not performed here.

## Rejected

- **Read-time join** on `project_path`: identity is not derivable from a path when `userEmailOverride`
  is set or the directory is not a git repo, so the join reintroduces the resolver as SQL and still
  cannot recover past overrides.
- **Project-scoped selection** as a proxy for identity: wrong axis; the profile is user-global and
  `project_path` is provenance only.
- **Independent resolution inside `record`**: reintroduces the drift class issue 177 just closed.
