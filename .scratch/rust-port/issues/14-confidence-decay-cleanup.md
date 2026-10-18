# 14 — W3.3 · Confidence, decay, AI cleanup & validation (US 3.2)

Status: needs-triage

## What to build

Confidence scoring and temporal decay for profile traits; AI-assisted deduplication/merge/cleanup via the configured provider validators. Decay/confidence math (pure) and the AI cleanup loop are independent lanes. Wave detail: `.scratch/rust-port/roadmap.md` §W3.3.

## Acceptance criteria

- [ ] Behaviors ported: `profile-utils`, `user-profile-decay`, `user-profile-learning-confidence`, `ai-cleanup`, `ai-cleanup-apply`.
- [ ] One-off traits decay rather than becoming permanent rules (decay semantics match upstream).
- [ ] Contradictory traits resolved by cleanup without data loss outside the merged pair.

## Blocked by

- 13 · W3.2
