# Issue 53 — built-in encryption × experimental multiprocess WAL: the four-probe gate

**Question.** Our tree forces built-in encryption **and** experimental multiprocess WAL on
every non-Windows vault open (`crates/mem-vault/src/store.rs:112-114`, `:271`), and the five
`mem-store` families enter the same cell when `database_encryption_enabled` is on
(`crates/mem-store/src/db.rs:259-261`, `:327-337`; default `false` at
`crates/mem-core/src/config/schema.rs:602`). Turso's manual excludes page-codec databases from
multiprocess WAL (`manual.md:1320-1323`, doc-sourced); the engine has a hard error for an
**external** codec with the flag (`turso_core-0.8.3/database.rs:908-919`) and **no** check for the
built-in cipher. Reachability was a source read. **The harm was unverified. This directory records
the measurement that closes that half of issue 53 — not the ruling.**

Status of every claim below: **MEASURED** on this machine, this run, unless labelled otherwise.
Environment: `environment.txt` (Linux 7.2.12_1, x86_64, rustc 1.98.0, turso/turso_core 0.8.3 from
`Cargo.lock`, temp roots under `/tmp` — **tmpfs** on this host, see Limits).

## Harness

Two probe files, both **test-only** and both `#[ignore]`d so a normal suite run compiles them but
never executes them. No production source, manifest or config was changed.

| file | probes |
|---|---|
| `crates/mem-store/tests/issue53_probe.rs` (copy here: `issue53_probe.rs`) | G1, G1B, G1C, G3, G3B, G3C, INVERSION, the concurrent-open observation |
| `crates/mem-vault/tests/issue53_vault_probe.rs` (copy here: `issue53_vault_probe.rs`) | G2 (the vault surface) |

The child-process pattern is the house one: `std::process::Command::new(std::env::current_exe())`
with `--exact <child entry> --ignored --nocapture --test-threads=1`, a stdout line protocol, and a
bounded reader thread (`turso_capabilities.rs:2361`, `open_guard.rs:161`). No sleeps; the parent
waits for the child's own lines.

**The open under test is the production tuple.**

- Multiprocess WAL **on**: `ConnectOptions::for_config(config, Platform::current())` +
  `resolve_database_encryption(config)` through `TursoConnectFactory` — literally
  `ConnectionManager::open_database` (`crates/mem-store/src/connection.rs:208-210`), open guard
  included.
- Multiprocess WAL **off** (the inversion arm only): the same `turso::Builder` calls minus the
  `experimental_multiprocess_wal` token. Issue 45's O3 makes the factory refuse that option set with
  `UnsupportedOpenMode` by design, and `allow_unguarded_open` is `pub(crate)`, so the arm is
  engine-level — precedent `turso_capabilities.rs::open_single_process`.
- Vault (G2): `VaultStore::ready()` for the writable opens (the forced-encryption path); the
  read-only second opener uses the scope's own key read from `vault-keys/{scope}.key` and the same
  factory tuple.

**Data volume.** The first cut used three tiny rows and passed. That is weak evidence for a
backfill/codec interaction, so both probes were re-run with a WAL spanning many pages:
128 filler rows × 4 KiB plus the id rows (measured `probe.db-wal=1 734 552` bytes), and the vault
16 filler entries × 4 KiB plus the canonical entries (measured `vault.db-wal=379 072` bytes). The
results below are from the volume run.

**Commands** (from the worktree, cargo serialized):

```sh
MEM53_REPORT=$PWD/.tmp/issue53-report.txt \
  sh .tmp/scripts/cargo-serialized.sh \
  cargo test -p mem-store --test issue53_probe -- --ignored --nocapture --test-threads=1

MEM53_REPORT=$PWD/.tmp/issue53-vault-report.txt \
  sh .tmp/scripts/cargo-serialized.sh \
  cargo test -p mem-vault --test issue53_vault_probe -- --ignored --nocapture --test-threads=1
```

Raw output: `run-store.log`, `run-vault.log`. Verdict lines only: `verdicts-store.txt`,
`verdicts-vault.txt`.

## The gate, probe by probe

### G1 — fresh encrypted database, multiprocess WAL on

`PROBE G1 RESULT: PASS`

Literal record (`run-store.log`):

