# 65 — three tracked docs describe a world that no longer exists: a stale test count and two stale `skills` 500 claims

Status: ready-for-agent — 2026-10-27 (premise measured at HEAD `39bddc2`; filed from the issue-61 debugger audit,
which found one of the three while auditing an unrelated ledger. Brief at the end of this file.)

## PREMISE — measured, not inherited

**1 — a stale test count in the registry's own prose.**

`docs/agents/testing.md:181` says `crates/mem-web/tests/api_endpoint_contract.rs` **(16)**. The file declares
**17** test functions (`grep -cE '^\s*#\[(tokio::)?test\]'`), and the binary reports 17 passed in a run. It has
been stale since whatever wave added the 17th test. This is the doc whose own header tells readers to re-derive
figures before citing them.

**2 — the skills `500` claim, twice, in the smoke doc.**

`docs/web-skeleton-smoke.md:205-209` says the production wiring leaves `SkillPort` at `unavailable()` "until W6.4
lands the skills manager", and that until then "the skills routes answer `500 …`". The route table at
`:498-504` (rows 29–35) repeats it: `500 | BLOCKED | issue 26 (W6.4): skills manager + .with_skills(...) not
wired`.

Both are false on two counts. `src/main.rs:173` and `:187` both call `.with_skills(skills_port)` — the port is
wired, on the production arm and on the degraded arm alike. And after issue 61 the answer for an absent port is
**501** with the absent-service envelope, not 500; the two `skills.rs` catalog arms that remain 500 are the
deliberate upstream mirrors, not the wiring state.

**3 — issue 37 contradicts itself on the same claim.**

`.scratch/rust-port/issues/37-composition-root-production-api-wiring.md:11` opens with *"**The `skills` 500 claim
is stale and is withdrawn.**"* — and then `:96` (`| GET /api/skills | 500 same |`) and `:155` (`**skills
answers 500**`) still assert it. A reader who starts at the AC table never learns it was withdrawn.

## WHY IT MATTERS

These are not stylistic nits. `docs/web-skeleton-smoke.md`'s table is a **verification instrument**: its rows are
the ones a smoke run walks, and a row that says `500 | BLOCKED` when the route answers 200 tells the next
operator their environment is broken. The registry entry is worse in kind — it is the file that tells future
waves which upstream test maps to which port-side file, and it carries a count that its own instruction says
must be re-derived.

Issue 37's contradiction is the same failure one level up: the correction exists, but it was appended next to
the claim instead of replacing it, so both survive.

## WHAT A WAVE SHOULD PRODUCE

1. **Re-derive every figure you touch**, and write the command beside it. `api_endpoint_contract` = 17 by
   declaration count; confirm by running the binary.
2. **Correct the smoke doc's skills rows and the note above them** to the post-61 truth: the port is wired, an
   absent port answers 501, and the two catalog arms keep their documented upstream 500 — say which is which so
   the next reader can tell a deliberate 500 from a fault.
3. **Resolve issue 37's self-contradiction**: `:11`'s withdrawal is authoritative, so `:96` and `:155` must be
   rewritten to match or deleted. Do not add a third statement.
4. **Sweep the two files for the same claim in other forms** (`500`, `BLOCKED`, `W6.4`, `unavailable()` for
   skills) and report what you find, so this is one pass rather than four future corrections.
5. **Do not restate a count you did not run.** If a figure cannot be re-derived in this wave, leave it and say
   so rather than copying the old value.

## ACCEPTANCE — what would close it

- [ ] `docs/agents/testing.md:181` carries the re-derived count, with the command that produced it.
- [ ] `docs/web-skeleton-smoke.md`'s skills note and rows 29–35 state the wired behaviour and the 501/500
      distinction correctly, naming which arms are deliberate mirrors.
- [ ] Issue 37 has one statement about the skills claim, not two contradictory ones.
- [ ] A sweep of both files for the same claim reports every hit, with each one corrected or explained.
- [ ] No figure in the diff is a copy; each is a command's output, and the diff is docs-only.

## Out of scope

- Any change to `crates/mem-web/src/api/skills.rs` behaviour — issue 61 landed the 501 class and left the two
  catalog arms as deliberate upstream mirrors.
- The bridge-schema contract statement — issue 37 AC4 owns that file.
- Re-measuring unrelated counts elsewhere in `docs/agents/testing.md`; report them, do not fix them blind.

## Agent Brief

**Category:** docs
**Summary:** three tracked docs describe a wiring state and a test count that the tree has moved past.

**Current behavior (verified at HEAD `39bddc2`):**
`docs/agents/testing.md:181` says `api_endpoint_contract.rs` (16); the file declares 17 and the binary runs 17.
`docs/web-skeleton-smoke.md:205-209` and `:498-504` say the skills routes answer `500` and are `BLOCKED` on
"issue 26 (W6.4)" because `.with_skills(...)` is not wired — but `src/main.rs:173` and `:187` both call it, and
after issue 61 an absent port answers 501. Issue 37 withdraws the `skills` 500 claim at `:11` while `:96` and
`:155` still assert it.

**Desired behavior:**
The registry's count matches the file. The smoke doc's skills rows describe the wired behaviour and distinguish
the deliberate upstream-mirror 500s from the absent-service 501. Issue 37 carries one statement about the claim.

**Key interfaces:**
- `docs/agents/testing.md` — the test-disposition registry; its header instructs re-derivation.
- `docs/web-skeleton-smoke.md` — the route table a smoke run walks; rows 29–35 are the skills group.
- `.scratch/rust-port/issues/37-composition-root-production-api-wiring.md` — `:11` is the withdrawal, `:96`/`:155`
  the stale assertions.
- `src/main.rs:173`/`:187` — the two `.with_skills(...)` calls that falsify "not wired".
- `crates/mem-web/src/api/skills.rs:260`/`:291` — the two deliberate catalog 500 mirrors.

**Acceptance criteria:**
- [ ] The count is re-derived and the command recorded.
- [ ] The smoke doc states the wired behaviour plus the 501/500 distinction, and names the mirror arms.
- [ ] Issue 37 has one statement, not two.
- [ ] A sweep reports every occurrence of the claim, corrected or explained.
- [ ] Docs-only diff; no copied figures.
