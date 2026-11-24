# 159 — the prompts v4 bump left three test reds behind, and they re-pin the numeral that caused them

Status: done — 2026-11-24 (shipped `c675a7a` tests, `0bb4185` tracker; reviewer APPROVED after adding a
third mutant; the accepted assertion is a version range paired with a `prompt_tombstones` existence check,
not `user_version == USER_PROMPTS_MAX_KNOWN`)

## The defect

`crates/mem-store/src/schema_migrations.rs:69` reads `pub const USER_PROMPTS_MAX_KNOWN: i64 = 4;`
and its doc comment (`:64-68`) says the bump is deliberate: v4 is the issue-143 prompt tombstone
(`prompt_tombstones`, `PROMPT_TOMBSTONE_DDL` at `:91`). The constant moved; three tests that pin
the old value did not.

| test | file:line | what it asserts | why it now fails |
|---|---|---|---|
| `test_v2_file_is_a_noop_and_stays_2` | `crates/mem-store/tests/user_prompts_schema_v2.rs:300` | `USER_PROMPTS_MAX_KNOWN, 3` | the constant is 4 |
| `test_ready_upgrades_a_legacy_prompts_file_to_v3` | `crates/mem-profile/tests/user_prompts_schema.rs:90` | `user_version == 3` | `ready()` now stamps the production maximum, 4 |
| `test_ready_refuses_a_newer_prompts_file_with_the_typed_label` | `crates/mem-profile/tests/user_prompts_schema.rs:135` | seeds v4, expects the typed refusal | v4 is now supported, so `ready()` returns `Ok` and the test hits its own `panic!` at `:148` |

The third is the sharp one: it is the no-downgrade refusal test, and the thing it manufactures as
"newer than the build" is now exactly the build's own maximum.

All three are recorded as pre-existing reds in the running known-red list (`mem-store`
`user_prompts_schema_v2::test_v2_file_is_a_noop_and_stays_2`, and the two `mem-profile` tests).
None was filed. Evidence class: READ from source, 2026-11-24. Re-deriving the reds by running the
suites is the wave's first step, not this issue's claim.

## Why this is a defect and not a flake

A production constant moved one way and the assertions that describe it stayed put. Nothing
environmental is involved: the assertions compare a compile-time constant against a literal, or
seed a file at the constant's current value and demand it be refused. They cannot pass while the
constant is 4.

## The fix shape — derive, do not re-pin

Editing `3` to `4` in three places fixes today and re-arms the same defect for the next bump. The
precedent for the correct shape is the episodic bump (issue 110, `EPISODIC_MAX_KNOWN` 6 → 7): the
tests there derive from the constant.

- `test_ready_refuses_a_newer_prompts_file_with_the_typed_label`: seed `USER_PROMPTS_MAX_KNOWN + 1`,
  assert `found == USER_PROMPTS_MAX_KNOWN + 1` and `supported == USER_PROMPTS_MAX_KNOWN`. Drop the
  numeral from the test's name. This keeps the behaviour pinned (newer refused, typed label, found
  passed through, supported is the build's maximum) while the numeral moves on its own.
- `test_ready_upgrades_a_legacy_prompts_file_to_v3`: **not** `user_version == USER_PROMPTS_MAX_KNOWN`.
  That assertion is wrong here and needs no measurement to refute: `apply_user_prompts_schema` stamps a
  literal per leg — `set_user_version_statement(3)` at `crates/mem-store/src/schema_migrations.rs:806`
  and `(4)` at `:826`, each gated on `options.max_known >= n` — and returns `version`. So a
  constant-only bump to 5 skips both legs, leaves the file stamped 4, and an equality assertion would
  red criterion 3. The stamp equals the constant only while a leg exists for it.
  What to assert instead: that the stamped version is one this build's legs actually reached, together
  with the proof that the v4 leg ran — `prompt_tombstones` exists (a version-range assertion alone
  would pass vacuously if the v4 leg were skipped and the file stayed at 3). Rename to something that
  does not carry a version numeral.
- `test_v2_file_is_a_noop_and_stays_2`: this one is a judgement call. Its section comment (`:283`)
  says "a v2 file is a no-op and the maximum is 2", and the assertion at `:300` looks like a guard
  on the test's premise rather than the property under test (the file is seeded at v2 and must stay
  at v2). If the leg being protected is "a legacy v2 file still opens as a no-op", the clean fix is
  to hand that test an explicit `max_known` of its own — `migration_options(&db_path)` gains a
  max_known, so pinning 3 or 4 locally and saying why in one comment makes the test independent of
  production bumps. If instead the assertion is a deliberate "the production maximum is what the
  suite pins", it derives like the other two. The planner picks one and states the reason; the
  comment at `:283` and the message at `:301` must stop claiming "the maximum is 2" either way.

## Acceptance criteria

1. All three named tests pass, run per target: `cargo test -p mem-store --test user_prompts_schema_v2`
   and `cargo test -p mem-profile --test user_prompts_schema`. Report the target counts as measured
   output; do not pin any count as a pass condition.
2. **No test in the touched files asserts the literal maximum version.** Grep-able: the constant is
   referenced, not restated.
3. **The recurrence property is measured, not argued.** Raise `USER_PROMPTS_MAX_KNOWN` by one in a
   throwaway mutant, run the three tests, and show them still green — that is the discriminating
   measurement for "derive, do not re-pin". Restore the constant, re-run green, prove the restore
   with `cmp -s` against a snapshot. A fix that only edits three literals fails this criterion.
4. **Anti-vacuity on the refusal test.** Deleting the newer-than-build refusal path must turn
   `test_ready_refuses_a_newer_prompts_file_with_the_typed_label` red. Show the mutant applied
   (grep the text the mutation creates), the assertion RED, then restored and green.
5. `cargo fmt --all --check` and both clippy gates clean on the touched crates. `cargo test
   --workspace` is not a gate and must not appear as one.
6. No production behaviour changes: `USER_PROMPTS_MAX_KNOWN` stays 4, no migration is added, no
   schema is touched. This is a test-and-comment defect only. If the planner concludes a production
   change is genuinely needed, that is a different issue and this one stops.

## Not in scope

- The other known reds and the `mem-pm-*` latent family (issue 158).
- The live store at `~/.opencode-mem/` — untouched, read only if a reading needs it.
- Issue 152's host-capability work.

## Ceilings

- Criterion 3 proves "a bump does not red these three tests". It does not prove no other test in the
  workspace pins `USER_PROMPTS_MAX_KNOWN`; a repo-wide grep for the constant's literal values is
  cheap and worth running once, but a whole-workspace bump simulation is not.

## Outcome — 2026-11-24T05:50:52Z

Derived the three stale pins (pub const USER_PROMPTS_MAX_KNOWN: i64 = 4;). No production change.