```
[G1] CELL encryption=true multiprocess_wal=true read_only=false
[G1] LIFETIME LiveDuringSecondOpen
[G1] OPENING Fresh
[G1] WRITER-OPEN ok
[G1] WRITER-CREATE ok
[G1] WRITER-IDS [1, 2, 3]
[G1] WRITER-FILLER rows=128 max_len=4096
[G1] WRITER-INTEGRITY ok
[G1] INVENTORY-WRITTEN .wal-mode=113 probe.db-tshm=86016 probe.db-wal=1734552 probe.db=4096
[child-ro] CHILD-OPEN-OK
[child-ro] CHILD-IDS 1,2,3
[child-ro] CHILD-INTEGRITY ok
[child-ro] CHILD-DONE
[child-rw] CHILD-OPEN-OK
[child-rw] CHILD-IDS 1,2,3
[child-rw] CHILD-INTEGRITY ok
[child-rw] CHILD-WRITE ok
[child-rw] CHILD-IDS-AFTER 1,2,3,100
[child-rw] CHILD-INTEGRITY-AFTER ok
[child-rw] CHILD-DONE
[G1] POST-CHILD-COMMIT ok (id 4)
[G1] POST-CHILD-INTEGRITY ok
[G1] INVENTORY-POST-CHILD .wal-mode=113 probe.db-tshm=86016 probe.db-wal=1742792 probe.db=4096
[G1] REOPEN ok
[G1] REOPEN-IDS [1, 2, 3, 4, 100] (exact)
[G1] REOPEN-FILLER rows=128 max_len=4096
[G1] REOPEN-INTEGRITY ok
[G1] KEYLESS-OPEN ERR factory open (encryption=true multiprocess_wal=true read_only=true): file is not a database
PROBE G1 RESULT: PASS
```

Which side, if it had errored: the record names `writer`, `child-ro`, `child-rw`, `parent` and
`reopen`. There is no error line in this run. `PRAGMA integrity_check` returned `ok` three times
(writer, read-only child, writable child) and again after the reopen.

### G2 — the vault surface (forced encryption, own scope, own key)

`PROBE G2 RESULT: PASS`

Literal record (`run-vault.log`):

```
[G2] PARENT-PUT-1 ok e6174ae220bf62313ec33b1b4badbc6883f193b0416ff8ceddf6e9c14186dca1
[G2] PARENT-FILLER-COUNT 16 of 16
[G2] PARENT-INTEGRITY ok
[G2] INVENTORY-WRITTEN .vault-encrypted-v1=17 vault.db-tshm=86016 vault.db-wal=379072 vault.db=4096
[vault-child-ro] VAULT-CHILD-OPEN-OK
[vault-child-ro] VAULT-CHILD-INTEGRITY ok
[G2] VAULT-CHILD-RO saw the parent's entry
[vault-child-rw] VAULT-CHILD-OPEN-OK
[vault-child-rw] VAULT-CHILD-INTEGRITY ok
[vault-child-rw] VAULT-CHILD-PUT ok 730834a3d556335c4bedb11ae730150b16c06f8b3af9091ec067177d2d451ec6
[G2] PARENT-PUT-2 ok 4a7c49f626f23170d1b924623e9c773cefe64de4231f88447ffeb23e2d482bbf
[G2] PARENT-INTEGRITY-AFTER ok
[G2] INVENTORY-POST-CHILD .vault-encrypted-v1=17 vault.db-tshm=86016 vault.db-wal=403792 vault.db=4096
[G2] REOPEN ok
[G2] REOPEN-INTEGRITY ok
[G2] REOPEN-HASHES count=19 exact match
[G2] REOPEN-GET e6174ae2... ok (20 bytes)
[G2] REOPEN-GET 4a7c49f6... ok (24 bytes)
[G2] REOPEN-GET 730834a3... ok (20 bytes)
[G2] KEYLESS-OPEN ERR keyless vault open: Database is encrypted but no encryption options provided
PROBE G2 RESULT: PASS
```

The writable second process is `VaultStore::ready` — the path that *forces* encryption — and its
entry is visible to the parent's fresh `VaultStore` after reopen (19 hashes exact: 17 parent
entries, 1 child entry, 1 post-child parent entry).

### G3 — the control: encryption off, multiprocess WAL on

`PROBE G3 RESULT: PASS`

```
[G3] CELL encryption=false multiprocess_wal=true read_only=false
[G3] WRITER-FILLER rows=128 max_len=4096
[G3] INVENTORY-WRITTEN .wal-mode=113 probe.db-tshm=86016 probe.db-wal=1734552 probe.db=4096
[G3] POST-CHILD-COMMIT ok (id 4)
[G3] POST-CHILD-INTEGRITY ok
[G3] REOPEN ok
[G3] REOPEN-IDS [1, 2, 3, 4, 100] (exact)
[G3] REOPEN-FILLER rows=128 max_len=4096
[G3] REOPEN-INTEGRITY ok
[G3] KEYLESS-READ ok ids=[1, 2, 3, 4, 100]
PROBE G3 RESULT: PASS
```

