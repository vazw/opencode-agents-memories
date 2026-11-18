# 153 — a supported delivery probe: proving an injected block reached the agent is still an ad-hoc ritual

Status: needs-triage — 2026-11-19 (filed by architect from the post-149 design discussion)

*Scope note. Issue 147 records four dead-path defects in the existing probe harness. Issue 148 rewrote the runbook to the
nonce instrument. This issue asks whether a supported probe belongs in the product at all, which is a different question
from fixing the harness.*

## The gap

Proving that an injected block reached an agent is currently a manual ritual: write a nonce to memory, spawn a throwaway
child, demand that it quote the token, then delete the nonce (`docs/evidence/issue-145/README.md`, the procedure
around its steps 1–3). READ.

The host does not persist the block in a readable row. MEASURED 2026-11-19 and recorded in the runbook's Trap A: the old
predicate `type='synthetic' and data like '{"text":"<memory_context>%'` matches only mid-October sessions, and none
since. Issue 148 records the same finding.

## What the product already emits, and what it does not

- The wrapper appends structured lines to the engine log (`plugin/src/plugin.js:220-234`, `writeStructured` and
  `appendFileSync`, READ). The log holds `inject.received` with `block_bytes`, `handling_ms`, `status` and `session_id`
  (MEASURED: the log has these lines; the count is not pinned here).
- It does **not** record whether the host placed the block in the model's context, or in a persisted message part. The
  wrapper pushes the part at `plugin/src/plugin.js:1430` (`output.message.parts.push`, READ) and has no read-back of it.

So `block_bytes > 0` is evidence the engine answered, not evidence the agent received it. This is the same gap 145 found,
restated for the probe.

## Why it matters, in the owner's terms

Every claim that a memory feature works currently rests on someone writing a nonce by hand and reading an answer. That
does not scale, it is easy to get wrong (147), and it has already produced a false reading (the dead predicate). A probe
the product owns would make delivery a checkable fact rather than a procedure.

## Consequence of leaving it

Feature issues (149, 150) each need their own bespoke measurement. Regressions in delivery are found by whoever runs the
ritual, not by a test.

## What it would touch

- `plugin/src/plugin.js:220-234` — the structured log, if the probe reads it.
- `docs/evidence/issue-145/README.md` — the current procedure.
- Possibly `experimental.chat.messages.transform`. The local SDK declaration lists it
  (`/home/opencode/.config/opencode/node_modules/@opencode-ai/plugin/dist/index.d.ts`, READ), which would see messages
  before the model call. Whether the V2 host calls it is **unverified**.

## Known limits

- A nonce proves the model could read the block. It does not prove the block was the only source of the nonce; a
  nonce written to memory is itself retrievable by other paths (a memory search would return it; INFERRED, not measured here).
  The probe design must answer that.
- The probe would be a second instrument next to the nonce. Both must agree or the disagreement must be recorded.
- This issue does not fix 147's harness defects; those are on 147.

## Inverted world

If the host persists the block in a readable row in a live run, the ritual is unnecessary and the product probe is
redundant. Measure this before building anything.

## Open questions for the planner

1. Should a delivery probe be a product feature, or an evidence-only harness kept in `docs/evidence/`? The owner has
   treated the harness as evidence so far.
2. What must the probe prove: receipt by the model, receipt by the host, or the block's contents? Each has a different
   instrument.
3. Does `experimental.chat.messages.transform` fire for V2 child sessions? Measure it before designing on it.
4. What does a probe do to a live child's context, and how is that cost bounded?
