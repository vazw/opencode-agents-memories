# 49 — Config · The inert half of the config surface: at most 35 dead keys, one of them a kill switch

Status: ready-for-agent — 2026-10-27 (re-verified at `8023dd8`: the kill-switch defect is real and
unchanged; 10 of the audit's 45 dead fields now have readers, so dead is at most 35. Brief under
`## Agent Brief`.)

## What to build

Make the config surface honest. Three deliverables, in this order:

1. **`chat_message.enabled` becomes a real gate** (it is the only dead key whose deadness is a *behaviour
   lie*, not a documentation lie — see below).
2. **A tracked census** of all 98 resolved fields with a disposition per dead key, in `docs/` rather than
   a scratch draft, so a user's config file is evidence of something.
3. **A startup diagnostic** naming the inert keys the user actually set, so the silence the audit named as
   the real defect cannot survive this issue.

### The corrected numbers — do not reuse the brief's

| | brief claimed | audit (2026-10-25) | at `8023dd8` |
|---|---|---|---|
| resolved fields | 112 | **98** (82 `Config` + 16 nested) | `Config` alone is **83** (the `+1` is `allow_unguarded_open`, live); my extraction of the nested structs sums to 15, not 16, so the total is **unreconciled** until the census wave's type-aware pass |
| no production reader | 53 | **45** (45.9%) | **at most 35** — ten of those 45 now have readers (table below) |
| live | 59 | **53** | **at least 63** by the same subtraction; the census must pin it |

The ten that moved to live, with the reader that moved them — all `READ` at `8023dd8`, none inherited:

| field | reader | landed |
|---|---|---|
| `compaction.enabled` | `inject.rs:507` | 33 |
| `compaction.memory_limit` | `inject.rs:550` | 33 |
| `inject_profile` | `profile_context.rs:91` | 44 |
| `user_profile_inject_preferences` | `profile_context.rs:93` | 44 |
| `user_profile_inject_patterns` | `profile_context.rs:94` | 44 |
| `user_profile_inject_workflows` | `profile_context.rs:95` | 44 |
| `deduplication_enabled` | `src/main.rs:180` | 54 |
| `deduplication_similarity_threshold` | `src/main.rs:181` | 54 |
| `chat_message.filter_injected_prompts` | `layer.rs:74` | 52 |
| `chat_message.injection_markers` | `layer.rs:77` | 52 |

Source: `.tmp/scratch/opsec-design-concerns-run-1/REPORT.md` §3.3 — verdict `WRONG (counts)`, record
`mem-core/config/inert-field-count`. The brief's 53 was the *live* count, i.e. an inversion. **"Lower
bound" is also wrong**: the method biases both ways, and for this field set the destructuring blind spot
does not fire (`Config` derives only `Debug, Clone, PartialEq` — `schema.rs:629` — there is no
`Config { .. }` destructuring and no whole-struct serialization).

Buckets for the 45 dead: **32** genuinely inert with no landing issue · **13** deliberate forward
declarations with a landing issue. The audit's third bucket is **not part of the 45**:
`Episodic.consolidation_age_days` is a **live** field the original method missed and sits outside the
partition, so **32 + 13 = 45** and the "1" must never be added to it
(`REPORT.md:207-209` — the bucket table).

### `chat_message.enabled` — the behaviour lie, and why it is first

`chat_message_enabled` is parsed (`schema.rs:540`), defaulted (`:608`), resolved
(`crates/mem-core/src/config/mod.rs:360`) and **read by nothing else**. Meanwhile the `chat.message` hook
exists and **injects unconditionally**: the adapter asks the engine for a block and appends it to the
message parts with no config check at all (`plugin/src/plugin.js:571-597`). The adapter holds no config —
its only environment read is `OPENCODE_MEM_STORAGE_ROOT` (`:111`) — so the whole `ChatMessage` struct
(`schema.rs:741-750`) is parsed-and-ignored, and `enabled: false` still injects.

