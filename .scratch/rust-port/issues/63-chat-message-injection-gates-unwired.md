# 63 — six chat-message config keys resolve and nothing reads them; the injection path has no gates

Status: ready-for-agent — 2026-10-27 (premise measured at HEAD `43cb442`; filed from the issue-20 Plan Gate,
which refused to let a blueprint's "N-A" retire behaviour that is unimplemented rather than inapplicable.
Brief at the end of this file.)

## Parent

Sequencing parent: issue 94. This issue owns the switch; 85 owns the session-id plumbing it depends on.

## PREMISE — measured, not inherited

`crates/mem-core` resolves the chat-message configuration and hands it to the engine. **Six resolved keys have
no `inject`-side reader under `crates/mem-bridge/`** — measured with `grep -rnw '<key>' --include='*.rs'
crates/mem-bridge`. One of them, `chat_message_enabled`, gained a **capture**-side reader in issue 98 (last
row of the table); the injection side still gates on none of them.

| key | resolved at | upstream gate | mem-bridge readers |
|---|---|---|---|
| `chat_message_enabled` | `crates/mem-core/src/config/mod.rs:360` | `example/opencode-mem/src/index.ts:670` — `if (!isConfigured() \|\| !CONFIG.chatMessage.enabled) return false`; the same gate also stands at `:735`, `:748`, `:767` | one **capture**-side reader (issue 98): `crates/mem-bridge/src/capture.rs` `chat_capture_enabled` + `engine.rs::call_capture`; the six `inject`-side keys stay at 0 |
| `exclude_current_session` | `config/mod.rs:366-370` | `services/memory-recall.ts:159-160` — `excludeSessionId` set at `:159` and applied at `:179-218` (`c81a04d` moved it out of `index.ts:691-693`) | **0** |
| `max_age_days` | `config/mod.rs:371-375` | `services/memory-recall.ts:159-160,179-218` — `createdAfter = Date.now() - maxAgeDays * 86400000` at `:160`, filtered at `:179-218` | **0** |
| `inject_on` | `config/mod.rs:375-379` | `index.ts:721` — `refreshOnPrompt: () => injectOn === "always"` | **0** |
| `max_memories` | `config/mod.rs` (resolved; 14 references outside mem-bridge) | `services/memory-recall.ts:179-218` — `opts.maxMemories` at `:179`, `:190`, `:201`, `:204`, `:216`, `:218` | **0** |
| `capture_child_sessions` | `config/mod.rs` (resolved; 7 references outside mem-bridge) | `index.ts:698` — `if (!CONFIG.chatMessage.captureChildSessions && (await isChildSession(...)))`; the same gate also stands at `:754` | **0** |

`InjectService::assemble` (`crates/mem-bridge/src/inject.rs:167`) opens with the A2 `project_id` guard and then
proceeds. It gates on **nothing else**: no enabled check, no age filter, no session exclusion, no count limit.

Two further keys are unwired but belong to **issue 52**, not here: `filter_injected_prompts` and
`injection_markers` (`index.ts:657-658`). Cross-reference, do not reimplement.

**Two of the six may have deliberate port-side replacements, and neither is recorded.** That distinction is
the whole reason this is an issue rather than a checklist:

- `max_memories` — the port limits injection by **token budget** (`greedy` over the byte budget), not by a
  row count. If that is the intended replacement, it is a legitimate divergence that needs a sentence and a
  test; if not, the upstream limit is simply missing.
- `capture_child_sessions` — issue 33's brief records that child sessions restore nothing as an *emergent
  property* of the capture gate (no tier-1 rows carry a child `session_id`). If that is the replacement, say
  so and pin it; an emergent property is not a gate.

## WHY IT MATTERS

Upstream's `v2-memory-context` behaviour is built on these gates. Because the port carries the keys and reads
none of them, a blueprint can state — in good faith — that the upstream cases "do not apply here". They do
apply; they are unimplemented. That is the same defect class as issue 62 (a provider gate that exists and is
never called), found the same week by the same gate, and retiring it with prose is exactly what this issue
exists to prevent.

## WHAT A WAVE SHOULD PRODUCE

