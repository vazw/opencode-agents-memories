# 0012 — Vault Key Custody and Rotation

- **Status:** Accepted (owner acceptance, 2026-10-23)
- **Date:** 2026-10-23 (UTC 2026-10-22) — the decision records of this session are dated 2026-10-23 while the UTC clock reads 2026-10-22T20:4x with identical local time; both readings are stated so neither is a lie.
- **Wave:** W7.2 — `mem-vault`. This ADR is the **design authority for that wave**: the crate ships key custody, the fail-closed open, keyed content ids and a v1 schema, with **no content write path** and **no rotation API**.
- **Supersedes:** nothing — no earlier decision is revoked. **Amends:** **this file** (W8 amendment, 2026-10-23): D-5, D-7, D-8, D-9, D-10 and D-11 are amended in place and D-13 … D-18 are added; the itemised scope, and every decision left untouched, is under *Amendments* below. ADR-0004 stays authoritative for global shard encryption and is untouched. ADR-0009's Tier-4 sharing cell was amended to per-project scope on the same date, by the same owner ruling, in ADR-0009's own W7.2 amendment note.
- **Source:** owner rulings — vault key = option C (per-project key files); rotation decided here rather than deferred; ADR-0009's vault scope corrected to per-project. Evidence and standing rules: ADR-0004 (encryption at rest; **zero** rotation clauses), ADR-0010 D-1/D-3/D-4/D-6 (Sound Memory: verbatim vault, no compression, `source_ref`, "NOT vault"), ADR-0009 D-1/D-3/D-7 (tier 4, cross-tier promotion, privacy), and W7.1's owner-accepted carve-out (M23/D7), which recorded option C as binding on this wave.
- **Amended (2026-10-23):** the W8 amendment below — ten owner rulings (Q1 … Q10) from the vault grilling session, recorded against the W8b/W8c split. Acceptance is unchanged; the amendment narrows, re-binds and adds, and nothing is revoked.

## Amendments

**W8 amendment (2026-10-23; UTC 2026-10-22).** Ten owner rulings from the
grilling session on the vault's remaining waves, recorded against a wave re-split that
happened after acceptance. The ADR stays **Accepted**: acceptance is not re-opened, no
decision below is revoked, and every amendment narrows, re-binds or adds. The re-split is
the premise of all of it — **W8b is the purge erasure leg** (D-11, D-13 … D-16) and
**W8c is the rekey, `recover_rekey` and the shared lock** (D-5's marker extension, D-7,
D-8, D-9, D-10, D-18). Rationale (Q9): the purge's erasure-truth verification must not
sit behind the rekey's crash-safety complexity.

| Decision | What this amendment does | Ruling |
|---|---|---|
| D-5 | amended in place: "until a writer exists … the extension is free" is corrected — W8a's feeder writes content, so W8c's generation field must read the vaults that already exist | — (truth correction) |
| D-7 | amended in place: (c) replaces "offline" with an enforced exclusive lock; new (e) mount gate; new (f) automatic deletion of a superseded generation; the "nothing does" parenthetical is corrected | Q3, Q10 |
| D-8 | amended in place: step 3 deletes only after validation, and automatically; the durability caveat becomes a precondition through D-7(e); `recover_rekey` is W8c | Q10 |
| D-9 | re-bound: rekey ships in **W8c**, not W8b; the no-deployment rule now spans two waves | Q9 |
| D-10 | **narrowed**: the operator-only deletion rule now covers only a key that is the sole generation able to open live ciphertext | Q10 |
| D-11 | re-bound and extended: the leg ships in W8b; the erasure gap W8a opened is named; the leg is specified by D-13 … D-16 and D-18 | Q1, Q2, Q7, Q8 |
| D-13 | added: erasure proof — counts **plus** a read-back inside the same lock | Q1 |
| D-14 | added: "erased" means logical deletion, with its two residuals | Q2 |
| D-15 | added: derive the scope by default, keep the explicit list as an override, report unnamed scopes | Q4, Q6 |
| D-16 | added: the session ref is the only ref erased; exhaustive `match`; `Manual` not purgable | Q7 |
| D-17 | added: `source_ref` required — landed in `55f16da`; presence, not truthfulness | Q5 |
| D-18 | added: one shared lock; a purge refuses a rekey because a race can resurrect erased content | Q8 |

**Already true, and cited rather than restated:** the text already says three of the
things these rulings require. D-8's durability caveat names the mount classes
D-7(e) refuses on; D-2 already forbids the `project_id → scope_hash` map that D-15 does
without; D-11 already binds the purge leg to W8b. Those sentences keep their wording;
where a ruling agrees with the ADR, the amendment says so instead of claiming a change.

**Untouched**, and this is a scope statement rather than an illustration: the `Status:`
and `Date:` lines; the `Source:` bullet's own text; the `Wave:` header line, which is
scoped to W7.2 by its own words ("the design authority for that wave") and is not a claim
about the tree today; D-1 … D-4, D-6 and D-12 verbatim (D-5 changes in one sentence);
the Context section except its third paragraph (the rotation wave); the **Rejected
alternatives** section (each added decision carries its own `Rejected:` paragraph); the
Consequences positive list except the erasure bullet and the bullet appended after it; the
Consequences negative list except the rotation bullet, which is replaced, and the two
bullets appended after it; the Consequences **Neutral / forward** subsection except its
third bullet, which no longer claims the crate has no consumer; and every References entry
except the one appended below.

## Context

The five-tier model (ADR-0009) gives the vault tier the one job no other tier can do:
hold verbatim bytes whose exact wording matters (ADR-0010 D-3: Vault = **L0**, no
compression; D-6: "NOT vault"). ADR-0004 already decided how the port encrypts a
database at rest — `aes256gcm` through libSQL's experimental codec, a 32-byte key
rendered as 64 hex, `0600` key files, fail-closed against key regeneration — but it
decided that for the **global** encrypted store, with **one** key per storage root,
and it says **nothing about rotation** (the document contains no rotation clause at
all; this was verified, not assumed).

Option C changes the key's blast radius: one key per memory scope, in
`{storage_root}/vault-keys/{scope_hash}.key`, rather than one key per storage root.
That decision was taken by the owner for containment — a leaked key must expose one
project's vault, not every project's — and it forces three consequences this ADR has
to own:

