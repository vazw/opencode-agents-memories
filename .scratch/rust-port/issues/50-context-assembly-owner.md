# 50 — Inject · Context assembly: the owner's rulings get a home, and the framing gets built

Status: needs-triage — 2026-10-25. **Five of the six context-assembly rulings exist only in a gitignored
draft, and the `<memory_context>` framing they describe is absent from the emitted block entirely.**

## The finding

The owner ruled the injection order on 2026-10-25. The ruling lives in
`.tmp/drafts/20261025-context-assembly-decision.md`, and `.tmp/` is **ignored** (`.gitignore:6`; `git
ls-files .tmp/drafts` → **0** files). It is not scratch — it is invisible to history.

The draft contains seven rules. The audit counted **six** as needing an owner — five with nobody, one
already owned. Mapped:

| # | ruling (draft §"The decision", rules 1–7) | owner |
|---|---|---|
| 1 | profile first, **unscored**, gated `inject_profile && user identity` | **issue 44** (owned) |
| 2 | tier order: profile → project knowledge → project/agent session history | **nobody** |
| 3 | within a tier, similarity descending, ties by recency | **nobody** |
| 4 | carry the score into the text — `relevance="N%"` | **nobody** |
| 5 | `project_agent_session_history` stays as its own block | **nobody** |
| 6 | everything else the agent queries for itself | **nobody** |
| 7 | the safety sentence goes at the **end** | **nobody** |

The draft's own `inject.rs:305` invariant restatement **is** owned — by **44.13** — and is the one item
this issue must not duplicate.

**And the framing is not a future feature; it is missing today.** `crates/mem-bridge/src/inject.rs:58-59`
defines only `HISTORY_BLOCK_OPEN`/`CLOSE`; `grep -rn "memory_context" crates src --include=*.rs` returns
exactly **one** hit — `crates/mem-bridge/tests/capture_inject_e2e.rs:251`, an assertion that the block does
**not** contain `<memory_context>`, i.e. a test that pins today's absence and must be inverted by 50.2.
`grep -rn relevance` returns **nothing**, the safety sentence appears nowhere, and the renderer wraps rows
in `<project_agent_session_history>` and stops (`inject.rs:338-349`). Upstream has the wrapper and the
framing (`example/opencode-mem/src/services/context.ts:14-47`) — so its absence here is a regression, not a
design choice, and the draft says so: *"the `<memory_context>` safety header was missing from our inject
path entirely"*.

## What to build

1. **ADR (number `0026` unless the guard ADR claims it — 0017–0024 are issue 43's, `43:400`, and 0025 is
   the guard, `43:56`) recording the five unowned rulings** — table rows 2, 3, 4, 5 and 7 — plus row 6 as
   a scope statement, plus the framing defect as Grounds, written so the gitignored draft can be deleted
   without losing a decision.
2. **The framing, implemented:** `<memory_context>` wrapper, the header framing, the end-placed
   disclaimer, `relevance="N%"` on scored tiers, and the retained history block inside the wrapper.
3. **One budget ruling that keeps the existing invariant true** — see below.

### The budget ruling (resolves the contradiction between this issue and 44.13)

`inject.rs:305` claims *"the emitted block can never exceed `4 × token_budget`"*. The draft found that
claim false once unbudgeted wrapper text is added, and 44.13 will restate it. Rather than restate a weaker
invariant, **charge the wrapper inside the budget** (the `greedy` pass and its `used` accumulator are
already there — `inject.rs:307`, `:546`). Then:

- `inject.rs:305` stays **true as written** for everything this issue adds;
- 44.13's restatement has exactly one carve-out to make — *the profile block is item-count-bounded, not
  byte-bounded* — instead of two;
- no later wave has to re-derive which bytes were charged.

That is a design decision, not an implementation detail: an invariant that a later feature can falsify
by adding a constant is the defect class this repo has spent two days removing.

## Acceptance criteria

- [ ] **50.1** `docs/adr/0026-*.md` records the five unowned rulings (table rows 2, 3, 4, 5, 7) with their
      precedence, row 6 as the scope statement it is, the Grounds entry for the missing-header regression
      (`context.ts:14-47` as the reference), and an explicit pointer to **44.13** for the invariant
      restatement — no duplication of 44.13's text.
