//! Vault content row types (W8a).
//!
//! `VaultEntry` is the *input* to [`crate::VaultStore::put`]: the caller
//! supplies content, mime type, provenance and authorship. It deliberately
//! carries **no `content_hash` field** — the store derives the identity from
//! its own cached `KeyHex`, so a caller can never name, forge or pre-compute
//! an id (ADR-0012 D-6). `VaultRecord` is what `get` returns and *does*
//! carry `content_hash`, because by then the store has authority over it.

/// The input to [`crate::VaultStore::put`].
///
/// ```compile_fail
/// use mem_vault::VaultEntry;
/// let _ = VaultEntry {
///     content: Vec::new(),
///     mime_type: None,
///     source_ref: None,
///     created_by: String::new(),
///     content_hash: String::new(),
/// };
/// ```
#[derive(Clone)]
pub struct VaultEntry {
    pub content: Vec<u8>,
    pub mime_type: Option<String>,
    pub source_ref: Option<String>,
    pub created_by: String,
}

/// The row returned by [`crate::VaultStore::get`].
pub struct VaultRecord {
    pub content_hash: String,
    pub content: Vec<u8>,
    pub mime_type: Option<String>,
    pub source_ref: Option<String>,
    pub created_at: i64,
    pub created_by: String,
}
