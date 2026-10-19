# ADR-0004: Database Encryption at Rest

**Status:** Accepted
**Date:** 2026-10-19
**Context source:** issue 08 (W1.5 — encryption at rest & operation locking, US 2.4) + roadmap §W1.5, Rev-2 §W1.5

## Context

Upstream `opencode-mem` stores every shard, `metadata.db` and the auxiliary dbs as
Turso/libSQL files, optionally encrypted at rest via libSQL's *experimental* encryption
(the `@tursodatabase/database` `experimental: ["encryption"]` flag +
`encryption: { cipher, hexkey }`). W1.5 ports that surface into `mem-store`:
key provisioning/storage (`encryption-key.ts`), the plaintext → encrypted migrator
(`encryption-migrator.ts`), and the cross-process, file-based operation lock with
stale-holder recovery (`operation-lock.ts`).

The load-bearing question for the port is whether the Rust `turso` crate can read
shards the upstream TypeScript binding wrote — an unproven assumption until this wave.

## Decision

### (a) Cipher: `aes256gcm`

Upstream's config default is `aes256gcm` (`DatabaseEncryptionCipher::Aes256Gcm`, the
`as_str()` rendering `"aes256gcm"`). The port keeps it as the default and as the
reference cipher for the read-compat contract. Other variants upstream exposes
(`aes128gcm`, `aegis256`, `aegis256x2`, `aegis128l`, `aegis128x2`, `aegis128x4`) remain
configurable — they are rendered through `DatabaseEncryptionCipher::as_str()` and never
hard-coded in `mem-store`.

### (b) Read-compat spike (the decision's evidence)

Recorded 2026-10-18T17:48Z in `/tmp/turso-probe` (Node binding + Rust turso, first-class
artifact):

- Wrote `/tmp/turso-probe/ts-encrypted.db` with the JS binding
  `@tursodatabase/database` — `experimental: ["encryption"]`,
  `cipher: "aes256gcm"`, a 64-hex key (`b1bb…6327`), then `CREATE TABLE t (x INTEGER)`
  + `INSERT INTO t VALUES (42)`.
- Rust **turso 0.8.3**:
  `Builder::new_local(...).experimental_encryption(true).with_encryption(EncryptionOpts { cipher: "aes256gcm", hexkey })`
  → `build().await` `Ok`, `connect()` `Ok`, `SELECT x FROM t` → `42`.
  **Read-compat: YES.**
- Negative controls: wrong cipher (`aegis256`) → `Decryption failed for page=1`;
  wrong key size (`aegis128l`, 32-byte key) → `Invalid key size`; plaintext open →
  `file is not a database` (proprietary codec confirmed — an encrypted shard is *not* a
  standard SQLite file).

**Conclusion:** upstream TS shards use `aes256gcm`; the Rust port reads them. The chosen
cipher is `aes256gcm`.

### (c) Key derivation & provisioned key format

- A key is **32 random bytes rendered as 64 lowercase hex chars** (AES-256). The
  validator also accepts 32 hex chars (AES-128) for compatibility:
  `is_valid_database_encryption_hex_key` = trimmed, all ASCII hex digits, length 32 or 64.
- Randomness comes from **`/dev/urandom`** (32 bytes, `read_exact`). There is **no
  fallback to a weak PRNG** — a read failure is a hard `Err`. Platform assumption
  (documented): Linux. `getrandom`/`rand` was deliberately not added (house law: no new
  dependency for one call site).
- No KDF is applied: the configured/provisioned key *is* the libSQL `hexkey` (upstream
  semantics). There is no passphrase path in the upstream contract.

### (d) Key storage

- Default key file: **`~/.config/opencode/opencode-mem-db.key`**
  (`DEFAULT_DATABASE_ENCRYPTION_KEY_PATH`, resolved at runtime from `HOME` /
  `USERPROFILE` / `HOMEDRIVE`+`HOMEPATH` — no `dirs` dependency).
- File mode **`0600`** on unix (set at `create_new(true)` open via
  `OpenOptionsExt::mode(0o600)`, then a best-effort `chmod`); the parent directory is
  created best-effort with mode **`0700`**.
- Contents: `<64-hex>\n`.
- Creation uses `create_new(true)` (`wx`) so a racing writer is never clobbered.
- **Fail-closed:** `generate` refuses when the encryption marker
  (`.tursodb-encrypted-v1`) already exists under the storage root — a lost key means
  unrecoverable ciphertext, so the port never silently mints a fresh one over encrypted
  data.
