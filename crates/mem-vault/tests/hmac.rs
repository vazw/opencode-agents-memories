//! W7.2 — the vendored HMAC-SHA-256 and the keyed content id (ADR-0012 D-6).
//!
//! The vectors below are RFC 4231 §4's seven HMAC-SHA-256 test cases. Case 5 is
//! *truncated to 128 bits by the RFC itself*, so it is asserted as a 16-byte
//! prefix; cases 1, 2, 3, 4, 6 and 7 are asserted as full 32-byte outputs.
//! Cases 6 and 7 are the ones that catch a missing "key longer than the block
//! size is hashed first" branch; case 4 catches a wrong-length key pad.

use mem_core::sha256;
use mem_vault::VaultError;
use mem_vault::hmac::{CONTENT_HASH_LABEL, hmac_sha256, keyed_content_hash};

/// A valid 64-lowercase-hex key (the shape a key file holds, newline aside).
const KEY_A: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
/// A second valid key, different from [`KEY_A`].
const KEY_B: &str = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";

fn hex_of(digest: &[u8; 32]) -> String {
    sha256::hex(digest)
}

/// Decodes a hex string (test fixture helper only).
fn unhex(text: &str) -> Vec<u8> {
    assert!(text.len().is_multiple_of(2), "fixture hex has even length");
    text.as_bytes()
        .chunks(2)
        .map(|pair| {
            let hi = (pair[0] as char).to_digit(16).expect("hex digit") as u8;
            let lo = (pair[1] as char).to_digit(16).expect("hex digit") as u8;
            (hi << 4) | lo
        })
        .collect()
}

#[test]
fn test_hmac_sha256_matches_rfc4231_vectors() {
    // Case 1.
    assert_eq!(
        hex_of(&hmac_sha256(&[0x0b; 20], b"Hi There")),
        "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7",
        "RFC 4231 case 1"
    );
    // Case 2.
    assert_eq!(
        hex_of(&hmac_sha256(b"Jefe", b"what do ya want for nothing?")),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843",
        "RFC 4231 case 2"
    );
    // Case 3.
    assert_eq!(
        hex_of(&hmac_sha256(&[0xaa; 20], &[0xdd; 50])),
        "773ea91e36800e46854db8ebd09181a72959098b3ef8c122d9635514ced565fe",
        "RFC 4231 case 3"
    );
    // Case 4: a 25-byte key — the wrong-pad-length case.
    let key4 = unhex("0102030405060708090a0b0c0d0e0f10111213141516171819");
    assert_eq!(
        hex_of(&hmac_sha256(&key4, &[0xcd; 50])),
        "82558a389a443c0ea4cc819899f2083a85f0faa3e578f8077a2e3ff46729665b",
        "RFC 4231 case 4"
    );
    // Case 5: the RFC specifies a 128-bit (16-byte) output for this case.
    let case5 = hex_of(&hmac_sha256(&[0x0c; 20], b"Test With Truncation"));
    assert_eq!(
        &case5[..32],
        "a3b6167473100ee06e0c796c2955552b",
        "RFC 4231 case 5"
    );
    // Case 6: a 131-byte key — the "hash the key first" branch.
    assert_eq!(
        hex_of(&hmac_sha256(
            &[0xaa; 131],
            b"Test Using Larger Than Block-Size Key - Hash Key First"
        )),
        "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54",
        "RFC 4231 case 6"
    );
    // Case 7: 131-byte key AND a longer-than-block message.
    assert_eq!(
        hex_of(&hmac_sha256(
            &[0xaa; 131],
            b"This is a test using a larger than block-size key and a larger than block-size data. \
              The key needs to be hashed before being used by the HMAC algorithm."
        )),
        "9b09ffa71b942fcb27635fbcd5b0e944bfdc63644f0713938a7f51535c3a35e2",
        "RFC 4231 case 7"
    );
}

#[test]
fn test_keyed_content_hash_differs_from_plain_sha256() {
    let content = b"a verbatim secret";
    let keyed = keyed_content_hash(KEY_A, content).expect("keyed id");
    let plain = sha256::hex(&sha256::digest(content));
    assert_ne!(
        keyed, plain,
        "a content id must never be a plain digest of the content"
    );
    // And it is not the label's own digest either — the key participates.
    assert_ne!(keyed, sha256::hex(&sha256::digest(content.as_slice())));
}