The control passes, and its keyless read succeeds where the encrypted cells' fails — the two arms
differ in exactly the intended variable.

### Inversion arm — encryption on, multiprocess WAL off

`PROBE INVERSION RESULT: PASS`

```
[INVERSION] CELL encryption=true multiprocess_wal=false read_only=false
[INVERSION] LIFETIME ClosedBeforeSecondOpen
[INVERSION] WRITER-FILLER rows=128 max_len=4096
[INVERSION] INVENTORY-WRITTEN probe.db-wal=1734552 probe.db=4096
[INVERSION] WRITER-CLOSED clean
[INVERSION] POST-CHILD-OPEN ok
[INVERSION] POST-CHILD-COMMIT ok (id 4)
[INVERSION] POST-CHILD-INTEGRITY ok
[INVERSION] REOPEN ok
[INVERSION] REOPEN-IDS [1, 2, 3, 4, 100] (exact)
[INVERSION] REOPEN-FILLER rows=128 max_len=4096
[INVERSION] REOPEN-INTEGRITY ok
[INVERSION] KEYLESS-OPEN ERR raw builder open (encryption=true multiprocess_wal=false read_only=true): file is not a database
PROBE INVERSION RESULT: PASS
```

**`NOT OBSERVED — the concurrent form of this arm is unmeasurable without the flag.`** With
multiprocess WAL off and the first writer's handle live, a writable second process is refused by
the engine's own single-process file lock, before any encryption code runs:

```
OBSERVATION INVERSION-CONCURRENT: writable second opener refused while the first handle is live
(single-process semantics): CHILD-OPEN-ERR raw builder open
(encryption=true multiprocess_wal=false read_only=false): Locking error: Failed locking file
'/tmp/mem-store-w12-issue53-INVERSION-CONCURRENT-.../probe.db'. File is locked by another process
```

That refusal is exactly the serialisation the flag removes, so the arm is run in the only shape the
option set permits: the writer closes cleanly, then the second process opens read-only and
writable. The inversion arm is therefore about **encryption alone**, not about two live processes —
which is what the issue asks it to discriminate.

### Extensions (not part of the gate's four cells)

These were added because the issue's only harm evidence names `nbackfills > 0`: a `-tshm` that is
created and backfilled from a pre-existing WAL. G1 does not force that; a database written with the
flag **off** and then opened with it **on** does.

| probe | shape | verdict | literal key lines |
|---|---|---|---|
| G1B | fresh MP-encrypted DB, writer **cleanly closes**, then second process RO + RW | `PROBE G1B RESULT: PASS` | `-tshm=86016`, `-wal=1734552` survive the close; `POST-CHILD-OPEN ok`; `REOPEN-INTEGRITY ok` |
| G1C | **encrypted DB created with MP off**, cleanly closed, then production MP-on open (creates/backfills the `-tshm`), second process RO + RW | `PROBE G1C RESULT: PASS` | `INVENTORY-PRIOR probe.db-wal=1734552 probe.db=4096` (no `-tshm`); `INVENTORY-WRITTEN ... probe.db-tshm=86016 ...`; `REOPEN-INTEGRITY ok` |
| G3B | plaintext control for G1B | `PROBE G3B RESULT: PASS` | as G1B with `KEYLESS-READ ok` |
| G3C | plaintext control for G1C | `PROBE G3C RESULT: PASS` | `INVENTORY-PRIOR ... probe.db=4096`; `-tshm=86016` after the MP-on open; `REOPEN-INTEGRITY ok` |

Full lines in `run-store.log`.

## Non-vacuity — why these passes are not empty

1. **The flag is really on.** Every MP cell leaves a `-tshm` of `86 016` bytes; the inversion arm
   leaves **no** `-tshm` (`INVENTORY-WRITTEN probe.db-wal=1734552 probe.db=4096`). The two arms
   differ in the intended variable.
2. **The cipher is really applied.** A keyless read is refused in every encrypted cell
   (`file is not a database` through the factory; `Database is encrypted but no encryption options
   provided` for the vault) and succeeds in every plaintext control.
3. **The second process is real and its write lands.** Each child is this test binary re-executed
   (libtest prints `running 1 test` in its own output), and the child's row `100` / its vault hash
   is visible in the parent's reopen. In the inversion arm a live writer's exclusive lock refuses a
   second writable process — proof that the "second process" really is a different process holding
   a different handle.
