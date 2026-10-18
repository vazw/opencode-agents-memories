//! Vendored, std-only SHA-256 (FIPS 180-4 "Secure Hash Standard").
//!
//! Provenance & verification: the FIPS 180-4 / RFC 4634 example vectors are
//! pinned in `crates/mem-core/tests/sha256.rs` and were additionally
//! cross-checked live against GNU `sha256sum` on this machine before
//! implementation was pinned.
//!
//! Single upstream consumer: `src/services/tags.ts::sha256`
//! (`createHash("sha256").update(input).digest("hex").slice(0, 16)`) — the
//! only hash that derives container-tag identity hashes. Call sites take
//! `&hex(...)[..16]` for byte-exact upstream parity.
//!
//! Inputs are public identity strings (project paths, emails, origin URLs);
//! this is a compatibility hash for identifier stability, NOT a security
//! primitive — no keyed/secret use, no side-channel surface.
//!
//! Implemented in safe Rust: no `unsafe`, no panicking indexing. The K/H₀
//! constants are NOT transcribed from a printed table — they are derived at
//! compile time from the standard's exact mathematical definition (§4.2.2 /
//! §5.3.3: fractional parts of the cube roots of the first 64 primes, and
//! square roots of the first 8 primes) by exact integer binary search, so a
//! typo can introduce no drift; the NIST vectors then pin the whole
//! algorithm byte-exactly.

const fn is_prime(n: u32) -> bool {
    if n < 2 {
        return false;
    }
    let mut i: u32 = 2;
    while i * i <= n {
        if n.is_multiple_of(i) {
            return false;
        }
        i += 1;
    }
    true
}

/// First `N` primes (the standard needs the first 64, all < 320).
const fn first_n_primes<const N: usize>() -> [u32; N] {
    let mut primes = [0u32; N];
    let mut count = 0;
    let mut n: u32 = 2;
    while count < N {
        if is_prime(n) {
            primes[count] = n;
            count += 1;
        }
        n += 1;
    }
    primes
}

/// Integer cube root (largest `r` with `r³ ≤ p`); primes here are ≤ 313.
const fn integer_cbrt(p: u32) -> u64 {
    let mut r: u64 = 0;
    while (r + 1) * (r + 1) * (r + 1) <= p as u64 {
        r += 1;
    }
    r
}

/// Integer square root (largest `r` with `r² ≤ p`).
const fn integer_sqrt(p: u32) -> u64 {
    let mut r: u64 = 0;
    while (r + 1) * (r + 1) <= p as u64 {
        r += 1;
    }
    r
}