**Ruling: the gate belongs in Rust, not in the adapter.** Giving the JS plugin a config surface to honour
one knob would create a second source of truth for configuration; the deep-module answer is that the
engine owns config and the adapter stays a thin transport. The gate sits at the *chat.message injection*
seam — the `inject` request handler, `crates/mem-bridge/src/engine.rs:351` (`call_inject`; dispatched
`:216`), not the method-resolution guard at `:199-213` — and if `inject` later grows callers other than
`chat.message`, the gate moves to that request variant, never into the renderer.

Upstream, for the record, gates the entire hook on this key (`example/opencode-mem/src/index.ts:646`), so
this is a parity restoration, not a new behaviour.

### Why the other 8 `ChatMessage` keys are not "just wire them"

They are the settings of a *chat-message injection mode* (cap, inject-on policy, current-session
exclusion, injected-prompt filtering and its markers, child-session capture). Wiring them means choosing
that mode's rules, and issue **50** is currently deciding the surrounding budget and ordering rules — so
wiring `max_memories` here would pre-empt 50 and re-open it later. Disposition per key, in the census,
with `max_memories` marked "wire after 50" rather than wired now.

**Two of the eight have a landing issue as of 2026-10-25, and this paragraph must not swallow them:**
`chat_message_filter_injected_prompts` (`schema.rs:544`) and `injection_markers` (`schema.rs:748`) are
owned by **issue 52** — the filter they configure exists (`crates/mem-ai/src/privacy/injection.rs`, 204
lines, tested) and is unreachable; the defect there is behavioural, not configurational. Their disposition
is "wire, issue 52", and this issue gives them no reader of its own.

## Acceptance criteria

- [ ] **49.1** *(land first)* With `chat_message.enabled = false`, no block reaches the `chat.message`
      parts; with `true` and eligible rows, a block does. The gate is Rust-side; `plugin/src/plugin.js`
      gains no config read. Tests assert both directions through the engine request, and the `false` arm
      is a *refusal to build the block*, not an empty string that still arrives.
- [ ] **49.2** `docs/config-surface.md` (or an equivalent tracked path — **not** `.tmp/`) lists all **98**
      resolved fields with: live/dead, the reader's file:line for live ones, and a disposition for dead
      ones. The census states the method and its biases; a reader must be able to reproduce 98/45/53 from
      it alone.
- [ ] **49.3** Every dead key has exactly one disposition, with no fourth option: **(i) wire** (only where
      an upstream reader exists *and* the behaviour exists here *and* a test can pin the difference),
      **(ii) delete** (the field and its resolver line), or **(iii) mark inert** — a doc comment that says
      "parsed; no reader", names the reason, and names the issue that would honour it.
- [ ] **49.4** The **13** forward declarations cite a landing issue, and the census distinguishes "issue
      open" from "issue done but the reader is still absent" — the exact defect the audit found in the
      existing deferral chain (`33-compaction-hook.md:41` → a lane that never existed). A citation to a
      closed issue with no reader fails review.
- [ ] **49.5** The census cannot rot: a test fails when a `pub` field in `crates/mem-core/src/config/schema.rs`
      is absent from the census (scraping precedent: `mem_core::logging::signatures`), **or** the issue
      records why such a test is infeasible and what replaces it. A census that silently misses a new key
      is the same defect one level up.
      **Method constraint (2026-10-27):** a grep census is not an oracle. Re-deriving this one produced a
      false positive (`.auto_cleanup_enabled` matched `mem-profile`'s own `ProfileConfig`,
      `profile_store.rs:66`) and a false negative (`episodic.consolidation_age_days`, whose reader binds a
      local named `episodic`, `mem-episodic/src/consolidation.rs:84`). The test must resolve types, or the
      census must publish its measured error rate on a hand-checked sample.
- [ ] **49.6** At startup, the engine logs **once** the inert keys the user explicitly set in a jsonc file
      (`crates/mem-core/src/config/mod.rs:80-101` are the candidates), naming whether each has a landing
      issue, capped at ten with a `+N more` tail, and pointing at the census. Test: a fixture setting three
      inert keys and one live key warns about exactly three.
- [ ] **49.7** The census distinguishes **dead upstream too** from **port regression**, because only the
      second is a parity defect: `max_profile_items` and the three `user_profile_display_*` keys are
      parsed-and-never-read upstream as well. `max_profile_items` stays unwired — issue **44.9** rules
      that explicitly; do not "complete" it.
