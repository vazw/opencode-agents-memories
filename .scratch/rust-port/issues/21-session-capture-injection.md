# 21 — W5.3 · Session capture & context injection end-to-end (US 1.1, 1.2, 1.3)

Status: done — 2026-10-21 (reviewer APPROVED iteration 2: all 8 rework defects closed, 164/164 workspace suites green, 29/29 plugin, fmt+clippy clean, security gate 10/10 PASS; `cargo audit` rc=1 is pre-existing dep-tree, out of scope)

## What to build

The Epic 1 headline capability: capture structured session summaries at turn/session end (via the host events from W5.1) and inject `<project_agent_session_history>` context at session start, within token budgets, blocker-first ordering. Child-session/agent attribution per W4.1 scoping. Wave detail: `.scratch/rust-port/roadmap.md` §W5.3.

## Acceptance criteria

- [x] Host-driven end-to-end test: capture → persist → inject on next session start.
- [x] Behaviors ported: `auto-capture`, `auto-capture-concurrency`, `memory-context` (token-budget trimming order matches upstream).
- [x] Injection respects the token budget with blocker-first priority; memory-count accounting verified.
- [x] Child-session/agent attribution flows through capture (integrates W4.1 + W4.2).

## Blocked by

- 13 · W3.2
- 15 · W4.1
- 16 · W4.2
- 19 · W5.1

## Comments

### 2026-10-21 — W5.3 implementation closed (blueprint `20261020-2157-w53-session-capture-injection-r5`, sha256 `be8c928f5bbd`)

**Outcome.** Capture → persist → inject is wired end to end. The wrapper captures on the host
`session.idle` event (gathering the eligible prompt, assistant responses and tool calls through
`input.client.session.messages`), the engine durably enqueues the capture before answering
`{"accepted":true}`, a serialized drain writes the episodic staging row and one session-linked tier-1
`auto-capture` summary, and the next eligible `chat.message` appends ONE
`<project_agent_session_history>` block as a synthetic part. **Injection happens through the
`chat.message` hook, never at session start** — no session-start hook exists or is claimed.

**Divergences (recorded in `docs/adr/0008-w53-capture-injection.md`).** D-W5.3-1 scope-triple recapture
with a capture-id ladder; D-W5.3-2 one port-owned history block; D-W5.3-3 checked token→byte budget
(byte-based `tokens` estimate); D-W5.3-4 deterministic `auto-capture-<64 hex>` row ids; D-W5.3-5 two
local commits with the defined partial-failure contract (tier-1 failure after the episodic commit keeps
the inbox entry retryable; five CONSUMED attempts dead-letter to an episodic-only row; a lock deferral
consumes NO attempt); D-W5.3-6 capture-summary retention stays with the existing tier-1 policy;
D-W5.3-7 the NAMED sequenced W5.1 follow-up **issue 34** carries the wrapper's
`project_id = input.directory` (frozen once from host input, never from transcript/event payload);
D-W5.3-8 the two distinct consolidation bounds (`CONSOLIDATION_MAX_PROJECTS = 64` per tick vs. the tree's
`DEFAULT_BATCH_LIMIT = 200` rows per batch), plus the actual capture-time trim port
(`crates/mem-bridge/src/capture_context.rs`, six upstream oracle cases, upstream marker supplied
explicitly).

**Residuals.** (1) Non-promotable milestone content is not injected. (2) Semantic retrieval can observe
stale vectors. No cross-database atomicity is claimed: the two tier writes are two local transactions and
the partial-commit state above is the honest boundary.

**Test failures.** None. `cargo test --workspace` reported 0 failed; the plugin suite
(`node --test` over `dist-sync`, `loader-contract`, `root-entrypoint`, `bundle-boundary`,
`wrapper-glue`, `host-config`, `shutdown`, `fixtures`) reported 29/29 pass; `cargo clippy --workspace
--all-targets -- -D warnings` and `cargo fmt --all --check` are clean; the W5.1 stub
(`capture.stub_not_durable`) is gone (`grep -c` == 0).

**Rework r1 (2026-10-21).** The first close-out claimed `cargo fmt --all --check` was clean while
`crates/mem-bridge/src/capture.rs` still had two unformatted `commit_episodic(...).is_err()` sites; the gate is
now genuinely clean (`cargo fmt -p mem-bridge`; `cargo fmt --all --check` = 0 diffs). Eight review defects are
closed: **(D1)** four AC6 oracles in `tests/inject_budget.rs` that asserted nothing or asserted a tautology now
prove the budget contract (byte-exact emitted block, UTF-8 cap with no replacement char, the read
container-tag filter against a same-shard foreign-tag row, and the measured 25% tier-1 subsection capped at
`budget/4`), each with a recorded mutation that turns it red; **(D2)** the fmt gate above; **(D3)** the
production composition root now derives `StoreConfig` with `mem_store::config::from_core`, so configured
encryption at rest is no longer silently disabled (oracle: a keyless open of `episodic.db` fails); **(D4)** the
tier-1 metadata document is serialized from a typed struct, so quotes in `session_id`/`promptId` can no longer
forge duplicate keys or re-classify a capture row as a promoted fact; **(D5)** the provider's parsed `type`
reaches the tier-1 `type` column on the drain path (no hardcoded `"memory"`); **(D6)** the summarizer's
`filesTouched` / `completedWork` / `decisions` / `openThreads` are carried, sanitized and bounded into the
episodic row (unknown `kind` → `WorkKind::Other`); **(D7)** section-heading bytes are charged inside the greedy
pass, so the emitted block can never exceed `4 × token_budget` and `tokens` is the emitted block's byte
estimate; **(D8)** the startup consolidation tick is scheduled in the same tracked task as the periodic loop
instead of being awaited before `serve_stdio` (the plugin's `hello` is timeout-bounded). Non-blocking findings
also fixed: duplicate capture-summary suppression now keys on the ACCEPTED episodic rows, malformed/oversized
tier-1 metadata is skipped with one bounded warning instead of rendering as a project fact, the scheduler uses
the single `DISTINCT_LIVE_PROJECT_IDS` SQL locus, and the e2e test's self-asserting parts vector is gone.
Re-measured: `mem-bridge` lib 8, `project_scope` 2, `capture_prompt` 2, `capture_tier1` 4, `inject_budget` 16,
`consolidation_scheduling` 6; full workspace battery and the 8-file plugin suite re-run green.

