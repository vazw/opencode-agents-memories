# 11 — W2.3 · Privacy: private-tag strip, prompt filter, secret resolution (US 4.2)

Status: ready-for-agent

## What to build

`mem-ai::privacy` — `<private>` tag stripping (including malformed tags), injected-prompt filtering, and secret resolution with strict file-permission checks. Nothing persisted or sent to embedding/LLM APIs may pass unfiltered. Wave detail: `.scratch/rust-port/roadmap.md` §W2.3.

## Acceptance criteria

- [x] Behaviors ported: `privacy`, `privacy-malformed-tags`, `injected-prompt-filter`, `secret-resolver-permissions`.
- [x] Malformed-tag handling matches upstream (no partial leakage).
- [x] Secret resolver rejects files with permissive permissions (strict checks).

## Blocked by

- 02 · W0.2 (can run concurrently with 09–10)

## Comments

- **2026-10-19:** Closed — last wave of Phase 2. Reviewer APPROVED (session `ses_eac9389baffeG0VGyMa5gobIb6`, go-tier after Beam Free rate-limited). Blueprint r1 `blueprint-20261019-0817-privacy-filters-r1.md` hash `3fc66017d7f3` (r0 gate-rejected with 6 mods, all verified folded in). Test evidence (reviewer-reproduced): `--test privacy` 117/0, `--lib privacy::layer` 3, `--test provider` 32 unchanged, workspace 652 passed / 0 failed / 2 ignored (532 baseline + 120). Security gate: 10 Mandates, 0 violations (7 PASS, 3 N/A). Leak-safety claims verified in code: FEFF-recognizing whitespace predicate (`tags.rs:36-38`), byte-scanner char-boundary coverage (`unicode_boundaries.rs`), request-side-only filtering asserted in both halves. W2.1 `SecretResolver` trait implemented as-is — no competing interface. Deviations accepted: zeroize `derive` unresolvable offline → hand-written `Zeroize`/`Drop`/`ZeroizeOnDrop` (reviewer independently re-derived the offline constraint; contract preserved); `#![allow(dead_code)]` + `#![allow(clippy::duplicate_mod)]` proven load-bearing via reversible byte-identical experiments. Ledger-stamp hygiene REMEDIATED: 11 strictly increasing fresh stamps corroborated by on-disk mtimes. Non-blocking nuance: Step-0 baseline provenance wording (irrelevant to verdict; reviewer re-measured). Testing.md registry backfilled with W2.1–W2.3 rows. Next: Phase 3 (W3.1 planner already in flight).
