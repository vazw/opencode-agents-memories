# Evidence-gated delivery workflow

**Established:** 2026-10-25 by owner directive. **Supersedes** the previous review chain for all
implementation work.

## The principle

> *"Don't trust, verify."*

No claim is accepted because it is stated confidently, by a human, by an agent, or by this document.
The fleet cannot get answers from authority — several load-bearing beliefs in this repo turned out false
when measured, and three of them came from reviewers checking the work. So: **evidence, measurement,
benchmark.** If something can be tested, it is tested before it is believed.

Every substantive claim carries a provenance label, and the label is not decoration:

| label | means |
|---|---|
| `MEASURED` | observed by running something here; the command or artifact is cited |
| `READ` | read from source at a named commit/path; not executed |
| `REPORTED` | taken from an earlier record without re-verification |
| `INFERRED` | reasoned, not observed; must name what would settle it |

A claim that cannot reach `MEASURED` stays open. It does not become a decision by repetition.

## The pipeline

```
architect  ──►  two competing drafts (A / B), each with pros, cons and a
                discriminating measurement
      │
      ▼
   A/B measure  ──►  one of: A wins · B wins · neither (the measurement
      │               refutes the premise — report that, do not force a pick)
      ▼
syseng gate  ──►  APPROVE / REJECT the surviving draft
      │
      ▼
coder  ──►  implementation
      │
      ▼
opsec review loop  ──►  APPROVE / REJECT; rejects return to coder
```

### 1. Architect A/B draft

The architect produces **two genuinely competing designs**, not one design and a strawman. Each draft
carries:

- **Pros** — what it buys, stated as an outcome, not a property.
- **Cons** — what it costs, including the failure mode it creates.
- **Predicted outcome** — what a measurement would show if this draft is right.
- **Refutation** — the observation that would kill it.
- **The discriminating measurement** — the one experiment that separates A from B. Shared by both
  drafts; agreed before either is chosen. If no measurement separates them, that is itself the answer:
  pick on cost, and say so.

A draft that cannot be distinguished from its alternative by any observation is not a design question.
Say that instead of filling a document.

### 2. A/B measure

Run the discriminating measurement. Report the result whether or not it is convenient:

- **A wins / B wins** — record why, with the numbers.
- **Neither** — the premise was wrong. Record the refutation and go back to step 1, or close the item as
  *measurement, not a deliverable*. A measurement that dissolves the work is a success, not a failure.

A/B here is a **design** A/B (two candidate designs), not two implementations and not two model runs.
If the work ever needs a behavioural A/B — two live paths measured against each other — it is named
explicitly as such, because the two mean different things.

### 3. syseng gate

`syseng` adjudicates the surviving draft before any code is written: soundness, scope discipline,
operational risk, and whether the measurement actually supports the choice. It answers `APPROVE` or
`REJECT — <required modifications, complete>`.

The gate is **read-only**: it never edits the draft and never spawns. A rejection returns to the
architect, with a hard cap (see `fleet-pipeline` revision caps).

### 4. Coder implements

The approved draft becomes the work. One gate pass after the edits, then measurement on the warm
target. Cargo is the last step of the wave.

### 5. opsec review loop

**This is the change from the previous chain.** `opsec` used to audit the design *before* it was built.
It now reviews the **implementation**: does the code do what the draft claimed, are the security
properties real in the shipped path, is any guarantee asserted rather than measured. It answers
`VERDICT: APPROVED` or `REJECTED`; rejects loop back to the coder.

`opsec` reviews the *implementation*, `syseng` gates the *design*. Neither substitutes for the other,
and neither substitutes for the measurement.

### 6. Wave closeout — worktree cleanup

After independent review approval and integration, retire finished worktrees and their exclusive
regenerable caches through this checklist. Unsafe trees stay retained with a reason and next action.

1. **Preserve evidence first.** Move required decisions, measurement artifacts, logs, blueprints and
   ledgers outside the retiring tree. Durable decisions and required evidence belong in tracked
   repository evidence/docs or durable linked issue/ADR records, not solely another `.tmp/`. Preserve
   historical ledgers byte-for-byte; record archive locations and checksums in a fresh closeout record.
   Verify copied artifacts and links remain usable after removal.
2. **Verify delivery.** Confirm branch commits are integrated into the intended destination and pushed
   when the wave's delivery contract requires it. Record destination/ref/commit and push status or an
   explicit not-applicable reason. Unknown integration or push status blocks cleanup. This does not
   authorize branch deletion or require every wave to push.
3. **Verify inactivity.** Confirm no sessions or processes use the tree or its caches: planners, coders,
   tests, build shells, and engine/plugin processes included. Move or finish sessions and stop only
   owned inactive work after coordination. Recheck immediately before removal; uncertainty blocks it.
   Run removal from a surviving checkout outside the candidate tree.
4. **Inspect all contents.** Check tracked modifications, staged changes, untracked files and ignored
   contents, including `.tmp/`, build caches and storage; clean `git status` alone is insufficient.
   Preserve required files first. Dirty, active, locked, unintegrated or otherwise unsafe trees remain
   with an explicit retained reason and next action. Unknown ownership, symlinks or concurrent work
   means retain, not guess. Never reset, clean, unlock or force away blockers to make removal succeed.
5. **Remove only verified safe paths.** Use `git worktree remove <absolute-worktree-path>` for a positively
   identified safe clean worktree; never use force without explicit separate approval. Git refusal is
   a retained blocker. Reclaim only confirmed exclusive regenerable target caches associated with the
   retired tree; verify resolved paths and ownership, not names alone. Shared/external
   `CARGO_TARGET_DIR`, live staged plugin, live storage, backups and uncertain paths are excluded.
   An embedded target disappears with safe worktree removal; do not delete it twice. Any separately
   scoped external exclusive cache cleanup needs the same inactivity/path verification, not broad
   recursive deletion.
