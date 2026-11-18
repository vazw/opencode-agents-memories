# 151 — a session with no cached injection gets nothing: the `context`-cache fallback (M1 proper)

Status: needs-triage — 2026-11-19 (filed by architect from the post-149 design discussion)

*Scope note. Issue 149 decision 2 names this as out of scope: its restore happens on the next prompt, and the
never-prompted case is excluded. Issue 150 owns the refresh policy for sessions that have been prompted. This issue
owns the other case: a session that gets a `context` event with no cached block at all.*

## The gap

`plugin/src/v2/adapter.js:86-89` — the `context` hook reads `contexts.get(event.sessionID)` and pushes it onto
`event.system` only if present. A session with no cache receives nothing from this path. The cache is written only at
`adapter.js:83` (`if (injected) contexts.set(...)`), which runs only inside the `prompt` hook (`:58`).

The absence is pinned. `plugin/tests/v2-plugin-adapter.test.mjs:178` asserts
`"a session with no cached injection gets nothing"`. READ.

## Why it matters, in the owner's terms

A child whose `context` event fires without a prior cached prompt gets no memory block at all. If such children exist,
they are the children the memory block is for, and nothing reaches them. Whether such children exist is **not verified
here** (see Known limits).

## Consequence of leaving it

If the premise holds, every session in that state is a session the memory system silently skips. The failure is quiet:
no error, no `inject.received` line, no block.

## What it would touch

- `plugin/src/v2/adapter.js:86-89` (the `context` hook) and `:58` / `:83` (the `prompt` path that is the only writer).
- `plugin/tests/v2-plugin-adapter.test.mjs:178`. This test is the contract, so a fix changes it, and the change must be
  argued, not patched around.
- An engine request without a query. The compaction path already sends one with no query text:
  `plugin/src/plugin.js:1516-1522` (`compactionParams`: `session_id` and `include.compaction`, no query). READ. So a
  query-less inject exists on the wire, but the chat-block path has never been exercised that way (REPORTED by reading;
  not run).

### What would have to change in the contract, and why it is not a regression

Test `:178` says "no cached injection, no block". The contract it encodes is that the cache is the only source of a
chat block. A fix makes a second source: a query-less request on a `context` event for a session with no cache. That
changes the assertion, and it is an extension rather than a regression, because the pinned test describes the cache's
behaviour and not a product requirement. The planner must state the new rule in the test's name before editing it.

## Known limits

- **Unverified premise.** This issue assumes some child sessions reach `context` without ever passing the `prompt` hook.
  Not measured. The 145 record shows children missing blocks while the engine answered `profile inject` (REPORTED,
  `docs/evidence/issue-145/README.md` and the 145 issue file); that does not show the `prompt` hook was skipped.
- Compaction-triggered delivery is not this issue; that is 150 and 149.
- The wrapper's `prompt` hook for child sessions is host behaviour. The wrapper cannot observe it from its own code.

## Inverted world

A live child that receives a `context` event with no prior `prompt` hook call is the premise. If every `context` event in
a live run is preceded by a `prompt` call for the same session, the premise is false and this issue closes as measurement,
not a deliverable.

## Open questions for the planner

1. Does the host call the `prompt` hook for every child session? Measure it: a spawned child, a logged hook call,
   checked against its first model step.
2. If a `context` event arrives with no cache, is that a child of the spawn path or some other session kind? Identify the
   session type before choosing a fix.
3. Does the premise survive 150's fix? If the `prompt` hook always fires, this issue may be moot.
4. Should a query-less block be allowed at all, or is the query the reason the cache exists? This is an owner question
   about the product contract.
