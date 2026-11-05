# 94 — Slice 3 (merged): memory obeys its switches, and attribution is correct

Sequencing parent — owns no work. Children: 63, 85, 86, 87.

Status: done — 2026-11-04 (SUPERSEDED — sequencing parent; child 87 fixed, children 63/85/86 tracked individually)
holds no deliverable of its own. Agents should grab the children — issue 63 (the six resolved-but-unread
injection keys), issue 85 (the chat path discards the session identity), issue 86 (attribution identity is
never produced) and issue 87 (the `capturedSessionId` race). Do not open a wave against this file.

## What to build

Two halves of the same problem — memory that reads and writes identity it never carries. Both are "a value
one layer sets and another never reads", the pattern behind every defect in this wave.

- **The switches:** injection gates on nothing, and one of the intended gates cannot even be built until
  the plumbing carries the value it needs. Issue 85 owns the plumbing, issue 63 owns the switches.
- **The identity:** child/subagent sessions are captured into the human's project journal and injected
  into their own context while the written identities that would separate them are never sent. Issue 86
  owns the producer half, issue 87 the shared-slot read.

## Acceptance criteria

- [ ] Close when its children close (issues 63, 85, 86, 87). No acceptance criterion is restated here —
      each child already holds one, and a duplicate copy in the parent drifts from it.

## Blocked by

None. The earlier `Blocked by issue 92` edge was spurious for the switches half: the injection gates are
assertable against a directly-built `InjectService` with a seeded store, so they do not wait for a capture
to persist end to end. Ordering that *is* real lives in the children: 85 before or with 63 (a switch wired
before the identity is carried cannot be honoured), and 83 before 86 (a parent row must exist before a
child row can resolve to it).

## Evidence

Issues 63 (six keys, zero readers), 85 (`session_id` discarded on the chat path), 86 (`parent_session_id`
never sent, child gate absent, `inject.agent_id` unread), 87 (the module-global read after two awaits).
Audit: `.tmp/progress/opsec-audit-20261101T090956Z.md` §S3–S7.