- [ ] **50.2** The emitted block is wrapped in `<memory_context>…</memory_context>` with the header framing
      and the safety sentence **at the end**, byte-identical to the wording the ADR records. A test asserts
      the disclaimer's position relative to the last tier block, not merely its presence, and the existing
      **negative** assertion at `crates/mem-bridge/tests/capture_inject_e2e.rs:251` is inverted — it pins
      today's absence, so leaving it untouched would make the suite assert the bug.
- [ ] **50.3** Tier order is asserted with all present tiers: profile → project knowledge → project/agent
      session history. If **44** has not landed, the test asserts the two memory-tier order and leaves the
      profile slot as an explicitly skipped assertion citing 44 — not silently omitted.
- [ ] **50.4** `relevance="N%"` is emitted on scored tiers from the actual similarity score, with the
      rounding rule recorded in the ADR and pinned by a test at a boundary (e.g. 0.995 → `100%`, 0.994 →
      `99%` — pick and record, do not leave it to `format!`).
- [ ] **50.5** `project_agent_session_history` remains inside the wrapper as its own block; the profile
      tier, when 44 lands, is **unscored** (no `relevance` attribute) — a test asserts the absence, because
      a scored profile would be a different design.
- [ ] **50.6** Wrapper bytes are charged inside the budget: a test emits at the budget boundary and asserts
      the total block (`<memory_context>` included) never exceeds `4 × token_budget`, and that removing the
      wrapper from the accounting makes that test fail (the inversion arm).
- [ ] **50.7** Nothing in this issue restates, weakens or edits the `inject.rs:305` sentence; if the charge
      inside the budget makes the comment's surrounding text stale, the issue records the exact one-line
      edit 44.13 should make and leaves it to 44.13.
- [ ] **50.8** Every row of the table is mapped ruling → ADR section (or → issue 44 / 44.13) in this issue
      body, and `git ls-files .tmp/drafts` is still empty afterwards: the draft is deletable, and no tracked
      file cites it as the authority for these rulings.
- [ ] **50.9** `InjectParams`/`InjectResult` change only additively; the frozen DTOs are untouched; no new
      dependency.

## Blocked by / Blocks

- **Blocked by:** nothing for 50.1, 50.2, 50.4–50.9. `50.3`'s profile slot and `50.5`'s unscored assertion
  are blocked by **issue 44** (the profile tier must exist to be ordered). Do not pull 44's renderer in
  here to unblock them.
- **Blocks:** whoever implements issue 44's tier-1 leg needs the wrapper decision (where the profile block
  sits and whether it is scored) — this issue supplies it. Also blocks any future claim that the injected
  block is wrapped.
- **Cross-reference:** the wrapper becomes an injected block a capture can pick up. **Issue 52** owns the
  filter that must recognise it (our own block is pushed with `synthetic: true`,
  `plugin/src/plugin.js:592`), and 52's tests add `<memory_context>` once this issue lands — the edge is
  `50 → 52.4/52.5`, and neither issue may assume the other's marker coverage.

## Comments

- Filed 2026-10-25 from the opsec design-concerns audit (concern C6, verdict `SOUND` and *stronger than
  written* — the file is gitignored, not merely scratch), per the audit's §6 recommendation 5.
- **Correction carried in, per the brief's AUDIT RESULT block**
  (`20261025-design-concerns-audit-brief.md:39`): 5 of 6 rulings are unowned; the sixth (`inject.rs:305`
  restatement) **is** owned, by **44.13**. The brief's unowned list included it; it does not.
- **ADR numbering** follows `43:400` (0017–0024 are issue 43's) and `43:56` (0025 is the mixed-version
  guard). If 45's guard ADR takes 0026 for a different purpose, this one takes the next free number — do
  not create it as 0017.
- **Why this is not "documentation".** 50.2/50.4/50.6 are code: the wrapper, the score rendering and the
  budget charge do not exist. The audit's "cheapest fix is an ADR or a tracked issue" understates one half
  and overstates the cost of the other — an ADR alone would record an order for a block that has no
  wrapper.
