# 176 — existing prompt and profile rows cannot be re-attributed after 174/175

Status: **ready-for-agent** — owner ruled 2026-11-28 (grilling) on a measured (read-only copy) store:
**two** identities, not one, so this does not close as measurement. Disposition: migration v4→v5 adds a
nullable `user_id`; a Rust backfill resolves each distinct `project_path` and stamps every resolvable
row with its email; rows resolving to `unknown`/a deleted directory stay `NULL`; delete nothing; touch
no profile content. See §Rulings.
Filed: 2026-11-28, with 174/175/177. No code changed.

## The defect

Rows written before 174 carry no identity and, if already analysed, were marked
`user_learning_captured = 1` globally (`crates/mem-profile/src/prompt.rs:718-732`) and their text was
merged into the one profile that existed (`crates/mem-profile/src/learning_service.rs:478-486`). After
174 they migrate to `user_id = NULL`; after 175 no learner selects a `NULL` row, so they become
permanently unanalysable orphans unless the migration dispositions them.

Backfill cannot invent identity from `project_path`: `user_email_override` supersedes it, and a non-git
directory resolves to `unknown` (`crates/mem-bridge/src/identity.rs:44-54`). Attribution is best-effort
at most.

## Measured on the live store (read-only copy, 2026-11-28)

- `user_profiles` holds **two** identities: `vaz@siamstr.com` and `jakkaphat.j@bangmod.co.th`.
- `user-prompts.db`: 184 rows, `user_version = 4`; **15 unanalyzed**. Rows span 8 `project_path`s.
- All **15 unanalyzed rows resolve to `vaz@siamstr.com`** (14 in this repo, 1 in `~/.config/opencode`).
- **29 analyzed rows sit in deleted directories** (`infinity`, `open-kernel-challenge`) that no longer
  resolve; they are already folded into a profile and never re-selected.
- `userEmailOverride` is currently commented out in the live config.

## Rulings (2026-11-28, owner, grilling)

- **Not a measurement-close.** The store is multi-identity, so orphan/backfill is a real disposition.
- **Stamp resolvable rows, delete nothing, leave the unresolvable `NULL`.** The backfill resolves each
  distinct `project_path` with the shared resolver and stamps the resulting email on every matching row
  (the 15 unanalyzed all land under `vaz`). A row that resolves to `unknown`, or whose directory is
  gone, stays `NULL` — never a guessed or fake identity. Profile content is not rewritten.
- **Historical profiles may mix identities.** The old singleton folded prompts from every path into
  whichever identity was active; that content is un-foldable without re-deriving from prompts. Rebuild
  is a separate, destructive, opt-in step — out of scope here, recorded as a caveat.

## Options (historical — superseded by §Rulings)

**A — resolve each `NULL` row's `project_path` and stamp the recovered email.** The chosen path. A
one-time Rust backfill after the v5 column exists: resolve the distinct `project_path`s, `UPDATE ...
SET user_id = ?` for each resolved email, leaving `unknown`/dead-dir rows `NULL`. Not the naive
"stamp the composition-time identity" (that would mislabel the bangmod rows); the email comes from each
row's own path.

**B — leave `NULL`, exclude from every learner, and document the orphan set.** Cheaper; loses no data
and never guesses an identity, but those prompts can never be analysed and the count surfaces as a
standing gap.

**Discriminating measurement.** On a copied v4 `user_prompts` DB seeded with two paths that resolve to
two emails plus one dead path, run the migration+backfill, then `count_unanalyzed_for_user_learning(<vaz>)`
and `<bangmod>`. Each returns only its own unanalyzed count; the dead-path row stays `NULL` and neither
count includes it. Under "stamp the launch identity" the bangmod rows land under vaz and the second
count is zero — the assertion reds.

## Acceptance criteria

1. The v4 → v5 migration adds a nullable `user_id`; a Rust backfill resolves each distinct
   `project_path` and stamps the recovered email; rows resolving to `unknown`/dead-dir stay `NULL`. The
   policy is stated in the migration/backfill comment and implemented exactly as ruled.
2. A fixture test on a v4 DB (two resolvable paths, one dead path) asserts each identity's unanalyzed
   count includes only its own rows and the dead-path row stays `NULL`; non-vacuous: **stamp the launch
   identity for all rows and the second identity's count is zero — the test reds.**
3. No existing `user_profiles` row is rewritten or deleted by the migration; a test asserts the
   profile ids and `profile_data` are byte-identical before and after.
4. The `NULL` (unknown/dead-dir) row count is surfaced once (log line or a `Status` note), never
   silently dropped.
5. `cargo test -p mem-store -p mem-profile` green; no count or duration pinned.

## Inverted world — what would close this as "not a defect" (already falsified)

If the store were single-identity, backfill would be a no-op and the orphan set empty — the issue would
shrink to a migration comment. Measured 2026-11-28 on a read-only copy: **two** identities, so this does
not close as measurement. The disposition above is the deliverable; it never opens the live store (tests
run on fixture/copies only).

## Dependencies

174 (the column and migration). 175 for the "no learner selects NULL" consequence.

## Comments

- 2026-11-28 — filed from the learner review. `needs-info`: the A/B is an owner policy call and the
  inverted world is measurable on a DB copy the owner holds. Read-only evidence; live store untouched.
- 2026-11-28 — measured on a read-only copy: two identities; 15 unanalyzed all resolve to `vaz`; 29
  rows in deleted dirs. `ready-for-agent` with the stamp-resolvable/leave-NULL disposition. The
  historical cross-identity profile mixing is recorded, not fixed (re-derivation is a separate
  opt-in).
