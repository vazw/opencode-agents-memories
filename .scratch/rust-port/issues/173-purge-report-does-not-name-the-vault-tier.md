# 173 — a purge success report does not name the vault tier (ADR-0019 D-19-9 unimplemented)

Status: ready-for-agent — 2026-11-26. Work: `crates/mem-bridge/src/purge.rs` (`NotErasedTier` gains an
`erased` tri-state; `NOT_ERASED` grows a `vault` entry answered `Unknown`), `crates/mem-bridge/src/sweep.rs`
(`not_erased` serialization), one bridge test asserting the vault appears as `Unknown` and no other tier
does, `crates/mem-vault/CONTEXT.md`'s "no longer complete or true" paragraph.
Filed: 2026-11-26 by the feature-gap audit (`docs/evidence/feature-gap-audit-20261126/FINDINGS.md` §4.2 / G4),
which found an **accepted** ADR decision with no implementing code and no owning issue.

## The defect

`purge.completed` carries `not_erased`, described by its own doc comment as the guarantee that "a success is
never a complete-erasure claim" (`crates/mem-bridge/src/purge.rs:95-97`). It names two tiers:

```rust
pub const NOT_ERASED: [NotErasedTier; 2] = [      // purge.rs:126
    NotErasedTier { tier: "memories-shard",  reason: "no per-session provenance; Phase 4" },
    NotErasedTier { tier: "milestones-recap", reason: "the merged milestone summary and its embedding survive …" },
];
```

The vault is **not** among them, and since W8a the vault holds content for any session that marked it. The
crate states the consequence in its own docs (`crates/mem-vault/src/lib.rs:45-47`): "`purge.completed`'s
`not_erased` is **no longer** complete or true without naming the vault … a purge run today erases no vault
rows and does not say so."

Severity: **disclosure**, not data escape. Vault ciphertext is per-scope encrypted (ADR-0012 D-14), so the
residue is not readable by whoever reads the purge report — the operator is simply told the erasure was more
complete than it was. That is exactly the fail-open direction ADR-0019 was written to close.

## What is already decided (do not redesign)

`docs/adr/0019-erasure-posture.md:257` — **D-19-9, Accepted**:

> `NOT_ERASED` grows to two tiers, `memories-shard` and `vault`, each carrying `erased: Yes | No | Unknown`.
> `memories-shard` is **`No` today**; the existing reason string names **`Phase 4`** as the condition under
> which that could change — a declaration, not a measurement. `vault` is **`Unknown`** until W8b ships the
> purge leg, and `Unknown` **on any failed open, never `No`**; `Yes` and `No` become reachable only when the
> leg can establish them, and what `Yes` is worth changes at W8c (key-generation destruction). No other tier
> ever answers `Unknown`.

D-19-9's "two tiers" counts the pair it names; **D-19-8** (`:246`) independently requires the milestone
residual to stay named, and it is named today. The shipped set is therefore **three** rows, not two.

## Acceptance criteria

1. `NotErasedTier` (`crates/mem-bridge/src/purge.rs:110` region) carries the tri-state — `erased: Erased`
   with `Erased::{Yes, No, Unknown}` — and every existing construction site compiles with an explicit value.
2. `NOT_ERASED` is three entries: `memories-shard` → `No` (its reason keeps naming `Phase 4`),
   `milestones-recap` → `No` (D-19-8: retained, so the answer is known), `vault` → `Unknown` with a reason
   naming W8b as the unshipped leg.
3. Both report surfaces serialize the field: the CLI `purge.completed` JSON and the sweep report's
   `not_erased` (`crates/mem-bridge/src/sweep.rs:424-438`). The field name is `erased`, values `Yes|No|Unknown`
   — the same capitalisation ADR-0019 uses, so an operator reading the ADR and the report sees one vocabulary.
4. A test asserts, on a real purge run: `vault` present with `erased: Unknown`; `memories-shard` and
   `milestones-recap` present with `erased: No`; **no other entry answers `Unknown`** (D-19-9's last clause).
5. `crates/mem-vault/CONTEXT.md:45-47` no longer says the report is "no longer complete or true": it says the
   vault is now named as `Unknown` pending W8b, and keeps the statement that no erasure path exists yet.
6. Existing assertions that treat `not_erased` as a two-entry list (e.g.
   `crates/mem-bridge/tests/promotion_end_to_end.rs:838`, `:1035`, `crates/mem-bridge/tests/purge_cli.rs`)
   are updated to the three-row shape rather than loosened.

## Explicitly out of scope

The **W8b purge erasure path** for the vault (D-11, D-13 … D-16, D-18 in `docs/adr/0012`) is *not* in this
issue. This issue makes the report truthful about what today's purge does not reach; it must not add vault
deletion, must not change `vault.db`, and must not claim `Yes` anywhere (nothing can establish it yet).

## Inverted world — what would close this as "not a defect"

(1) A superseding ADR or owner ruling that removes the vault tier from `NOT_ERASED`
(`git grep -n 'D-19-9' docs/adr` returns only 0019 today), or (2) a shipped W8b erasure path, which would
make the vault answer reachable and turn this into "the entry exists elsewhere". Neither holds.

## Evidence

- `docs/adr/0019-erasure-posture.md:257` (decision), `:118`, `:183`, `:345` (A10 inventory row), `:371` (B11 row).
- `crates/mem-bridge/src/purge.rs:95-101`, `:126-138`; `crates/mem-bridge/src/sweep.rs:42`, `:424-438`.
- `crates/mem-vault/src/lib.rs:44-47`.
- Audit: `docs/evidence/feature-gap-audit-20261126/FINDINGS.md` §4.2, G4.
