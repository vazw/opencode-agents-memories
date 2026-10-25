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