4. **The failure channel works.** The same recorder reports the inversion-concurrent refusal
   verbatim; a silent failure is not being swallowed.
5. **The volume is real.** `-wal` = 1 734 552 bytes (store) and 379 072–403 792 bytes (vault), not
   a two-page fixture.

## Limits — what this does **not** establish

- **No simultaneous in-flight commits.** The protocol interleaves opens and alternating writes: the
  child's commit completes before the parent's next commit. Two transactions racing at the same
  instant are issue 45's collision matrix, not this gate.
- **No crash or power-loss arm.** Every close in these probes is clean (`WRITER-CLOSED clean`).
  A `SIGKILL` with an open transaction is measured for the flag-off path in
  `turso_capabilities.rs`, not here.
- **One filesystem.** The probe roots are under `/tmp`, which is **tmpfs on this host**
  (`environment.txt`). Locking and `fsync` semantics differ from the XFS `/home` filesystem real
  storage lives on. A filesystem-sensitive failure would not appear here.
- **One engine version, one platform.** turso/turso_core 0.8.3 on x86_64 Linux. Windows never gets
  the flag (`ConnectOptions::for_config`), so the cell does not exist there.
- **The ingest's error string is not reproduced or explained.** The archived record
  `Corrupt("database header magic mismatch")` does not match
  `shared_wal_coordination.rs:438-440` (`"shared WAL coordination header magic mismatch"`); this
  measurement neither confirms nor disproves that record — it simply does not observe any failure
  in the shapes above.
- The doc-vs-code divergence stands regardless: the engine **permits** the cell, the manual says
  page codecs do not support the flag. A pass does not make the manual say otherwise.

## Verdicts

```
VERDICT: G1 PASS
VERDICT: G2 PASS
VERDICT: G3 PASS
VERDICT: INVERSION PASS
VERDICT: G1B PASS (extension: clean-close backfill shape)
VERDICT: G1C PASS (extension: flag-off → flag-on upgrade, the nbackfills > 0 shape)
VERDICT: G3B PASS (extension control for G1B)
VERDICT: G3C PASS (extension control for G1C)
VERDICT: INVERSION-CONCURRENT NOT OBSERVED — the writable second opener is refused by the
         engine's single-process file lock ("File is locked by another process"), so the
         concurrent form cannot be measured without the flag; the arm is run sequentially.
```

## What this implies for O1–O4 (implication, not a ruling)

- **O1 (refuse at config resolution)** is not forced by any observed corruption. It would still
  make the vault unopenable on non-Windows unless the vault also changes — a consequence this
  measurement does not soften.
- **O2 (drop the flag when the key is on)** is *correctness-viable* per this measurement: the
  encrypted database is written, read by a second process, committed from both sides, reopened and
  `integrity_check`-clean with the flag off (INVERSION, G1C's prior writer). It costs the vault its
  `-tshm` coordination: a second writable vault process is refused by the exclusive lock while the
  first is live (`INVERSION-CONCURRENT`), rather than queued. Whether that cost is acceptable is an
  owner call, not a measurement.
- **O3 (keep it and document)** is consistent with these results: no harm was observed in the
  fresh, clean-close-backfill, or flag-off→flag-on upgrade shapes. The manual's exclusion means
  the documentation debt remains either way.
- **O4 (wait for 43's W6)** is neither advanced nor blocked. If W6 drops the flag engine-wide,
  G1C is the transition every existing encrypted database would take, and it passed here.

No option is recommended here; the issue's ruling step owns that.

## Reproduction

```sh
git switch issue-53/measurement
sh /home/vaz/Code/Personal/Rust/opencode-agents-memory/.tmp/scripts/cargo-serialized.sh \
  cargo test -p mem-store --test issue53_probe -- --ignored --nocapture --test-threads=1
sh /home/vaz/Code/Personal/Rust/opencode-agents-memory/.tmp/scripts/cargo-serialized.sh \
  cargo test -p mem-vault --test issue53_vault_probe -- --ignored --nocapture --test-threads=1
```

## Provenance

- **MEASURED (this directory):** every `PROBE … RESULT` line, every inventory figure, every error
  string quoted above, from `run-store.log` / `run-vault.log` on 2026-10-27 (UTC).
- **READ (issue 53 / source):** the option-set citations in the opening paragraph, the engine
  validator citations, and the manual claim — all quoted from the issue, which quotes the sources.
- **REPORTED (issue 53):** the archived ingest record and its string mismatch; not observed here.