6. **Record closeout in a surviving location.** Record removed paths or retained reasons, next actions,
   preserved evidence locations and command outcomes. Measure before/after allocated usage, reclaimed
   bytes and remaining repository usage; state scope and units, with external exclusive cache usage
   separate where applicable. Disclose concurrent growth or failed measurements; never guess savings.
   A docs-only workflow update performs no cleanup and reclaims no disk.

## Recording — always

Every run leaves three things behind:

1. **Pros and cons** of both drafts, in the A/B record. The losing draft is never deleted: a design that
   was rejected for a stated reason is the most useful page in the file six months later.
2. **The decision**, as an ADR when it is architectural or hard to reverse, as an issue comment when it
   is scoped. Never only in `.tmp/` — that tree is gitignored, and a ruling that lives only there is one
   `rm -rf` from gone.
3. **The measurement**, with its artifact path, so a later reader can re-run it instead of trusting the
   summary.

Where a decision supersedes an earlier one, the earlier one stays visible marked superseded. Silent
overwrites destroy the only thing this record has that a search engine does not: the history of what was
believed and why it changed.

## Wave ledgers — authority, stamps and template

### Authority and provenance

A ledger row is a measurement or it is nothing. The wave ledger (`.tmp/progress/<blueprint_id>.ledger.md`)
is the sole authority after an interruption or uncertainty; reconstructed or ungrounded timestamps
destroy that authority.

### Rules

- **Rule 1 (Live Timestamps)**: Every `STARTED` and `DONE` stamp must be the literal output of
  `date -u +%FT%TZ` executed at the moment the step begins or its gate passes. Handwritten or
  batch-reconstructed stamps without disclosure are strictly prohibited.
- **Rule 2 (Anti-Fabrication & Clock Skew Guard)**: A stamp must never exceed the host clock or the
  ledger file's own mtime (no stamps from the future). Future-dated stamps invalidate the row.
- **Rule 3 (Mandatory Reconstructed Disclosure)**: If an agent fails to record a stamp live and writes it
  post-hoc, it MUST explicitly disclose `reconstructed <time>` (e.g. `- [x] DONE reconstructed 2026-11-04T12:00:00Z — <proof>`).
  Honest disclosure is acceptable; disguised backdating is a falsification.
- **Rule 4 (Manifest Bracketing)**:
  - Single-shard waves must open with `WAVE START: <live UTC timestamp> (HEAD <commit>, clean/dirty)`
    and close with `WAVE END: <live UTC timestamp> (HEAD <commit>)`.
  - Multi-shard waves must bracket each shard with `SHARD START: <shard-name> <live UTC timestamp> (HEAD <commit>)`
    and `SHARD END: <shard-name> <live UTC timestamp>`.
- **Rule 5 (Explicit Rework State)**: Every ledger must contain an explicit `rework:` state field per
  wave/shard (`rework: none (initial execution)`, `rework: none (accepted)`, or `rework: iteration N (<finding IDs, actions taken>)`).
- **Rule 6 (Verbatim Removed Assertion Record)**: Any test, assertion, or test helper removed, disabled,
  or weakened must be recorded verbatim in the ledger under `## Removed assertions / tests`, citing the exact code,
  file, and justification. Coverage deletion is presumed regression until verified.
- **Rule 7 (Forward-Only Immutability)**: Historical ledgers in `.tmp/progress/*.ledger.md` are immutable
  audit records and must never be edited or retroactively rewritten. All ledger rules are forward-only.

### Canonical Ledger Template (Single-Shard Wave)

````markdown
# Ledger: <blueprint_id>
WAVE START: 2026-11-04T18:00:00Z (HEAD a1b2c3d, tree clean)
rework: none (initial execution)

- [ ] Step 1 — <name> — gate: <proof>
- [ ] Step 2 — <name> — gate: <proof>

## Removed assertions / tests
None.

WAVE END: 2026-11-04T18:30:00Z (HEAD e4f5g6h)
````

### Canonical Ledger Template (Multi-Shard / Fan-Out Wave)

````markdown
# Ledger: <blueprint_id> (shard <shard_name>)
SHARD START: <shard_name> 2026-11-04T18:00:00Z (HEAD a1b2c3d)
rework: none (initial execution)

- [ ] Step 1 — <name> — gate: <proof>
- [ ] Step 2 — <name> — gate: <proof>

## Removed assertions / tests
None.

SHARD END: <shard_name> 2026-11-04T18:25:00Z
````

## Standing rules

- **Second-hand figures are re-derived, never inherited.** Every count that arrives from a report, an
  audit or a review gets counted again in the tree. Three figures in one day were wrong this way, and
  one of them was inverted.
- **Every gate carries an inverted world.** State the observation that would close the item as
  *measurement* rather than as *a fix*. A gate that cannot fail is not a gate.
- **Refuted by measurement outranks plausible by argument.** When they disagree, the measurement wins,
  and the argument is recorded as refuted rather than quietly dropped.
- **A reviewer's prescription is a claim, not an instruction.** Apply it only after checking it against
  the tree; reviewers in this repo have been wrong about which files to edit and which routes exist.
- **Never claim a measurement that was not run.** "Not covered by any evidence here" is an acceptable
  answer. A confident sentence with nothing behind it poisons everything downstream of it.