1. **A per-scope key can only open a per-scope file.** libSQL's encryption is a
   whole-file page codec with one key per file, so the vault database must live at
   `{storage_root}/vault/{scope_hash}/vault.db`.
2. **Rotation becomes thinkable.** ADR-0004's single global key made rotation a
   re-encryption of *everything*; a per-scope file makes it a re-encryption of one
   directory. Leaving "unsolved" in place would waste the only property option C was
   chosen for, so this ADR decides rotation instead of deferring it again.
3. **Ids must not be plaintext digests.** A `SHA256(content)` id in a file that may
   leak is an offline confirmation oracle for low-entropy secrets (an API key, a
   password inside a snippet): anyone holding the database can test guesses. The id
   is keyed (Q2b). Rotation and keyed ids then collide — see D-7, the decision this
   ADR exists to make.

W7.2 itself writes no vault content. Every rule below therefore protects the
*contract* before the first secret byte exists; the API that exercises it (rotation,
D-9) belongs to W8c, two waves after the first content writer (W8a), and the erasure path
D-11 owes ships in the wave between them (W8b).

## Decision

### D-1 — Custody layout: keys outside the data tree, one directory per scope

| Object | Path | Mode |
|---|---|---|
| scope key | `{storage_root}/vault-keys/{scope_hash}.key` | `0600` file, `0700` directory |
| vault database | `{storage_root}/vault/{scope_hash}/vault.db` | `0700` directory |
| encrypted marker | `{storage_root}/vault/{scope_hash}/.vault-encrypted-v1` | `0600`-class, content in D-5 |

Keys live **outside** `vault/` so that the one destructive command an operator might
reach for — `rm -rf {storage_root}/vault` — destroys ciphertext and leaves the keys,
which is the recoverable direction. The opposite mistake (`rm -rf
{storage_root}/vault-keys`) leaves unrecoverable ciphertext and is refused by D-3's
key rule; that is the accepted cost of the asymmetry. The marker lives **inside** the
scope directory because it describes that directory's file.

**Rejected:** keys inside `vault/{scope_hash}/` — the directory an operator deletes to
reset one project would also destroy its key, converting a recoverable mistake into
permanent loss.

### D-2 — Scope derivation: the container-tag path only, with its consequences

`scope_hash` is produced by exactly one producer in the tree:
`mem_core::tags::TagResolver` → `TagInfo.tag = "{prefix}_{project|user}_{16hex}"` →
`mem_core::scope::extract_scope_from_container_tag` → `ScopeRef { hash: Some(ScopeHash) }`,
validated by `is_valid_scope_hash` (exactly 16 lowercase hex). The 16 hex is the first
16 hex characters of SHA-256 over a project identity — in order `git-common:{canonical
.git path}`, `remote:{origin url}`, `path:{directory}` — or over a user identity
(email, else name, else `$USER`/`USERNAME`, else `anonymous`).

**There is no project→scope_hash table and this ADR forbids adding one.** The tag
resolver is the single source of truth; a stored mapping would be a second one that
drifts the moment a repository moves.

Four consequences are accepted, not hidden:

1. **The vault address is path-derived.** For an ordinary git checkout the identity is
   the *canonical `.git` path*, so moving a project changes its `scope_hash`, hence its
   key name and its vault directory. The bytes survive — old key and old file remain
   under the storage root — but nothing derives the old hash, so *discovery* is lost.
   Re-addressing is a rename of two paths, not a cryptographic operation; it is out of
   scope here and must not be smuggled into a rekey (D-7 is about keys, not names).
2. **`scope_hash` is a 64-bit prefix, not a secret.** Anyone who can guess a project
   path or remote can compute it and therefore *locate* that project's key file and
   vault directory. Confidentiality rests on the 256-bit key, never on the address.
3. **Because the storage root is shared, `vault-keys/` enumerates every project's
   vault.** The containment option C buys is per *key compromise*, not per *store
   access*; filesystem permissions are the only barrier at the directory level.
4. **Adding the `.opencode-mem-project` marker flips a project's identity** from
   `git-common:` to `path:`, which changes its `scope_hash` exactly as a move does.

**Rejected:** deriving the key path from `project_id` — that is the episodic key space,
not the scope-hash space, and mixing them would make two namespaces look
interchangeable.

### D-3 — Fail-closed open order

`ready(scope)` performs, in this order:

1. **validate** the scope hash (`InvalidScope`, fixed message; a rejected value is not
   echoed because it may be arbitrarily long) — before any path join;
2. **refuse when encryption is disabled** (`EncryptionRequired`) — **before** any
   `vault-keys`/`vault` directory, any key read, any `ConnectionManager`, and before the
   per-scope map is touched, so a refused open leaves no trace on disk;
3. resolve `key_path`, `db_path`, `marker_path` (each validated again, then contained
   under the storage root);
4. initialise the scope once: key material (D-4), marker (D-5), encrypted open;
5. apply the v1 schema.

**Why step 2 is load-bearing and cannot be delegated.** `resolve_or_create_database_encryption_key`
returns an explicit hex key **even when encryption is disabled**, and
`resolve_database_encryption` turns any resolvable key into encryption options
**regardless of the `enabled` flag**. A vault that merely injected its key and trusted
the resolver would happily create an *encrypted* file in a configuration the operator
believes is plaintext-off. The explicit check is the only thing standing between
"fail-closed" and "silently encrypted".

**Rejected:** checking `enabled` inside the open path — the refusal must precede every
disk touch, and a test must be able to assert that no directory was created.

### D-4 — Key material: 32 bytes, 64 lowercase hex, minted atomically, never regenerated

- **Format.** `getrandom::fill` supplies 32 bytes, rendered as 64 **lowercase** hex
  plus one `\n`. Validation on read = exactly 64 characters, all `0-9a-f`, one optional
  trailing newline. This is deliberately **stricter than
  `is_valid_database_encryption_hex_key`**, which also accepts 32 hex characters
  (AES-128) and uppercase; a 128-bit vault key would silently weaken the one tier that
  holds verbatim secrets.