/// The K/H₀ word for prime `p`: `floor(frac(root(p)) · 2³²)`, computed by
/// exact integer binary search. For a prime `p`, `root(p)` is irrational, so
/// the strict/weak comparisons below are stable at every boundary:
///
/// - integer part `R` = integer root of `p`;
/// - `k` is the largest value in `[0, 2³²)` with `(R + k/2³²)^exp ≤ p`
///   (equivalently `(R·2³² + k)^exp ≤ p·2^(32·exp)`), i.e.
///   `k = floor(frac(root(p)) · 2³²)` — "the first 32 bits of the fractional
///   part" in FIPS wording.
///
/// Increments are `u128`-safe: `R+1 ≤ 7` for `∛313`, so the terms stay
/// below `2^101`.
const fn frac_root_bits<const EXP: u32>(p: u32) -> u128 {
    let target: u128 = (p as u128) << (32 * EXP as u128);
    let root: u128 = if EXP == 3 {
        integer_cbrt(p) as u128
    } else {
        integer_sqrt(p) as u128
    };
    let base: u128 = root << 32;
    let mut lo: u128 = 0; // invariant: (base + lo)^EXP <= target
    let mut hi: u128 = u32::MAX as u128; // invariant: (base + hi)^EXP > target
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        let t = base + mid;
        let mut powered: u128 = 1;
        let mut i = 0;
        while i < EXP {
            powered *= t;
            i += 1;
        }
        if powered <= target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

/// FIPS 180-4 §4.2.2 round constants derived at compile time from the
/// standard's definition (fractional cube roots of the first 64 primes).
pub(crate) const K: [u32; 64] = {
    let primes = first_n_primes::<64>();
    let mut k = [0u32; 64];
    let mut i = 0;
    while i < 64 {
        k[i] = frac_root_bits::<3>(primes[i]) as u32;
        i += 1;
    }
    k
};

/// FIPS 180-4 §5.3.3 initial state derived at compile time (fractional square
/// roots of the first 8 primes).
pub(crate) const H0: [u32; 8] = {
    let primes = first_n_primes::<8>();
    let mut h = [0u32; 8];
    let mut i = 0;
    while i < 8 {
        h[i] = frac_root_bits::<2>(primes[i]) as u32;
        i += 1;
    }
    h
};

/// The σ₀/σ₁/Σ₀/Σ₁ and padding per FIPS 180-4 §6.2.2, applied to one
/// 512-bit block against the working state.
fn compress(state: &mut [u32; 8], block: &[u8; 64]) {
    let mut w = [0u32; 64];
    let mut i = 0;
    while i < 16 {
        w[i] = u32::from_be_bytes([
            block[4 * i],
            block[4 * i + 1],
            block[4 * i + 2],
            block[4 * i + 3],
        ]);
        i += 1;
    }
    i = 16;
    while i < 64 {
        let w15 = w[i - 15];
        let w2 = w[i - 2];
        let s0 = w15.rotate_right(7) ^ w15.rotate_right(18) ^ (w15 >> 3);
        let s1 = w2.rotate_right(17) ^ w2.rotate_right(19) ^ (w2 >> 10);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
        i += 1;
    }

    let mut a = state[0];
    let mut b = state[1];
    let mut c = state[2];
    let mut d = state[3];
    let mut e = state[4];
    let mut f = state[5];
    let mut g = state[6];
    let mut h = state[7];

    i = 0;
    while i < 64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ (!e & g);
        let t1 = h
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(K[i])
            .wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);

        h = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
        i += 1;
    }

    let add = [a, b, c, d, e, f, g, h];
    let mut i = 0;
    while i < 8 {
        state[i] = state[i].wrapping_add(add[i]);
        i += 1;
    }
}

/// FIPS 180-4 SHA-256 of `input` (64-round big-endian compression, standard
/// message-length padding).
pub fn digest(input: &[u8]) -> [u8; 32] {
    let mut state = H0;
    let bit_len = (input.len() as u64).wrapping_mul(8);

    let (full_blocks, remainder) = input.as_chunks::<64>();
    for block in full_blocks {
        compress(&mut state, block);
    }

    // Final message: remainder | 0x80 | zero padding | 64-bit big-endian
    // bit length. When the remainder leaves no room for a 1-byte 0x80 plus
    // 8 length bytes (≥ 56), the length lands in a second zero block.
    let rem: &[u8] = remainder;
    let mut tail = [0u8; 64];
    tail[..rem.len()].copy_from_slice(rem);
    tail[rem.len()] = 0x80;
    if rem.len() < 56 {
        tail[56..64].copy_from_slice(&bit_len.to_be_bytes());
        compress(&mut state, &tail);
    } else {
        compress(&mut state, &tail);
        let mut tail2 = [0u8; 64];
        tail2[56..64].copy_from_slice(&bit_len.to_be_bytes());
        compress(&mut state, &tail2);
    }

    let mut out = [0u8; 32];
    let mut i = 0;
    while i < 8 {
        out[4 * i..4 * i + 4].copy_from_slice(&state[i].to_be_bytes());
        i += 1;
    }
    out
}

/// Lowercase hex rendering, 64 characters, each byte `{:02x}`.
pub fn hex(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len() * 2);
    for byte in input {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

#[cfg(test)]
mod unit {
    use super::*;

    #[test]
    fn derived_constants_equal_the_precomputed_standard_values() {
        // A compile edge: the derived constants must be stable byte tables.
        assert_eq!(K[0], 0x428A_2F98);
        assert_eq!(K[63], 0xC671_78F2);
        assert_eq!(H0[0], 0x6A09_E667);
        assert_eq!(H0[7], 0x5BE0_CD19);
    }

    #[test]
    fn digest_matches_hex_shortcut() {
        let d = digest(b"abc");
        assert_eq!(
            hex(&d),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
