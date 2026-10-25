# 53 — Storage · A supported config lands in a combination turso's manual excludes: encryption × multiprocess WAL

Status: needs-triage — 2026-10-25. **On every non-Windows platform the vault opens encrypted *and* in
experimental multiprocess WAL — a cell turso's manual excludes for page-codec databases — and a user who
enables `databaseEncryptionEnabled` puts every shard there too.** Reachability is a source read; the harm is
`needs_validation` (gate below).

## PREMISE — reachability established, harm not

**What our tree asks for (source read, this repo).**

| fact | evidence |
|---|---|
| the option vector always *lists* `ExperimentalFeature::Encryption` — but that variant is **read by nothing at the connect site**; the effective switch is the resolved key | `crates/mem-store/src/db.rs:259` (the entry) vs `:327-328` (`.experimental_encryption(opts.encryption.is_some())`) and `:332-337` (`.with_encryption(..)`) |
| and multiprocess WAL on every non-Windows platform | `crates/mem-store/src/db.rs:259-261` (`ConnectOptions::for_config`) |
| the resolved key is attached to that same option set, per open | `crates/mem-store/src/connection.rs:208-210` |
| the key is **opt-in** for the store — disabled ⇒ `None` | `crates/mem-store/src/db.rs:279-292`; default `false` at `crates/mem-core/src/config/schema.rs:602` |
| **the vault is not opt-in**: it refuses to open without encryption and forces it on for its own manager | `crates/mem-vault/src/store.rs:112-114` (refuses), `:271` (`database_encryption_enabled = true`), cipher at `:154` |

So the combination is a user choice for shards, prompts, profiles, episodic and facts — and
**unconditional for the vault** on non-Windows. That second half is the load-bearing one: it is not a
configuration a user opted into, and it cannot be flipped off without making the vault unusable.

Two precision notes, because the first draft of this table overstated them. `ExperimentalFeature::Encryption`
is carried in the option vector but consumed by nothing (`grep` for it outside `for_config` and the tests
returns no production read) — the vector entry is decorative and the real switch is `opts.encryption`
(`db.rs:327-328`). And "encrypted" means *a key resolved*: with `database_encryption_enabled: false`
(`schema.rs:602`) `resolve_database_encryption` returns `None` (`db.rs:279-292`) and the open is plaintext
with the flag still on — which is today's default for the five store families and is why the cell is not the
default configuration for them.

**What the engine says (source read, `turso_core-0.8.3`, the shipped version).**

- `validate_open_options` refuses an **external** `PageCodec` when multiprocess WAL is on:
  `"external page codecs are not supported with experimental multiprocess WAL"`
  (`database.rs:908-919`, called at `:922`). HEAD `aa93d669` is identical (`core_database.rs:968-980`).
- The same function also refuses built-in encryption **combined with** an external page codec:
  `"built-in encryption cannot be combined with an external page codec"` (`:923-927`; HEAD `:990-1000`).
- There is **no** check for built-in `encryption` + `enable_multiprocess_wal`. The engine **permits** our
  cell; the manual does not describe it as supported.

**What the docs say (doc-sourced — from the owner's ingest of the turso repo, archived at
`raw/turso-repo-20261025/manual.md`; not on disk here, so this is a documentation claim, not a source read).**
Page codecs support in-process WAL, checkpointing, `VACUUM` and `VACUUM INTO`, and *"do not support MVCC,
experimental multiprocess WAL, `ATTACH`, or partial sync"* (`manual.md:1320-1323`).

**Two questions this issue must not answer for the owner.**

1. **Does "page codecs" include the built-in cipher?** The code suggests not, and says so in a way that
   treats the two as distinct species: an external codec is refused *with* multiprocess WAL, and built-in
   encryption is refused *with an external codec* — but built-in encryption with multiprocess WAL is never
   mentioned. That is an argument, not a specification: the manual does not say which kind it means, and the
   validator's existence proves the exclusion is only *enforced* for the external kind.
2. **Does the failure need a second opener?** The only evidence of harm needs one (below). If the answer is
   "only with a second process", the exposure is the mixed-mode window itself — issue **45**'s subject — and
   this issue reduces to a documentation cell, not a new hazard.

**The only harm evidence, and why it is weak.** The archived vault ingest records
`Corrupt("database header magic mismatch")` after `nbackfills > 0` once a second process joins. Three
caveats: it is an ingest record, not our measurement; it is one path, not a matrix; and **its error string
does not match the code** — `turso_core-0.8.3/storage/shared_wal_coordination.rs:438-440` says
`"shared WAL coordination header magic mismatch"`, so either the ingest paraphrased or it saw a different
failure. Treat it as a lead, not as the finding.

## GATE — before any option is chosen

- **G1.** Two-process probe on a **fresh** encrypted database with multiprocess WAL: writer commits → second
  process opens (once read-only, once writable) → commit, reopen, `integrity_check`. Record pass/fail
  verbatim, including which error and from which side.
- **G2.** The same on the **vault** — its own scope directory, its own key, encryption forced on
  (`mem-vault/src/store.rs:112`) — because the vault is the always-affected surface.
- **G3.** The control: the same probe with encryption **off**. If it fails too, the flag is the problem, not
  the combination, and this issue collapses into **45** rather than standing alone.