- [ ] **49.8** `Episodic.consolidation_age_days` is recorded **live**, with its reader
      (`crates/mem-episodic/src/consolidation.rs:84`), because it is the one field the original census
      misclassified and the count's credibility rests on it.
- [ ] **49.9** The `deduplication_*` attribution is corrected in the census: they gate the **manual**
      endpoint (upstream `example/opencode-mem/src/services/deduplication-service.ts:37`), not
      capture-time dedup. The route is real (`crates/mem-web/src/api/maintenance.rs:245`, registered
      `crates/mem-web/src/api/mod.rs:180-186`, commit `6bf7b28`); and **the factory is now called in
      production** — `src/main.rs:178` calls `production_deduplication_port_with_cancel` with
      `core_config.deduplication_enabled` and `core_config.deduplication_similarity_threshold`
      (`src/main.rs:180`–`:181`). So both keys are live, the attribution to the *manual* endpoint stands,
      and the census records them live rather than "factory uncalled".
- [ ] **49.10** No behaviour change to any key outside 49.1 and 49.6; no manifest change; `schema.rs`'s
      resolution behaviour and every default are untouched.
- [ ] **49.11** **The bucket arithmetic is restated once, visibly.** Issue **52 landed**, and its two
      keys now have production readers (`layer.rs:74`, `:77`), so they leave the dead set entirely
      rather than moving from bucket (a) to bucket (b); the audit's **32 / 13** partition is therefore
      superseded by the HEAD count above, not shifted by two. The live-field correction
      (`episodic.consolidation_age_days`) stays outside the partition and is not counted in either shift.
      This is *my* re-bucketing of the audit's numbers, not a re-derivation; a reader must be able to see
      which two rows moved and why. **Also reconcile bucket (a)'s itemised list with its stated 32** — the
      list at `REPORT.md:207` enumerates 31 (23 named + 8 `chat_message.*`); either a row is missing from
      the list or the count is 31, and 49.2's census cannot ship until the two agree.

## Out of scope

- Wiring `inject_profile` and the three `user_profile_inject_*` — **issue 44** owns them.
- `compaction.enabled` / `compaction.memory_limit` (33); `ai_session_retention_days` (39);
  `auto_cleanup_retention_days` (41); the three toast keys (20); `chat_message_filter_injected_prompts` /
  `injection_markers` (**52** — the filter exists and is unreachable; the fix is behavioural). The census
  lists them all; the issues own them.
- The budget/ordering rules that `chat_message.max_memories` would interact with — issue **50**.
- A user-facing config reference for the *live* keys. This issue makes the dead ones honest; documenting
  the live ones is a separate, additive task.

## Blocked by / Blocks

- **Blocked by:** nothing. 49.1 is independent and cheap; 49.9 depends on nothing.
- **Blocks:** nothing formally. Informally it blocks any future claim that a config file describes this
  tool's behaviour — which is the point.

## Comments

- Filed 2026-10-25 from the opsec design-concerns audit (concern C4, verdict `WRONG (counts)` — the
  concern survives, the evidence did not), per the audit's §6 recommendation 4.
- **Corrections carried in, per the brief's AUDIT RESULT block**
  (`20261025-design-concerns-audit-brief.md:37`): 98/45/53, not 112/53; drop "lower bound"; drop
  "capture-time dedup"; do not call issue 40 delivered. The **32 / 13** bucket split is the audit's and is
  not re-derived here; its third row (`episodic.consolidation_age_days`, one **live** field) sits outside
  the 45 and is never added to them.
- **Deviation from the audit's recommendation, stated.** The audit says "decide per key: wire (upstream
  reads it), mark as inert in docs, or delete". Taken literally, "upstream reads it" would wire eight
  `ChatMessage` keys into a feature mode nobody has designed, and they would be re-decided by issue 50
  within the month. This issue wires exactly one key, because exactly one is a behaviour lie; the other
  44 get dispositions that do not require guessing a design.
