# Issue 141 — Auto-cleanup status row in the Settings maintenance section

Blueprint `blueprint-20261118-2207-issue-141-cleanup-status-row` (`sha256=c642072032df`, SYS-APPROVED).
Executed at HEAD `6c80902` on 2026-11-18 (UTC 2026-11-18 05:20–12:20). No commit was made. The
tree holds seven `web-ui/` paths plus the ledger; `web-ui/.tmp/` is untracked scratch and not evidence.

## What changed

- `web-ui/src/paths.rs`: `EndpointId::CleanupStatus` with `GET /api/cleanup/status`, ENDPOINTS row,
  tripwire count 37 → 38. `SETTINGS_PATHS` is **unchanged** (review 1 D-2: reverted, see Residues).
- `web-ui/src/core_screens.rs`: `cleanup_status_spec`, `CleanupAutoStatus` (camelCase, all fields
  required except `last_run_ms`), `CleanupAutoOutcome`, `cleanup_auto_outcome` (501 →
  `Unavailable`, malformed or other failure → `Failed`), `cleanup_last_run_label` (`None` → `never`),
  `cleanup_disabled(state, auto_running)`, `CleanupAutoState`, `fetch_cleanup_status`, and the
  `SettingsPanel` wiring at three refresh sites: mount effect, Reload button, post-run continuation.
- `web-ui/public/styles.css`: `.maintenance-auto` rules using theme tokens only.
- `web-ui/tests/api_paths.rs`, `web-ui/tests/core_screens.rs`: pins and unit tests.
- `web-ui/scripts/test-wave6-cdp.cjs`, `web-ui/scripts/fixtures/issue-118-wave6.json`: six G7 lane
  cases, status mock (scenario-switched; the GET after a POST returns the stamped status).

## Bundle measured

Final bundle `u3-dist` built from the reverted tree: `web-ui-bc64730d57082f3d_bg.wasm`. Earlier
bundles: `u1-dist` (`web-ui-15374e00badada0a`) is the RED build from before view wiring
(`red/results.json` bundle key, same hash); `u2-dist` (`web-ui-35d356fea6e44324`) was built before the
`SETTINGS_PATHS` revert, so its wasm is not the final one.

## Acceptance criteria

| AC | Instrument | Result |
|---|---|---|
| Status row renders four values | lane `maintenance_status_renders_four_values` | PASS on u3 |
| `null` lastRunMs reads `never`, not 1970 | lane `maintenance_status_never_ran` | PASS on u3 |
| 501 → unavailable note, no enabled value, action enabled | lane `maintenance_status_unavailable_not_off` | PASS on u3 |
| `running: true` disables the action | lane `maintenance_server_running_disables_action` | PASS on u3 |
| Re-reads status after a run completes | lane `cleanup_run_refreshes_status` + mutant (below) | PASS on u3; mutant RED |
| Settings renders with the row present | lane `settings_renders_clean` | PASS on u3 |
| Classifier, label, predicate, spec | wasm units in `web-ui/tests/core_screens.rs` | 81/81 core_screens ok |

Lane on u3 (`.tmp/scratch/issue141/green3/lane.log`): `EXIT=0`, 24 PASS, 0 FAIL. This is 18
pre-existing cases plus 6 G7 cases, including `native_select_contracts`.

Lane RED on u1 (`.tmp/scratch/issue141/red/lane.log`): `EXIT=1`, 18 PASS, 6 FAIL, each on the missing
`maintenance-auto-*` testid.

## Mutants

| Mutant | Compiling line | Reds | Output |
|---|---|---|---|
| Remove post-run `refresh_status.run(())` (`core_screens.rs:2940`) | `Compiling web-ui v0.1.0 (…/.tmp/scratch/issue141/mutant-ac4)` in `mutant-ac4/build.log:195` | `cleanup_run_refreshes_status` | `FAIL cleanup_run_refreshes_status: no status GET after the POST (stale status after a run)`, exit 1 |
| Same mutant restored (u2 bundle, pre-revert) | `Compiling web-ui` not re-logged; restore used the unmutated u2 build | — | `PASS cleanup_run_refreshes_status`, exit 0 |
| Dropped `#[serde(rename_all = "camelCase")]` | `Compiling web-ui v0.1.0` (`.tmp/scratch/issue141/red/red-2a.log`) | four-fields test | `Failed("malformed request or response body")` vs `Answered` |
| `cleanup_last_run_label` symbol absent | `error[E0432]` | label tests | unresolved import |
| `cleanup_disabled` without `auto_running` | `error[E0061]` | `test_cleanup_disabled_carries_the_server_running_flag` | arity |
| POST verb in `cleanup_status_spec` | `Compiling web-ui v0.1.0` (`.tmp/scratch/issue141/red/red-step1-spec.log`) | spec test | `left: "POST" right: "GET"` |

The AC4 mutant is the only evidence for "re-reads after a run completes". Its RED comes from the
harness assertion, not from a layout or timing guess.

## Gates (final tree)

