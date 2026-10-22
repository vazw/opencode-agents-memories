//! The vault v1 schema — one fresh file per scope, one migration, no history.
//!
//! # `content_hash` is a keyed id, never `SHA256(content)`
//!
//! It is HMAC-SHA256 under the domain-separated subkey
//! `HMAC(key_file_bytes, "mem-vault/content-hash/v1")` (ADR-0012 D-6), so a
//! leaked database is not an offline confirmation oracle for a low-entropy
//! secret. A plain digest of the content must never be substituted here.
//!
//! # Rotation is not decided in this file
//!
//! Rotation is decided in ADR-0012 D-7 (a full rekey of one scope directory;
//! every `content_hash` is rewritten, so ids are **not** stable across a
//! rekey) and D-8 (the marker is the single commit point). It is implemented
//! by the first writer wave (D-9). This crate ships no rotation API and
//! nothing here may be read as a second rotation design.
//!
//! # The v2 rule: additive only
//!
//! A v2 that adds a nullable column is a normal migration. A v2 that changes
//! the *semantics* of `content_hash` (a different MAC, a different derivation,
//! a different content encoding) is a rotation-shaped decision: it needs an
//! accepted ADR plus a rehash of every row, because it would invalidate every
//! id already on disk.
//!
//! # Deleting the file is a valid reset — only while there is no writer
//!
//! With no writer, a scope holds nothing, so deleting `vault.db` is a
//! legitimate v0→v1 reset. The moment a writer exists that stops being true
//! (a lost key over live ciphertext is unrecoverable by design, D-10).

use mem_store::db::Statement;
use mem_store::schema_migrations::SchemaMigration;

/// The vault database file name (inside `{storage_root}/vault/{scope}/`).
pub const VAULT_DB_NAME: &str = "vault.db";

/// The encryption marker file name (inside `{storage_root}/vault/{scope}/`).
pub const VAULT_MARKER_NAME: &str = ".vault-encrypted-v1";

/// The key directory under the storage root — deliberately **outside**
/// `vault/`, so deleting the data tree cannot destroy the keys.
pub const VAULT_KEYS_DIR: &str = "vault-keys";

/// The data directory under the storage root.
pub const VAULT_DIR: &str = "vault";

/// The `vault_entries` v1 DDL: six columns, no writer.
///
/// `content_hash` is the identity **and** the keyed-digest contract;
/// `content` is verbatim (L0, no normalization); `mime_type` renders a `BLOB`;
/// `source_ref` is provenance (ADR-0010 D-4) and the future erasure join
/// (ADR-0012 D-11), indexed now so that leg stays implementable;
/// `created_at`/`created_by` are the audit trail ADR-0010 D-4 asks for.
pub const VAULT_ENTRIES_DDL: &str = "CREATE TABLE IF NOT EXISTS vault_entries (\n\
 \x20 content_hash TEXT PRIMARY KEY,\n\
 \x20 content      BLOB NOT NULL,\n\
 \x20 mime_type    TEXT,\n\
 \x20 source_ref   TEXT,\n\
 \x20 created_at   INTEGER NOT NULL,\n\
 \x20 created_by   TEXT NOT NULL\n\
 \x20)";

/// The v1 indexes (one: the `source_ref` join the erasure leg will need).
pub const VAULT_INDEXES: [&str; 1] =
    ["CREATE INDEX IF NOT EXISTS idx_vault_entries_source_ref ON vault_entries(source_ref)"];

/// The v1 migration set: one version, two statements, no history — the file
/// is fresh, so a fresh open yields `from_version == 0` and creates no backup.
pub fn vault_migrations() -> Vec<SchemaMigration> {
    let mut statements = vec![Statement::new(VAULT_ENTRIES_DDL, Vec::new())];
    for index in VAULT_INDEXES {
        statements.push(Statement::new(index, Vec::new()));
    }
    vec![SchemaMigration {
        version: 1,
        description: "vault-v1: vault_entries (W7.2)",
        statements,
    }]
}
