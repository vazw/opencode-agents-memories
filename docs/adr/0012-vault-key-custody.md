# 0012 — Vault Key Custody and Rotation

- **Status:** Accepted (owner acceptance, 2026-10-23)
- **Date:** 2026-10-23 (UTC 2026-10-22) — the decision records of this session are dated 2026-10-23 while the UTC clock reads 2026-10-22T20:4x with identical local time; both readings are stated so neither is a lie.
- **Wave:** W7.2 — `mem-vault`. This ADR is the **design authority for that wave**: the crate ships key custody, the fail-closed open, keyed content ids and a v1 schema, with **no content write path** and **no rotation API**.
- **Supersedes:** nothing. **Amends:** nothing in this file. ADR-0004 stays authoritative for global shard encryption and is untouched. ADR-0009's Tier-4 sharing cell was amended to per-project scope on the same date, by the same owner ruling, in ADR-0009's own W7.2 amendment note.
- **Source:** owner rulings — vault key = option C (per-project key files); rotation decided here rather than deferred; ADR-0009's vault scope corrected to per-project. Evidence and standing rules: ADR-0004 (encryption at rest; **zero** rotation clauses), ADR-0010 D-1/D-3/D-4/D-6 (Sound Memory: verbatim vault, no compression, `source_ref`, "NOT vault"), ADR-0009 D-1/D-3/D-7 (tier 4, cross-tier promotion, privacy), and W7.1's owner-accepted carve-out (M23/D7), which recorded option C as binding on this wave.

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
D-9) belongs to W8b, the wave immediately following the first content writer (W8a).

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
generation 1, and the rekey wave extends the format with a generation field. Until a
writer exists there is no content and the extension is free; no vault created by W7.2
can need it.

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
- (c) **an owner-initiated offline operation**, not a background task: it needs both
  keys resident simultaneously and must exclude concurrent writers for its duration;
- (d) **identity-breaking by construction**: `content_hash` is not stable across a
  rekey. Nothing outside a vault may persist a vault `content_hash` as a durable
  reference. References *into* the vault must be by `source_ref` (which points out of
  the vault at episodic/facts ids and is unaffected), or by a surrogate the writer wave
  adds if it needs one. `source_ref` is shipped and indexed in v1 precisely so that
  this constraint costs nothing later. W8a ships the first producer (the session-end
  feeder) and therefore the first opportunity for an external holder to appear; the
  contract forbids it, and a repo-level test guards it.

**Why not the alternative.** Keeping the old MAC key alive so ids survive would mean
the rekey retains the very secret whose compromise motivated it — a rotation that keeps
the old key is not a rotation. It is also self-defeating: re-inserting content that
already exists after a rekey would produce either a duplicate row (same content, new id,
integrity gone) or a need to recompute the old id (old key retained — the same circle).
Rewriting is the only option that makes the old key disposable, and disposability is
the whole point.

**Consequence accepted:** because ids change, a rekey is visible to anything that cached
a `content_hash`. Today nothing does — W7.2 ships no reader, writer or consumer — and
the writer wave inherits the rule in (d).

**Rejected:** per-row key generations (impossible under a whole-file codec); an
envelope key that never changes with only the encryption subkey rotating (a single
32-byte key file cannot hold two independent generations, and changing the file changes
both derivations — the owner's key format makes this option unavailable rather than
merely unattractive); deferring rotation again (the reason option C was chosen was that
rotation becomes tractable; deferring leaves a leaked key with no remedy but deletion).

### D-8 — Rekey protocol invariant: the marker is the single commit point

This ADR does not specify a byte-level procedure; it specifies the invariant a future
implementation must satisfy, because the implementation belongs to W8b (D-9).

