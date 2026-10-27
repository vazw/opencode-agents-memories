# 68 — Engine-version skew is ungated: a pre-compaction engine silently double-counts

Status: needs-triage — 2026-10-27 (review-found in issue 33; recorded, not fixed, by the lane's records pass)

Issue 33 added `include.compaction` to the frozen `inject` request. ADR-0007 D-43 accepts an older
same-major engine (`engine.protocol >= wrapper.min_protocol`; the wrapper sends `protocol` and
`min_protocol` = `1.0.0` at `plugin/src/plugin.js:34-35`, the engine answers `1.0.0` at
`crates/mem-bridge/src/engine.rs:44`), and nothing in `hello` advertises whether the engine's
`inject` understands the new option. An engine built before issue 33 ignores `include.compaction` —
`fc41bbd`'s `include_options` reads only `recent_count` and `query` — and answers the compaction
request with the ordinary project chat block. The wrapper appends that block verbatim
(`plugin/src/plugin.js:717-718` at `6f8801b`: the `Array.isArray(output.context)` guard, then
`output.context.push(result.block)`), and the restore prompt carries it too.

That is the double-count AC 1 forbids — the injected `<project_agent_session_history>` block, plus the
episodic `## Recent sessions` / `## Related older sessions` legs, arriving a second time inside the
compaction context. It is **silent**: the engine answers successfully, the hook sees a non-empty block,
and no log records the version mismatch as one.

## Why it is reachable

`plugin/engine/<platform>-<arch>/` is gitignored and staged by `plugin/scripts/stage-engine.sh`. A
stale build left there beside a newer wrapper satisfies D-43's major-equal floor and is spawned. The
non-blocking argument is only that a shipped wrapper and engine come from one build; any hand-staged,
downgraded or partially updated install breaks that premise.

## What, located precisely

- **Advertise the capability.** D-43's `hello.result.capabilities` object is the additive home: emit
  `capabilities.compaction: true` from an engine that reads `include.compaction`.
- **Gate on it.** The wrapper records the capability from `hello` and passes `include.compaction` only
  when the engine advertised it; when absent it must **no-op** the two compaction hooks (no ingest
  context, no restore prompt, no toast) rather than fall through to a chat-block answer — the failure
  mode is a silent wrong answer, not a visible error.
- **The alternative** is raising `MIN_PROTOCOL` past the pre-compaction engine line, which refuses
  the old engine at the handshake (D-38 degradation) instead of silently mis-answering. Choose one;
  prefer the capability flag, because it does not shrink the accepted engine range.

## Cross-references

- **ADR-0007 D-43** — the `hello` handshake, the semver rule, and `capabilities` as the additive
  surface.
- **ADR-0017 "Known limitations"** — the same property, recorded where the compaction contract lives.

## Acceptance

- A wrapper test: a `hello` result without `capabilities.compaction` yields no compaction `inject`
  on the wire, and neither compaction hook changes `output.context` or calls `session.prompt`.
- An engine test: the capability is present in `hello` exactly when `include.compaction` is honoured.
- The two hooks' no-op is pinned, not assumed: an implementation that instead appends the chat block
  fails the first test.

## Blocked by / Blocks

- Blocked by: nothing — `plugin/` and `crates/mem-bridge/src/engine.rs` are both free to change after
  issue 33's merge.
- Blocks: nothing. Issue 33 ships correct behaviour against an engine from the same build.