- **Mint is atomic.** `create_new(true)` + `write` is not: a kill between create and
  write leaves a zero-length `.key`, which read validation rejects as `InvalidKey`, and
  the never-regenerate rule (D-10) then forbids repairing it — a self-inflicted permanent
  lockout. The mint writes `{scope_hash}.key.tmp-{pid}` (`create_new`, `0600`,
  `sync_all`), then `std::fs::hard_link`s it to the final name (fails with
  `AlreadyExists` rather than clobbering a racing winner), then unlinks the temporary
  file. Where `hard_link` is unsupported, the documented fallback is `create_new` +
  write + `sync_all` + unlink-on-error, and the test must pin which path shipped.
- **Never regenerated.** An existing file that fails validation is `InvalidKey` (path
  only, never content) and the database is not opened.
- **Randomness.** `getrandom`, not `/dev/urandom`. ADR-0004 (c) chose `/dev/urandom`
  specifically *to avoid adding a dependency*; that premise no longer holds
  (`getrandom` is already a workspace dependency and is already in `Cargo.lock`), and a
  portable API beats a Linux assumption. The divergence from `mem-store`'s sibling
  generator is intentional and is recorded in the crate's module header.
- **`0600` at open, `0700` on both directories**; the directory mode is best-effort
  (same caveat ADR-0004 (d) records), the file mode is set at creation.

### D-5 — Marker contract: the cipher is recorded, and a mismatch fails before the open

`.vault-encrypted-v1` contains exactly one line, `cipher=<DatabaseEncryptionCipher::as_str()>\n`;
it is created `create_new` with mode `0600` (it names a cipher, it is not secret, but it is not world-readable either).
It is written **before** the first database open, and on every later open it is read and
compared with the cipher the store is about to hand the codec. A mismatch or an
unparsable marker is `CipherMismatch` / `MarkerInvalid` **before** the file is opened.

Why: `databaseEncryptionCipher` is a user-editable configuration value and ADR-0004 (a)
keeps six alternatives configurable. Without a record, switching `aes256gcm → aegis256`
produces an opaque `Decryption failed for page=1` from inside the codec, on a file that
is perfectly intact. With it, the operator is told which cipher the vault was written
with. **No cipher is forced** — the vault inherits the configured cipher and pins it at
creation, which leaves ADR-0004 (a) intact.

The marker is also the future **commit point** of a rekey (D-8): a v1 marker means
generation 1, and the rekey wave extends the format with a generation field. When this was
written no writer existed, and the extension was free; W8a's feeder (D-17) now writes
vault content, so the extension is no longer free. A v1 marker keeps meaning
generation 1, and W8c's extension must read the vaults that already exist rather than
assume an empty tier. No vault created by W7.2 can need the field.

**Rejected:** forcing `aes256gcm` for the vault regardless of configuration (removes a
documented user surface and hard-codes a cipher outside `mem-store`); an existence-only
marker (surfaces drift as an opaque codec error); a bare `key_ref` column in the schema
instead of a file-level record (a per-row generation label cannot mean anything under a
whole-file codec).

### D-6 — Keyed content ids: HMAC-SHA256 under a domain-separated subkey

`content_hash` — the vault row's identity — is:

```
k_mac        = HMAC-SHA256(key_file_bytes, "mem-vault/content-hash/v1")
content_hash = lowercase_hex( HMAC-SHA256(k_mac, content_bytes) )
```

It is never `SHA256(content)`, and it is never the AES key used directly as a MAC key:
the key file's bytes are the libSQL hexkey, and cross-primitive key reuse is avoided by
one HMAC call with a fixed label (HKDF-Expand's single-block case). Content is hashed
byte-exactly — no normalization, no whitespace collapsing (the tier is L0).

**ADR-0004 (c)'s "no KDF" rule is preserved**: the key file's bytes are passed to the
codec unchanged; the derivation applies only to the MAC key and never to the encryption
key.

The primitive is **vendored**: `Cargo.lock` contains no `hmac` and no `sha2`, so
`mem-vault` implements RFC 2104 over the existing, FIPS-vector-pinned
`mem_core::sha256::digest` and pins itself to the RFC 4231 HMAC-SHA-256 vectors. No
dependency is added. One prose correction follows: `crates/mem-core/src/sha256.rs`'s
header currently states the module has "no keyed/secret use, no side-channel surface";
once an HMAC is built on it that sentence is false and must say so instead.

Scope: **vault only.** `facts.id` and segment ids are digests over non-secret keys and
identities, not over content; this rule's confirmation-oracle argument does not reach
them and they are deliberately unchanged.

**Rejected:** `hmac` + `sha2` as new dependencies (two direct dependencies plus their
trees, for ~25 lines and a construction the RFC pins exactly — against this port's
precedent of vendoring a hash and pinning it to standard vectors); `SHA256(key ‖ content)`
(length-extension and a hand-rolled construction); a random per-row id with a separate
unkeyed content digest (keeps the oracle).

### D-7 — Rotation: a rekey rewrites every `content_hash`, and the old key is destroyed

**The decision.** Rotation of a vault scope's key is a **full rekey of one scope
directory**: every row is read under the old key, its `content_hash` is **recomputed**
under the new key's MAC subkey, and the whole database is rewritten encrypted with the
new key. After the commit the old key is **destroyed**. A rekey is therefore:

- (a) **the MAC key's lifetime is the key generation's lifetime** — `k_mac` is derived
  from the key file (D-6), so a new key file means a new MAC key, and no generation's
  MAC key outlives its generation. There is no "keep the old MAC key forever" mode;
- (b) **a full-table rewrite, not an in-place header change.** The codec has no
  re-encrypt-in-place facility, and the ids change, so every row is rewritten;
- (c) **an owner-initiated operation that excludes writers by enforcement, not by
  convention**, and not a background task: it needs both keys resident simultaneously.
  "Offline" was the wrong word — nothing in the tree enforced it: no lock covers the
  vault's rekey (the `mem-store` operation lock exists but neither the feeder nor a
  rekey takes it, and no `flock` call exists in any crate — ADR-0004 names `flock` only
  in prose), `MultiprocessWal` is enabled on every platform except Windows
  (`crates/mem-store/src/db.rs:255-257`), so two processes can hold the same database at
  once, and W8a added an asynchronous writer that can put at any time. The rekey therefore
  takes the scope's **exclusive lock** (D-18); a writer that finds it held drops the write
  fail-closed and counts the drop, in the shape the feeder already uses for
  `EncryptionRequired`;