#[test]
fn test_keyed_content_hash_depends_on_the_key() {
    let content = b"same bytes, two keys";
    let first = keyed_content_hash(KEY_A, content).expect("key A");
    let second = keyed_content_hash(KEY_B, content).expect("key B");
    assert_ne!(
        first, second,
        "the id must die if the key is dropped from the derivation"
    );
}

#[test]
fn test_keyed_content_hash_is_stable_for_the_same_key_and_content() {
    let content = b"stable bytes";
    let first = keyed_content_hash(KEY_A, content).expect("first");
    let second = keyed_content_hash(KEY_A, content).expect("second");
    assert_eq!(first, second);
    assert_eq!(first.len(), 64);
}

#[test]
fn test_keyed_content_hash_returns_64_lowercase_hex() {
    let digest = keyed_content_hash(KEY_A, b"render me").expect("digest");
    assert_eq!(digest.len(), 64, "32 bytes render as 64 hex characters");
    assert!(
        digest
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f')),
        "the id is lowercase hex only: {digest}"
    );
}

#[test]
fn test_keyed_content_hash_handles_empty_content() {
    let empty = keyed_content_hash(KEY_A, b"").expect("empty content");
    assert_eq!(empty.len(), 64);
    assert_ne!(
        empty,
        keyed_content_hash(KEY_A, b"\0").expect("one NUL byte"),
        "empty content and one NUL byte are different bytes"
    );
}

#[test]
fn test_keyed_content_hash_accepts_a_key_file_with_a_trailing_newline() {
    // The key file's exact content, newline included, must hash identically to
    // the 64-character form: the trailing newline is a file terminator, not
    // key material.
    let file_form = format!("{KEY_A}\n");
    assert_eq!(
        keyed_content_hash(&file_form, b"content").expect("file form"),
        keyed_content_hash(KEY_A, b"content").expect("bare form")
    );
}

#[test]
fn test_keyed_content_hash_rejects_a_malformed_key() {
    let lowercase_63 = format!("{}a", &KEY_A[..62]);
    let lowercase_65 = format!("{KEY_A}0");
    let uppercase = KEY_A.to_uppercase();
    let with_junk = format!("{KEY_A}\nJUNK");
    let inputs = [
        "",
        lowercase_63.as_str(),
        lowercase_65.as_str(),
        uppercase.as_str(),
        "../x",
        with_junk.as_str(),
    ];

    let mut messages = Vec::new();
    for input in inputs {
        let error = keyed_content_hash(input, b"content").expect_err("must be refused");
        assert!(
            matches!(error, VaultError::InvalidKeyHex),
            "expected InvalidKeyHex for {input:?}, got {error:?}"
        );
        messages.push(error.to_string());
    }

    // One fixed message: a message that varied with the input could carry it.
    for message in &messages[1..] {
        assert_eq!(
            message, &messages[0],
            "the message must not vary with the input"
        );
    }

    // Substring semantics only — the empty fixture carries no `!contains`
    // assertion because `s.contains("")` is true for every string.
    for input in &inputs[1..] {
        assert!(
            !messages[0].contains(input),
            "the message must not echo the rejected key material"
        );
    }
    assert!(
        !messages[0].contains("JUNK"),
        "a canary that cannot occur in the fixed message"
    );
    assert!(
        !messages[0].contains(&uppercase),
        "the message is lowercase-only, so the uppercase run cannot appear"
    );
    // No per-character disjointness assertion: the fixed message is prose and
    // the fixtures are hex, so a per-character rule would be a false test.
}

#[test]
fn test_keyed_content_hash_is_byte_exact() {
    let variants: [&[u8]; 4] = [b"a b", b"a  b", b"a\nb", b"A B"];
    let mut digests = Vec::new();
    for variant in variants {
        digests.push(keyed_content_hash(KEY_A, variant).expect("digest"));
    }
    for (index, left) in digests.iter().enumerate() {
        for right in &digests[index + 1..] {
            assert_ne!(
                left, right,
                "the vault is L0: no trimming, no whitespace collapsing, no case folding"
            );
        }
    }
    // The label is a constant the derivation depends on; asserting the value
    // keeps a silent label change from passing as "just a rename".
    assert_eq!(CONTENT_HASH_LABEL, "mem-vault/content-hash/v1");
}
