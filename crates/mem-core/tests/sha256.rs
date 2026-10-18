//! Port of the SHA-256 hash used by upstream `src/services/tags.ts::sha256`
//! (the single hash deriving memory scope / container-tag identity hashes:
//! `createHash("sha256").update(input).digest("hex").slice(0, 16)`).
//!
//! Upstream test file: none for the hash itself — the hash is exercised
//! indirectly through tags tests. Justified new coverage: the vendored
//! FIPS 180-4 implementation is pinned against the NIST / RFC 4634
//! example vectors (also cross-checked live against `sha256sum`), so the
//! drop-in-compatibility crux with Node's `createHash` is verified
//! byte-exactly without a supply-chain dependency.
//!
//! The inputs here are public identity strings (paths, emails, URLs) —
//! the hash is a compatibility identifier, not a security primitive.

use mem_core::sha256;

const NIST_EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
const NIST_ABC: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
const NIST_448_BIT: &str = "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1";
const NIST_896_BIT: &str = "cf5b16a778af8380036ce59e7b0492370b249b11e8f07a51afac45037afee9d1";
const NIST_MILLION_A: &str = "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0";

const NIST_448_BIT_INPUT: &str = "abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq";
const NIST_896_BIT_INPUT: &str = "abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu";

/// Upstream call shape: `createHash("sha256").update(x).digest("hex")` —
/// digest bytes, then lowercase hex rendering.
fn hexdigest(input: &[u8]) -> String {
    sha256::hex(&sha256::digest(input))
}

#[test]
fn test_sha256_matches_nist_empty_vector() {
    assert_eq!(hexdigest(b""), NIST_EMPTY);
}

#[test]
fn test_sha256_matches_nist_abc_vector() {
    assert_eq!(hexdigest(b"abc"), NIST_ABC);
}

#[test]
fn test_sha256_matches_nist_448_bit_vector() {
    assert_eq!(hexdigest(NIST_448_BIT_INPUT.as_bytes()), NIST_448_BIT);
}

#[test]
fn test_sha256_matches_nist_896_bit_vector() {
    assert_eq!(hexdigest(NIST_896_BIT_INPUT.as_bytes()), NIST_896_BIT);
}

#[test]
fn test_sha256_matches_nist_million_a_vector() {
    assert_eq!(hexdigest(&vec![b'a'; 1_000_000]), NIST_MILLION_A);
}

#[test]
fn test_sha256_basic_properties() {
    // Digest length is always 32 bytes.
    for input in [
        "",
        "a",
        "abc",
        "a".repeat(55).as_str(),
        "a".repeat(64).as_str(),
    ] {
        assert_eq!(sha256::digest(input.as_bytes()).len(), 32);
    }

    let digest_of = |bytes: &[u8]| sha256::digest(bytes);
    // Equal inputs give equal digests.
    assert_eq!(digest_of(b"abc"), digest_of(b"abc"));

    // Padding-boundary lengths all digest differently (55/56/57: one block
    // fits the 1-byte 0x80 divider just barely vs. spills to two; 63/64/65-class
    // pairs around the full multiple).
    let boundary_pair = |a: usize, b: usize| {
        assert_ne!(digest_of(&vec![b'a'; a]), digest_of(&vec![b'a'; b]));
    };
    boundary_pair(55, 56);
    boundary_pair(56, 57);
    boundary_pair(63, 64);
}

#[test]
fn test_sha256_hex16_prefix_matches_upstream_slice() {
    // The exact upstream container-tag form: hex(...).slice(0, 16).
    assert_eq!(&hexdigest(b"")[..16], "e3b0c44298fc1c14");
    assert_eq!(&hexdigest(b"abc")[..16], "ba7816bf8f01cfea");
    assert_eq!(hexdigest(b"abc").len(), 64);
    assert!(
        hexdigest(b"abc")
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    );
}
