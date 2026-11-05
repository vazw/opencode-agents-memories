# 93 — Slice 2: the capture's structured half is populated

Sequencing parent — owns no work. Children: 84.

Status: done — 2026-11-04 (SUPERSEDED — child 84 landed 585956d)
no deliverable of its own. Agents should grab the child — issue 84. Do not open a wave against this file.

## What to build

Make a persisted capture carry its structured payload: files touched, work completed, decisions, open
threads — and make recall able to filter by them.

The mechanism is the child's: the schema that asks for those four fields was silently dropped by the
streaming provider arm the summariser used, so the base shape parsed, the extension defaulted to empty and
the touched-files filter matched nothing forever, with a green suite. Issue 84 owns that repair, its RED
rows and its evidence.

## Acceptance criteria

- [ ] Close when its child closes (issue 84). No acceptance criterion is restated here.

## Blocked by

None. The earlier `Blocked by issue 92` edge was spurious: the structured extension can be asserted on a
directly-built capture service with a scripted structured provider, so it does not wait for a breadth-first
capture to persist end to end.

## Evidence

Issue 84 (7 of 8 streaming arms drop the schema; 5 distinct builders + 2 delegations). Probes (copied into
`docs/evidence/` as a residual of issue 84 — they currently live under gitignored `.tmp/scratch/`):
`.tmp/scratch/probe-stream.out` (308 bytes of markdown, `parses as JSON: NO`) and
`.tmp/scratch/probe-capture-format.out` (structured arm returns a valid tool call with a real summary).
