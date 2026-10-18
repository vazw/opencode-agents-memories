# 08 — W1.5 · Encryption at rest & operation lock (US 2.4)

Status: ready-for-agent

## What to build

Encryption-key management + encryption migrator; cross-process operation lock (file-lock based) with stale-holder recovery. Sequence: key derivation/storage, then a **read-only compatibility spike on a shard produced by the TS version** before any migrator work, then the migrator (plaintext→encrypted), then `OperationLock`. Cipher/mode decision goes to an ADR (roadmap §0b D1, R4). Wave detail: `.scratch/rust-port/roadmap.md` §W1.5.

## Acceptance criteria

- [ ] Behaviors ported: `encryption-at-rest`, `turso-encryption-migrator`, `turso-multiprocess`, `turso-dedup-extract` (lock interplay).
- [ ] Read-compat spike result documented: can the port open an upstream-produced encrypted shard (cipher/mode identified)?
- [ ] Stale-holder recovery + short-write hardening verified in repeat-loop cross-process tests; any flake is a blocking defect.
- [ ] ADR: chosen cipher + key derivation, and the ruling on the experimental-encryption flag.

## Blocked by

- 05 · W1.2
