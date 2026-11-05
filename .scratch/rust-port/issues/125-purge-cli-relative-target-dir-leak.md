# 125 — `purge_cli` spawns `cargo build` with an inherited relative `CARGO_TARGET_DIR`, leaking 3.8 GB into the package tree

Status: done — 2026-11-05

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

- [x] 1. `purge_cli.rs` passes an absolute path (`std::env::temp_dir()`-rooted or the workspace target resolved from
   `CARGO_MANIFEST_DIR`) to its `cargo build` child, or explicitly unsets `CARGO_TARGET_DIR` for it.
- [x] 2. A test that runs the spawn from a **different** cwd with a relative `CARGO_TARGET_DIR` set and asserts no
   directory is created under `crates/mem-bridge/.tmp/` — the killing mutant is "inherit the env unchanged",
   which the current test cannot distinguish from correct behaviour because it never varies the cwd.
- [x] 3. Record the disk evidence: `du -sh crates/*/.tmp` after a full `cargo test --workspace` should print nothing.

The two leaked trees were moved to `.tmp/scratch/leaked-target-crates-bridge{,-2}` rather than deleted, so the
sizes above are still measurable there.

## Comments

### 2026-11-05 — Closeout (Issue 125)
- **Target normalization implemented**: In `crates/mem-bridge/tests/purge_cli.rs`, `workspace_root()`, `resolve_target_dir()`, and `build_root_binary_cmd()` normalize relative `CARGO_TARGET_DIR` to an absolute path anchored at the workspace root (`CARGO_MANIFEST_DIR` ancestors 2) and set child working directory to `workspace_root`.
- **Integration & unit tests**: 5 unit tests verify path normalization and command builder behavior; integration test `test_relative_cargo_target_dir_does_not_leak_into_crate_tree` runs `cargo build` with a relative `CARGO_TARGET_DIR` (`.tmp/scratch/issue-125-probe` symlinked to active target) and asserts `!manifest_dir.join(".tmp").exists()`. All 28 tests in `purge_cli.rs` pass in 15.6s.
- **Mutant kill proof**: Target mutant omitting `cmd.current_dir(workspace_root)` and passing relative `target_dir_env` raw was verified. The mutant failed `test_build_root_binary_cmd_sets_absolute_target_dir_when_relative` in 0.01s and created `crates/mem-bridge/.tmp/`. After cleanup and reverting to GREEN, the test passed cleanly and `crates/mem-bridge/.tmp/` remained absent.
- **Disk audit**: `du -sh crates/*/.tmp` verifies no `.tmp` directories exist under any crate root (exits 1: No such file or directory).
- **Future hardening**: `crates/mem-bridge/tests/reembed_scope_cli.rs:146` shares the identical `binary_path()` child cargo spawn pattern and can be hardened using the same helpers.

