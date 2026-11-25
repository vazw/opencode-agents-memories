# 170 — `cargo audit` reports `RUSTSEC-2026-0592` against `url 2.5.8`

Status: needs-triage
Filed: 2026-11-25, noticed by the issue-169 review (`.tmp/progress/issue-169-score-determinism.review-2.md`)

## The observation

REPORTED by the issue-169 review, not yet re-derived by me: `cargo audit` flags `RUSTSEC-2026-0592` against
`url 2.5.8`. The advisory text, the severity, the affected range and the reachable call path are all unread.

Pre-existing and unrelated to that diff — the 169 change adds no dependency and does not touch `Cargo.toml` or
`Cargo.lock`. Which means it has been there for some time without being caught, since no CI runs `cargo audit`
(`AGENTS.md`: "There is no CI, no release automation").

## Why this is filed rather than fixed

The repo has a specific posture for this: `cargo audit` runs from the repository root only, and `.cargo/audit.toml`
is the sole file it reads accepted findings from, and that file documents its own drift check. So there are exactly
three possible states and they need different actions:

1. **Already an accepted finding in `.cargo/audit.toml`** and the report is stale or the ignore list has drifted —
   then the fix is the drift check, not the dependency.
2. **Not in `.cargo/audit.toml` and the advisory fires on the ignored path** — then either the ignore entry is
   missing or the advisory was published after the list was last touched. Read the advisory before writing anything.
3. **Not in `.cargo/audit.toml` and `url 2.5.8` is reachable from a path that matters** — then this is a real
   security item and outranks ranking work.

Nothing here should be "fixed" by bumping `url` before establishing which state holds: `url` is pulled in by the
dependency tree, not chosen directly, and a bump may need a parent bump instead.

## First step (what settles it)

Run `cargo audit` at `HEAD` from the repository root, then read `.cargo/audit.toml` and compare. Report: the
advisory's own id text, the severity and the affected range as the advisory states them, the dependency path to
`url 2.5.8` (`cargo tree -i url@2.5.8`), and whether an ignore entry exists and why. Do not write an ignore entry
before reading the advisory — an accepted finding nobody re-read is the same vacuity this project keeps finding in
its own gates.

## Inverted world

Closed as *measurement, not a deliverable* if `cargo audit` at `HEAD` reports no `RUSTSEC-2026-0592` (the review's
observation was of a different tree or a stale database), or if the advisory is already accepted in
`.cargo/audit.toml` with a reason that still holds and the drift check confirms it.

## Not in scope

- Any dependency bump: that is the follow-up if state 3 holds.
- Issue 169's scoring change, which is unrelated and lands separately.
