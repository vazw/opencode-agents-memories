# 18 — W5.0 · Bridge-strategy decision — JSON-RPC stdio vs N-API (ADR) (US 5.2)

Status: done — 2026-10-20 (reviewer APPROVED: ADR-0007 bridge-strategy landed — 52/52 checklist rows, aggregate CHECKLIST PASS, §Security posture verified by reading, citations spot-checked; grilling complete: 12 decisions resolved)

## What to build

The Phase 5 entry decision gate: choose the bridge between the JS/TS OpenCode plugin shell and the Rust engine — JSON-RPC over stdio (default stance in the roadmap: reversible, host-agnostic, matches the V1→V2 server/client split) vs an N-API addon (higher ceiling, per-host prebuild burden). **User grilling is mandatory** before closing. Output: an ADR in `docs/adr/`. Wave detail: `.scratch/rust-port/roadmap.md` §W5.0.

## Acceptance criteria

- [ ] ADR written: decision, alternatives (stdio JSON-RPC vs N-API), reversibility analysis, latency/dependency trade-offs.
- [ ] User grilling session completed on the trade-offs.
- [ ] Message-schema implications for W5.1 noted (frozen before W5.1 starts).
- [ ] W5.1 unblocked with the chosen strategy.

## Blocked by

- None — gates 19 (W5.1); take at Phase-4 completion.

## Comments

- **2026-10-20 — W5.0 bridge decision recorded:** the bridge-strategy ADR is written at `docs/adr/0007-bridge-strategy.md` (sha256 `e3c26c2116f2`) — **JSON-RPC over stdio** chosen; the N-API addon is
  rejected (host crash domain, native teardown inside the host's Node runtime, per-platform prebuild burden,
  reversibility). The 2026-10-20 user grilling ratified ten decisions (ADR-0007 D-30 … D-39) and the earlier
  session's twelve carry over unchanged; two 2026-10-20 user rulings land with it ("Accept the bounded
  guarantee" for the `capture_id` cardinality, and "Durable capture inbox before acking" for the durable capture
  inbox, ADR-0007 D-48); the five-method message schema is frozen in ADR-0007 D-40 … D-47, which unblocks
  issue 19 (W5.1) — the JS wrapper lane and the Rust stdio-server lane may now split against that frozen
  schema. Blueprint: `blueprint-20261020-1602-w50-bridge-strategy-adr-r3.md`
  (sha256 `a37f09a977c6`, `SYS-APPROVED 2026-10-20T16:16Z`).
- **2026-10-20 — wave completed & verified:** fleet pipeline on blueprint `blueprint-20261020-1602-w50-bridge-strategy-adr-r3.md` (sha256 `a37f09a977c6`, SYS-APPROVED 2026-10-20T16:16Z): planner → Plan Gate **REJECTED** (rev 1 — the checklist's axum-blind greps, the false "episodic tier is unchanged" claim, a security set unreachable from any step; plus one item escalated to the user) → revision → Plan Gate **REJECTED** (rev 2 — three verification-scaffolding defects: a grep that failed on the blueprint's *own* bolded text, a whole-file count that passed a hollow document, and a scope gate no correct implementation could satisfy) → revision → Plan Gate **APPROVED** → coder → reviewer `STATUS: APPROVED`. Two escalations were the user's, both now encoded: **"Accept the bounded guarantee"** (the `capture_id` cardinality — bounded to *while the row carrying it exists*, a recapture is W5.3's per ADR-0005's own follow-up) and **"Durable capture inbox before acking"** (D-48 — acked means *durably queued*; the inbox is durable, inside the storage root, bounded, idempotent by `capture_id`, drained on restart, purged once applied, holds transcripts so it inherits ADR-0004's at-rest protection, and a full queue produces a specific error, never a silent drop). The reviewer's independent re-run: the blueprint's aggregate block prints `CHECKLIST PASS` (exit 0), all **52** rows pass, the `## Security posture` section was read end-to-end (all ten items (a)–(j) confirmed correctly-scoped normative sentences, not merely present), the **20** `Grounds:` paragraphs confirmed substantive with real citations, the TDD-EXEMPT claim verified (no test file, no `crates/**`, no manifest, no `docs/agents/testing.md` row), and both rulings confirmed encoded with both open questions still open. Wave ledger: `/tmp/opencode/progress/20261020-1602-w50-bridge-strategy-adr-r3.ledger.md` — 10/10 steps DONE, 0 unchecked, 0 open cursors. Deliverable: `docs/adr/0007-bridge-strategy.md` (sha256 `e3c26c2116f2`, 668 lines) — **D-30 … D-39** the ten ratified bridge decisions (stdio JSON-RPC; one engine process with session-scoped logical instances; the axum server inside the engine; one agent-prioritised DB lane; an explicit `session_id`; `inject`-only blocking with the 5 ms / p95 / 100-turn trigger; `capture_id` as a column on the episodic row; the per-platform engine binary in the npm tarball; fail-open-but-never-silent; the standalone-capable engine behind an exclusive lock), **D-40 … D-47** the frozen five-method schema, **D-48** the durable inbox. **Still open for the user:** whether a second OpenCode instance should degrade under the exclusive engine lock or be pointed at a separate storage root, and whether `darwin/x64` is in the port's own engine target matrix. Unblocks issue 19 (W5.1) — the JS wrapper lane and the Rust stdio-server lane may now split against the frozen schema.