- (d) **identity-breaking by construction**: `content_hash` is not stable across a
  rekey. Nothing outside a vault may persist a vault `content_hash` as a durable
  reference. References *into* the vault must be by `source_ref` (which points out of
  the vault at episodic/facts ids and is unaffected), or by a surrogate the writer wave
  adds if it needs one. `source_ref` is shipped and indexed in v1 precisely so that
  this constraint costs nothing later. W8a ships the first producer (the session-end
  feeder) and therefore the first opportunity for an external holder to appear; the
  contract forbids it, and a repo-level test guards it;
- (e) **gated on the mount**: the rekey refuses to start unless the filesystem holding
  the scope can honour D-8's recipe (`fsync` of the file, a rename, `fsync` of the
  directory). No single source sees all of the classes, so the gate reads one source per
  class: the filesystem **type** from `statfs`'s `f_type` (the network classes —
  `NFS_SUPER_MAGIC`, `CIFS_SUPER_MAGIC`, `FUSE_SUPER_MAGIC` — and
  `OVERLAYFS_SUPER_MAGIC` are visible there); `data=writeback` and an OverlayFS
  `fsync=strict` override from the mount table (`/proc/self/mountinfo` on Linux), which
  records only what was set explicitly — an overlay option it does not record is the
  `fsync=auto` default, so "not listed" is not "strict"; and a volatile write cache from
  the block device's `queue/write_cache` attribute
  (`/sys/block/*/queue/write_cache`), which is not a mount option at all.
  Neither `statfs` nor `statvfs` exposes the filesystem-specific option strings this gate
  must read (`data=writeback`, an overlay `fsync=`); `f_flag` carries only the generic
  `ST_*` flags — so a gate built from `statvfs` alone would claim a check it cannot
  perform, which is the class of gate D-16 refuses.
  A refusal is typed and names the filesystem, the class it
  refused and the source it read, and W8c pins the refuse list (D-8's caveat names the
  same classes) with a test per class. This port targets Linux, where
  `/proc/self/mountinfo` and the queue attribute exist; where a class's source is
  unavailable, the refusal names the class it could not evaluate rather than letting a
  pass imply a check that did not run;
- (f) **the superseded generation is deleted automatically, after validation** — not
  remembered by an operator, not left behind "just in case" (D-8 step 3, D-10). A rekey
  that leaves the old key on disk has not rotated anything, and disposability of the old
  key is the whole point.

**Why not the alternative.** Keeping the old MAC key alive so ids survive would mean
the rekey retains the very secret whose compromise motivated it — a rotation that keeps
the old key is not a rotation. It is also self-defeating: re-inserting content that
already exists after a rekey would produce either a duplicate row (same content, new id,
integrity gone) or a need to recompute the old id (old key retained — the same circle).
Rewriting is the only option that makes the old key disposable, and disposability is
the whole point.

**Consequence accepted:** because ids change, a rekey is visible to anything that cached
a `content_hash`. Nothing outside the vault caches one today — W8a's feeder calls `put`
and keeps no id, and the store's own cache holds key material, not content hashes — and
the writer wave inherits the rule in (d).

**Rejected:** per-row key generations (impossible under a whole-file codec); an
envelope key that never changes with only the encryption subkey rotating (a single
32-byte key file cannot hold two independent generations, and changing the file changes
both derivations — the owner's key format makes this option unavailable rather than
merely unattractive); deferring rotation again (the reason option C was chosen was that
rotation becomes tractable; deferring leaves a leaked key with no remedy but deletion).

### D-8 — Rekey protocol invariant: the marker is the single commit point

This ADR does not specify a byte-level procedure; it specifies the invariant a future
implementation must satisfy, because the implementation belongs to W8c (D-9).

1. A rekey produces **generation n+1** beside generation n: the new database is written
   under a temporary name, `sync_all`'d, and its new key file is written beside the old
   one (D-4's atomic mint). Nothing at the canonical paths changes yet.
2. **The commit is a single marker replacement**, not a file swap: the marker (naming
   generation and cipher, D-5's extension) is written to a temporary file, `sync_all`'d,
   and **renamed** over the old marker; **then the parent directory is `fsync`'d.**
   Before the rename, generation n is authoritative; after it, generation n+1 is.
3. Only **after** the commit, and only after generation n+1 has been **validated** (the
   marker names it, its key opens it, a read of the table succeeds), are the previous
   generation's key and database deleted — automatically, by the rekey, not by an
   operator (D-10). A residue (`.prev`) is therefore the only acceptable extra file, it
   is reported rather than silently trusted, and its existence means "a rekey was
   interrupted".
4. Open reads the marker, then the generation it names. An interrupted rekey is
   recovered from the marker — never by guessing which file "looks newer".

**Durability caveat (the correction this amendment makes).** POSIX `rename(2)` guarantees
atomicity, not durability: POSIX makes no crash guarantee for `rename()` at all, and a
rename is not durable until the parent directory is `fsync`'d (`fsync(file)` alone does
not persist the directory entry). The portable minimum recipe is
`write → fsync(file) → rename → fsync(parent dir)`
(POSIX/kernel convention, not a Ferrite finding). It is not guaranteed on every
filesystem: `data=writeback` mounts, network filesystems, volatile write caches, and
OverlayFS in `volatile` mode give no crash safety (OverlayFS defaults to `fsync=auto`,
not `strict`). Per Bornholt et al.,
"Specifying and Checking File System Crash-Consistency Models" (Ferrite, ASPLOS 2016,
§3.5), among the filesystems tested only btrfs provided the atomic replace-via-rename
(ARVR) guarantee; ext4's `auto_da_alloc` (per the ext4 documentation the paper cites
as ref [42]) forces out the new data blocks on rename but rename does not wait for that
flush, so ext4 provides no atomicity guarantee and a crash can leave partial new
content. Recovery is marker-driven, never mtime/name-driven: an interrupted rekey leaves
generation n authoritative (or, at worst, a `.prev` residue reported rather than
silently trusted) — **no moment in which the key on disk cannot open the database on
disk and the old key is already gone.** `ready()` does not auto-recover; it returns
`RekeyInterrupted` on residue, and recovery is an explicit `recover_rekey` call (W8c).
D-7(e) turns this caveat into a precondition: a rekey does not attempt the recipe on a
mount that cannot honour it.

