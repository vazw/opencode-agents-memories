# 0017 — The Compaction Hook (ingest before, restore after)

- **Status:** Proposed
- **Date:** 2026-10-27 — measurement bases: `fc41bbd` (the lane's branch point; issue 37 and issue 52
  landed) and `6f8801b` (the lane's commit). Every `file:line` pointer states its base; an unlabelled
  number is at `6f8801b`. Blueprint `20261027-0824-w5-5-compaction-hook-r17` (SYS-APPROVED
  2026-10-27T14:43Z).
- **Binding on:** `plugin/src/plugin.js`, `plugin/src/compaction.js`, `plugin/package.json`,
  `crates/mem-bridge/src/{compaction,inject,composition}.rs`,
  `crates/mem-core/src/logging/signatures.rs`, and the two new JS test files.
- **Supersedes / amends:** implements the last host-lifecycle seam left by
  `docs/bridge-schema.md` (frozen; no wire change). Records a deliberate non-use of
  `experimental.compaction.autocontinue`.

## Context

OpenCode exposes two halves of the compaction seam to a V1 plugin
(`@opencode-ai/plugin@1.19.4`, `dist/index.d.ts`; the locally installed 1.19.0 carries the same
signature at `dist/index.d.ts:283-288`):

- `experimental.session.compacting(input: {sessionID}, output: {context: string[]; prompt?: string})`
  — runs **before** compaction; `output.context` entries are appended after the default prompt,
  joined with `\n\n`.
- the `session.compacted` host event — fires **after** compaction succeeds and before a synthetic
  auto-continue turn.

Upstream (`example/opencode-mem/src/index.ts:1301-1370`) only uses the second: it reads the
session's tier-1 memories, renders them with `formatMemoriesForCompaction`
(`example/opencode-mem/src/index.ts:1425-1441`), resolves the agent and the model
(`example/opencode-mem/src/index.ts:76-146`; the two declarations open at `:75` and `:153`, the
bodies run through `:178`), sends ONE synthetic no-reply message into the agent that was running,
and toasts the count. The port had neither half: `git grep -n "compacting" fc41bbd -- plugin/src
plugin/tests` printed nothing, and `session.compacted` existed only inside issue 20's V2 adapter.

At `fc41bbd` the port therefore had a genuine capability gap, not an unreachable one. This ADR
records the shape chosen to close it and every decision that shape forecloses.

## Version lines

Measured 2026-10-27; each line supports a different premise:

| line | value | supports |
|---|---|---|
| application | `opencode v2.1.4-local` (`opencode --version`) | which application the wave ran against; **not** evidence for P12/P16 |
| installed V1 typings | `@opencode-ai/plugin` **1.19.0** | P12's hook signature, read in-tree at `dist/index.d.ts:283-288` |
| published V1 typings | `@opencode-ai/plugin` **1.19.4** (unpkg) | P12 and P16 |
| published V2 typings | `@opencode/plugin` **2.0.27** / **2.1.4** (unpkg) | P18's registrable surface only |
| V2 typings installed locally | **no** (`~/.config/opencode/node_modules/@opencode/plugin/package.json` absent) | no local V2 `.d.ts` may be cited; host-side V2 dispatch is unconfirmed |

## Decisions

**DECISION 1 — `compaction.enabled` / `compaction.memory_limit` reach the injector through a
two-line `composition.rs` insertion at the existing `InjectService` construction site.**
`InjectService` holds no `Config`; its only production construction site is the
`let inject = InjectService::new(` chain that closes at
`.with_profile(profile.clone(), ProfileInjectPolicy::from_core(config));` (measured `:239`-`:250`
at `6f8801b`; `:238`-`:249` at `fc41bbd`, before this lane's insertions). The chain is
semicolon-terminated, so an inline `.with_compaction(..)` would be a deletion; the accepted form is
a `use crate::compaction::CompactionPolicy;` line plus a
**rebind** immediately after the `.with_profile(...)` statement and before the `// Q8` comment:
`let inject = inject.with_compaction(CompactionPolicy::from_core(&config.compaction));`.
`git diff --numstat` for the file therefore reads `2 0`. Issue 52's `.with_privacy(..)` hangs off
the `CaptureService` chain — a different statement — and is not touched.

**DECISION 2 — AC 4's child gate is explicit and positive-evidence-only.** Upstream's agent-only
property is emergent because `captureChildSessions` defaults to `false`
(`example/opencode-mem/src/index.ts:674-676`; default `config.ts:318`). In the port that key has
zero readers under `crates/mem-bridge` (`grep -rn "capture_child_sessions" crates/mem-bridge` →
nothing at `fc41bbd`), so a child session's capture does write a tier-1 row under the child's own
`session_id`, and a session-keyed restore would find it. The divergence is recorded rather than
papered over: the lane gates both halves on the host's own child predicate
(`session.get().parentID`, upstream's `isChildSession` at `example/opencode-mem/src/index.ts:240`).
The gate is **positive-evidence-only**: a non-empty `parentID` means child; an absent
`session.get` (the normal V2 case — `plugin/src/v2/legacy-client.js:117-134` exposes no
`session.get`), a `null` response or a throw means **not a child, proceed**. Treating "unknown"
as "child" would silently disable the restore for every V2 session. When issue 63 wires the
capture gate the gate becomes redundant but stays correct; the tests pin the observable
(`promptCalls`, `contextAfter`), never the mechanism.

**DECISION 3 — the pre-compaction context is the session-scoped tier-1 render.** The port's own
injected `<project_agent_session_history>` block and the episodic `## Recent sessions` /
`## Related older sessions` legs are never appended: they are part of *that* block
(`HEADING_RECENT`/`HEADING_OLDER` at `crates/mem-bridge/src/inject.rs:77-78` at `6f8801b`; `:74-75`
at `fc41bbd`), so appending them
would double-count. The row-level overlap with `## Captured session summaries` is intentional and
bounded by `memory_limit`: the guarantee is that the facts survive compaction, not that the block
does.

**DECISION 4 — no frozen-schema change.** `session_id` is already a documented `inject` param
(`docs/bridge-schema.md:83`) and `include` is already the additive options object
(`crates/mem-bridge/src/inject.rs:107-120`). The compaction request rides
`params.session_id` plus `include: {compaction: "ingest" | "restore"}`. No new wire method, no new
response key, no fixture adjustment, no edit to the frozen document.

**DECISION 5 — V2: the `session.compacted` half is inherited and pinned; the
`experimental.session.compacting` half is not wired.** The V2 host exposes compaction under a
different extension point (`SessionHooks { readonly compaction: SessionCompaction }`,
`@opencode/plugin@2.0.27` `dist/promise/session.d.ts`): registration is
`ctx.session.hook("compaction", cb)`, the callback is a single input with no output, and it can
mutate `input.messages` / `input.system` or set `input.result`. The port's adapter registers only
`prompt` (`plugin/src/v2/adapter.js:36`) and `context` (`:54`), and `plugin/src/v2/**` is frozen by
issue 20's carve. That is a numbered follow-up created by this wave — **issue 67**
(`.scratch/rust-port/issues/67-v2-compaction-hook.md`), derived as
`max(zero-stripped existing numbers) + 1` — not a non-existent API. Its first task is to confirm
host-side V2 dispatch: P18 confirms the *registrable surface* only, from published typings, and
the caller is unconfirmed. Under V2 the restore read still happens (the event is inherited), but
the message cannot be sent: `createLegacyClient` exposes neither `session.prompt` nor
`session.get`. The handler therefore proceeds on an unknown session so the read still reaches the
engine, and skips the send and the toast when the capability is absent — it must not throw.
V2's own cache invalidation (`plugin/src/v2/adapter.js:66-68`) delivers the memory context on the
next turn. The child gate is not enforced under V2 today; the follow-up owns both it and the send
path.

**DECISION 6 — restore formatting lives in Rust, not in the wrapper.** The wrapper has no store
and no config path; putting the renderer there would force a sixth wire method (engine is frozen)
or a JS config surface (forbidden by D-38/§9). The engine already owns the read, the scope and the
config; the wrapper owns only the host capabilities. The renderer exposes exactly one production
entry point, `render(mode, rows, policy)`; there is no `render_ingest`/`render_restore` twin.

**DECISION 7 — the compaction calls use a 2 s request timeout.** `sendRequest` defaults to 15 s
(`plugin/src/plugin.js:46` at `6f8801b`); a wedged engine would stall the host's compaction for 15 s.
`COMPACTION_REQUEST_TIMEOUT_MS = 2000` sits beside `REQUEST_TIMEOUT_MS` and both compaction calls
pass it explicitly. The driver guard is 3000 ms, not 2000: the harness registers its guard before
the hook call, so an equal 2000 ms guard wins the race on timer-registration order.

**DECISION 8 — the pre-compaction half is net-new; its no-double-count rule is a request-shape
rule.** Upstream declares `hooks: ["chat.message","chat.params","event"]`, has exactly one
`session.compacted` in `src/index.ts` (`:1301`) and `grep -rn compacting example/opencode-mem/src`
returns 0. So only the restore is a byte-pinned port; the ingest heading
(`## Session Memory Before Compaction`) is port-only. The hook cannot see the transcript
(`input` is `{sessionID}`, `output` is `{context, prompt}`), so the discrimination lives in
(a) the request shape the wrapper emits — `include.compaction === "ingest"`, the hook's own
`session_id`, and the same `project_id` the preceding `chat.message` inject carried — and (b) the
Rust renderer's output (`output.context` deep-equals one byte-exact string). The `output.prompt`
prohibition is pinned, not merely stated: the ingest test asserts both `output.prompt === undefined`
and `"prompt" in output === false` (the second catches an explicitly-set `prompt: undefined`). The
host composes `compacting.prompt ?? [buildPrompt(...), ...compacting.context].filter(Boolean).join("\n\n")`
(`packages/opencode/src/session/compaction.ts` @ `v1.19.4`), and `??` is nullish, so even `""`
replaces the prompt and drops the conversation.

**DECISION 9 — the restore toast reports the engine's `memory_count`, never a heading count.**
On a successful restore the wrapper shows exactly one toast
`{title: "Memory Restored", message: "<memory_count> memories injected after compaction", variant:
"success", duration: 3000}`, mirroring `example/opencode-mem/src/index.ts:1351-1362`. The count is
`result.memory_count` from the wire; counting `### Memory` headings is forbidden, because the two
agree only where the engine did not truncate or dedupe. The toast is fire-and-forget, un-awaited,
never throws, logs one `toast.failed` on failure, and is **not** routed through `notifyHost`'s
per-reason `NOTIFIED` dedupe: upstream toasts every restore, and a dedupe would silence all but the
first in a process. `test_restore_toast_is_not_deduped_per_process` drives two
`session.compacted` events and requires two toasts and two prompts.

**DECISION 10 — `experimental.compaction.autocontinue` is deliberately NOT used; this is a
recorded non-use, not an omission.** Three reasons:

1. It fires **after** compaction succeeds, so it cannot supply the pre-compaction identity AC 3
   needs. The typings are explicit — "Called after compaction succeeds and before a synthetic user
   auto-continue message is added" (`@opencode-ai/plugin@1.19.0`, `dist/index.d.ts:289-295`) — and
   say nothing about `input.agent` / `input.model` being the values that were running before the
   compaction. The host has already reset the session by then, so an identity read from this hook
   cannot be shown to be the pre-compaction one.
2. `output.enabled = false` is forbidden: it suppresses the host's synthetic continue turn for
   **every** session the hook is registered for, not only the sessions this lane restored — a
   global behaviour change smuggled in as a per-event flag, on a lane whose contract is fail-open.
3. It is already out of scope (`.scratch/rust-port/issues/33-compaction-hook.md:46`, `:100`), so
   this is a record, not a scope change: no code, no manifest entry, no test. `plugin/package.json`
   lists exactly one new hook (`experimental.session.compacting`), and this one must not appear.

## The renderer, byte-for-byte

Ported from `example/opencode-mem/src/index.ts:1392-1441`:

- `## Restored Session Memory\n\n` (restore) / `## Session Memory Before Compaction\n\n` (ingest);
- per row `### Memory N\n` + body + `\n\n`;
- `Tags: ` + tags joined `", "` + `\n\n` only when the row has tags;
- body = `stripMatchingEmbeddedTagsFooter(content, tags)`:
  `EMBEDDED_TAGS_FOOTER_RE = /\n*Tags: ([^\n]*)\s*$/` — the footer is stripped only when the tag
  **set** matches after trim/sort/join; the same content with a different set renders unmodified;
- sort by `created_at` **descending**, then truncate to `policy.memory_limit`;
- zero rows or a disabled policy → `None`, never a bare heading (upstream returns early at zero
  results, `example/opencode-mem/src/index.ts:1316`).

The Rust leg filters rows with `mem_store::vector::resolve_session_id` — "explicit non-empty wins,
else metadata" (`crates/mem-store/src/vector.rs:57-62`) — so a row whose own `session_id` belongs to
another session is dropped even when its metadata mentions this one. An `A or B` union would keep
it: a fail-open inside an authorization-shaped filter. The SQL's LIKE arm over-matches (the id is
interpolated raw into the pattern), which can only add rows; the exact Rust comparison contains it
and the SQL is pinned by `crates/mem-store/tests/golden_sql.rs:1496` — never changed here.

The leg's statement order is part of the contract: the `policy.enabled` check comes **before any
store access** (before `self.shards`, `self.store`, `self.profile_tier`, before
`scope_resolver.resolve` and before the session-id read), then the session id, then the read plus
the exact filter, then `render`, then the `COMPACTION_MEMORY_INJECTED` emission. The first
statement's position is the disabled-path oracle's whole contract: a leg that reads first and only
then renders `None` re-creates the shard file and turns a real difference into a false pass. Every
failure arm after it is the same `Ok(empty)` after one `COMPACTION_HANDLER_ERROR` log — never a
`-32602`, never a throw. `memory_count` is the **rendered** count
(`filtered.len().min(memory_limit)`), never the read count; `tokens` is `emitted_tokens(Some(&block))`
(the crate's one accounting rule, `inject.rs:468` at `6f8801b`), and `profile` is
`ProfileInjection::NoProjectScope` on every arm — deliberately, because it is the crate's only
"the tier never ran" value and `Default` would read `Disabled`. Its `A2:` prefix does not apply to a
compaction request; that is recorded here rather than papered over. `profile` never reaches the
wire.

## The envelope distinction

The `session.compacted` branch dispatches on the envelope's `event.type` — `EventSessionCompacted =
{type: "session.compacted"; properties: {sessionID: string}}` at the installed
`@opencode-ai/sdk/dist/gen/types.gen.d.ts:419-424`, reached through the hook's own `Event` input
(`@opencode-ai/plugin@1.19.0`, `dist/index.d.ts:175-178`). It never dispatches on
`properties.type`: that key is V2's `toLegacyEvent` merge
(`plugin/src/v2/legacy-client.js:142-156`), and the adapter reads the top-level `event.type` itself
(`plugin/src/v2/adapter.js:66`). The adjacent idle branch's `properties.type` read
(`plugin/src/plugin.js:752` at `6f8801b`) is the V2-merged shape and is left alone — changing it
would be a V2 regression, not this lane's. Two reads, two questions: `event.type` **dispatches**;
`event.properties.sessionID` **identifies**. The fixture keeps `type` on the envelope and never
moves it into `properties`.

## Idempotence

Two `session.compacted` events restore twice, exactly as upstream
(`example/opencode-mem/src/index.ts:1301-1362` has no dedupe). No cache is invented; the per-process
`NOTIFIED` set deliberately does not cover the restore toast.

## Known limitations

**Engine-version skew is ungated (known, unfixed).** ADR-0007 D-43 accepts an older same-major engine
(`engine.protocol >= wrapper.min_protocol`; both lines are `1.0.0`), and nothing in `hello` says
whether the engine's `inject` understands `include.compaction`. An engine built before this lane
ignores that key and answers the compaction request with the ordinary project chat block, which the
wrapper appends verbatim (`plugin/src/plugin.js:717-718` at `6f8801b`: the
`Array.isArray(output.context)` guard, then `output.context.push(result.block)`); the restore prompt
would carry the same block. That is exactly the double-count AC 1 forbids, and it is silent. It is
reachable through a stale gitignored `plugin/engine/<platform>-<arch>/` staging directory (a previous
build left beside the wrapper), and non-blocking only because the shipped wrapper and engine come from
one build. D-43's `hello.result.capabilities` is the additive home for the fix — advertise
`capabilities.compaction`, and make the wrapper fall back to a no-op when it is absent (or raise
`MIN_PROTOCOL`). Tracked as **issue 68**
(`.scratch/rust-port/issues/68-engine-skew-ungated.md`).

## The follow-up

**Issue 67 — V2 first-class compaction hook.** Wire `ctx.session.hook("compaction", cb)` in
`plugin/src/v2/` so V2 gets a pre-compaction ingest, with three preconditions: confirm host-side V2
dispatch (registrable surface only today), deliberately unfreeze `plugin/src/v2/**` (issue 20's
carve), and decide the V2 child gate and send path — both depend on capabilities
`createLegacyClient` deliberately omits today.

## Consequences

- `compaction.enabled` and `compaction.memory_limit` gain their first production readers, so issue
  63's census loses two rows and gains one declared reader site
  (`crates/mem-bridge/src/inject.rs` + `crates/mem-bridge/src/compaction.rs`). No third unread
  `compaction.*` key is added: the `Compaction` struct has exactly these two fields
  (`crates/mem-core/src/config/schema.rs:754-760`).
- The dispatch runs after `include_options`, which sits below `byte_budget` in `assemble`, so a
  request carrying both `include.compaction` and a malformed `token_budget` would fail before
  reaching the leg. No such request is reachable through this wrapper (`compactionParams` sets
  neither); the one-line move belongs to whoever first needs the compaction path to outrank a
  budget argument.
- The wrapper's `notifyHost` comment now names **two** producers (the inject failure alternate and
  the post-compaction restore). `notifyRestore` lives in `plugin/src/plugin.js` beside `notifyHost`,
  where the `NOTIFIED` set it deliberately does not use also lives; `plugin/src/compaction.js` holds
  the read side only, so a sibling import back would be a cycle.