- The hexkey is **never logged and never appears in an error**; `tracing` carries the
  cipher and the path only.

### (e) Key resolution chain (upstream parity)

`resolve_or_create_database_encryption_key(config)`:

1. disabled **and** no raw key → `None`;
2. raw value that is valid hex and contains no `://` → the explicit hex key (returned
   even when encryption is disabled — upstream parity);
3. `env://VAR` → `std::env::var(VAR)`, trimmed, validated (else `Err`); no secret-resolver
   indirection is ported (the `secret-resolver` seam is not part of this wave's scope);
4. `file://path` → ensure (generate-once) the key file at the expanded path;
5. a bare value containing `/` or starting `~` → treated as a file path (expanded);
6. any other raw value → `Err` ("must be env://VAR, file://path, or a 32/64-char hex
   string");
7. enabled with no raw key → ensure the **default** key file (auto-create once).

Recorded divergence: upstream reads the process-global `CONFIG`; the port passes the
value-typed `StoreConfig` and threads `storage_path` explicitly, so
`ensure_database_encryption_key_file` takes a `storage_path` argument for the marker
check.

### (f) Experimental-encryption flag: KEEP IT ON

The turso `experimental_encryption(true)` flag is **retained**. Rationale: (i) read-compat
is proven both directions of the format for `aes256gcm`; (ii) the flag gates only the
codec, not correctness of the surrounding storage engine — the W1.4 test baseline stays
green with it on; (iii) turning it off would make every existing encrypted shard
unreadable. It also means an encrypted shard is **not** a standard SQLite file — a
plaintext tool (e.g. `sqlite3`) cannot read it; this is documented and expected.

### (g) Alternative D3 (costed, NOT taken) — SQLCipher via `rusqlite`

If turso's experimental encryption ever breaks or is withdrawn, the fallback is
`rusqlite` + a bundled **SQLCipher** + `sqlite-vec`. Cost estimate: this is not a
drop-in swap — it re-implements the entire `Db` seam (`mem-store/src/db.rs`) on
`rusqlite::Connection`, rewrites every statement in `src/sql.rs` that relies on Turso
extensions (`vector32`, `vector_extract`, `vector_distance_cos`, `F32_BLOB`), replaces
`experimental_multiprocess_wal` with SQLCipher's own locking model, and re-validates all
migration/legacy paths. It is recorded here as the **switch trigger + cost**, not as
work in this wave; the switch is only justified if the experimental codec fails.

### (h) Operation lock stays a PID-file lock (no OS advisory locks)

The cross-process operation lock is `.turso-operation.lock` (and the legacy
`.turso-migrate.lock`), written atomically with `create_new(true)`, `sync_all`'d
best-effort; a corrupt/empty file is unlinked as stale; a **live** holder is never stolen;
release is PID-guarded; stale recovery uses PID liveness (`/proc/<pid>` on Linux, a
conservative "alive" verdict elsewhere). OS advisory locks (`flock`/`fcntl`) were assessed
and **rejected**: upstream's semantics are stale-owner recovery via `process.kill(pid, 0)`,
which advisory locks do not reproduce and which add a different failure mode (stale handle
after a crash). std `fs` suffices; `fd-lock`/`fs4` are not needed. This is the artifact
W3.1's profile-learning lock builds on.

## Rejected alternatives

- **`randomBytes` parity via a new crypto crate** — one call site; `/dev/urandom` read
  with `read_exact` needs no dependency and never weakens the key.
- **Silent key regeneration when the marker exists** — irreversible data loss; fail-closed
  is the only safe behaviour.
- **OS advisory file locks (`flock`/`fcntl`) for the operation lock** — see (h).
- **D3 SQLCipher now** — costed in (g); only on experimental-codec failure.

## Consequences

- `mem-store` gains `encryption_key` + `encryption_migrator`; `db::resolve_database_encryption`
  resolves/provisions keys (no longer just rejects enabled-without-key).
- The ready gate composes `legacy-migration → encryption-migration → path-migrate-recovery
  → shard-warmup` via `build_ready_gate_with_encryption`; `build_ready_gate_with` stays for
  W1.3 compat and unencrypted defaults.
- The cross-process **operation lock** hardening (short-write `sync_all`, corrupt/empty
  unlink) preserves the W1.3 legacy-lock code paths unchanged.
- `mem-store` must not depend on `mem-embed` (unchanged; guarded by the step-8 tree check).
