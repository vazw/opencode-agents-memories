# 101 — `cargo audit` is red: three `zstd` vulnerability advisories and ten `zstd-safe` unsound ones

Status: ready-for-agent — the fix is a dependency bump plus a decision about `.cargo/audit.toml`;
it needs a network run, which no wave in this repo has done yet.

## Observation (MEASURED 2026-11-03, repo root)

```
$ cargo audit --no-fetch
error: 3 vulnerabilities found!
warning: 10 allowed warnings found
```

The chain, from `Cargo.lock` (MEASURED, graph walk):

```
turso_core → tantivy → tantivy-columnar → tantivy-sstable 0.7.0 → zstd 0.13.3 → zstd-safe 7.3.0
```

- **`zstd 0.13.3` — three `vulnerability`-class findings:** RUSTSEC-2026-0391 (stream
  `with_context` cross-stream data leak), RUSTSEC-2026-0398 (`seekable::Seekable::decompress`),
  RUSTSEC-2026-0403 (`DCtx::decompress_stream` legacy-frame uninitialised memory).
- **`zstd-safe 7.3.0` — ten `unsound` advisories:** 0395, 0396, 0397, 0399, 0400, 0401, 0402, 0404,
  0405, 0406 — `try_clone` double-free, seekable out-of-bounds read, MT/dictionary use-after-free,
  uninitialised-memory exposure among them.

**Pre-existing, not a regression:** no wave's diff contains a `Cargo.toml` or `Cargo.lock` change
(`git diff --name-only 5956bcb..HEAD -- 'Cargo.toml' 'Cargo.lock'` is empty across every 2026-11-03
wave).

**Reachability (INFERRED, not measured at runtime):** the only in-tree consumer is
`tantivy-sstable`, which uses `zstd::bulk::{Compressor, Decompressor}` and `zstd_safe::compress_bound`
and never calls `try_clone`, `seekable::*`, `NbWorkers` or any dictionary API. `zstd` *is* built with
`features = ["experimental"]`, so the feature gate on 0404 is open even though the function is never
called. The three vulnerability-class findings are reachable only through streaming APIs tantivy does
not use.

## What the repo says about accepted findings

`.cargo/audit.toml` is the sole file `cargo audit` reads accepted findings from, and it documents its
own drift check. Today it does not carry these, which is why the gate is **red** rather than noisy.

## Acceptance criteria

- `cargo audit --no-fetch` exits `0` from the repo root, either because the advisories are gone or
  because every remaining one is listed in `.cargo/audit.toml` with a justification.
- Path A (preferred, needs network): `cargo update -p zstd --precise 0.13.4` and
  `cargo update -p zstd-safe --precise 7.4.x` — both in range (`zstd 0.13.3` requires `zstd-safe
  ^7.1.0`; `tantivy-sstable` requires `zstd ^0.13`). Verify with a real update run, not from these
  ranges.
- Path B: ten `unsound` ignores plus three vulnerability dispositions in `.cargo/audit.toml`, each
  with the reachability argument above and a re-check trigger.
- The workspace suite and both clippy gates stay green either way — a `zstd` bump touches the
  turso/tantivy path the episodic store sits on.
