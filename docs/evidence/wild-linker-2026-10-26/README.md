# wild linker — measurement and adoption, 2026-10-26/27

Record of why this repository pins the wild linker in `.cargo/config.toml`, and of the one flag that
makes it usable. Everything below is **MEASURED** on this machine unless labelled otherwise.

## Summary

| linker (same inputs, same machine) | debug link, 195 MB binary | link RSS |
|---|---|---|
| rust-lld (rustc's bundled lld 22.1.8 — the previous default here) | **1.89 – 2.01 s** | 1116 – 1142 MiB |
| wild 0.11.0, default flags | **7.8 – 25.3 s** | ~928 MiB |
| **wild 0.11.0 + `--no-madvise-huge-pages`** | **1.22 – 1.45 s** | 867 – 928 MiB |

So: wild is unusable at its defaults on this machine and about **35 % faster than rust-lld with ~20 % less
link memory** once huge pages are disabled on the output mapping. Release-profile links are equal in both
(2.24 – 2.36 s wild vs 2.33 – 2.44 s lld) — the effect is specific to the large debug binary.

## Root cause

wild 0.11.0 made two write-path changes default-on: `fallocate` + huge pages on `ext4`/`xfs`
(CHANGELOG 0.11.0, "Use fallocate and hugepages on ext4/xfs" #2468, "Enable hugepages by default for all
filesystems" #2508), and `MADV_HUGEPAGE` on the output mmap (`libwild/src/fs.rs`, `OutputFileDefaults`:
`madvise_huge_pages: true` unconditionally).

This machine's `target/` is **xfs**, and transparent huge pages are `[madvise]`:

```
$ stat -f -c %T .            → xfs
$ cat /sys/kernel/mm/transparent_hugepage/enabled   → always [madvise] never
$ cat /proc/sys/vm/dirty_bytes                      → 134217728   (128 MiB)
```

Faulting a 195 MB `MAP_SHARED` output in 2 MB pages drives direct compaction, which is why the cost is
both huge and non-deterministic (7.8 s and 25 s for the same link). `--no-madvise-huge-pages` removes it
completely and consistently.

`fallocate` is **not** the cause: disabling it alone changes nothing (see matrix), and the combination
`--no-huge --no-fallocate --no-update-in-place` is no faster than `--no-huge` alone.

## Method

- **Link time.** Force a relink with `touch src/main.rs`, then
  `cargo build -p opencode-agents-memory` — exactly one crate recompiles, so the wall clock is
  link-dominated; every run's log was checked for `Compiling opencode-agents-memory` (1 line) and
  `Finished … in N s`. For per-variant linker flags, `cargo rustc -p opencode-agents-memory
  --bin opencode-agents-memory -- <flags>` re-links one binary without touching dependency artefacts.
- **Link RSS.** `ps -eo rss=,comm=` sampled every 20 ms, keeping only `rust-lld` and `wild` (never
  `rustc`: an earlier sampler that included it reported the compiler's peak as the linker's, and those
  numbers were discarded).
- **Serialisation.** Every cargo call went through `.tmp/scripts/cargo-serialized.sh` (one compile at a
  time). The three fastest and three slowest wild runs were reproduced with the sampler removed
  entirely, so the measurement is not an artefact of the sampler.
- **Control.** rust-lld was re-measured in the same window as each wild run (`m-lld-r*.log`,
  `m-wild-nohuge-r*.log`), so machine load is shared by both sides of the comparison.

## The decisive flag matrix

Two reps each, wild 0.11.0, debug link (`v-*.log` — each log is the raw cargo output of one rep):

| flags | rep 1 | rep 2 |
|---|---|---|
| default | 16 451 ms | 7 853 ms |
| `--no-madvise-huge-pages` | **1 096 ms** | **1 132 ms** |
| `--no-fallocate-output-file` | 18 113 ms | 17 293 ms |
| `--no-update-in-place` | 24 194 ms | 7 433 ms |
| `--no-huge --no-fallocate --no-update-in-place` | 1 133 ms | 1 111 ms |
| `--strip-debug` | 4 326 ms | 6 041 ms |

## Negative and adjacent results

- **`--no-fork` does not fix it.** 14.3 / 3.8 / 20.1 s with `--no-fork`, 24.9 / 25.3 / 11.6 s with wild's
  default fork path. The variance is the hugepage stall, not the fork.
- **Bare `--threads` is a documented no-op** in wild (it sets the default), which matches: 16.7 / 17.0 /
  13.9 s. Default thread count is `available_parallelism()` capped at 32 — 12 here, so wild was already
  using all cores while rust-lld ran `--threads=2`.
- **`--strip-debug` reduces but does not remove the cost** (4.3 / 6.0 s with huge pages still on): output
  size matters to the stall, but disabling huge pages is what actually fixes it.
- **wild 0.6.0 did this link in 1.2 – 2.4 s** (three reps, before the upgrade). The 0.7→0.11 window is
  therefore where the regression lives, and 0.11.0's hugepage default is the documented change that
  matches the timing.
- **No changelog entry or open issue reports this.** A web search of the 0.6.0→0.11.0 CHANGELOG and the
  issue tracker found no split-DWARF or debug-link regression note; the only admitted 0.11.0 slowdown is
  COMDAT support. This is therefore an undocumented interaction of the new defaults with xfs + THP.

## Adoption

`.cargo/config.toml` (repo-local, tracked) pins:

```toml
[target.x86_64-unknown-linux-gnu]
linker = "clang"
rustflags = [
    "-Clink-arg=--ld-path=/home/vaz/.cargo/bin/wild",
    "-Clink-arg=-Wl,--no-madvise-huge-pages",
]
```

`linker = "clang"` is required because gcc's `-fuse-ld` accepts only named linkers, never a path.
`--no-madvise-huge-pages` is **required**, not a preference: without it the 10× regression returns.
Repo-local rather than `~/.cargo/config.toml` because machine-wide `--ld-path=wild` does not compile an
aya-ebpf project on this machine; scoping it here leaves that build alone.

Revert is `rm .cargo/config.toml` — rustc falls back to its bundled rust-lld.

## Adoption gate (2026-10-27, MEASURED)

With the config above in force, on this tree:

| check | result |
|---|---|
| `cargo test --workspace` | **exit 0** — 231 result lines, **1840 passed, 0 failed** (447 s including the full rebuild the rustflags change forces) |
| clippy gate 1 (ADR-0013 D-5, `--lib` panic families) | exit 0 |
| clippy gate 2 (ADR-0016 D-3, `--all-targets -D warnings`) | exit 0 |
| `readelf -p .comment` on the root binary **and** a test binary | `clang version 21.1.8` in both — i.e. wild linked the artefacts, not a silent fallback |
| wild invocations observed | 242 |

Raw log: `adoption-gate-workspace.log`.

## What this does NOT establish

- Only the 195 MB debug binary was timed. Small test binaries, proc-macros and build-script links were
  not separately measured; the win there is **INFERRED** to be smaller.
- Release links were timed (equal) but not RSS-measured.
- One machine, one filesystem (xfs). On ext4 or without THP the defaults may behave differently; the flag
  is harmless either way but the 10× is not reproducible elsewhere.
- wild 0.11.0 is a young linker. Correctness here rests on `purge_cli` 21/21 with a wild-linked binary
  (`correct-purge.log`) plus the full-workspace battery run as the adoption gate; it is not a proof of
  link-level equivalence with lld, and the binaries differ in size (192 495 528 vs 195 493 096 bytes).

## Files

`results-final.txt` (the earlier lld/wild comparison, superseded by `m-*.log`), `diag-time.log` (a full
`--time` tree, showing `Link 20350.07 ms` and the phase split), `m-lld-r{1,2,3}.log`,
`m-wild-nohuge-r{1..5}.log`, `v-*.log` (the flag matrix), `correct-purge.log`.
