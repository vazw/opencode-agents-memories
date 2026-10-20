# 17 — W4.3 · Episodic consolidation & compaction (US 1.3, 1.5)

Status: done — 2026-10-20 (reviewer APPROVED: 1168/0 workspace, 96 mem-episodic lanes, security gate all PASS)

## What to build

Age-based consolidation of episodic records into milestones, promotion of durable facts into permanent memory, and compaction that preserves agent identity/attribution. Interacts with the confidence/decay layer (W3.3) via clean service seams. Wave detail: `.scratch/rust-port/roadmap.md` §W4.3.

## Acceptance criteria

- [ ] Behavior ported: `compaction-agent-preservation` (agent attribution survives compaction).
- [ ] Age-threshold tests: eligible records consolidate; ineligible untouched (thresholds from config, W0.2).
- [ ] Promoted facts land in the permanent memory tier with normal scope rules.
- [ ] Cleanup/dedup interplay verified (no double-processing with W3.3 cleanup).

## Blocked by

- 09 · W2.1
- 11 · W2.3
- 15 · W4.1

## Comments

- **2026-10-20 — wave completed & verified:** fleet pipeline on blueprint `blueprint-20261020-1223-episodic-consolidation-r3.md` (sha256 `a5a92ae03233`, SYS-APPROVED 2026-10-20T12:36Z): coder → reviewer **REJECTED** on evidence → debugger FIX PRESCRIPTION → rework → reviewer `STATUS: APPROVED` (iteration 2, the cap's last). The first review rejected on **evidence, not behaviour**: two properties under review were provably invisible to the suite and seven ledger DONE stamps were future-dated. The debugger measured the root cause the first review's prescriptions missed — the own-lock property is defended by **two independent guards**, so each single-guard mutation is an *equivalent mutant* and the original ask ("M15 must then fail") was unreachable from the integration level alone; and the proposed F2 fixture was *impossible* (`validate` caps a summary at `MAX_SUMMARY_BYTES` 100 000, so a 200 KB row cannot exist — the unbounded **decision text** is the working lever, giving `k = 1` of 2 claimed rows). Final independent re-run by the reviewer in an isolated scratch tree: `cargo fmt --all --check` 0 diff · `cargo build --workspace` 0 warnings · `cargo clippy --workspace --all-targets -- -D warnings` exit 0 · `cargo test --workspace` **1168 passed / 0 failed** over 132 suites · `cargo audit` 0 vulnerabilities · 10-mandate security gate **zero violations** · all **69** blueprint-named tests present, none ignored, none renamed. The reviewer **reproduced** all three mutation rounds rather than inheriting them (M15 dies only against the crate-internal unit in `src/promotion.rs`; the mem-store guard dies only against its own test; the composed fail-open dies against the end-to-end test) and proved the rework's behaviour delta by diffing against the pre-rework copy: *test module appended only*, *doc comments only*, *the A1 check only*. F1 closed by 4 new tests (2 mem-episodic integration + 1 crate-internal + 1 mem-store); F2 closed by 1 new test; F3 closed by correcting **seven** stamps (the review had flagged four — step 4 was missed) to a labelled **upper bound** `2026-10-20T13:14:17Z` plus a live read for step 10, with a "Stamp audit" section recording the rule. Advisories A1–A5 all closed: A1 was a **real behaviour change** (`batch_limit == 0` now returns `Validation` instead of a silent no-op, checked before the gate and the lock); A2 a doc note; A3 a raw `COUNT(*)` so the hard-delete test carries the property it is named for; A4/A5 ADR-0006 text (PID-only ownership and both residual cases; the per-run token deferred, since it changes the on-disk `LockState` shared with the TS upstream). Wave ledger: `/tmp/opencode/progress/20261020-1223-episodic-consolidation-r3.ledger.md` — 11/11 steps DONE, 0 unchecked, 0 open cursors. Deliverable: v3 `episodic_milestones` migration · upstream-exact attribution resolver · one-stripping-path summarizer + transcript builder + `fit_prefix` · pure promotion selection (singletons included) + `ShardPermanentMemorySink` + `VectorCountBump` · milestone/contributor model carrying the retained source summaries · per-agent pipeline (a)–(f) with the skip/abort split · std-only in-process gate + busy mapping + one atomic phase-(f) batch · ADR-0006 D-18…D-29. One behaviour note for the record: `crates/mem-store/src/shard_manager.rs` gained `get_write_shard_allowing_own_lock` — the run holds `.turso-operation.lock` for its whole duration (D-25) while the promotion sink resolves a write shard, and the strict path refuses even its own holder; the tolerant path is additive, re-reads the live lock, refuses a foreign pid, and leaves the legacy migration lock blocking unconditionally.
