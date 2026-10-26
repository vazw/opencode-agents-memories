# sccache adoption — measurement record (2026-10-26)

Why this directory exists: the admissibility decision for sccache rests on measurements that
originally lived only in the gitignored `.tmp/`. AGENTS.md is explicit that a ruling recorded only
there is one `rm -rf` from gone, so the logs and the scripts that produced them are promoted here
verbatim and unedited.

**Artefacts**

| file | what it is |
|---|---|
| `sccache-ab.txt` | round 1 run log: cases A, B, C, D plus the debug-info check |
| `sccache-ef.txt` | round 2 run log: E1 (populate) and E2 (the fleet case) |
| `sccache-ab.sh` | script that produced `sccache-ab.txt` |
| `sccache-ef.sh` | script that produced `sccache-ef.txt` |

Environment: `sccache 0.18.0` (Void `rust-sccache-0.18.0_1`), `rustc 1.98.0`, repo at `fa8012e`,
dev profile with `debug = 1` and `split-debuginfo = "unpacked"`. Workload in every case:
`cargo test --workspace --no-run`, so build and link only, no test execution.

## Results

| case | setup | wall | peak RSS (sampled) | Rust hit rate |
|---|---|---|---|---|
| A | no sccache, cold | 437.5 s | 2870 MiB | — |
| B | sccache, cold (population) | 431.7 s | 2616 MiB | 0.00 % |
| C | `cargo clean`, warm cache | 312.5 s | 2756 MiB | 98.90 % |
| D | fresh target dir, no `BASEDIRS` | 388.4 s | 3018 MiB | **0.00 %** |
| E1 | populate under `BASEDIRS` + no incremental | 390.3 s | 2782 MiB | 0.00 % |
| E2 | fresh target dir + `BASEDIRS` + no incremental | **277.0 s** | 2791 MiB | **96.30 %** |

**~1.4x as configured, ~1.58x with `BASEDIRS` and `CARGO_INCREMENTAL=0`**, for a **1.2 GiB** cache
against a 25 GB `target/`.

## Method caveat — sampled, not exact

Void ships no `/usr/bin/time`, so peak RSS is **sampled**: `ps` every 200 ms, summed over
`cargo`/`rustc`/`cc`/`clang`/`ld.lld`. Sampled maxima **undershoot** true peaks. The figures are
comparable between cases, which is all the comparison needs; they are not absolute.
Wall time is exact.

## Three negative results, each of which changed the story

1. **No RAM benefit.** Peak RSS is flat (2616–3018 MiB) across all six runs. 325–338 calls per
   workspace build are non-cacheable and the expensive link steps still run. **The motive for
   looking at sccache — linker memory — is not addressed by it.** `debug = 1` is the lever for
   that, and its effect on link RAM is *not measured here*.
2. **No cross-worktree sharing.** A build in a git worktree at a different absolute *source* root
   misses **even when both roots are listed in `SCCACHE_BASEDIRS`** (probe: 1 compile, 0 hits).
   `BASEDIRS` normalises *target-dir* differences — which is why E2 hits — but not two distinct
   source roots. D's zero is **365 Rust misses**, not refusals: the compilations ran and were
   stored under different keys.
3. **`CARGO_INCREMENTAL=0` is required for our own crates to be cacheable** (229 calls in a
   workspace build are non-cacheable for reason `incremental`). It was therefore scoped to the
   fleet's `cargo-serialized.sh`, **not** set machine-wide: forcing it globally would slow every
   single-file edit loop, and this measurement never isolated its contribution from `BASEDIRS`.

## Adopted configuration

`~/.cargo/config.toml` (backup: `config.toml.bak-sccache`):

```toml
[build]
rustc-wrapper = "sccache"

[env]
SCCACHE_CACHE_SIZE = "10G"
SCCACHE_BASEDIRS = "/home/vaz/Code/Personal/Rust"
```

plus `CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"` exported inside the fleet's
`cargo-serialized.sh` (opt out with `CARGO_INCREMENTAL=1`).

Verified end-to-end with no shell environment: a wiped `libmem_core` rebuild hits the cache
(1.59 s → 0.30 s), and `sccache --show-stats` reports the configured `Base directories`.

## Debug info survives a cache hit

`split-debuginfo = "unpacked"` emits per-CGU `.dwo` sidecars, so a cache hit that restored the
binary but not the sidecars would break debug info silently, and only on hits. Measured on the
rebuilt test binary: 12 `.debug_*` sections, 17,997 `.dwo` files under `deps/`,
`readelf --debug-dump=info` parses cleanly, and a panicking probe compiled twice resolves
`p.rs:1` on the **second (cached)** compile.

## Operational gotchas (each cost time to find)

- `sccache --zero-stats` **from a bare shell starts a server without the configured environment**;
  later builds then report `Base directories (none)` and stop hitting. Correct order:
  `sccache --stop-server`, then let `cargo` start it so it inherits the cargo `[env]` table.
- sccache 0.18.0 has **no `--show-config` and no `--clear`** (`error: unexpected argument`). The
  Earthly article documents a newer surface.
- sccache treats an explicit `-o` as non-cacheable, and `--out-dir` on a hand-built rustc line is
  parsed as "multiple input files". Hand-rolled `sccache rustc …` probes are useless for hit
  testing — test through `cargo`.
- `Compilation failures 3` appears in several runs while cargo exits 0. **Unexplained.** Most
  likely build-script capability probes; recorded rather than explained away.

## Open questions

- How much of the 1.58x is `CARGO_INCREMENTAL=0` rather than `BASEDIRS` — not isolated.
- Whether 0.19.0 restores `--show-config`/`--clear`, or shares across source roots.
- `Compilation failures 3` with cargo exiting 0.
- Whether machine-wide `CARGO_INCREMENTAL=0` is a net win for single-file edit loops.
- The pre-commit gate's ≈35 s warm cost under the wrapper — plausible unchanged (link steps
  untouched), not measured.