- **Inversion arm:** the same probe with multiprocess WAL **off** and encryption on must pass. If that also
  fails, the encryption half is the problem and the issue is mistitled.

If G1/G2 show no failure, the issue **does not vanish**: it becomes "our docs and the ADR must state the
divergence from turso's manual, or we stop shipping the combination". The doc-vs-code gap is a finding
either way — the same class as 45.A4.

## What to build

The ruling is the owner's; the issue records it. Four options, and only one is "do nothing":

- **O1 — refuse at config resolution.** Enabling encryption while multiprocess WAL is on becomes a typed
  error (decision-2 posture, `SchemaNewerThanBuild`'s shape). Honest and loud, but on non-Windows it would
  make **the vault unopenable** unless the vault also changes — that consequence must be stated, not
  discovered.
- **O2 — drop the flag where the key is on.** Per-open, in `ConnectOptions`: an encrypted open does not get
  `MultiprocessWal`. Cheap, removes the cell, and 45's collision matrix shrinks at the same time. Cost: the
  vault loses `.tshm` coordination, which is a behaviour change for a second vault process.
- **O3 — keep it and record it.** One ADR sentence plus the vault's known limitation, with G1/G2 as the
  evidence and 45's guard covering the cross-process window. Only defensible if G1/G2 pass.
- **O4 — wait for issue 43's W6 flag decision.** W6 contemplates dropping `experimental_multiprocess_wal`
  engine-wide; that would delete this issue. This is a dependency, not a plan.

Whichever is chosen, this issue owns **only** the cell. It does not re-open 43's W6 promise question
(`43:312-320`), and it is not the mixed-version guard (45).

## Acceptance criteria

- [ ] **53.1** A reachability table exists here first: which databases can be in the cell, under which
      configuration, on which platform — including the explicit note that the **vault is always in it** on
      non-Windows (`mem-vault/src/store.rs:112-114`, `:271`) and that `mem-store`'s five families are in it
      only when encryption is enabled (`schema.rs:602`).
- [ ] **53.2** The doc-vs-code contradiction is recorded with both citations: `manual.md:1320-1323`
      (labelled doc-sourced) against `turso_core-0.8.3/database.rs:908-927` and HEAD `core_database.rs:968-1000`.
      The fact that the engine has a hard error for *external* codecs and none for the built-in cipher is the
      crux and must be stated as such.
- [ ] **53.3** **G1, G2, G3 and the inversion arm run** (W0-style harness: `crates/mem-store/tests/turso_capabilities.rs`
      already builds a flag-off variant at `:2012`), and the results are pasted in — including which side
      errored, if any. A result that cannot be produced records the blocker, not a guess.
- [ ] **53.4** The ruling is recorded as **O1 / O2 / O3 / O4** with a one-paragraph rationale, and the two
      questions of the premise are answered in it rather than left to the implementer.
- [ ] **53.5** If O1 or O2: the change lands in the config/connect seam (`db.rs:258-268` or the vault's
      `build_manager`), with tests for both directions — encrypted ⇒ no multiprocess WAL (or a typed refusal),
      unencrypted ⇒ unchanged — and every affected family exercised, **the vault included**.
- [ ] **53.6** If O3: the limitation is written where a user meets it (the ADR, and the vault's own
      documentation), with G1/G2 numbers, not adjectives.
- [ ] **53.7** **No re-litigation:** 43's W6 flag decision is referenced, never decided here (`43:312-320`);
      45's guard is referenced, never restated; 49's config census is not touched (this is a behaviour cell,
      not a dead key).
- [ ] **53.8** If O2/O4 land, **45's open-path table is updated in the same wave** — the vault stops being a
      flag-on opener, and 45 must not be left asserting it is one. Record the edge in both issues' comments.

## Blocked by / Blocks

- **Blocked by:** nothing for 53.1 and 53.2. **53.3** needs a two-process harness, which exists
  (`turso_capabilities.rs:2229` spawns child writers) — no new infrastructure.
- **Blocks:** nothing formally. **Cross-reference:** issue **45** (if O2/O4 remove the cell, its matrix
  shrinks and its vault row changes); issue **43**'s W6 (O4 defers to it); issue **47/48** only in that the
  encryption posture decides whether erasure residue is plaintext — which 43 already owns, and this issue
  must not re-open.

## Comments

- Filed 2026-10-25 from the owner's turso HEAD review; sources fetched to
  `.tmp/scratch/turso-head-aa93d669/`. **Every claim above was re-checked at `aa93d669` and in the shipped
  `turso_core-0.8.3`**, because HEAD is not what we run — and both versions agree on the validator, the
  absent check for built-in encryption, and the read-only asymmetry 45.A3 records.
- **Not issue 49**: 49 is a census of config keys with no reader. These keys have readers; the *combination*
  they produce is the defect. **Not issue 45**: the guard defends a collision window; this is a supported
  configuration the engine's docs say is out of scope — a matrix cell, not a race.
- **Provenance discipline.** Source-read here: our five open families, the vault's forced encryption, the two
  engine validators, the absence of a built-in-encryption check. Doc-sourced and labelled: `manual.md:1320-1323`
  (manual is archived, not on disk). Ingest-sourced and labelled: the `Corrupt` record and its
  string mismatch. Nothing here is a runtime observation, and nothing claims the read-only fallback or the
  corruption is measured.