1. A rekey produces **generation n+1** beside generation n: the new database is written
   under a temporary name, `sync_all`'d, and its new key file is written beside the old
   one (D-4's atomic mint). Nothing at the canonical paths changes yet.
2. **The commit is a single marker replacement**, not a file swap: the marker (naming
   generation and cipher, D-5's extension) is written to a temporary file, `sync_all`'d,
   and **renamed** over the old marker; **then the parent directory is `fsync`'d.**
   Before the rename, generation n is authoritative; after it, generation n+1 is.
3. Only **after** the commit are the previous generation's key and database deleted. A
   residue (`.prev`) is therefore the only acceptable extra file, it is reported rather
   than silently trusted, and its existence means "a rekey was interrupted".
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
`RekeyInterrupted` on residue, and recovery is an explicit `recover_rekey` call (W8b).

The one thing this invariant exists to prevent: a moment in which the key on disk cannot
open the database on disk and the old key is already gone. That is unrecoverable, and it
is the failure mode a naive "write the key, then rewrite the file" sequence produces.

**Rejected:** a database-first swap (the new file is readable only with a key that is not
yet in place — the same window, mirrored); a journal file besides the marker (a second
commit point is a second way to disagree); trusting filename ordering/mtime (not a
commit protocol).

### D-9 — Rotation API ownership: rekey ships in W8b, immediately after the first content writer

W7.2 ships **no** rotation API, no rekey CLI, no export/import of key material. Rewriting
`content_hash` is a content write path, and this wave's binding rule is that it has none.

**The rotation API belongs to the wave that first writes vault content.** Under the
owner's resequencing, the content write path ships in W8a and rekey ships in the
immediately following wave W8b; no production deployment may occur between them, and
W8b is a hard prerequisite of any production use of the vault. W8a ships no rotation
API. Rationale: the first content-bearing vault is the first thing whose key can
matter, and the only remediation for a leaked key without rekey is deleting data — the
opposite of what the vault exists for. Shipping the first write path without rekey would
create exactly one wave's worth of unrotatable secrets; the no-deployment-between-waves
rule is what makes that window safe. This is a constraint on those waves, not a deferral
of the design: the design is D-7/D-8.

### D-10 — A lost or corrupt key is unrecoverable, and is never regenerated

Over **existing** encrypted data, a missing key is `KeyMissingForExistingData` and a
present-but-invalid key is `InvalidKey` (D-4). Neither is repaired, and neither may be
"fixed" by deleting the file: deletion is an operator action, never a library one. A
vault whose only key generation is lost is unrecoverable by design; the price of
blast-radius containment is exactly this, and it must be documented rather than
discovered. After a *completed* rekey the previous generation is gone (D-7), so a lost
post-rotation key is likewise unrecoverable.

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
with content addressing.

Until a writer exists the vault holds nothing for any session, so today's
`purge.completed` report remains complete and true without naming the vault. The wave
that adds the writer must (i) implement the vault leg, or (ii) deliberately decline and
add a vault entry to `not_erased` — silence is not an option. v1 ships and indexes
`source_ref` so that (i) stays implementable.

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

**Negative**

- Moving a project changes its `scope_hash` and therefore the vault's address; the bytes
  are not lost, but discovery is (D-2). Re-addressing is out of scope and is a rename,
  not a rekey.
- A lost key means permanent loss of that vault (D-10) — by design.
- A rekey is a full-table rewrite and is identity-breaking inside the vault (D-7).
- `vault-keys/` enumerates projects and is a single point of access at the directory
  level (D-2).
- Rotation code does not exist yet; D-9 makes it a hard requirement of W8b (immediately
  following W8a), not an optional follow-up.

**Neutral / forward**

- The marker format gains a generation field when the rekey lands (D-8); no W7.2-created
  vault can need it, because W7.2 creates no content.
- ADR-0009's vault row now says per-project; nothing else in that ADR was touched.
- W7.2 ships no consumer, reader or writer. Deleting the crate would break only its own
  tests — the value it buys is this contract, pinned before anything can violate it.

## References

- ADR-0004: Database encryption at rest (Accepted; no rotation clause)
- ADR-0009: Multi-agent memory architecture (Tier 4; amended 2026-10-23 to per-project scope)
- ADR-0010: Sound Memory — D-1 (raw context lifetime), D-3 (Vault = L0), D-4 (`source_ref`), D-6 ("NOT vault")
- ADR-0011: Hot-cold memory split (orthogonal to the vault; no tiering applies — see
  ADR-0011 D-10 for the explicit exclusion of the vault from hot/cold migration)
- RFC 2104 (HMAC), RFC 4231 (HMAC-SHA-256 test vectors), FIPS 180-4 (SHA-256)
- W7.1 blueprint r5, `[W7.2 FORWARD REFERENCE]` — the owner-ratified hand-off (option C, keyed ids, fail-closed, no write path)