| Gate | Command | Exit |
|---|---|---|
| wasm suite | `cd web-ui && cargo test --target wasm32-unknown-unknown` | 0 (20 binaries ok) |
| fmt | `cd web-ui && cargo fmt --all --check` | 0 |
| clippy lib | `cargo clippy --workspace --lib -- -A clippy::all -D clippy::unwrap_used -D clippy::expect_used` | 0 |
| clippy all-targets | `cargo clippy --workspace --all-targets -- -D warnings -A clippy::await_holding_lock -A clippy::assertions_on_constants -A clippy::doc_lazy_continuation` | 0 |
| class coverage | `node web-ui/scripts/check-class-coverage.mjs --self-test --min-emitted=192` | 0 — `emitted=192 hooks=192 missing=0 dead=0 unresolved=0` |
| thaw census | `node web-ui/scripts/thaw-adoption-census.mjs --src=web-ui/src --ruling=docs/evidence/thaw-adoption-census/ruling.md --probe=.tmp/scratch/issue141/census-probe.json` | EXIT=3, `census kinds=25 unclassified=0 stale=12 replace=6 worklist=6 violations=0 drift=12 harness=0 exit=3`, per blueprint Step 7 |

The `--min-emitted=192` floor is the gate's own emitted count, measured on the first run that
reported `emitted=192`. The forecast of 191 was wrong and was not used.

## Census: what `--probe` is

Read from `web-ui/scripts/thaw-adoption-census.mjs`: `--probe` is optional. When given it must
name an existing file; a missing path yields `EXIT2 probe <path> probe` and counts one `harness`
finding. `harness` is the count of exit-2 findings.

Measured:

| Invocation | Result | Exit |
|---|---|---|
| no `--probe` | `kinds=25 unclassified=0 stale=12 replace=6 worklist=6 violations=0 drift=12 harness=0 exit=3` | 3 |
| `--probe=docs/evidence/thaw-adoption-census/probe-table-progress.md` | `kinds=25 unclassified=0 stale=13 replace=6 worklist=6 violations=0 drift=13 harness=0 exit=3` | 3 |
| `--probe=.tmp/scratch/issue141/does-not-exist.md` (negative control) | `EXIT2 probe .tmp/scratch/issue141/does-not-exist.md probe` / `harness=1 exit=2` | 2 |

The earlier `harness=1` in the wave-1 handoff was this missing-file probe, pointed at
`.tmp/scratch/issue141/census-probe.json`, which did not exist at that time. It was not a scanner gap.
`src/shard_consolidation.rs` sits at repo-root `src/`, outside `--src=web-ui/src`, and appears in
zero census lines.

## No timer, no polling

The status row's only network path is `refresh_status` (`web-ui/src/core_screens.rs`), called from
three sites: the panel's mount effect, the Reload button, and the continuation after a completed
cleanup run. There is **no timer, no interval, and no polling**. The panel's own posture comment says
the same (`core_screens.rs:2874-2875`, `:2887-2888`, `:2922`). The reason is ADR-0019 G3
(`docs/adr/0019-erasure-posture.md:63`): erasure is never automatic, and a status read that
re-polled would be the first step toward a timer that drives cleanup. The status fetch is read-only,
so a poll would not erase anything, but the ADR's posture is that no automatic path should exist
around this feature.

## Residues

- **`SETTINGS_PATHS`**: the `/api/cleanup/status` entry added in wave 1 was reverted. Nothing on
  the status fetch reads `SETTINGS_PATHS`; `cleanup_status_spec` uses `endpoint(EndpointId::CleanupStatus)`.
  `endpoints_for` only feeds informational `nav.rs` metadata. Reverted, not pinned.
- **Census exit 3, composition** (measured): the 13-finding figure with the committed probe is
  1 probe staleness (`EXIT3 probe report probe stale (d58a94f ≠ 6c80902)`) + 12 pre-existing
  ruling-side findings (6 stale kinds `table, thead, tbody, tr, td, progress` + 6 worklist
  overstatements in `core_screens.rs`). The same 12 are produced at base `abf2bce`, before any
  issue-141 code. `stale` and `drift` are the same counter (`thaw-adoption-census.mjs:681` and `:685`
  both filter `exit === 3`), so they are not two residues. The census re-run for this README used
  `.tmp/scratch/issue141/census-probe.json`, the committed probe with its HEAD line rewritten to
  `6c80902`: `stale=12 drift=12`. Exit 3 is ruling-side drift, not a violation; the blueprint permits it.
  Editing the ruling is out of scope here.
- **Ledger path**: the blueprint's step-0 path was not used. Writes went to the `LEDGER:` directive's
  file, `.tmp/progress/20261111-1304-lossless-shard-consolidation.ledger.md`.
- **wasm-bindgen-cli**: the machine runner was 0.2.100 and failed every wasm binary with a schema
  mismatch. Reinstalled to 0.2.129 to match `Cargo.lock`. Machine-level, not a repo change.
- **Live engine**: pid 4719 on 127.0.0.1:3742 was not restarted or written to.