The one thing this invariant exists to prevent: a moment in which the key on disk cannot
open the database on disk and the old key is already gone. That is unrecoverable, and it
is the failure mode a naive "write the key, then rewrite the file" sequence produces.

**Rejected:** a database-first swap (the new file is readable only with a key that is not
yet in place — the same window, mirrored); a journal file besides the marker (a second
commit point is a second way to disagree); trusting filename ordering/mtime (not a
commit protocol).

### D-9 — Rotation API ownership: rekey ships in W8c, after the purge leg in W8b

W7.2 ships **no** rotation API, no rekey CLI, no export/import of key material. Rewriting
`content_hash` is a content write path, and this wave's binding rule is that it has none.

**The rotation API must land within the sequence that opens vault content, and the
window it leaves is named.** Under the owner's resequencing the content write path ships
in W8a; the split (Q9) puts the purge erasure leg in W8b and rekey — with
`recover_rekey` and the shared lock — in W8c. No production deployment may occur until
**both** have landed: W8b because a vault that cannot be erased is not deployable
(D-11), W8c because the only remediation for a leaked key without rekey is deleting data,
the opposite of what the vault exists for. W8a ships no rotation API. The window of
unrotatable secrets is therefore two waves wide, not one, and this paragraph is where
that cost is recorded: it was accepted so that the erasure promise's verification does
not wait on the rekey's crash-safety work. This is a constraint on those waves, not a
deferral of the design: the design is D-7/D-8.

### D-10 — A lost or corrupt key is unrecoverable, and is never regenerated

Over **existing** encrypted data, a missing key is `KeyMissingForExistingData` and a
present-but-invalid key is `InvalidKey` (D-4). Neither is repaired, and neither may be
"fixed" by deleting the file: a key that fail-closes an open is never deleted by the
library, and deleting **the only generation able to open live ciphertext** stays an
operator action, because that is the act that converts recoverable ciphertext into
permanent loss. A vault whose only key generation is lost is unrecoverable by design; the
price of blast-radius containment is exactly this, and it must be documented rather than
discovered.

That rule is about a key in its **only** generation, and rotation narrows it. D-7 has
said since it was written that "After the commit the old key is **destroyed**." What
stood in the way was this decision's absolute sentence, not D-7's. D-8 step 3 now
requires the rekey to delete a **superseded** generation automatically, once generation
n+1 is committed and validated. That is a different object in a different state — its
ciphertext has been rewritten under n+1 — and deleting it is what makes the old key
disposable (D-7). So after a *completed* rekey the previous generation is gone because
D-8 step 3 removed it, not because an operator remembered to; a key in the only surviving
generation is still never deleted by the library, and a lost post-rotation key is
likewise unrecoverable.

**Rejected:** regenerate-when-no-marker (mints a second key over live ciphertext as soon
as a marker is deleted); regenerate-when-the-file-is-empty (indistinguishable, without
the marker, from a truncated key whose ciphertext is intact).

### D-11 — Purge: a vault writer owes the purge an erasure path

ADR-0010 D-1, as amended in effect by W7.1, is the authoritative sentence: *raw context
is never destroyed by consolidation or by any lifecycle stage; it is destroyed only by an
explicit, owner-initiated purge (`purge_session`).* The vault is a lifecycle **target**,
not a lifecycle **stage** (ADR-0009 D-3 promotes "any tier → vault"), so it does not
license destruction — and it does not exempt itself from erasure either.

