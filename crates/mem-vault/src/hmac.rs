//! HMAC-SHA-256 (RFC 2104) vendored over `mem_core::sha256`, and the keyed
//! content id of ADR-0012 D-6.
//!
//! # Why vendored
//!
//! `Cargo.lock` contains no `hmac` and no `sha2`. ADR-0012 D-6 pins the
//! primitive to RFC 4231's HMAC-SHA-256 vectors and forbids the two dependency
//! trees for ~30 lines of construction, so this module implements RFC 2104
//! directly over the FIPS-vector-pinned digest in `mem_core::sha256`.
//!
//! # Allocation shape (deliberate, recorded)
//!
//! The inner hash needs `ipad ‖ message` in one contiguous slice:
//! `mem_core::sha256::digest` takes a `&[u8]` and has no streaming API, and
//! adding one to `mem-core` is out of scope for this wave. The inner input is
//! therefore exactly **one** `Vec` of `64 + message.len()` bytes — proportional
//! to the message, with no growth beyond it. The outer input is a fixed
//! 96-byte stack buffer (`opad ‖ inner_digest`).
//!
//! # Scope: the vault only
//!
//! `facts.id` and segment ids are digests over non-secret keys and identities;
//! the confirmation-oracle argument that produced this rule does not reach
//! them, and they are deliberately unchanged (ADR-0012 D-6).
//!
//! No key material is ever interpolated into an error: [`VaultError::InvalidKeyHex`]
//! carries no data at all.

use mem_core::sha256;

use crate::VaultError;

/// The domain-separation label of the MAC subkey (`k_mac`). Changing this
/// value changes every content id; it is versioned by the schema's v2 rule.
pub const CONTENT_HASH_LABEL: &str = "mem-vault/content-hash/v1";

/// The HMAC block size for SHA-256 (RFC 2104: B = 64).
const BLOCK_SIZE: usize = 64;

/// HMAC-SHA-256 of `message` under `key` (RFC 2104 §2).
///
/// A key longer than the block size is hashed first; a shorter key is
/// zero-padded to the block size. `ipad = k ^ 0x36`, `opad = k ^ 0x5c`.
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block = [0u8; BLOCK_SIZE];
    if key.len() > BLOCK_SIZE {
        block[..32].copy_from_slice(&sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0u8; BLOCK_SIZE];
    let mut opad = [0u8; BLOCK_SIZE];
    for index in 0..BLOCK_SIZE {
        ipad[index] = block[index] ^ 0x36;
        opad[index] = block[index] ^ 0x5c;
    }

    // One allocation, exactly `64 + message.len()` bytes (see the module note).
    let mut inner_input = Vec::with_capacity(BLOCK_SIZE + message.len());
    inner_input.extend_from_slice(&ipad);
    inner_input.extend_from_slice(message);
    let inner_digest = sha256::digest(&inner_input);

    let mut outer_input = [0u8; BLOCK_SIZE + 32];
    outer_input[..BLOCK_SIZE].copy_from_slice(&opad);
    outer_input[BLOCK_SIZE..].copy_from_slice(&inner_digest);
    sha256::digest(&outer_input)
}

/// The keyed content id of ADR-0012 D-6.
///
/// ```text
/// k_mac        = HMAC-SHA256(key_file_bytes, "mem-vault/content-hash/v1")
/// content_hash = lowercase_hex(HMAC-SHA256(k_mac, content_bytes))   // 64 chars
/// ```
///
/// `key_hex` is accepted **as the key file holds it**: exactly 64 lowercase
/// hex characters plus at most one trailing newline. Anything else is
/// [`VaultError::InvalidKeyHex`], whose message never echoes the value. The
/// content is hashed byte-exactly — the vault is L0: no normalization, no
/// whitespace collapsing, no trimming.
pub fn keyed_content_hash(key_hex: &str, content: &[u8]) -> Result<String, VaultError> {
    let key = validate_key_hex(key_hex)?;
    let mac_key = hmac_sha256(key.as_bytes(), CONTENT_HASH_LABEL.as_bytes());
    Ok(sha256::hex(&hmac_sha256(&mac_key, content)))
}

/// Strips at most one trailing newline, then requires exactly 64 lowercase
/// hex characters. The two callers of this rule (here and `key::KeyHex`) are
/// deliberately separate: `key` ships one step later, and a shared helper
/// would invert the wave's step order.
fn validate_key_hex(raw: &str) -> Result<&str, VaultError> {
    let stripped = raw.strip_suffix('\n').unwrap_or(raw);
    let is_valid = stripped.len() == 64
        && stripped
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
    if is_valid {
        Ok(stripped)
    } else {
        Err(VaultError::InvalidKeyHex)
    }
}
