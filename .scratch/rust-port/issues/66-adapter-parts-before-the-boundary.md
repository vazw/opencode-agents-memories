# 66 — The adapter drops synthetic parts before the boundary (52.5's implementation)

Status: needs-triage — 2026-10-27 (planning-created re-home from issue 52; the next wave triages it)

Issue 52 measured the 52.5 question and decided **a1** (filter in the adapter, not a2's
part-metadata protocol). The code that decision needs is `plugin/src/plugin.js`, which issue 33 owns for
the compaction/restore work in the same wave — so 52 re-homes the implementation here and keeps the
measurement. Nothing in this file is a 52 deliverable.

## Owner of the code, and the sequencing constraint

- **Owner of the code:** `plugin/src/plugin.js` (`extractText`, `parseIdleTranscript`).
- **Carve note:** issue 33 owns that file for compaction/restore and runs **after** 52's merge. This
  issue runs **after 33's merge**; the two waves reconcile through the ledger, never by editing each
  other's hunks live.

## What, located precisely

- `extractText` must drop a part whose `synthetic` is `true`, and the check must be an **early return
  placed before the `typeof node.text === "string"` branch**. This is not a join filter: today a
  `{ type: "text", text: "...", synthetic: true }` part returns at that branch and never reaches the
  array join, so filtering the join would miss every real payload.
- `parseIdleTranscript` then hands the boundary a transcript with no port-injected block.
- `parseIdleTranscript`'s **`textResponses`** collection reads `part.text` directly and never calls
  `extractText`, so it is a **second surface** the same rule must cover.

## Why it is not blocked on the measurement

Dropping `synthetic` parts is correct whether or not the host returns them: a synthetic part is by
definition not user-authored text.

## Acceptance

- A plugin test that pushes a `synthetic: true` part and asserts it never reaches the capture payload
  (`transcript` and `metadata.autoCaptureContext.userPrompt`).
- A `synthetic: false` companion asserting it does.
- A case covering the `textResponses` path.
- The 52.5 verdict quoted with its evidence path (`docs/evidence/issue-52-idle-payload/MEASUREMENT.md`).

## Blocked by / Blocks

- **Blocked by:** 33 (`plugin/src/plugin.js` ownership — 33 merges after 52).
- **Blocks:** any claim that the self-feeding loop is closed.

## Cross-reference

- 50 — `<memory_context>` marker coverage (the other wrapper this filter must eventually cover).
- 52 — the store-side backstop this issue makes the *second* line of defence.
- `docs/agents/testing.md`'s `v2-memory-context` row — `:83`'s unowned transcript predicate now points
  here.
