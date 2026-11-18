# 146 — the engine's inject block has no background-shell section

Status: needs-triage — filed 2026-11-18 by issue 145 Step 9 (inject block carries no background-shell section)

*(Filed by issue 145, blueprint `20261118-0715-issue-145-inject-sink-r24`, Step 0 item 7.)*

## The gap

The engine's inject block (`crates/mem-bridge/src/inject.rs:85-88`) carries four history headings:
`## Project facts`, `## Captured session summaries`, `## Recent sessions`, `## Related older sessions`.
None of them is a background-shell section. Measured 2026-11-18: `grep -c -i 'shell' crates/mem-bridge/src/inject.rs` = `0`.

## Consequence

A child agent cannot be told which background shell it left running. The owner's background-shell need is therefore
unmet by issue 145. Issue 145 ships the sink and the post-restart runbook; it does not add this section.

## Ask

Triage whether the engine block should carry a background-shell section, and where that data comes from.
The runbook at `docs/evidence/issue-145/README.md` (Step 11) measures this gap per run as
`OPEN — block carries no background-shell section`.
