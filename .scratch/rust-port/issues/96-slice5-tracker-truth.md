# 96 — Slice 5 (housekeeping): the tracker tells the truth

Sequencing parent — owns no work. Children: 89.

Status: done — 2026-11-04 (SUPERSEDED — parent of 89; 89 done)
no deliverable of its own. Agents should grab the child — issue 89. Do not open a wave against this file.

## What to build

Four places where the tracker disagrees with the tree. None is a code bug; each makes the next agent
believe something false, which is how a backlog stops being a plan.

Issue 89 owns all four (T1 the untracked reconciliation PRD, T2 the unowned `sweep_decay_daily`
carry-over, T3 issue 64's falsified conclusion, T4 the quoted `Status:` token inside issue 51). This file
does not restate them.

## Acceptance criteria

- [ ] Close when its child closes (issue 89). No acceptance criterion is restated here.

## Blocked by

None — can start immediately.

## Evidence

Issue 89. Audit: `.tmp/progress/opsec-audit-20261101T090956Z.md` §3 "Tracker defects found while doing
this".
