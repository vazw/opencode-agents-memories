# mem-vault — domain context

**Purpose:** the verbatim, encrypted, per-project vault tier: one key per scope
hash, one encrypted `vault.db` per scope, keyed content ids (`content_hash`),
a v1 schema, and a content write/read path (`put`/`get`) whose identity is
derived from the store's cached `KeyHex`.

Design authority: `docs/adr/0012-vault-key-custody.md` (D-1 … D-18). Encryption
of the global shards stays ADR-0004's, untouched (D-12).

## Ubiquitous language

- **Scope hash** — exactly 16 lowercase hex, produced by the container-tag path
  (`mem_core::tags` → `mem_core::scope`). It is an *address*, never a secret.
  There is no project→scope table and none may be added (D-2).
- **Vault key generation** — one 32-byte key per scope, rendered as 64
  lowercase hex plus one newline, stored at
  `{storage_root}/vault-keys/{scope_hash}.key`. Its lifetime is the
  generation's lifetime; `k_mac` is derived from it (D-7).
- **Marker** — `.vault-encrypted-v1` in the scope directory, one line
  `cipher=<name>`. It records which cipher wrote the file and is the future
  single commit point of a rekey (D-5, D-8).
- **`content_hash`** — the row identity: lowercase hex of
  `HMAC-SHA256(k_mac, content_bytes)`, 64 characters. Never `SHA256(content)`.
- **`k_mac`** — `HMAC-SHA256(key_file_bytes, "mem-vault/content-hash/v1")`, a
  domain-separated subkey: the key file's bytes are the codec's hex key, and
  cross-primitive key reuse is avoided by that one HMAC call (D-6).

## Invariants

- **One key per scope**, minted once. A key is never regenerated and never
  deleted by the library (D-10).
- **A lost or corrupt key is unrecoverable.** Over live ciphertext, a missing
  key is `KeyMissingForExistingData` and an invalid key is `InvalidKey`; the
  database is not opened in either case.
- **Fail-closed before any disk touch.** Scope validation, then the
  encryption-enabled refusal, then paths, then initialisation. A refused open
  creates neither `vault-keys/` nor `vault/`, and a second call cannot heal it
  (D-3). The check is load-bearing: `mem-store`'s resolver returns an explicit
  hex key even when encryption is disabled.
- **Keys live outside the data tree** (`vault-keys/`, not `vault/`), so
  deleting the data tree destroys ciphertext and leaves the keys — the
  recoverable direction (D-1).
- **Encrypted at rest is measured, not assumed**: a plaintext open of
  `vault.db` cannot read `vault_entries`.
- **The mint is atomic, and the branch the only mint site reports is pinned by
  a test.** A `create_new` temp + `hard_link` means a kill between create and
  write cannot leave a valid-looking short key that the never-regenerate rule
  would then refuse to fix. `ensure_key_material` returns `MintPath`, and
  `test_vault_mint_pins_the_shipped_link_path` asserts the shipped variant —
  an honest branch change fails the test; a rewrite that keeps the label over
  different syscalls is a diff-review obligation, not something a return value
  can witness.
- **One manager per scope.** A `ConnectionManager` carries one config, hence
  one key; sharing it would encrypt a fresh file with the wrong scope's key —
  silent and unrecoverable.
- **`content_hash` is byte-exact** over the content: no normalization, no
  whitespace collapsing, no case folding. The vault is L0.

## Content path (W8a)

`src/` now ships `put`/`get` over `vault_entries`. `VaultEntry` carries no
`content_hash` field; the store derives the id from its cached `KeyHex`
(ADR-0012 D-6). `tests/no_write_path.rs` is amended to
`test_vault_content_addressing_invariant` and checks the content-addressing
rules; the compile-fail doctest on `VaultEntry` is the stronger guarantee.

## Provenance (W8a r7)

`source_ref` is **required** (a `String`, never `Option`). Absence would make
a row permanently un-addressable — no purge constructs a ref matching `NULL`
and the tier is precisely where a user's secrets would live. The API enforces
presence, not truthfulness: a caller can pass `session:…` for content that
never came from a session. Content with no external referent uses
`VaultSourceKind::Manual` → `manual:<opaque>`, the honest escape hatch.
`mime_type` stays optional. The column stays nullable at the schema level;
a bypassed `NULL` degrades to `""` in `get`.

## Clock seam (W8a)

`created_at` is stamped by the store via `ids::now_millis()` (store-owned,
not injectable). Vault tests assert the store sets it, not a specific value.

## Rotation stance

Rotation is decided in ADR-0012 D-7/D-8 and implemented by **W8c**: the rekey,
`recover_rekey`, and the shared rekey/purge lock (D-18) ship in one change. W8b,
between W8a and W8c, is the purge erasure leg (D-9, D-11). A rekey rewrites every
`content_hash`, so ids are not stable across a rekey, and no reference outside
the vault may persist one. This crate ships no rotation API and nothing here may
be read as a second rotation design.

## Purge stance

The purge erasure leg is **not shipped yet** — W8b owns it (D-11, D-13 … D-16,
D-18). The gap is live: W8a's feeder writes vault content, so `purge.completed`'s
`not_erased` is **not** complete without a vault entry. Until W8b lands, a purge
leaves this tier's bytes in place and over-reports completeness; `mem-bridge`'s
purge is proved unable to touch these directories only because no erasure path
exists yet — that is the residual, not a guarantee.

## Accepted, documented, not mitigated

- **Identity drift:** a git project's scope hash derives from the canonical
  `.git` path (or the remote, or the directory), so moving a repository — or
  adding `.opencode-mem-project`, which flips `git-common:` to `path:` —
  changes the address. The bytes survive; discovery does not. Re-addressing is
  renaming two paths, not a rekey.
- **Enumerability:** `vault-keys/` enumerates every project's vault under one
  storage root. Containment is per key compromise, not per store access;
  filesystem permissions are the directory-level barrier.

## Boundaries

- May depend on `mem-core` and `mem-store` only (ADR-0001).
- The `mem-ai` privacy-filter ceiling (ADR-0009 D-7) and every consumer edge
  (`mem-bridge`, `mem-web`) stay **prose** until the phase that first declares
  them.
- Hot/cold tiering (ADR-0011), embeddings or any derived representation (the
  vault is L0), role enforcement and key export/import are out of scope.