- **The audit's "start with `chatMessage.enabled`" is endorsed and strengthened**: the hook half is
  already live and ungated in our own tree (`plugin/src/plugin.js:571-597`), so this is not an
  armchair claim about a feature we lack. That is also why it is `AC 49.1` rather than a closing note.
- The startup diagnostic (49.6) is an addition to the audit's scope: the audit named the defect as "the
  silence, not the count", and a census alone does not fix silence — it moves it from `schema.rs` into a
  doc nobody reads.

- **2026-10-27 — triaged: the defect is real, the arithmetic is not.** The kill switch still kills nothing:
  `chat_message.enabled` is parsed and resolved and read by no production path, and `call_inject` never checks a
  config gate. But the census has drifted — `Config` now resolves ~99 fields (issue 45 added
  `allow_unguarded_open`), and issue 54 wired `production_deduplication_port`, making two more keys live, so the
  dead count is at most 43, not 45. A census wave must re-derive at HEAD rather than transcribe this issue.

- **2026-10-27 — re-verified at `8023dd8` (defect real; arithmetic now wrong by ten).**
  `chat_message.enabled` still has zero readers outside config resolution, `call_inject` (`engine.rs:358`)
  consults no config, and the adapter hook (`plugin.js:670`–`:697`) injects unconditionally — `enabled:
  false` still injects. But ten of the audit's 45 dead fields now have production readers:
  `compaction.enabled` (`inject.rs:507`) and `compaction.memory_limit` (`inject.rs:550`) — 33;
  `inject_profile` and the three `user_profile_inject_*` (`profile_context.rs:91,93,94,95`) — 44;
  `deduplication_enabled` / `deduplication_similarity_threshold` (`src/main.rs:180,181`) — 54;
  `chat_message.filter_injected_prompts` / `chat_message.injection_markers` (`layer.rs:74,77`) — 52.
  So dead is **at most 35**, not 45, before bucket (a)'s own 31-vs-32 list discrepancy. `Config` now
  defines **83** fields, and the `+1` (`allow_unguarded_open`) is live (`mem-store/src/db.rs:286`).
  Two ACs are falsified: **49.9** — the dedup factory has a production caller at `src/main.rs:178`, so
  the keys are not "factory uncalled"; and **49.11** — the two 52 keys are wired on the capture path, not
  merely re-homed between buckets.
  **Method warning for 49.5:** a grep census is not rot-proof. Re-deriving it produced a false positive
  (`.auto_cleanup_enabled` matching `mem-profile`'s different `ProfileConfig`) and a false negative
  (`episodic.consolidation_age_days`, whose reader binds a local named `episodic`, not `config.episodic`).
  The census oracle needs type-aware resolution, or the census must publish its residual error rate.
  **State not changed at triage time.**

## Agent Brief

> *This was generated by AI during triage.*

**Category:** bug
**Summary:** make `chat_message.enabled` a real gate, then produce a census that cannot lie.

**Current behaviour (verified at `8023dd8`):** `chat_message_enabled` is parsed, defaulted and resolved and
read by nothing; `call_inject` (`crates/mem-bridge/src/engine.rs:358`) consults no config; the adapter's
`chat.message` hook (`plugin/src/plugin.js:670`–`:697`) pushes whatever block the engine returns, with no
config read. `enabled: false` still injects. Ten of the audit's 45 dead fields have since gained readers
(table above), so the dead count is at most 35.

**Desired behaviour:** gate the block at the engine's chat.message injection seam (49.1), then produce the
census (49.2) from a type-aware pass — a grep pass is demonstrated to have both false positives and false
negatives. Every dead key gets exactly one disposition. The startup diagnostic names the inert keys the user
actually set.

**Key interfaces:** `Config.chat_message`, `Engine::call_inject`, the census document, the startup logger.
The injection-marker list belongs to issue 50, not to this lane.

**Acceptance criteria:** 49.1–49.11, with 49.5's method constraint, 49.9's corrected dedup attribution
(`src/main.rs:178`–`:181`) and 49.11's corrected bucket arithmetic.

**Out of scope:** wiring the other seven `ChatMessage` keys; issue 50's budget/order rules; the frozen DTOs.
