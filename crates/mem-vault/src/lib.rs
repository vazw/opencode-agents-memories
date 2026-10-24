//! mem-vault — the verbatim, encrypted, per-project vault tier (W7.2).
//!
//! # What this crate is
//!
//! The tier that holds bytes whose exact wording matters (ADR-0010 D-3: the
//! vault is **L0** — no compression, no normalization, no derived
//! representation). W8a opens the content surface: writes go through
//! [`VaultStore::put`] only, and reads through [`VaultStore::get`].
//! `VaultEntry` carries no `content_hash` field — the store derives the
//! identity from its own cached `KeyHex` (ADR-0012 D-6) — so a caller can
//! never name, forge or pre-compute an id. Rekey and `recover_rekey` are
//! **W8c**; the purge erasure leg and `VaultStore::erase` stay in **W8b**.
//! The structural invariant is enforced by the compile-fail doctest on
//! `VaultEntry` (AC-1)
//! and the lexical tripwire in `tests/no_write_path.rs` (AC-7).
//!
//! # Design authority
//!
//! `docs/adr/0012-vault-key-custody.md` decides:
//!
//! - **D-1** — keys live outside the data tree:
//!   `{storage_root}/vault-keys/{scope_hash}.key`, database at
//!   `{storage_root}/vault/{scope_hash}/vault.db`, marker at
//!   `{storage_root}/vault/{scope_hash}/.vault-encrypted-v1`.
//! - **D-2** — `scope_hash` comes from the container-tag path only
//!   (`mem_core::scope::is_valid_scope_hash`; exactly 16 lowercase hex). There
//!   is no project→scope table and this crate does not add one.
//! - **D-6** — `content_hash` is HMAC-SHA256 under the domain-separated subkey
//!   `HMAC(key_file_bytes, "mem-vault/content-hash/v1")`, vendored over
//!   `mem_core::sha256` — no new dependency, and never a plain digest of the
//!   content.
//! - **D-7 / D-8** — rotation is decided there: a rekey rewrites every
//!   `content_hash` (so ids are not stable across a rekey) and the marker is
//!   the single commit point.
//! - **D-9** — rekey ships in **W8c**, after W8b's purge leg (the amended D-9
//!   heading, `docs/adr/0012-vault-key-custody.md:388`); this crate ships no
//!   rotation API and nothing here may be read as a second rotation design.
//! - **D-11** — the purge erasure path ships in **W8b** (D-13 … D-16). W8a
//!   shipped the writer without it, so the obligation is no longer "the wave
//!   that adds the writer" — that wave has passed — and the gap is live.
//!
//! # Purge
//!
//! W8a has landed, so the premise of the old sentence is gone: the vault now
//! holds content for any session that marked it, and `purge.completed`'s
//! `not_erased` is **no longer** complete or true without naming the vault. The
//! W8b leg (D-11, D-13 … D-16, D-18) is not shipped, so the gap is live: a
//! purge run today erases no vault rows and does not say so. `mem-bridge`'s
//! purge is proved unable to reach these directories only because no erasure
//! path exists yet — that is the residual, not a guarantee.
//!
//! # Identity drift (accepted, documented, not mitigated)
//!
//! For a git project the scope hash derives from the canonical `.git` path, so
//! moving a repository changes the vault's address — the bytes survive,
//! discovery does not. Re-addressing is renaming two paths, not a rekey.
//!
//! # Enumerability (accepted, documented, not mitigated)
//!
//! `vault-keys/` enumerates every project's vault under one storage root.
//! Containment is per *key compromise*, not per store access; filesystem
//! permissions are the directory-level barrier. The scope hash is a 64-bit
//! prefix, not a secret, and provides no confidentiality of its own.
//!
//! # Content write path
//!
//! W8a opens the content surface: writes go through [`VaultStore::put`] only,
//! and reads through [`VaultStore::get`]. `VaultEntry` carries no
//! `content_hash` field — the store derives the identity from its own cached
//! `KeyHex` (ADR-0012 D-6) — so a caller can never name, forge or
//! pre-compute an id. Rekey and `recover_rekey` are **W8c**; the purge
//! erasure leg and `VaultStore::erase` stay in **W8b**.

use std::path::PathBuf;

/// Stable crate identifier for composition checks.
pub const CRATE_NAME: &str = "mem-vault";

pub mod entry;
pub mod hmac;
pub mod schema;
pub mod store;

