# 125 — `purge_cli` spawns `cargo build` with an inherited relative `CARGO_TARGET_DIR`, leaking 3.8 GB into the package tree

Status: needs-triage — measured 2026-11-03 during the Wave 1 implementation (`fc82366..ec4891e`).

## What happened

`crates/mem-bridge/tests/purge_cli.rs` shells out to `cargo build` for the purge CLI. When the outer
process's `CARGO_TARGET_DIR` is **relative**, the child inherits it verbatim and resolves it against its own
cwd — the package root — so the build lands in `crates/mem-bridge/.tmp/<relative path>` instead of the
workspace target.

Measured: **two** directories of **3.8 GB each** appeared under `crates/mem-bridge/.tmp/` during one
implementation session. They are gitignored (`.tmp/`) and were never staged, so nothing entered history; the
cost is disk and the surprise of a multi-GB build inside a crate directory.

## Why it is a real defect and not a workstation accident

- The test decides where its child builds; it should pass an **absolute** target dir, or none at all and let
  cargo resolve the workspace target.
- It reproduces whenever a caller uses a relative `CARGO_TARGET_DIR` — which is exactly what this repo's
  agents are told to do (`.tmp/scratch/target-*`), so the two conventions collide.
- `crates/mem-bridge/.tmp/` is inside the crate's own tree: a `cargo package`/`cargo publish` dry run or a
  naive `du -sh crates/*` reads it as crate weight.

## What would close it

1. `purge_cli.rs` passes an absolute path (`std::env::temp_dir()`-rooted or the workspace target resolved from
   `CARGO_MANIFEST_DIR`) to its `cargo build` child, or explicitly unsets `CARGO_TARGET_DIR` for it.
2. A test that runs the spawn from a **different** cwd with a relative `CARGO_TARGET_DIR` set and asserts no
   directory is created under `crates/mem-bridge/.tmp/` — the killing mutant is "inherit the env unchanged",
   which the current test cannot distinguish from correct behaviour because it never varies the cwd.
3. Record the disk evidence: `du -sh crates/*/.tmp` after a full `cargo test --workspace` should print nothing.

The two leaked trees were moved to `.tmp/scratch/leaked-target-crates-bridge{,-2}` rather than deleted, so the
sizes above are still measurable there.
