# Vault limitation draft — issue 53 O3

**Status: A is draft only; B is applied in this wave.** The paragraph exists in two formats for two
surfaces. **B** is applied by this wave to `crates/mem-vault/CONTEXT.md` in this repository, in the same
commit as ADR-0012's paragraph. **A** is for the owner's vault wiki page
(`wiki/concepts/multiprocess-wal.md`) in the owner's separate `AgentsBrain` repository (`e468012` when this
was drafted): that repository's commit is the owner's, and this wave does not write to it. The authority for
the same facts inside the port is ADR-0012 `## Consequences`.

Every figure and verdict below was re-derived from `docs/evidence/issue-53-encryption-multiprocess/`
(`README.md`, `run-store.log`, `run-vault.log`, `verdicts-store.txt`, `verdicts-vault.txt`) for this draft,
not copied from the issue's summary.

## Where the paragraph goes

**A — the owner's vault wiki, draft only, not applied here.** `wiki/concepts/multiprocess-wal.md`, as its own
blockquote immediately after the existing 2026-10-25 correction block: after the line ending
`> [[turso-repo-docs-and-head-20261025]] §5).` and before the paragraph beginning
`Under [[adr-0007-bridge-strategy]]`. That block is where the page says the built-in-cipher question is
"not settled by any source here" and records the `.tshm` backfill-proof defect; the paragraph answers the
first by measurement and qualifies the second.

**B — `crates/mem-vault/CONTEXT.md`, applied in this wave.** Section
`## Accepted, documented, not mitigated`, as the third bullet, after "Enumerability" and before
`## Boundaries`. The text applied is the block below, byte for byte.

## Text for A — the owner's vault wiki, paste verbatim when the owner applies it

```markdown
> **Built-in encryption × experimental multiprocess WAL: excluded by turso's
> manual for page codecs, permitted by the engine, forced by the vault,
> measured.** Every non-Windows vault open carries both: `VaultStore::ready`
> refuses a config with encryption disabled and `build_manager` enables it,
> while `mem-store`'s `ConnectOptions::for_config` enables the flag on every
> non-Windows platform. Turso's manual says page codecs "do not support MVCC,
> experimental multiprocess WAL, `ATTACH`, or partial sync"
> (`manual.md:1320-1323`, doc-sourced) and does not say whether the built-in
> cipher counts as one; the engine refuses that pairing only for external page
> codecs (`turso_core-0.8.3/database.rs:908-927`, HEAD `aa93d669`
> `core_database.rs:968-1000`) and has no check for the built-in cipher.
> Measured 2026-10-27 on turso 0.8.3, x86_64 Linux: four gate probes and four
> extensions, all 8 PASS — two-process read/write on an encrypted vault with
> `REOPEN-HASHES count=19 exact match` and `REOPEN-INTEGRITY ok`, a `-tshm` of
> 86 016 bytes created and backfilled when a flag-off database is opened flag-on
> (`probe.db-tshm=86016`), and a keyless open refused ("Database is encrypted
> but no encryption options provided"). The archived record of a `.tshm`
> backfill-proof `Corrupt("database header magic mismatch")` was not observed in
> any shape measured here, and this measurement neither confirms nor disproves
> it. Not covered: simultaneous in-flight commits, crash or power loss,
> tmpfs probe roots, one engine version on one platform. This is a documented
> divergence, not a configuration turso supports; ADR-0012 `## Consequences`
> carries the full record and `docs/evidence/issue-53-encryption-multiprocess/`
> holds the logs.
```

## Text for B — `crates/mem-vault/CONTEXT.md`, applied in this wave, paste verbatim

```markdown
- **Built-in encryption × experimental multiprocess WAL: excluded by turso's
  manual for page codecs, permitted by the engine, forced by the vault,
  measured.** Every non-Windows vault open carries both: `VaultStore::ready`
  refuses a config with encryption disabled and `build_manager` enables it,
  while `mem-store`'s `ConnectOptions::for_config` enables the flag on every
  non-Windows platform. Turso's manual says page codecs "do not support MVCC,
  experimental multiprocess WAL, `ATTACH`, or partial sync"
  (`manual.md:1320-1323`, doc-sourced) and does not say whether the built-in
  cipher counts as one; the engine refuses that pairing only for external page
  codecs (`turso_core-0.8.3/database.rs:908-927`, HEAD `aa93d669`
  `core_database.rs:968-1000`) and has no check for the built-in cipher.
  Measured 2026-10-27 on turso 0.8.3, x86_64 Linux: four gate probes and four
  extensions, all 8 PASS — two-process read/write on an encrypted vault with
  `REOPEN-HASHES count=19 exact match` and `REOPEN-INTEGRITY ok`, a `-tshm` of
  86 016 bytes created and backfilled when a flag-off database is opened flag-on
  (`probe.db-tshm=86016`), and a keyless open refused ("Database is encrypted
  but no encryption options provided"). The archived record of a `.tshm`
  backfill-proof `Corrupt("database header magic mismatch")` was not observed in
  any shape measured here, and this measurement neither confirms nor disproves
  it. Not covered: simultaneous in-flight commits, crash or power loss,
  tmpfs probe roots, one engine version on one platform. This is a documented
  divergence, not a configuration turso supports; ADR-0012 `## Consequences`
  carries the full record and `docs/evidence/issue-53-encryption-multiprocess/`
  holds the logs.
```

## What the paragraph claims, and where each claim comes from

| claim | source |
|---|---|
| the manual excludes page codecs from multiprocess WAL (its sentence is scoped to **external** page codecs) | `manual.md:1320-1323`, doc-sourced (issue 53; the manual is not in this repository) |
| the engine refuses only external page codecs | `turso_core-0.8.3/database.rs:908-927`; HEAD `aa93d669` `core_database.rs:968-1000` (source read, issue 53) |
| the vault forces both on every non-Windows open | `crates/mem-vault/src/store.rs` (`ready`, `build_manager`); `crates/mem-store/src/db.rs` (`ConnectOptions::for_config`) |
| 8 of 8 probes report PASS | `verdicts-store.txt` (7 lines), `verdicts-vault.txt` (1 line) |
| `REOPEN-HASHES count=19 exact match` | `run-vault.log` |
| an 86 016-byte `-tshm` created and backfilled on the flag-off to flag-on upgrade | `run-store.log`, G1C |
| `REOPEN-INTEGRITY ok` after the reopen, in every cell | `run-store.log`, `run-vault.log` |
| the keyless open is refused on encrypted cells and succeeds on the plaintext control | `run-store.log` (G1, G3, INVERSION), `run-vault.log` (G2) |

## What it must not be read as

- **Not an upstream-supported configuration.** No source says the pairing is supported; the manual's exclusion
  is written for external page codecs and the engine's silence is not a statement of support.
- **Not a claim that the recorded defect is impossible.** It was not observed in the shapes measured here,
  and the measurement neither confirms nor disproves the archived record.
- **Not a substitute for the port's guards and decisions.** Issue 45's mixed-version guard and issue 43's
  W6 flag decision are referenced, never decided or restated here.