pub(crate) mod ids;
pub(crate) mod key;

pub use entry::{VaultEntry, VaultRecord};
pub use store::VaultStore;

/// The vault-tier error surface.
///
/// # The `Display` contract (load-bearing)
///
/// A path-bearing variant formats **its path** with `path.display()` and
/// nothing else derived from the input. No variant ever interpolates key
/// material, a key file's content, or a rejected scope string: a message that
/// varied with the input could carry it. `InvalidKeyHex` and `Random` carry no
/// data at all, so their message is one fixed sentence.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum VaultError {
    /// The scope hash is not exactly 16 lowercase hex characters. Fixed
    /// message: the rejected value may be arbitrarily long.
    #[error("invalid scope hash: expected exactly 16 lowercase hex characters")]
    InvalidScope,

    /// A vault entry failed validation. `field` names the offending field
    /// only; no input value is ever interpolated.
    #[error("invalid vault entry field: {field}")]
    InvalidEntry {
        /// The field that failed validation.
        field: &'static str,
    },

    /// Database encryption is disabled in the store configuration. The open
    /// refuses **before** any directory, key read or connection exists.
    #[error("vault open refused: database encryption is disabled in this configuration")]
    EncryptionRequired,

    /// An existing key file does not hold exactly 64 lowercase hex characters
    /// (plus at most one trailing newline). Never regenerated, never deleted.
    #[error(
        "invalid vault key file at {}: expected exactly 64 lowercase hex characters; \
         the library never regenerates or removes it",
        .path.display()
    )]
    InvalidKey {
        /// The key file that failed validation.
        path: PathBuf,
    },

    /// The key file is gone while encrypted data (or its marker) exists. A lost
    /// key is unrecoverable by design (ADR-0012 D-10).
    #[error(
        "vault key file at {} is missing while encrypted data exists; \
         a lost key is unrecoverable by design",
        .path.display()
    )]
    KeyMissingForExistingData {
        /// The key file that should exist.
        path: PathBuf,
    },

    /// The encryption marker is unparsable (expected `cipher=<name>` plus at
    /// most one trailing newline).
    #[error(
        "vault marker at {} is malformed: expected `cipher=<name>`",
        .path.display()
    )]
    MarkerInvalid {
        /// The marker file that failed to parse.
        path: PathBuf,
    },

    /// The marker records a different cipher than the configuration asks for.
    /// Reported before the database is opened, so an intact file yields a
    /// named operator error instead of an opaque codec failure.
    #[error(
        "vault cipher mismatch: the marker records cipher={recorded} \
         but the configuration asks for {configured}"
    )]
    CipherMismatch {
        /// The cipher recorded in the marker.
        recorded: String,
        /// The cipher the configuration asks for.
        configured: String,
    },

    /// A derived path escaped the storage root (defence-in-depth; the scope
    /// validator already makes traversal unrepresentable).
    #[error("vault path at {} escapes the storage root", .path.display())]
    PathEscape {
        /// The offending derived path.
        path: PathBuf,
    },

    /// Key material handed to [`hmac::keyed_content_hash`] is not 64 lowercase
    /// hex characters plus an optional single trailing newline. Fixed message,
    /// no data.
    #[error("invalid vault key hex: expected exactly 64 lowercase hex characters")]
    InvalidKeyHex,

    /// The operating system refused to supply 32 random bytes.
    #[error(
        "vault key randomness is unavailable: the operating system refused to \
         supply 32 random bytes"
    )]
    Random,

    /// A filesystem operation failed at a named path.
    #[error("vault io failure at {}: {source}", .path.display())]
    Io {
        /// The path the operation touched.
        path: PathBuf,
        /// The underlying failure.
        source: std::io::Error,
    },

    /// Storage-layer pass-through.
    #[error(transparent)]
    Store(#[from] mem_store::error::StoreError),

    /// The vault's per-scope map was poisoned (ADR-0013 D-2 `D2-b`): the map
    /// holds key material and the manager built from it, so a vault that
    /// cannot vouch for its key state refuses instead of continuing.
    ///
    /// The payload is two `&'static str` literals — never a path, never key
    /// material (ADR-0012's `Display` contract).
    #[error("vault lock `{lock}` poisoned while {operation}; refusing to continue")]
    LockPoisoned {
        /// The guarded value's name (a closed set of literals).
        lock: &'static str,
        /// The operation that found the lock poisoned.
        operation: &'static str,
    },
}