Decision: **a vault writer owes `purge_session` an erasure path.** Under the owner's
resequencing the erase path ships in W8b (the purge erasure leg), testable against real
rows produced by W8a's feeder; W8a's feeder follows the `source_ref` contract (an opaque
tagged string constructed at a single site in mem-bridge) so that W8b's leg is
implementable without a rekey or row rewrite. `purge_session` promises that a session's
content is erased; a vault
copy that survives would make "erased" false for the most sensitive tier. The vault's
immutability binds **overwrite** (an entry is never mutated in place) and **identity**
(the id is the content's keyed digest), not **erasure**: because ids are not references
to other rows, deleting one row changes no other row's hash, so erasure is compatible
with content addressing. The leg itself is specified by D-13 (what it must prove),
D-14 (what "erased" promises), D-15 (which scopes it addresses), D-16 (which refs it can
erase) and D-18 (the lock it shares with a rekey).

W8a has landed, so the premise of the sentence this paragraph replaces is gone: the
vault now holds content for any session that marked it, and `purge.completed` is **no
longer** complete or true without naming the vault. The choice this decision gave the
writer wave — (i) implement the vault leg, or (ii) deliberately decline and add a vault
entry to `not_erased` — was not taken in W8a, so the gap is live: a purge run today
erases no vault rows and does not say so, and `not_erased` names only the memories
shard. W8b's leg is (i). Until it lands, the only honest reading of a `purge.completed`
event is "episodic and facts, not the vault", and no production deployment may precede
it (D-9). v1 ships and indexes `source_ref` so that (i) stays implementable, and W8a's
`55f16da` made it mandatory (D-17).

**Rejected:** treating "immutable" as "not erasable" (it would defeat the one erasure
promise the system makes); a vault exempt from purge but named in `not_erased` forever
(honest, but worse than erasing — the vault is precisely where a user's secrets would
live).

### D-12 — Relationship to ADR-0004: scope split, no supersession

ADR-0004 is **Accepted** and says **nothing** about rotation. This ADR therefore
**does not amend and does not supersede it**, and a reader must keep both:

| Question | Authority |
|---|---|
| cipher choice, read-compat, experimental-encryption flag | ADR-0004 (a)(b)(f) — unchanged |
| global key provisioning/resolution chain, `0600`/`0700`, fail-closed against regeneration, no KDF for the hexkey | ADR-0004 (c)(d)(e) — unchanged and restated by D-4/D-6 |
| the operation lock | ADR-0004 (h) — unchanged |
| vault key custody, scope derivation, keyed ids, rotation, vault purge obligation | **this ADR** |

"ADR-0004 has no rotation clause" means rotation was **out of 0004's scope**, not that
0004 forbids it. The earlier formulation "rotation is unsolved until an ADR supersedes
ADR-0004" used *supersede* loosely: a supersession would revoke 0004's Accepted cipher
and read-compat decisions, which nothing here intends. This ADR fills the gap for the
vault scope; rotation for the **global** store remains undecided and is not implied by
anything here.

### D-13 — Purge erasure proof: counts plus a read-back, inside the same lock

The vault leg reports **row counts**, in the shape of `EpisodicPurgeReport` and
`FactsPurgeReport` (`VaultPurgeReport`, which W8b ships), and it runs inside the locks the
purge already holds — the in-process gate and the cross-process operation lock — plus,
from W8c once the lock exists (D-18), the per-scope lock D-18 adds, for the reason the
sibling legs do: a count taken outside them cannot describe what the delete did.

After the `DELETE`, and still under those locks, the leg **re-counts the rows matching
the refs it just constructed**. If any remain, the purge fails with `PurgeError::Failed`.
Success is never inferred from a count of zero: `PurgeReport::is_empty()` reads zero as
the second-call shape, so "the predicate matched nothing" and "a correct re-run" are
indistinguishable by count alone.

What the read-back proves, exactly: that the delete took effect in the database the leg
opened — a wrong file, an uncommitted transaction, a statement that never ran, or a
partially applied batch all leave a non-zero remainder. What it does **not** prove: that
the predicate matches what the feeder wrote (that is one construction site, D-16), or
that the scope was the right one (that is D-15). A guard whose stated strength exceeds
what it can observe is the defect this amendment exists to remove, so the limit is
written here rather than implied.

Two consequences of the same rule:

- un-addressable content is declared through the existing `not_erased` mechanism, so a
  success is never a complete-erasure claim (D-16 names the class);
- the leg carries the same-root refusal forward. It refuses when the store it would erase
  through is not rooted at the purge's storage root, and when `{storage_root}/vault/`
  holds the named scope but no store is supplied for it — the shape of the facts leg's
  refusal to report `skipped_no_database` while `facts.db` exists. A purge never creates
  a vault: an absent scope is `SkippedNoDatabase`, never `Ran` with zero.

**Rejected:** a count-only report (it cannot tell a no-op from a re-run); a read-back
after the locks are released (a concurrent writer's rows would prove nothing about the
delete); treating a zero count as success (the exact inference D-01's defect class
depends on).

### D-14 — "Erased" means logical deletion, and the residuals are named

The vault purge removes rows. It does **not** promise that the bytes are gone from the
medium: page images and the write-ahead log may retain **ciphertext** until a
checkpoint, and nothing in the purge forces one. Two residuals follow, stated rather
than tidied:

1. **Logical only.** The rows are gone; old pages and the WAL may still hold encrypted
   copies of their content until the storage engine checkpoints or reuses them.
2. **Key-holder recovery.** An attacker who holds the scope key can recover purged
   content from those old pages while they survive. Deleting rows does not defend against
   a key holder. Destroying a key generation does, and that is W8c's operation, not a
   purge's.

**No `VACUUM` and no `secure_delete` in the purge path.** `VACUUM` rewrites the whole
database: it is expensive, it needs the key, and it can fail on a busy database, which
would make erasure *less* reliable than leaving the page images alone. `secure_delete` is
a per-connection flag, so it covers only writes made through the connection that set it,
and it does not scrub the WAL either. The episodic and facts legs delete rows and do not
compact; the vault was asked the same question and the answer chosen was **consistency**,
not a stricter standard for one tier.

**Rejected:** `VACUUM` after the delete (makes the promise depend on a rewrite that can
fail, turning a definite outcome into a maybe); `PRAGMA secure_delete` (per connection,
partial, and still not a WAL scrub); claiming physical erasure (untrue — and the tier is
where a user's secrets live, so an untrue claim here is worse than a bounded one).

### D-15 — Which scope a purge erases: derived by default, explicit only where nothing derives it

The vault leg addresses the **derived** scope: `resolve_project_scope(project_id,
&resolver)` — the same resolver, at the same single construction site, that W8a's feeder
uses (`crates/mem-bridge/src/project_scope.rs`). `PurgeRequest` already carries
`project_id`, so the default needs no new operator input, and a hand-typed hash is not
part of the normal path.

The reason is D-2, one level up. A hand-written scope hash is a **second construction
site**: when it disagrees with the feeder's, the purge builds refs in a scope where no
rows exist, deletes nothing, and D-13's read-back *still passes*, because it re-counts
with the same wrong predicate in the same wrong scope. Derivation is what makes "erased
nothing" and "erased everything" distinguishable. D-2 forbids a stored
`project_id → scope_hash` **table**; computing the hash from the named single source of
truth is what D-2 requires, so this is not an exception to D-2 but its application to
the purge.

An explicit **override** survives, and it is required, not decorative: by D-2 consequence
1 a moved project's old vault directory has no hash anything can derive, and that
directory is exactly what a purge of the moved project has to reach. So:

- **default:** the derived scope, and nothing else is guessed;
- **override:** the operator names scopes, and a named scope that does not resolve, or
  whose key material is missing, is a **refusal** — a detectable operator error, the
  same-root precedent applied where the knowledge exists;
- **everything else:** `{storage_root}/vault/` is enumerated, and every present scope the
  leg did not address is **reported**, never refused. The report has two halves. A
  **standing** `not_erased` entry declares the class: it is present on every run —
  including a run whose enumeration finds nothing — because `NotErasedTier`'s fields are
  `&'static str` and a per-scope entry could not carry a hash. It is a declaration about
  the tier, not a claim that this run found an unnamed scope. The **per-run** facts —
  which scopes were enumerated, and which of them were left unaddressed — live in
  `VaultPurgeReport` only.

**Why report, not refuse (Q6).** After the default above, the scope the leg addressed is
the derived one, so an unnamed present scope is either another project's — legitimately
out of scope — or an orphan of the project being purged. The leg **cannot tell which**,
because the artifact that would tell it is the `project_id → scope_hash` map D-2 forbids.
A refusal would demand a certainty the design deliberately gave up.

**Residual, named:** a purge of a moved project erases the derived (new) scope and
**cannot detect that the old scope survived**. Naming it in the report depends on the
enumeration finding it; a scope whose directory was moved away or deleted is invisible
to this leg.

**Rejected:** scanning the unnamed scopes for matching refs (it needs every scope's key,
and an unrelated scope with a missing key would fail the purge, turning blast-radius
containment into "this project's purge cannot complete because another project's vault
is broken"); refusing on an unnamed scope (needs certainty that was given up on purpose).

### D-16 — Ref kinds: the session ref only, and the `match` is exhaustive

W8a's feeder constructs exactly one ref:
`vault_source_ref(VaultSourceKind::Session, "{project}:{session}:{agent}")`, and
`PurgeRequest` carries all three parts. That is the only ref kind a purge can erase, so
it is the only one the eraser handles.

`Milestone` and `Fact` refs get **no eraser**, even though the milestone ids are already
pre-scanned under the purge's locks and a milestone eraser would cost nothing to write.
There is no producer for either kind, so such an eraser **cannot fire** — it would be a
gate that reads as protection and can never be reached, which is the defect class this
amendment exists to remove. The wave that ships a producer adds that kind's eraser and
cites this decision.

The eraser `match`es on `VaultSourceKind` **exhaustively**, one explicit arm per variant,
so a future variant is a compile error rather than a silent omission:

- `Session` — the only arm that builds a predicate and deletes;
- `Milestone`, `Fact` — explicit "no producer, nothing to erase" arms, not a wildcard;
- `Manual` — explicitly **not purgable**: `manual:<opaque>` names content with no
  external referent, so no session purge can address it and none pretends to.

Because `Manual` rows are un-addressable by construction, the leg declares that class
through `not_erased` (D-13), so a purge that leaves manual content behind says so.

**Rejected:** a milestone/fact eraser "while we are here" (unreachable code that looks
like a guarantee); a wildcard arm (a new variant would erase nothing and say nothing); a
`Manual` sweep by scope (it would erase content whose provenance is precisely "no
referent", which no purge was asked to remove).

### D-17 — Provenance is required, and it is presence, not truthfulness

**Implemented, not merely decided.** `VaultEntry.source_ref` and `VaultRecord.source_ref`
are `String`, not `Option<String>` — commit `55f16da`, on top of W8a's `efd66b9`. `put`
validates `source_ref` unconditionally (non-empty after trim, no NUL, at most 256 bytes →
`InvalidEntry { field: "source_ref" }`); absence is a **compile error**, and the crate
carries a compile-fail doctest that says so.

Why the type changed: a row with no provenance is permanently un-addressable — no purge
constructs a ref that matches `NULL` — so it would be a silent permanent resident in the
tier this ADR calls precisely where a user's secrets would live. `VaultSourceKind::Manual`
is the honest escape: content with no external referent says so, in the ref.

Three limits are recorded rather than papered over:

1. **Presence, not truthfulness.** The API cannot tell a `session:…` ref that describes
   the content from one that does not. A caller can label a manual paste as
   `session:{project}:{session}:{agent}` — it will then be erased by that session's purge
   — or label session content `manual:`, which no purge will reach. There is no
   provenance verification in v1.
2. **The schema still allows `NULL`.** Enforcement is at the API, not the column (no
   migration), so a hand-written or DBA-inserted row can carry `NULL`, which reads back as
   the empty string and can never be matched by a purge.
3. **A ref is not a reference.** The vault stores it and a purge compares it; nothing
   resolves it (D-7(d)). It names where content came from; it proves nothing about where
   it went.

**Rejected:** keeping `Option<String>` and validating only when present (the W8a r7
defect: an accepted state no purge can ever address); an "unknown provenance" bucket
(the same silent resident with a name); verifying truthfulness in v1 (there is nothing to
verify against — re-deriving the producer's triple from stored bytes is not something
this system can do).

### D-18 — One shared lock: the rekey excludes writers, and a purge refuses a rekey

One exclusive lock per scope, held beside the scope's key material in
`{storage_root}/vault-keys/` (a `0600` file, like the key it sits next to), so that a
directory-level mistake hits both or neither. Three consumers, one mechanism:

- **the rekey takes it exclusively** for its whole duration (D-7(c)): it needs both
  generations resident and a stable table to read;
- **the feeder drops, fail-closed.** It checks the lock immediately before each `put`; if
  the lock is held, the write is dropped and counted — the same shape as the
  `EncryptionRequired` flag it already carries. That is affordable only because the vault
  is **derived**: raw context still lives in episodic untouched, so a dropped promotion
  loses no source of truth;
- **the vault purge leg takes it too, and holds it for its whole duration.** The leg
  acquires the lock before it constructs its predicate and releases it only after D-13's
  read-back, so the feeder's `put` — a spawned task off the capture path — sees a held
  lock and drops instead of landing a row the purge has already counted as gone. Without
  that the purge would be a lock *checker* that never holds anything, and one late `put`
  would resurrect content for a ref the purge reported erased. When the lock is already
  held — a rekey in progress — the purge refuses with `PurgeError::Busy`: the existing
  variant, exit 75, `purge.busy`, nothing read and nothing erased. That is deliberately
  distinct from D-13's `PurgeError::Failed`: a held lock means "retry later", a failed
  read-back means the store is wrong.

**Why the purge refuses: a race can resurrect erased content.** The rekey rewrites the
table from rows it read *before* a concurrent purge deleted them, and its marker commit
makes generation n+1 authoritative. A purge running alongside could therefore report a
successful erasure while the committed generation still contains the rows it deleted.
Together with the feeder `put` that lands after D-13's read-back (the reason the leg holds
the lock rather than merely checking it), this is one of the two interactions in this ADR
where an ordering mistake silently undoes an erasure — so the refusal is a correctness
requirement with a named failure mode, not a courtesy.

**Wave assignment, and why the three parts ship together.** The lock is W8c, with the
rekey. W8b ships the purge leg and no lock: the feeder side of the lock lands with the
lock in W8c, so the lock, the feeder's check and the purge's hold move as one change, and
a purge that held a lock no writer ever checked would exclude nothing. The leg starts
taking the lock only when the lock exists. **Residual, named:** until W8c, a feeder `put`
that lands after D-13's read-back can resurrect a ref the purge reported erased — the race
the third bullet above closes — so W8b's leg proves the delete in the database it opened
and cannot prove that a later writer will not undo it. That one-wave residual is covered
only by D-9's no-deployment rule, not by a mechanism.
W8c ships the lock, the rekey and the purge-side refusal in **one
change**, because any two of the three alone are either an unreachable gate (a purge
checking a lock no rekey ever takes) or an unexcluded writer. The converse is chosen here
rather than separately ruled (F-9): a rekey that finds the purge's lock held refuses
instead of waiting. Waiting would be equally safe under one exclusive lock — the refusal
is policy, not a correctness requirement — and it is chosen because a rekey is
operator-initiated and a retry is cheap, while a rekey parked behind a purge would hold
both key generations resident for the purge's duration.

**Rejected:** a procedural rule ("the operator takes the vault offline") — D-7(c)'s
original wording, and the reason this decision exists; a second lock for purges (two
locks that can disagree are two ways to resurrect content); making the feeder queue or
block instead of dropping (a rekey would then stall the capture path on a derived tier);
relying on the storage engine's own locking (the generation swap happens outside the page
lock, and `MultiprocessWal` deliberately admits concurrent processes).

## Rejected alternatives

- **One global vault key.** A single key opens every project's vault, which defeats
  blast-radius containment — the entire reason option C was chosen — and makes a rekey a
  re-encryption of every project at once.
- **A per-user vault** (ADR-0009's original wording). A per-scope key cannot serve a
  store shared across projects, and the owner's binding decision is per project; ADR-0009
  was corrected rather than left contradicting the implementation.
- **Content-addressed ids as `SHA256(content)`.** An offline confirmation oracle for
  low-entropy secrets; the reason Q2b exists.
- **Keying only the id and not the file** (or vice versa). A keyed id protects nothing if
  the bytes sit in a plaintext file, and an encrypted file with unkeyed ids still leaks
  equality and confirmations.
- **Keeping the pre-rekey MAC key forever** so ids survive a rekey. It retains the
  compromised secret and cannot give stable ids for post-rekey re-inserts anyway (D-7).
- **Per-row key generations / a `key_ref` column.** A whole-file codec has one key per
  file; a per-row label would encode a rotation model that cannot exist.
- **Rotation as a background task.** It requires both keys resident and exclusive access;
  an automatic rekey can strand a vault mid-window with nobody watching.

## Consequences

**Positive**

- One key per scope: a leaked key exposes one project; a destroyed key is a bounded loss.
- Rotation is decided, not deferred: the old key is disposable, and the cost (a
  full-table rewrite, D-7) is stated where an implementer will read it.
- The fail-closed order and the key rules are executable properties (refuse before disk,
  never regenerate, atomic mint), not aspirations.
- Keyed ids make a leaked *database* useless for offline confirmation attacks.
- The vault is honest about erasure: the writer wave owes the purge a leg, and v1 ships
  the index that makes it possible.
- Erasure is specified to a mechanism rather than a promise: counts with a read-back, a
  derived scope, one ref kind behind an exhaustive `match`, and a declared residual
  (D-13 … D-16).

**Negative**

- Moving a project changes its `scope_hash` and therefore the vault's address; the bytes
  are not lost, but discovery is (D-2). Re-addressing is out of scope and is a rename,
  not a rekey.
- A lost key means permanent loss of that vault (D-10) — by design.
- A rekey is a full-table rewrite and is identity-breaking inside the vault (D-7).
- `vault-keys/` enumerates projects and is a single point of access at the directory
  level (D-2).
- Rotation code does not exist yet; D-9 makes it a hard requirement of W8c (two waves
  after W8a, behind W8b's purge leg), not an optional follow-up, and the unrotatable
  window is two waves wide rather than one.
- The split widens that window (D-9). The no-deployment rule is the only thing covering
  it, and it is a rule, not a mechanism.
- Until W8b lands, a `purge.completed` event erases episodic and facts rows and no vault
  rows, while the vault is not named in `not_erased` (D-11). The gap is live in the tree
  as of W8a (`55f16da`); W8b's leg closes it, and no production use may precede it.

**Neutral / forward**

- The marker format gains a generation field when the rekey lands (D-8); no W7.2-created
  vault can need it, because W7.2 creates no content.
- ADR-0009's vault row now says per-project; nothing else in that ADR was touched.
- In W7.2 the crate shipped no consumer, reader or writer, and deleting it would have
  broken only its own tests; that was the point — the value it bought is this contract,
  pinned before anything could violate it. W8a has since added the one consumer edge
  (`mem-bridge → mem-vault`, ADR-0001) and the feeder that uses it, so the contract now
  constrains a caller as well as describing a tier.

## References

- ADR-0004: Database encryption at rest (Accepted; no rotation clause)
- ADR-0009: Multi-agent memory architecture (Tier 4; amended 2026-10-23 to per-project scope)
- ADR-0010: Sound Memory — D-1 (raw context lifetime), D-3 (Vault = L0), D-4 (`source_ref`), D-6 ("NOT vault")
- ADR-0011: Hot-cold memory split (orthogonal to the vault; no tiering applies — see
  ADR-0011 D-10 for the explicit exclusion of the vault from hot/cold migration)
- RFC 2104 (HMAC), RFC 4231 (HMAC-SHA-256 test vectors), FIPS 180-4 (SHA-256)
- W7.1 blueprint r5, `[W7.2 FORWARD REFERENCE]` — the owner-ratified hand-off (option C, keyed ids, fail-closed, no write path)
- W8a landing commits: `efd66b9` (vault content write/read path, `source_ref` contract,
  session-end feeder) and `55f16da` (provenance required, D-17)
