# 176 — existing prompt and profile rows cannot be re-attributed after 174/175

Status: **needs-info** — the disposition (stamp the old rows vs leave them orphaned) and whether the
live store is multi-identity are owner rulings. May close as *measurement* if the store is provably
single-identity.
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

Scope of the problem, measured on the migration's input shape (not the live store — see inverted
world): every pre-174 row is `NULL`; only rows that were never captured (`user_learning_captured = 0`)
are candidates for later analysis.

## Options

**A — stamp `NULL` rows with the composition-time identity at first boot after v5.** Safe under the
current construction: the store is single-identity by design today, so the launch identity is the only
candidate, and the existing profile already belongs to it. In practice a no-op that keeps the ledger
coherent.

**B — leave `NULL`, exclude from every learner, and document the orphan set.** Cheaper; loses no data
and never guesses an identity, but those prompts can never be analysed and the count surfaces as a
standing gap.

**Discriminating measurement.** On a copied v4 `user_prompts` DB, run the migration and then
`count_unanalyzed_for_user_learning(<launch identity>)`. Under A it returns the pre-migration
unanalyzed count; under B it returns 0 while the rows still exist. Which figure is *correct* is the
owner's ruling (is the existing store single-identity?), not a code question — hence `needs-info`.

## Acceptance criteria

1. The v4 → v5 migration's policy for pre-existing rows is stated in the migration comment, and the
   code implements exactly one of A/B.
2. A fixture test on a v4 DB asserts the post-migration count matches the chosen policy, and is
   non-vacuous: **drop the backfill (A) or count filtered `NULL` rows (B) and the assertion reds.**
3. No existing `user_profiles` row is rewritten or deleted by the migration; a test asserts the
   profile ids and `profile_data` are byte-identical before and after.
4. If B is chosen, the orphan count is exposed once (log line or a `Status` note), never silently
   dropped.
5. `cargo test -p mem-store -p mem-profile` green; no count or duration pinned.

## Inverted world — what would close this as "not a defect" (measurement, not a deliverable)

If the existing store is provably single-identity, A is a no-op and B's orphan set is empty: the issue
shrinks to a migration comment and closes as measurement. The check is one read-only query the owner
runs on a copy: `SELECT DISTINCT user_id FROM user_profiles;` (expected one row today) and
`SELECT COUNT(*) FROM user_prompts WHERE user_learning_captured = 0;`. Neither this issue nor its
tests may open the live store.

## Dependencies

174 (the column and migration). 175 for the "no learner selects NULL" consequence.

## Comments

- 2026-11-28 — filed from the learner review. `needs-info`: the A/B is an owner policy call and the
  inverted world is measurable on a DB copy the owner holds. Read-only evidence; live store untouched.
