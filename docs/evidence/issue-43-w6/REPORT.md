# Issue 43 W6 — erasure close-out (W6a + W6c)

Blueprint: `.tmp/blueprints/blueprint-20261108-w6-erasure-r1.md` (stamp
`sha256=1c8ab8d40e11…`). Branch `issue-43/w6c-erasure`, base `4878834` (the W6a
merge). Every number below was re-derived from the run named beside it.

## What landed

| wave | scope | commits |
|---|---|---|
| W6a | `MultiprocessWal` dropped from `ConnectOptions::for_config` (`[Encryption]` on every platform), `decide` inverted, the `-tshm` sensor wired, ADR-0025 written `Proposed` | `3db778d`, `8b44e56`, `d62edb0` (branch `issue-43/w6a-substrate`, merged at `4878834`) |
| W6c | the sweep (stamp migrations, `Vacuum`/`for_sweep`, preflight, runner, `.bak` reaper, CLI dispatch), the milestone counter, the disclosure DTO/route, the web-ui copy, the canary battery | `ac22307` (C1), `ed2c11a` (C2), `ab5dde8` (C3), `1435d63` (C4), `647d5e6` (C6), `3755eb0` (C7), C8 = this commit |

Commit-plan deviations, recorded rather than hidden:

* **C5 is folded into C3.** The Step-17 canary cases live in the same two files
  as the runner's own contract (`crates/mem-bridge/tests/sweep.rs`,
  `crates/mem-store/tests/open_guard.rs`), so they cannot be split by file
  without partial staging. The canary output is recorded separately
  (`sweep-canary.txt`).
* **C2 also carries the W6a review's P2-1 remediation** (the read-only
  divergence seam test): it belongs to the open-guard substrate, which is C2's
  subject.

## Test counts (final gates)

| gate | result | log |
|---|---|---|
| `cargo test --workspace` | **313 suites, 2538 passed, 0 failed, 33 ignored** | `.tmp/scratch/w6c-workspace.log` |
| `cargo test -p mem-web` | **43 suites ok, 0 failed** | `.tmp/scratch/w6c-step19-green.log` |
| `cargo test -p mem-bridge --test sweep` | **9 passed, 0 failed** | `.tmp/scratch/w6c-step17-canary.log` |
| `cargo test -p mem-store --test open_guard` | **29 passed, 0 failed, 1 ignored** | `.tmp/scratch/w6c-carried-p2.log` |
| `cargo test -p mem-episodic --test milestone_compaction` | **3 passed, 0 failed** | `.tmp/scratch/w6c-step16-green.log` |
| `cargo test -p opencode-agents-memory --test engine_shutdown` | **11 passed, 0 failed** | `.tmp/scratch/w6c-engine-shutdown.log` |
| `cargo fmt --all --check` (+ `web-ui`) | clean | `.tmp/scratch/w6c-fmt.log` |
| clippy `--lib -D unwrap_used -D expect_used` | exit 0 | `.tmp/scratch/w6c-clippy-lib.log` |
| clippy `--all-targets -D warnings` | exit 0 | `.tmp/scratch/w6c-clippy-all.log` |
| `cd web-ui && trunk build` | exit 0, `✅ success` | `.tmp/scratch/w6c-step20-trunk.log` |
| `cargo test --target wasm32-unknown-unknown --test confirm` | **6 passed, 0 failed** | `.tmp/scratch/w6c-step20-wasm.log` |

The scratch logs are gitignored (`.tmp/`); every one of them is reproducible with
the command in its row.

## RED → GREEN windows (all MEASURED, in this tree)

| step | RED | GREEN |
|---|---|---|
| 9–10 sweep stamp | 0/8 passed, `.tmp/scratch/w6c-sweep-stamp-red.log` | 8/8 |
| 11 vacuum cell | `.tmp/scratch/w6c-step11-red.log` | `mem-store` green |
| 12 sweep probe | E0425 ×7, `.tmp/scratch/w6c-step12-red.log` | 57 lib tests passed |
| 13–14 runner | `E0432: unresolved import mem_bridge::sweep`, `.tmp/scratch/w6c-step13-red.log` | 9/9 |
| 16 milestone counter | E0433 ×3, `.tmp/scratch/w6c-step16-red.log` | 3/3 |
| 18–19 disclosure | `E0432: not_erased_map`, `.tmp/scratch/w6c-step18-red.log` | 43 suites ok |
| carried P2-1 seam | mutant below | 29 passed |

### Mutation log

| mutant | victim | result |
|---|---|---|
| `Writability::from_options` inverted (read-only → `Writable`) | `test_the_read_only_divergence_row_is_wired_at_the_open_seam` | **killed**: `0 passed / 1 failed` (`.tmp/scratch/w6c-carried-p2-mutant.log`); file restored from `.tmp/scratch/open_guard.rs.bak` |

## Canary

See `sweep-canary.txt`: the marker bytes are proven present in each tier (or its
WAL) BEFORE the sweep, absent from all three databases, all three WALs and the
reaped `.bak` AFTER it, and each tier still answers and is stamped.

## The `vacuum-under-legacy-holder` cell

`needs_validation`, never fabricated: see
`NEEDS_VALIDATION-vacuum-legacy-holder.md`. Post-W6a no process in this tree can
open flag-on, and the sweep holds the engine lock, so the cell is unreachable by
construction without a purpose-built pre-drop binary.

## Windows / macOS

**Documented untested.** No runner exists for either platform in this
repository. The post-drop open guard's Windows cell
(`Divergence::NoProbeOnPlatform`) and the macOS POSIX-lock flavour are table
rows and unit cells only; no measurement was taken and none is claimed.

## D1/D2/D3 findings carried from the blueprint

* **D1** — `secure_delete` does not exist in this engine; the recovery is
  `wal_checkpoint(TRUNCATE)` + `VACUUM` under `experimental_vacuum(true)`, which
  the sweep now performs per target.
* **D2** — the stamp table is the disclosure's source of truth: a tier with no
  row answers `notYetSwept`, so `notYetSwept` is data, not a constant
  (`test_sweep_state_is_empty_before_any_sweep`).
* **D3** — `milestones_scrubbed` is the shipped spelling (ADR-0019 amended); a
  delete reports 0 by construction because it rewrites no milestone.

## Residuals (named, not closed)

1. The pre-issue-45 binary blind spot: a still-launchable pre-drop binary can
   open the same file flag-on. The guard refuses it when it holds the `-tshm`
   (`test_post_flip_open_succeeds_refuses_a_flag_on_holder_and_clears_when_free`)
   but a peer that holds nothing is invisible — ADR-0025's accepted posture.
2. `vacuum-under-legacy-holder` unmeasured/unreachable (above).
3. Windows post-drop opens untested (above).
4. ADR-0025 stays `Proposed` — owner review has not landed.
5. A3/A4 are fenced to issue 131.
