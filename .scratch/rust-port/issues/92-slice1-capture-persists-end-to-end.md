# 92 — Slice 1 (tracer bullet): one capture persists end to end

Sequencing parent — owns no work. Children: 83, 88.

Status: done — 2026-11-04 (SUPERSEDED — sequencing parent; children 83 fixed, 88 superseded/closed)
holds no deliverable of its own. Agents should grab the children — issue 83 (the wrapper supplies the
identity the store requires) and issue 88 (the host wire, measured before edited). Do not open a wave
against this file.

## What to build

The thinnest path that proves the capture pipeline works at all: a real turn produces a capture, the host
summarises it, and a row exists that `list` / `recall` can return.

Nothing in this path works today, and it fails in three places in sequence — which is why the children
ship together. Fix the payload alone and the capture still dies at the wire (`errorKind:"protocol"`); fix
the wire alone and it dies at the write (`episodic_write_failed`). Neither half is demoable on its own.

The measurement step lives in issue 88 (capture one request/response pair between the engine and the live
OpenCode host, a scratch copy and never the live engine). The identity step lives in issue 83. This file
does not restate either.

## Acceptance criteria

- [ ] Close when its children close (issues 83 and 88). It has no acceptance criteria of its own; any
      duplicate stated here would be a second, drifting copy of a child's contract.

## Blocked by

- Issue 91 (the payload-shape harness) gates **issue 83's landing**, not the children's start — it
  supplies the fixture-as-oracle that makes 83's RED honest.

## Evidence

Issues 83, 84, 88. Live measurement 2026-11-01 (as of that date, point-in-time): `Auto-capture provider
ready … providerId:"opencode-go", hostVersion:"2.2.5"` immediately followed by `errorKind:"protocol"`;
0 persisted that day, and 0 Rust-format `Auto-capture memory persisted` lines across all 22 log files.