1. **Derive each semantic from upstream before deciding anything** — `example/opencode-mem/src/index.ts:644-723`
   is the whole gate block; cite it rather than recalling it.
2. **Decide per key: implement, or record a divergence** with a reason and a test pinning current behaviour.
   "The port never wired it" is not a reason, and neither is "the behaviour is emergent" without the test.
3. **The gate belongs at the one seam.** `assemble` is the single entry point for the chat path and the recall
   path; a per-caller check is the defect this issue exists to avoid. If `chat_message_enabled` and `inject_on`
   gate *different* paths, name both call sites.
4. **`max_age_days` should be pushed into the query if the store supports it.** A post-filter reads rows it
   then discards, which corrupts the byte-budget accounting. State which the store API allows and prove it.
5. **`exclude_current_session` is an identity question.** The current session id is not a config value: name
   where it enters `InjectParams` and what `None` means. Absence must not silently mean "exclude nothing" if
   upstream treats it otherwise.
6. **`inject_on` has more than two values.** It is an enum whose `"always"` arm drives `refreshOnPrompt`; a
   boolean reading of it would be wrong.

## ACCEPTANCE — what would close it

- [ ] Each of the six keys carries: gated path, default, upstream citation, and either an on/off observation
      or a recorded divergence with a pinning test.
- [ ] The gate is at the single `assemble` seam; no per-caller duplication.
- [ ] `max_age_days` is enforced where the store allows it, with the choice justified in one line.
- [ ] A test shows `chat_message_enabled = false` producing no block while the memory tiers stay intact, and
      a second shows the profile tier unaffected if the keys gate different paths.
- [ ] `max_memories` and `capture_child_sessions` are each resolved one way or the other — no row left saying
      "possibly deliberate".
- [ ] Issue 20's tracker entry can cite this issue for the four upstream cases it currently cannot claim.
- [ ] No schema change, no new dependency, and the frozen fixtures under `tests/fixtures/bridge/` stay
      byte-identical unless a deliberate wire decision is recorded.

## Out of scope

- `filter_injected_prompts` and `injection_markers` — issue 52.
- The profile tier's own knobs — issue 44.
- The learner — issue 47.1 (tier read-only this release).
- Any change to `example/` (read-only reference).

## Agent Brief

**Category:** enhancement
**Summary:** six chat-message injection keys are parsed, resolved and never read; `assemble` gates on nothing.

**Current behavior (verified at HEAD `43cb442`):**
`chat_message_enabled`, `exclude_current_session`, `max_age_days`, `inject_on`, `max_memories` and
`capture_child_sessions` are resolved in `crates/mem-core` and have **zero** readers under
`crates/mem-bridge/`. `InjectService::assemble` (`crates/mem-bridge/src/inject.rs:167`) gates only on the A2
`project_id` guard, so injection happens regardless of any of them. Upstream reads all six in one block at
`example/opencode-mem/src/index.ts:644-723`.

**Desired behavior:**
Each key either gates the path it names or carries a recorded divergence with a reason and a pinning test. The
gate lives at the single `assemble` seam. `max_age_days` is enforced at the query where the store supports it.
`max_memories` and `capture_child_sessions` are each explicitly resolved — the token budget may legitimately
replace the first, and the capture gate may legitimately replace the second, but neither may remain an
unstated assumption.

**Key interfaces:**
- `InjectService::assemble` — the single seam; extend the A2 guard's pattern, do not duplicate it.
- The resolved `Config` fields and defaults (`crates/mem-core/src/config/mod.rs:360-379`, `schema.rs:438-440`).
- `InjectParams` — where a current-session id must enter for `exclude_current_session` to mean anything.
- The history query in `mem-store` — whether an age bound can be pushed down.
- Upstream `example/opencode-mem/src/index.ts:644-723` as the behavioural reference (read-only).

**Acceptance criteria:**
- [ ] Six keys, six documented dispositions, each with an upstream citation.
- [ ] One seam; a per-caller check fails review.
- [ ] On/off observation for every implemented key.
- [ ] `max_memories` and `capture_child_sessions` resolved explicitly, not left as "possibly deliberate".
- [ ] Issue 20 can cite this issue for the four upstream cases it currently cannot claim.
