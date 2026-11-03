# 116 — `cargo audit` is red: three `zstd` vulnerability advisories and ten `zstd-safe` unsound ones

Status: done — 2026-11-03 — landed at `ef4dc6d` (Path A: `Cargo.lock` only, `zstd 0.13.3 → 0.13.4` and
`zstd-safe 7.3.0 → 7.4.0`; `cargo audit --no-fetch` from the repo root exits 0 and prints no findings —
13 advisories gone, `.cargo/audit.toml` unchanged because all three of its accepted IDs still fire).

_Original:_ ready-for-agent — the fix is a dependency bump plus a decision about `.cargo/audit.toml`;
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

## Comments

### 2026-11-03 — Path A landed at `ef4dc6d` (MEASURED)

The owner ruled the bump in: issue 58's "hold the gate red" rested on "no in-range fix exists", and
that premise expired once the advisory DB named patched ranges.

**The update (MEASURED, repo root):**

```
$ cargo update -p zstd --precise 0.13.4
    Updating crates.io index
    Updating zstd v0.13.3 -> v0.13.4
    Updating zstd-safe v7.3.0 -> v7.4.0
    note: pass `--verbose` to see 58 unchanged dependencies behind latest
```

`git diff -- Cargo.lock` is four changed lines in two `[[package]]` blocks — the two versions and the
two checksums. `zstd-sys` does not move, no other row is touched, and no `Cargo.toml` needed editing
(`tantivy-sstable` wants `zstd ^0.13`; `zstd 0.13.4` wants `zstd-safe ^7.1.0`).

**The gate (MEASURED, repo root):**

```
$ cargo audit --no-fetch ; echo rc=$?
      Loaded 1403 security advisories (from /home/vaz/.cargo/advisory-db)
    Scanning Cargo.lock for vulnerabilities (495 crate dependencies)
rc=0
```

A clean run in cargo-audit 0.22 prints no `error:`/`warning:` summary line at all — silence is the pass.
The 3 vulnerability findings (zstd 0391/0398/0403) and the 10 unsound ones (zstd-safe 0395–0406) are
absent; `.cargo/audit.toml` was **not** edited.

**Drift check on the accepted findings (MEASURED, per `.cargo/audit.toml`'s own procedure):** the bumped
`Cargo.lock` copied into a config-less directory (outside the repo, so no ancestor `.cargo/audit.toml`)
and audited with `cargo audit -n`:

```
Crate:     fxhash   Version: 0.2.1  Warning: unmaintained  ID: RUSTSEC-2025-0057
Crate:     paste    Version: 1.0.15 Warning: unmaintained  ID: RUSTSEC-2024-0436
Crate:     lru      Version: 0.16.4 Warning: unsound       ID: RUSTSEC-2026-0253
warning: 3 allowed warnings found   (rc=0)
```

Exactly the three accepted IDs, each in the class the file records — no stale ignore, no reclassification,
no new finding. Nothing to remove.

**The battery on the bumped tree (MEASURED):**

- `cargo test --workspace`: exit 0, **297 result lines, 2391 passed / 0 failed / 31 ignored** — identical
  to the `e9cfde9` baseline.
- clippy gate 1/2 verbatim from `.githooks/pre-commit` (`--workspace --lib -- -A clippy::all
  -D clippy::unwrap_used -D clippy::expect_used`): exit 0.
- clippy gate 2/2 verbatim (`--workspace --all-targets -- -D warnings -A clippy::await_holding_lock
  -A clippy::assertions_on_constants -A clippy::doc_lazy_continuation`): exit 0.
- `cargo fmt --all --check`: exit 0.

**One discarded run, recorded for honesty.** The first `cargo test --workspace` on the bumped tree ended
`FAILED. 20 passed; 2 failed` in `mem-portability --test portability_roundtrip`
(`roundtrip_durable_tiers`, `roundtrip_without_staging_restores_milestones`, both
`ImportAbortedMilestones { count: 1 }`). Cause was a second `cargo test --workspace` running concurrently
in the same checkout's target directory: `scratch_root()` in that test file builds a **fixed** path from a
label (`<target>/tmp/scratch/mem-portability/<label>`) and `remove_dir_all`s it, so two processes stomp
each other's store and the second one sees the first one's `mile-1` milestone row. The serial re-run above
is green with the same binary, and the concurrently-running peer run also finished 2391/0/31, so the
failure is an interference artifact, not a `zstd` behaviour change. No test file was modified.
