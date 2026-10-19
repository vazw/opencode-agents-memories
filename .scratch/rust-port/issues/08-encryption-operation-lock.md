# 08 — W1.5 · Encryption at rest & operation lock (US 2.4)

Status: done — 2026-10-18

## What to build

Encryption-key management + encryption migrator; cross-process operation lock (file-lock based) with stale-holder recovery. Sequence: key derivation/storage, then a **read-only compatibility spike on a shard produced by the TS version** before any migrator work, then the migrator (plaintext→encrypted), then `OperationLock`. Cipher/mode decision goes to an ADR (roadmap §0b D1, R4). Wave detail: `.scratch/rust-port/roadmap.md` §W1.5.

## Acceptance criteria

- [ ] Behaviors ported: `encryption-at-rest`, `turso-encryption-migrator`, `turso-multiprocess`, `turso-dedup-extract` (lock interplay).
- [ ] Read-compat spike result documented: can the port open an upstream-produced encrypted shard (cipher/mode identified)?
- [ ] Stale-holder recovery + short-write hardening verified in repeat-loop cross-process tests; any flake is a blocking defect.
- [ ] ADR: chosen cipher + key derivation, and the ruling on the experimental-encryption flag.

## Blocked by

- 05 · W1.2

## Comments

- **2026-10-18:** Closed. Reviewer APPROVED after rework iteration 1 (session `ses_ead26407fffe0VsfDgbuw6ywEN`). Test evidence: 439 passed / 0 failed / 2 ignored (baseline 435 + 4 new regression tests). Rework fixed 5 findings: restore-on-verify-failure (Mandate 7), staged-file cleanup, SQL identifier quoting (Mandate 3), is_already_encrypted side-effect removal, doc staleness. Security gate 10 Mandates all PASS/N/A after fixes. ADR-0004 written (cipher aes256gcm, key 32 bytes→64 hex mode 0600, experimental-encryption flag kept). Fan-out wave: sE (steps 3-4, 7 tests), sL (steps 5-6, 8 tests), captain (steps 1-2, 7-8). L1-L6 ledger battery green over all three ledgers. 3 foreign lints attributed (2 frozen vector_utils.rs, 1 s3 embedding_service.rs:289). **Phase 1 exit achieved.** Next: Phase 2 (issues 09-11) under standing promotion.
