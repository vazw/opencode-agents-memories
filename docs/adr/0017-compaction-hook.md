# 0017 — The Compaction Hook (ingest before, restore after)

- **Status:** Proposed
- **Date:** 2026-10-27 — measurement bases: `fc41bbd` (the lane's branch point; issue 37 and issue 52
  landed) and `6f8801b` (the lane's commit). Every `file:line` pointer states its base; an unlabelled
  number is at `6f8801b`. Blueprint `20261027-0824-w5-5-compaction-hook-r17` (SYS-APPROVED
  2026-10-27T14:43Z).
- **Binding on:** `plugin/src/plugin.js`, `plugin/src/compaction.js`, `plugin/src/v2/adapter.js`,
  `plugin/package.json`, `crates/mem-bridge/src/{compaction,inject,composition}.rs`,
  `crates/mem-core/src/logging/signatures.rs`, and the three JS test files that pin this seam
  (`plugin/tests/compaction-hook.test.mjs`, `plugin/tests/compaction-capability.test.mjs`,
  `plugin/tests/v2-compaction-hook.test.mjs`).
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
(`docs/bridge-schema.md:89`) and `include` is already the additive options object
(`crates/mem-bridge/src/inject.rs:107-120`). The compaction request rides
`params.session_id` plus `include: {compaction: "ingest" | "restore"}`. No new wire method, no new
response key, no fixture adjustment, no edit to the frozen document. This decision governs the
`inject` request and response only; issue 68's `hello` capability is the additive change D-43
permits and moves `hello-response.json` with it.

**DECISION 5 — V2: the host's `compaction` hook is registered in the adapter, delegates to the V1
`experimental.session.compacting` closure and appends the render to `event.system`; the restore stays a
read, announced by a typed line (the restore half is SUPERSEDED by the 2026-11-02 amendment at the end of
this decision: it now sends).** Measured **READ** on the host at `8faeb81e92` (v2.2.0); the full chain,
the re-derivation greps and their output are in
`docs/evidence/issue-67-v2-compaction-dispatch/MEASUREMENT.md`:

1. `SessionHooks.compaction: SessionCompaction` (`packages/plugin/src/promise/session.ts:142`), registered
   through `ctx.session.hook(name, callback)` (`packages/plugin/src/promise/registration.ts:15-19`).
2. `summarize` calls `requests.compaction({...})` (`packages/core/src/session/compaction.ts:840`), and
   `SessionModelRequest.compaction` runs
   `prepare("compaction", input, agentHook("compaction", input.agent))`
   (`packages/core/src/session/model-request.ts:414`).
3. `agentHook` is `hooks.trigger("session", name, { ...draft, agent, tools })`
   (`packages/core/src/session/model-request.ts:408-410`), and `PluginHooks.trigger` iterates the
   registered callbacks and `yield*`s each one (`packages/core/src/plugin/hooks.ts:88-94`).
4. The promise bridge wraps each callback as `Effect.promise(() => Promise.resolve(callback(event)))`
   (`packages/plugin/src/promise/adapter.ts:599-603`), so the callback is **awaited** — a rejected
   promise is an Effect defect that aborts the host's compaction, not a no-op.

The callback takes a **single input and has no output pair** (`SessionContext extends SessionRequest`,
`system: Array<SystemPart>`, `packages/plugin/src/promise/session.ts:25-36`). The adapter therefore calls
the V1 closure with `{sessionID: event.sessionID}`, collects `output.context`, and pushes every non-empty
string onto `event.system` as `{type: "text", text}` — the array the request builder reads back. Never
`event.messages` (a render there becomes a user-role transcript message) and never `event.result` (setting
it skips the model call and replaces the host's summary). `plugin/src/v2/adapter.js` registers
`hook("compaction", ...)` beside `prompt` and `context`; the issue-68 capability gate and the 2 s request
timeout are **inherited by delegation**, not re-implemented — there is no engine handle in
`plugin/src/v2/**` to gate on a second time. ADR-0017 DECISION 2's child rule degrades to *proceed*
under V2, because the legacy client cannot read a session at all (below).

Two decisions are pinned by tests, not prose (`plugin/tests/v2-compaction-hook.test.mjs`):

- **The child gate is not applied under V2.** `readSession` returns null because `createLegacyClient`
  exposes no `session.get` (`plugin/src/v2/legacy-client.js:120-134`), and DECISION 2's
  positive-evidence rule says *proceed*. A child session may now ingest its **own** captured rows — a
  recorded divergence from V1's agent-only property, whose real fix is the capture gate (issue 63), not a
  second check here. `legacy-client.js` is untouched; adding `session.get` would buy no observable gain
  and would move the pinned `sessionGetCalls` oracle of `compaction-v2-inheritance.test.mjs`.
- **There is no send on V2; the restore half stays a read.** `createLegacyClient` exposes no
  `session.prompt`, and the host's `session.prompt` input cannot carry V1's
  `{parts, noReply, agent, model, variant}` body (`PromptInput = {text, files?, agents?, skills?}`,
  `packages/schema/src/prompt-input.ts:25-31`; the endpoint at
  `packages/client/src/effect/generated/client.ts:538-559`). `session.synthetic` is the closest analogue
  but wakes the runner unless `resume: false` (`packages/core/src/session/session.ts:323`), and it would
  need a V1→V2 translation of agent and model. The restore read still happens (the event is inherited)
  and `plugin/tests/compaction-v2-inheritance.test.mjs` pins it; the adapter announces the skipped
  restore with exactly one `compaction.restore_skipped` line per forwarded `session.compacted` event,
  carrying `{reason: "session.prompt unavailable"}` and nothing else — no session id, path, render or
  error text, written to stderr by `logWarn` (`plugin/src/plugin.js:198`). The line is unconditional for
  this bridge: `typeof ctx.session.prompt` is a function on the real host (`SessionDomain` includes
  `prompt` and `synthetic`, `packages/plugin/src/promise/session.ts:154-174`), and the absence is a
  property of `createLegacyClient`, which the adapter cannot see.

**Amendment 2026-11-02 (issue 112) — the V2 restore SENDS: a visible synthetic, then the toast.** The
"no send / restore stays a read" half of this decision is superseded (the earlier text is kept as the
record of what was believed). `createLegacyClient` now exposes `session.prompt`
(`plugin/src/v2/legacy-client.js`), which maps the V1 `{parts, noReply}` body onto
`ctx.session.synthetic({sessionID, text, description: "memory context", resume: false})` and every other
body onto `ctx.session.prompt({sessionID, text, delivery: "queue"})`. `plugin/src/v2/adapter.js` no longer
emits the unconditional `compaction.restore_skipped` line; the one surviving skip is the agent gate in
`plugin/src/plugin.js`, announced with `{reason: "agent unresolved"}` and nothing else. `session.get` is
still deliberately absent (D-112-4 in `.scratch/rust-port/issues/112-v2-compaction-restore-send.md`), so
the child gate still degrades to *proceed* and `resolveSessionModel` still returns `undefined` on V2 —
nothing legitimate is lost, because the synthetic route takes no agent/model/variant. `description:
"memory context"` is load-bearing in both renderers
(`packages/session-ui/src/timeline/projection.ts:1179-1185`,
`packages/tui/src/routes/session/rows.ts:392`); `resume: false` keeps the row visible at admission while
deferring its entry into the model request to the next promotion
(`packages/core/src/session/session.ts:323`). The test that pinned the old behaviour was replaced, not
silently dropped: `test_v2_compaction_seam_ingests_then_reads_after_without_a_send` →
`test_v2_compaction_seam_ingests_then_sends_the_restore_and_toasts`. Re-derivation, and the live
residual measurement, are in `docs/evidence/issue-112-v2-restore-send/MEASUREMENT.md`.

**The hook covers the summarisation path only.** `compaction.ts:303` chooses
`native ? compactNatively(...) : summarize(...)` (`:317-321`): a native model never reaches `prepare`, so
it gets no ingest. A background `precompute` **does** run `summarize` earlier (`compaction.ts:337-389`,
its own `:840` request with a message prefix), so the hook runs there and the held summary carries the
ingest; swapping that summary in (`usePrecomputed`, `compaction.ts:395-419`, chosen at `:317-318`) runs
no new hook — the held summary already carries the ingest. There is no second hook and none is added.

**Carve.** Issue 20 is landed, and its tracker records no standing freeze of `plugin/src/v2/**`; that carve
belonged to that wave. This lane owns `plugin/src/v2/plugin.js` and `adapter.js` for the compaction
registration only, and `legacy-client.js` only if the child gate needed a capability it does not expose —
it does not, so `legacy-client.js` is byte-unchanged. The new registration adds nothing to the V2 entry's
cleanup order (`skills`, then the adapter): the host owns the registration lifecycle.

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

**DECISION 11 — the restore's notification is the existing toast RPC; `ctx.attention.notify` is a
recorded non-use, not an omission (issue 112).** A successful restore shows exactly one toast through
`plugin/src/v2/toast-rpc.js` → `plugin/src/tui.js` → `ctx.ui.toast.show` — already wired, tested
(`tui-companion.test.mjs`, `v2-toast-rpc.test.mjs`) and auto-loaded by the TUI process for a package
whose `exports["./tui"]` resolves (`packages/core/src/plugin/module.ts:147-152`,
`packages/tui/src/plugin/context.tsx:95-104`, `:670`). `ctx.attention.notify({title, message,
notification: {when: "blurred"}})` exists (`packages/tui/src/plugin/api.tsx:80`) and is deliberately
**not** used this wave: discriminating a restore from the injection-failure warning would need either
`title` string-matching or widening the upstream-verbatim `MemToast` schema — a second unproven channel
on a path that had never delivered a single toast. This is DECISION 10's recorded-non-use pattern. To
un-defer: add the call beside `notifyRestore` in `plugin/src/plugin.js` with a distinct `title` and a
`notification: {when: "blurred"}` gate, then widen `MemToast` (or match the title) so the two intents
stay separable; neither change is in this wave.

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

**RESOLVED (issue 68, 2026-10-28) — engine-version skew is capability-gated.**

ADR-0007 D-43 accepts an older same-major engine (`engine.protocol >= wrapper.min_protocol`; both
lines are `1.0.0`), and before issue 68 nothing in `hello` said whether the engine's `inject`
understood `include.compaction`. An engine built before this lane ignored that key and answered the
compaction request with the ordinary project chat block, which the wrapper appended verbatim
(`plugin/src/plugin.js:717-718` at `6f8801b`: the `Array.isArray(output.context)` guard, then
`output.context.push(result.block)`); the restore prompt would have carried the same block. That was
exactly the double-count AC 1 forbids, and it was silent. It was reachable through a stale gitignored
`plugin/engine/<platform>-<arch>/` staging directory (a previous build left beside the wrapper), and
non-blocking only because the shipped wrapper and engine come from one build. Tracked as **issue 68**
(`.scratch/rust-port/issues/68-engine-skew-ungated.md`).

The mechanism that closes it: the engine's `hello` advertises `capabilities.compaction: true` exactly
when its `inject` honours `include.compaction` (`crates/mem-bridge/src/engine.rs::handshake_result`),
the wrapper records that additive object at the handshake, and both compaction hooks read it with
`=== true` before any host read or wire write. An absent key, a non-object or a non-boolean-`true`
value means both hooks no-op and one `engine.capability_missing` line names the gap — the capability
name and two protocol lines only, never a session id, transcript, path or error text. Raising
`MIN_PROTOCOL` was rejected: it refuses every pre-33 engine wholesale instead of degrading one
capability.

**R1 — the flag/behaviour coupling is a contract, not a runtime check.** A future engine that stops
honouring `include.compaction` must drop the flag in the same change; nothing enforces the coupling at
runtime. The inverted world is `test_include_compaction_restore_never_returns_the_chat_block` going
red while `test_hello_response_schema_equal_after_mask` stays green.

## The follow-up

**Issue 67 — V2 first-class compaction hook — RESOLVED (2026-10-28).** The wiring and both decisions are
DECISION 5 above; the host dispatch is recorded, with its re-derivation greps, its three-way compaction
shape and its honest limit, in `docs/evidence/issue-67-v2-compaction-dispatch/MEASUREMENT.md`. The
residual measurement is a live host run: the source read is the only dispatch evidence in the wave that
closed it, and the driver cases call the registered callback themselves.

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
