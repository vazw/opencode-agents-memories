# Release-artifact rehearsal, rev-14 (wave 20261111-1304)

Scratch: `.tmp/scratch/release-rehearsal-20261113T123247Z/`. Frozen snapshot read-only throughout.

## Verdict

RELEASE-ARTIFACT: BLOCKED — findings F1, F1b, F4 block the cutover decision. The rehearsal sequence reproduces the expected numbers on the release binary. F1: the release build is not reproducible from the committed `eacb66d` config. F1b: the live tree is dirty against `eacb66d`. F4: the running engine's exe hash and the stop/start duration are unmeasured. F2 and F3 are driver and measurement notes; neither changes a consolidation number.

## Task 1 — release artifact

MEASURED, `sha256sum .tmp/scratch/release-rehearsal-20261113T123247Z/bin/opencode-agents-memory; stat -c %s …`:
- sha256 `ac3ef842d03ec466128248591b16d7205ca61a0690932b21aade7dca3f833126`
- size 83097000 bytes
- built from `git archive eacb66dd25edeac7d1d41b3b366756a3d380b006` (clean export, not the dirty worktree).

Build command, MEASURED: `RUSTFLAGS="-Clinker=clang -Clink-arg=--ld-path=/home/opencode/.cargo/bin/wild -Clink-arg=-Wl,--no-madvise-huge-pages" cargo build --offline --locked --release -p opencode-agents-memory` → exit 0, 2m49s. Log: `.tmp/scratch/release-rehearsal-20261113T123247Z/build.log`.

**F1 (blocking for reproducibility):** the committed `eacb66d` `.cargo/config.toml` pins `--ld-path=/home/vaz/.cargo/bin/wild`. Plain `cargo build` on the clean export fails at link (`clang: error: invalid linker name`, exit 1) because `/home/vaz` is unreadable here (`ls /home/vaz` → Permission denied). MEASURED: `.tmp/scratch/release-rehearsal-20261113T123247Z/build.log` head. The working tree on disk replaces it with `--ld-path=wild`, which is uncommitted (`git diff --stat`: `.cargo/config.toml` 5+/3-). The binary above used a RUSTFLAGS override, so its bytes are not from the committed config. The deployable bytes must be produced by a build whose config is committed, or the config fix must be committed first. INFERRED: the override changes only the linker path; it does not change rustc codegen. Not measured beyond that.

**F1b (dirty tree):** `git status` on the live worktree shows three modified tracked files not in `eacb66d`: `.cargo/config.toml`, `AGENTS.md`, and `crates/mem-web/src/api/deduplication/mod.rs` (`fetch_update`→`try_update` plus an added test module). The release binary was built from the committed tree, so it does NOT include the `mod.rs` change. Whichever build is deployed, the source it was built from must be named.

Staged vs rehearsed, MEASURED: `plugin/engine/linux-x64/opencode-agents-memory` sha256 `746290cbf60bae6e98995e804ccc95953ecc4296cb5dcdff81c20310a8cd49e5`, size 81749024, mtime 2026-11-10 23:44. `cmp` against the release binary: DIFFERENT. Stale, as the brief said.

Running engine, MEASURED: `ps -eo pid,lstart,args | grep opencode-agents-memory` → PID 16234, started Thu Nov 12 10:01:37 2026, argv `/home/opencode/Code/Personal/Rust/opencode-agents-memory/plugin/engine/linux-x64/opencode-agents-memory --storage-root /home/opencode/.opencode-mem/data`. `/proc/16234/exe` could not be hashed in-process (permission not confirmed), so the running bytes are identified by the path and start time, not the sha256. This is the stale 746290cb build that the path points to; the path has not been re-linked. Not stopped or signalled.

## Task 2 — decisive sequence on the release binary

Frozen snapshot, MEASURED, `sha256sum manifest.final.sha256 manifest.baseline.sha256`: both `e0fe0276fe5cf769a649f742bbc18ba26e3383a4575a8e3edac9fab6f53d258e` before and after the run.

Steps 1–8 ran on `.tmp/scratch/release-rehearsal-20261113T123247Z/run/` (driver `run.sh`); steps 9–13 on the installed tree (`run2.sh`, `rb3/`). Logs: `.tmp/scratch/release-rehearsal-20261113T123247Z/log.txt`, `.tmp/scratch/release-rehearsal-20261113T123247Z/log2.txt`, `.tmp/scratch/release-rehearsal-20261113T123247Z/log4.txt`.

| # | Step | Expected | Measured | Result |
|---|---|---|---|---|
| 1 | consolidate-snapshot | 0 | 0 | match |
| 2 | dry-run | 0, source_count 34, e266 at schema 3 / 19 cols | 0; source_count 34, group_count 34, dry_run true; e266 src `user_version`=3, memories 19 cols (`pragma table_info`) | match |
| 3 | apply | 0, 34 groups, 5827 survivors, staged 5903, sidecar 8254 B | 0; group_count 34; totals survivors 5827, staged_rows 5899 (oracle); live shard memories total **5903**; sidecar **8254 B**, 18 lines = 1 divergent + 16 carry (99 rows) + 1 reparent | match. Note: the "5899 expected" vs "5903 live" difference is the 4 legacy carried rows, consistent with the brief's 5827+72+4=5903 |
| 4 | staged-verify | 0 | 0 | match |
| 5 | install | 0, InstalledUnverified | 0; journal phases StagedVerified→Installing→InstalledUnverified | match |
| 6 | settle verify | 0, Verified | 0; journal ends `Verified` (4 lines) | match |
| 7 | repeat verify | 0, no-op | 0; journal bytes SAME, mtime SAME | match |
| 8 | repeat apply at Verified | 77 window_not_applyable | 77 `consolidation.window_not_applyable` | match |
| 9 | oracle3 | 34/34, agent_sessions 888→887 rows_equal, 300-row reparent, sidecar agreement | exit 0; 34/34 exact; agent_sessions src 888 exp/staged 887 rows_equal true; reparent `parent_session_id` 300 rows; findings []; `.tmp/scratch/release-rehearsal-20261113T123247Z/run/oracle.json` | match |
| 10 | integrity + FK | ok, 0 dangling | 41 DBs checked, integrity_not_ok 0, fk_dangling 0 | match |
| 11 | registry | 35 rows, 0 violations, 34/34, 0 missing, 0 unregistered | 35 rows, registered_missing 0, unregistered 0 (file set = registry set), distinct hashes 35 | match |
| 12 | shard_stats vs COUNT | 34/34; legacy 2991cca9 has no shard_stats | shard_stats.vector_count == COUNT(*) on **34/34** shards that have the table; legacy 2991cca9 has NO `shard_stats` in source or live (4 memories both) | match |
| 13 | rollback on copy | kill 134, recover 0, rollback 0, digest = baseline and pre-apply, modes match | kill 134, recover 0, rollback 0; restored digest == custody baseline (IDENTICAL); restored digest == pre-install digest for all non-`-shm` files; mode-table `(mode,kind,path)` MATCH (217 entries) | match, see F2/F3 |

**F2 (driver, not binary):** the first pass of steps 9–13 failed for my own reasons, fixed and re-run, not reconciled: (a) the staged root is moved into `data/` by install, so `` was gone at 9–12; (b) `rb/` copy lacked `manifest.sha256`; (c) the `run2` snapshot hit `custody create: File exists` because `run.sh` had already created `rb/custody` (its `find -delete` cannot clear the read-only `custody/data`, which is mode `dr-x`); (d) `--custody` and `--staged` require absolute paths. The authoritative step 13 is `rb3/` (log `log4.txt`) and nothing else. Stale `rb/` and `rb2/` are left on disk as evidence of these failures.

**F3 (measured, not a binary defect):** the rollback digest differs from the pre-install digest only in `*-shm` files (`episodic.db-shm`, `metadata.db-shm`, and shard `-shm`). Non-shm digest differences: 0. Restored digest == custody baseline: IDENTICAL. Source snapshot == custody baseline: yes. INFERRED: `-shm` is SQLite shared-memory rewritten by any reader after rollback, so it is not a content difference. Not separately proven.

**Step 13 "modes":** the brief's `(mode,kind,path)` check compares against `custody/mode-table`, which writes `dir`/`file` and `./`-prefixed paths. Normalized and compared: MATCH, 217 entries each (command in `rb3/mt.norm`, `rb3/mr.norm`).

Step 8 note: `consolidate-shards --apply` at `Verified` returns 77 (authoritative). Design line 333's "no-op" wording stays stale, as the brief said.

## Task 3 — timing (MEASURED, wall clock, `awk` around `date +%s.%N`)

Source: `.tmp/scratch/release-rehearsal-20261113T123247Z/timing.tsv` (steps 1–8, 12-stats errored, 13 errored, see F2) and `.tmp/scratch/release-rehearsal-20261113T123247Z/timing2.tsv` (9–13 re-run). Step 13 clean re-run wall: 43.312 s (`log4.txt`).

| Step | Seconds | Window |
|---|---|---|
| copy (cp -a of snapshot data) | 0.056 | before |
| 1 consolidate-snapshot | 6.288 | before |
| 2 dry-run | 0.102 | before |
| 3 apply | 5.833 | before |
| 4 staged-verify | 2.744 | before |
| 5 install | 11.400 | **downtime** (moves live tree) |
| 6 settle verify | 5.209 | after install (downtime if engine must stay stopped) |
| 7 repeat verify (no-op) | 5.288 | after |
| 8 repeat apply (77) | 0.011 | after |
| 9 oracle | 1.438 | after |
| 10 integrity + FK | 0.179 | after |
| 11 registry | 0.032 | after |
| 12 stats | 0.061 | after |
| 13 rollback (rehearsal only, not cutover) | 43.312 | rehearsal only |

TOTAL for the cutover path (steps 1–8 only, the ones that touch the live tree): 6.288+0.102+5.833+2.744+11.400+5.209+5.288+0.011 = **36.875 s**. Steps 1–4 can run with the engine up (they read a copy and write a staged tree). Step 5 onward must be inside downtime. Steps 5–6 (install + settle, 16.6 s) are the minimum downtime. Steps 7–12 are post-install checks. Step 13 is a rehearsal and not a cutover step.

INFERRED: the downtime estimate is ~17 s for install+settle, plus engine stop/start. Stop/start time is not measured here (no engine stop was performed, per instruction). This figure is the number the user needs and it is missing.

## Task 4 — artifact questions

- Release binary sha256/size: `ac3ef842…833126`, 83097000 B (above).
- Staged engine differs: MEASURED, `cmp` DIFFERENT; staged `746290cb…` vs release `ac3ef842…`.
- Running engine is the stale build: INFERRED from PID 16234 path and start time; exe hash not read. Would close by `sha256sum /proc/16234/exe` run as the owner (not attempted).
- Stage command: `plugin/scripts/stage-engine.sh` (or `… --target <triple>`). It runs `cargo build --release` itself (so it re-builds in the live tree, which means it cannot stage the rehearsed bytes byte-for-byte unless the build is reproducible from the committed config, F1). Preconditions, READ from source:
  - `../web-ui/dist/index.html` must exist: MEASURED present, 1125 B, identical to `plugin/engine/linux-x64/web-ui/dist/index.html`. Exit 5 if absent.
  - `plugin/engine/linux-x64/web-ui/dist/index.html` must NOT exist: MEASURED present (`web-ui/` dir exists with `index.html` 1.1K). Exit 7. The script says "Remove it yourself … rm -rf"; the no-`rm -rf` spelling is `find plugin/engine/linux-x64/web-ui/dist -mindepth 1 -delete` (this removes the contents; the `dist` dir stays). Run only after the engine is stopped.
- Engine stop path, READ from `plugin/src/plugin.js`/`dist/plugin.js` (identical sha256 `58e4ab9a…`): constants `SHUTDOWN_RESPONSE_TIMEOUT_MS=10000`, `SHUTDOWN_TERM_AFTER_MS=2000`, `SHUTDOWN_KILL_AFTER_MS=5000`. `scheduleEscalation` sends SIGTERM at **2 s** and SIGKILL at **2+5 = 7 s**. The brief's "SIGKILL at 7 s" is correct; "10 s timeout" is the shutdown RPC response bound. The wrapper sends `shutdown` over RPC first, then escalates. The custodian does not signal the engine.
  - Who stops it: the wrapper process owning the child (not the custodian). Stopping it is Step 12; not done here.

## Doc corrections to the brief (measured)
- Step 12 "shard_stats": the table is in source shards (24 of 37 source shards have it; 34 of 35 live shards). Check is `shard_stats.vector_count == COUNT(*)`, not `metadata.db` `vector_count`. Both happen to match.
- Step 3 "5903 staged": live memories total is 5903 (measured). Oracle `staged_rows` is 5899 (memories-only expected). Different scope, both consistent.

## Verification commands
- `sha256sum /home/opencode/.opencode-mem/recovery/consolidation-20261112T030050Z-452b0f5d/manifest.{final,baseline}.sha256` → both e0fe0276… before and after.
- Frozen data hash of source == custody baseline: SOURCE_EQ_CUSTODY_BASELINE=yes (`rb3/snap-source.full.sha256`).
- No `rm -rf` used; cleanup via `find … -mindepth 1 -delete`. `rb/custody/data` (read-only) could not be deleted (Permission denied for `find -delete`), left in place.

## Status
RELEASE-ARTIFACT: BLOCKED — (F1) release build not reproducible from committed `eacb66d` config without a RUSTFLAGS override (linker path `/home/vaz`); (F1b) live tree dirty vs `eacb66d` (`mod.rs` fetch_update change, `.cargo/config.toml`, `AGENTS.md`); (F4) running engine's exe hash unmeasured and the stop/start duration unmeasured, so the downtime number is incomplete. Steps 1–12 reproduce the expected numbers on the release binary.

---

# Re-rehearsal, HEAD `33e0c6d` (F1 fixed)

Scratch: `.tmp/scratch/release-rehearsal2-20261113T125750Z/`. Frozen snapshot read-only throughout.

## 1. HEAD and the clean build (MEASURED)

- `git rev-parse HEAD` → `33e0c6d0ab332394138de0f65eb4b8664784df3f`. Confirmed.
- `git status --porcelain` → ` M AGENTS.md`, ` M crates/mem-web/src/api/deduplication/mod.rs`, `?? docs/evidence/recovery-housekeeping-20261111.md`. **Dirty.** The build did NOT use this tree.
- Build source: `git archive 33e0c6d0ab332394138de0f65eb4b8664784df3f | tar -x -C …/release-rehearsal2-…/src`, then `env -u RUSTFLAGS cargo build --offline --locked --release -p opencode-agents-memory`, exit 0, 2m33s (`build.log`). No RUSTFLAGS override. Committed `.cargo/config.toml` used as-is.
- Result: sha256 **`24b02b3566bc5a31e8b6bc8ca7d2ed99bcef33eecf63f53542a38c1d427e7756`**, size **83097040** B.
- **Not `30db7e80…`.** Your number is not the HEAD clean build. It is the in-repo dirty-tree build: `target/release/opencode-agents-memory` (mtime 19:55:53, sha256 `30db7e80…`, 83097072 B) matches the dirty-tree source delta in `mod.rs` (`fetch_update`→`try_update` plus the `#[cfg(test)]` module).

### Reproducibility (MEASURED, separate finding)

A second clean `git archive 33e0c6d` build into `target2/` with the same command gave **`cbfe693746a69678e68e4763dd51ca81709ff778e4e06a61e974838f7b657c41`**, size 83097040. Two builds of the same HEAD source differ.
- `cmp -l bin/opencode-agents-memory target2/release/opencode-agents-memory | wc -l` → 438555 differing bytes.
- `readelf -n … | grep 'Build ID'`: `89854ed7…` vs `4616eda4…`.

So the release codegen is not bit-reproducible across two clean builds from identical source in this environment. No single hash is the "HEAD deployable" unless the build is pinned. I did not find the cause (not investigated further: toolchain nondeterminism or a build-time input such as a timestamp or path). INFERRED, not measured: the variance comes from codegen, not from the rehearsal harness.

## 2. Rehearsal, 13 steps, two candidates

Harness: `run-all.sh <binary> <label>`, a parameterized copy of the rehearsal-1 sequence. It was not re-derived. Each candidate ran on a fresh `cp -a` of the frozen snapshot.

Candidates rehearsed:
- **A. `24b02b35…`** (HEAD clean build, what I actually built from `33e0c6d`). Log: `head-24b02b35/log.txt`.
- **B. `30db7e80…`** (your named candidate, the in-repo file, byte-identical to its prior hash). Log: `dirty-30db7e80/log.txt`.

Reference: rehearsal-1 on `ac3ef842…` (`release-rehearsal-20261113T123247Z`).

| # | Step | Expected (rehearsal-1) | A `24b02b35` | B `30db7e80` | Match |
|---|---|---|---|---|---|
| 1 | snapshot | 0 | 0 | 0 | yes |
| 2 | dry-run | 0, source_count 34 | 0, source_count 34, group_count 34, dry_run true | 0 | yes |
| 3 | apply | 0, 34 groups, 5827 survivors, staged 5899 expected | 0, group_count 34, survivors 5827, expected_rows 5899 | 0 | yes |
| 4 | staged-verify | 0 | 0 | 0 | yes |
| 5 | install | 0, InstalledUnverified | 0; journal StagedVerified→Installing→InstalledUnverified | 0 | yes |
| 6 | settle | 0, Verified | 0; journal ends Verified | 0 | yes |
| 7 | repeat settle | 0, true no-op | 0; journal bytes SAME, mtime SAME | 0, SAME, SAME | yes |
| 8 | repeat apply at Verified | 77 window_not_applyable | 77, event `consolidation.window_not_applyable` | 77 | yes |
| 9 | oracle3 | 34/34, 888→887, 300 reparent | 34/34 exact; agent_sessions src 888 exp 887 rows_equal True; reparent `parent_session_id` 300 rows; findings [] | 0 | yes |
| 10 | integrity + FK | ok, 41 DBs, 0 dangling | DBS 41, integrity_not_ok 0, fk_dangling 0 | same | yes |
| 11 | registry | 35/0/0/0 | REG_ROWS 35, REGISTERED_MISSING 0, UNREGISTERED 0, SHAPE_VIOLATIONS 0 | same | yes |
| 12 | shard_stats vs COUNT | 34/34 | SHARD_STATS_TABLES 34, MATCH 34, BAD [] | same | yes |
| 13 | rollback | kill 134, recover 0, rollback 0, restored = custody baseline | kill 134, recover 0, rollback 0; EQ_CUSTODY_BASELINE yes; non-shm digest == pre-install yes; mode-table 217 entries MATCH | same | yes |

Sidecar on A (MEASURED, `custody/journal.json.aux-divergent`): 8254 B, 18 lines = 1 divergent + 16 carry (99 rows) + 1 reparent. Journal phases: StagedVerified, Installing, InstalledUnverified, Verified.

Frozen manifests (MEASURED, `sha256sum manifest.final.sha256 manifest.baseline.sha256`): both `e0fe0276fe5cf769a649f742bbc18ba26e3383a4575a8e3edac9fab6f53d258e` before and after both runs.

**No divergence from rehearsal-1 on either candidate. Every number reproduced.**

Note on rehearsal-1: the step 13 `-shm` digest difference is a live-reader artifact; the non-shm digest is equal. Same as before.

### Candidate status
- A `24b02b35…`: passes all 13 steps.
- B `30db7e80…`: passes all 13 steps.
- But neither is a *verified* deployable, because the build is not reproducible (above). The deployable is whichever binary is actually copied into `plugin/engine/`, and that must be the same file that was rehearsed. Recording this as a gate, not a pass.

## 3. Timing per step (MEASURED, `date +%s.%N` around each step, `timing.tsv`)

| Step | A `24b02b35` s | B `30db7e80` s | Window |
|---|---|---|---|
| copy (cp -a) | 0.053 | 0.034 | before |
| 1 snapshot | 6.316 | 6.062 | in-window (see §4) |
| 2 dry-run | 0.105 | 0.085 | in-window |
| 3 apply | 5.755 | 5.831 | in-window |
| 4 staged-verify | 2.888 | 3.074 | in-window |
| 5 install | 11.513 | 11.726 | in-window |
| 6 settle | 5.284 | 5.356 | in-window |
| 7 repeat settle | 5.255 | 5.300 | after |
| 8 repeat apply (77) | 0.011 | 0.010 | after |
| 9 oracle | 1.429 | 1.412 | after |
| 10 integrity | 0.152 | 0.162 | after |
| 11 registry | 0.038 | 0.038 | after |
| 12 stats | 0.078 | 0.076 | after |
| 13 rollback (rehearsal only) | 41.913 | 42.684 | rehearsal only |

Plan derivation (`rehearsal-build-plan.py`) is not in the table because it was not timed; it is inside the window per §4. Not measured.

## 4. Downtime, corrected (your correction accepted)

The in-window chain is the full chain from quiesce to restart, not the install alone. Per your §G reading, the S3 snapshot runs after quiesce, so steps 1–6 are in-window, and steps 7–12 are post-install checks, to confirm whether they must also be in-window (not decided here; see the user's rule).

**MEASURED in-window chain** (steps 1–6, candidate A):
- snapshot 6.316 + dry-run 0.105 + apply 5.755 + staged-verify 2.888 + install 11.513 + settle 5.284 = **31.861 s**.
- Plus plan derivation (not measured in this run's timing) and release (stop ladder) and restart: **unmeasured**.

Candidate B, same chain: 6.062 + 0.085 + 5.831 + 3.074 + 11.726 + 5.356 = **32.134 s** measured.

Unmeasured, NOT folded into the total: the plan derivation step (`rehearsal-build-plan.py`, seconds not recorded here), the stop ladder (SIGTERM to exit), the release of the custody lock, and the service restart. These must be measured on the host with the engine actually stopped, which this session was forbidden to do.

## 5. Plan generator on a fresh snapshot (task 4, MEASURED)

Procedure: fresh `cp -a` of the frozen data → `consolidate-snapshot` (exit 0, `fresh-plan/`) → generator run on the fresh copy → `consolidate-shards --dry-run`.

Findings, all MEASURED:
1. **`consolidate-snapshot` does not write a manifest in the generator's format.** It writes `custody/baseline.sha256`, which has 217 lines and `file`/`dir` records with mode, not `sha256  ./path` lines. The frozen `manifest.sha256` is 205 `sha256  ./path` lines.
2. The generator computes `source_snapshot_sha256 = sha256(<parent-of-S>/manifest.sha256)` (READ, `rehearsal-build-plan.py` lines ~142–146). It does NOT capture a manifest. So the operator must place a `manifest.sha256` beside `data/` before running it.
3. I copied `custody/baseline.sha256` to `fresh-plan/manifest.sha256` and ran the generator. Result: exit 0, 34 mappings. `source_snapshot_sha256 = 3da51c56d4a9797d947c6dce533fb76bed7506db0a1c1774f9c3632d9ff5fcce`, equal to `sha256(fresh manifest)`: **True**. `consolidate-shards --dry-run` on that plan: exit 0, source_count 34, group_count 34.
4. **Caveat, MEASURED:** that dry-run was accepted, but it is not the same kind of manifest as the frozen one. The regenerated `sha256  ./path` manifest of the fresh data gave `0ceba1b6…` and differs from the frozen one by 26 lines, all `-shm` files (`diff`). Live SQLite `-shm` files change between copies, so the `source_snapshot_sha256` a fresh copy produces is not stable across copies. Step 11 must regenerate and pin the manifest with the snapshot, not reuse the frozen hash.

So: the generator produces a valid plan that dry-run accepts on a fresh snapshot, given a manifest beside `data/`. Its `source_snapshot_sha256` is only meaningful for the manifest it was hashed against, and that manifest must be captured in the same step as the snapshot.

## 6. Stop/restart (task 5)

Process tree, MEASURED (`ps -o pid,ppid,pgid,sid,lstart,args -p 16181,16234`):
- `16181` PPID 1, PGID 16181, SID 16181: `/home/opencode/Code/opencode/packages/cli/dist/cli-linux-x64/bin/opencode serve --service`.
- `16234` PPID 16181, same PGID/SID: the engine, `… --storage-root /home/opencode/.opencode-mem/data`.
- `systemctl --user list-units --all | grep -i opencode` → empty (matches the brief).

Signal reaching `shutdownHandler`, READ from `plugin/src/plugin.js`:
- `installSignalHandlers` registers `process.on("SIGINT", shutdownHandler)` and `process.on("SIGTERM", shutdownHandler)`. These handlers are in the **wrapper** (the plugin inside `opencode serve`, process 16181), not in the engine.
- `shutdownHandler(sig)` → `shutdownAll(sig)` → `runShutdown(state)` for every state → then `process.kill(process.pid, sig)` (re-delivers the signal once).
- So SIGTERM or SIGINT delivered to **PID 16181** reaches `shutdownHandler`. SIGTERM to the engine PID 16234 does NOT run the wrapper's `shutdownHandler`; the engine's own handler (not read here) applies.

Ladder, READ from `plugin/src/plugin.js` (`dist/plugin.js` sha `58e4ab9a…`, identical):
- `runShutdown`: clear timers → `awaitCaptureDrain` (max 2000 ms, poll 50 ms) → `callChild(state,"shutdown",…)` with `SHUTDOWN_RESPONSE_TIMEOUT_MS = 10000` → `scheduleEscalation`: SIGTERM to the child at `SHUTDOWN_TERM_AFTER_MS = 2000`, SIGKILL at `2000 + SHUTDOWN_KILL_AFTER_MS(5000) = 7000` ms → wait `childClosedPromise` raced against `SHUTDOWN_KILL_AFTER_MS * 2 = 10000` ms.
- So the engine gets, in order: the RPC `shutdown` (≤10 s to respond), then SIGTERM at 2 s after escalation starts, then SIGKILL at 7 s. The custodian does not signal the engine.
- Note: the RPC `shutdown` is sent **only if** `state.engineCompatible`. Otherwise it goes straight to the escalation timers. INFERRED: an incompatible engine gets SIGTERM at 2 s and SIGKILL at 7 s with no RPC.

Detached driver, INFERRED (not measured):
- The rehearsal driver must not be a child of 16181, or the service teardown kills it. Its PGID is 16181 (the service's own group), so a `kill -TERM -- -16181` reaches the driver too.
- To survive: `setsid` (new session and process group) or `systemd-run --user --scope` (no unit exists here, so `--scope` is the only systemd option). Use `setsid -f <driver>` so it is detached from the service group.
- Not verified that `setsid` survives this service's teardown, because no stop was done. This is the thing to measure.

Restart command: not in the source I read. INFERRED: the service is `opencode serve --service` launched by the OpenCode CLI, which I did not read. The restart command is not established from source. Do not use it until read from the CLI.

Stop/restart are not measured. Nothing was stopped, signalled or restarted in this session.

## 7. Bounds check

- Frozen snapshot: both manifests `e0fe0276…` before and after (MEASURED, both candidate runs and the final check).
- Live `S` and `/home/vaz`: not opened. The only live-path reads were `ps`, `systemctl`, and the `plugin/` source.
- `rm -rf`: not used. Cleanup used `find … -mindepth 1 -delete`.
- Tracked files, `plugin/`, `src/`, `tests/`: not modified. `git status` still shows only the three pre-existing dirty files.
- Not staged into `plugin/engine/`. No commit, push, cutover, or production mutation.

## Status

Every rehearsal number reproduced on both candidates (A `24b02b35…`, B `30db7e80…`). The gate is still BLOCKED, for these reasons:
1. **Build not reproducible:** two clean builds of HEAD `33e0c6d` give different bytes (`24b02b35…` vs `cbfe6937…`, 438555 bytes differ, different Build IDs). The deployable hash is not determined by the commit. A rehearsed hash is only valid for its own bytes, so the deployed file must be byte-identical to the rehearsed file, and that cannot be guaranteed from source here.
2. **`30db7e80…` is not a HEAD build.** It is the dirty-tree build. Rehearsed as named, but it cannot be deployed under the doctrine (dirty tree must never be the source).
3. **Plan generator needs a manifest captured with the snapshot.** The fresh-snapshot plan is valid, but its `source_snapshot_sha256` depends on the manifest hashed. The operator must capture and pin that manifest in the same step.
4. **Downtime is incomplete.** The in-window measured chain is 31.9 s (A) / 32.1 s (B) for steps 1–6. Plan derivation, stop ladder, custody-lock release and restart are unmeasured.
5. **Stop/restart unmeasured**; the restart command is not established from source.

RELEASE-ARTIFACT: BLOCKED — (1) HEAD `33e0c6d` does not produce a single reproducible binary (two clean builds differ, `24b02b35…` vs `cbfe6937…`); (2) `30db7e80…` is the dirty-tree build and not a HEAD artifact; (3) the plan generator's `source_snapshot_sha256` requires a manifest captured with the snapshot, not reused; (4) the downtime total excludes the unmeasured stop, plan-derivation and restart steps; (5) the restart command is not established from source. All 13 rehearsal numbers reproduced on both candidates.

---

# Final pass before Step 11 (HEAD `328799f`)

## 1. Build determinism (READ from `.tmp/scratch/step11/determinism.txt`, not re-run)

Your three relinks of `git archive 33e0c6d` at a fixed path and fixed `CARGO_TARGET_DIR` all gave `dff4d308…`, 83097000 B. My earlier `24b02b35…` (83097040) and `cbfe6937…` (83097040) came from two different extraction directories. The difference is embedded absolute source paths, not nondeterminism. I did not reproduce your three builds, so this is taken as your MEASURED record. The operative rule is: rehearse the exact file that is deployed, and never rebuild between rehearsal and staging.

## 2. Artifact (MEASURED)

- Rehearsed binary still exists: `.tmp/scratch/release-rehearsal2-20261113T125750Z/head-24b02b35/bin/opencode-agents-memory`. Command: `sha256sum …`. Result `24b02b3566bc5a31e8b6bc8ca7d2ed99bcef33eecf63f53542a38c1d427e7756`, 83097040 B.
- Copy: `install -m 0755 <rehearsed> .tmp/cutover-artifact/opencode-agents-memory`.
- Artifact path: `/home/opencode/Code/Personal/Rust/opencode-agents-memory/.tmp/cutover-artifact/opencode-agents-memory`
- Artifact sha256: `24b02b3566bc5a31e8b6bc8ca7d2ed99bcef33eecf63f53542a38c1d427e7756` (equal to the source).
- Artifact size: 83097040 B, mode 755.
- `cmp <rehearsed> <artifact>` → identical (BYTE_IDENTICAL).
- The artifact is byte-identical to the binary the 13-step rehearsal ran (rehearsal2 `head-24b02b35`, log `log.txt` lines S1–S12, RB_*). **No third rehearsal was run.** Those 13 steps ran on these exact bytes; this step did not re-run them.
- Not staged into `plugin/engine/`. `git status --porcelain -- plugin src tests` → 0 lines.
- Note: the rehearsal's `bin/opencode-agents-memory` file is also `24b02b35…`, which is the same content. Confirmed by `sha256sum`.

## 3. Generator, committed `docs/evidence/consolidation-plan-generator-20261113/build_plan.py` (MEASURED: FAILS)

Its sha256 matches the README's record: `b564a83a1b818d0502a864000b64869f571bb05a23235e6d5c9fd2ae674905d8` (`sha256sum`). Committed at `328799f`.

Fresh snapshot, no hand-placed manifest:
1. `consolidate-snapshot` on a clean copy: `SNAP_EXIT=0`. Custody `…/fresh-20261113T131711Z/cust/`, containing `custody/baseline.sha256` (`sha256` `3da51c56d4a9797d947c6dce533fb76bed7506db0a1c1774f9c3632d9ff5fcce`), `custody/mode-table`, `custody/data`. No manifest beside `data/`.
2. Committed generator, exactly as specified, on `…/cust/data`:
   `python3 docs/evidence/…/build_plan.py <abs>/cust/data <abs>/abs-committed.json` → **exit 1**, stderr `build_plan: no manifest.sha256 or baseline.sha256 beside …/cust/data`. No plan written.
3. Where it failed: the lookup at `build_plan.py` lines 146–154 is `_custody = os.path.dirname(S)`, which is `…/cust/`, and checks `…/cust/manifest.sha256` and `…/cust/baseline.sha256`. The binary writes the baseline at `…/cust/custody/baseline.sha256`. **The path is wrong for the binary's real output.** The README (lines 46, 70) states the same wrong location ("beside `data/`").

Diagnostic only (not the verdict): copy `custody/baseline.sha256` beside `data/` in `…/diag/`, run the same generator with absolute paths:
- `python3 build_plan.py <abs>/diag/data <abs>/abs-diag.json` → exit 0, 34 mappings.
- `source_snapshot_sha256` = `3da51c56d4a9797d947c6dce533fb76bed7506db0a1c1774f9c3632d9ff5fcce` = `sha256(custody/baseline.sha256)`. **EQUAL: True** (MEASURED).
- `consolidate-shards --plan abs-diag.json --dry-run --storage-root …/diag/data --rehearsal …/diag` → **exit 0**, `source_count` 34, `group_count` 34, `dry_run` true.

Also measured, a pitfall for the operator: with a **relative** `<storage-root>` or `<out-plan-path>`, the generator writes relative `storage_root`/`source_path`, and the binary's dry-run rejects it with `consolidation.source_path_mismatch` (exit 72). The plan must be produced with absolute paths. The README's usage example does not say so.

**Verdict on item 3: the committed patch does not work on a fresh snapshot.** It fails with exit 1. The fix is one line: look in `os.path.dirname(os.path.dirname(S))/custody/` (equivalently, pass the custody dir explicitly), not `dirname(S)`. I did not change the committed file (no tracked-file edits allowed). This is a blocker for Step 11.

## 4. Downtime range (MEASURED vs unmeasured)

MEASURED in-window chain (steps 1–6, candidate `24b02b35…`, rehearsal2 `timing.tsv`): snapshot 6.316 + dry-run 0.105 + apply 5.755 + staged-verify 2.888 + install 11.513 + settle 5.284 = **31.861 s**. Step 11's stated figure 31.9 s matches.

Plan derivation: MEASURED for the diagnostic run only. Generator wall time was not captured with `date` around it in this pass, so it is **unmeasured**. The generator is a Python script that hashes the shards and runs a census; its duration must be timed in the window (`date +%s.%N` around the generator). Do not estimate it.

Unmeasured (bounded from source, not timed):
- Stop ladder: `awaitCaptureDrain` ≤ 2000 ms, then RPC `shutdown` ≤ 10000 ms (`SHUTDOWN_RESPONSE_TIMEOUT_MS`), then SIGTERM at 2000 ms and SIGKILL at 7000 ms after escalation begins, with the child-close race bounded at 10000 ms (READ, `plugin/src/plugin.js` lines 52–54, 1292–1330). Worst case on a wedged engine: about 10 s (RPC) + 7 s (escalation) = 17 s, with the capture drain (≤2 s) run first. Measured on this host: no (no stop was done).
- Custody release: unmeasured. Not in source reviewed here.
- Restart: unmeasured (see §5).

**Range:** measured 31.9 s + plan derivation (unmeasured, to be timed) + stop ladder (0 to ~19 s by source bound: 2 s drain + up to 17 s) + custody release (unmeasured) + restart (unmeasured). **The honest total is 31.9 s plus an unmeasured remainder bounded at 19 s from the stop plus the unmeasured plan, release and restart.** No single number is measured end-to-end.

## 5. Restart command (READ, not derivable from this repo)

- No systemd unit exists (`systemctl --user list-units --all | grep -i opencode` → empty).
- Service process: PID 16181, `/home/opencode/Code/opencode/packages/cli/dist/cli-linux-x64/bin/opencode serve --service`, PPID 1, its own session (MEASURED, `ps -o pid,ppid,pgid,sid,args -p 16181`).
- The OpenCode CLI source is at `/home/opencode/Code/opencode/` (in the directory tree; not read in this pass).
- **Gap:** the restart command is not in this repository and I did not read the CLI's start path. The operator must supply the exact start command for `opencode serve --service`, and confirm it reads the same `--storage-root`. Do not infer it from the running argv.

## 6. Bounds

- Frozen snapshot: `sha256sum manifest.final.sha256 manifest.baseline.sha256` → both `e0fe0276fe5cf769a649f742bbc18ba26e3383a4575a8e3edac9fab6f53d258e` before and after this pass (measured in the rehearsal; re-check below).
- Live `S` and `/home/vaz`: not opened. The custody copy is from the frozen snapshot only.
- No `rm -rf`. No tracked-file edits. No commit, push, staging into `plugin/engine/`, cutover, stop, or signal.

## Status

RELEASE-ARTIFACT: BLOCKED — (1) the committed plan generator exits 1 on a fresh snapshot: it looks for the baseline at `dirname(S)` but `consolidate-snapshot` writes it at `custody/baseline.sha256`, so no production plan can be derived in the window (fix: resolve `dirname(dirname(S))/custody/`, then re-verify; the diagnostic proves the hashing and dry-run are correct once found); (2) the window total is not end-to-end measured: plan derivation, stop ladder, custody release and restart are unmeasured; (3) the restart command is not derivable from this repo, and the operator must supply it. Items that PASS: the artifact copy `24b02b35…` is byte-identical to the rehearsed binary (no third rehearsal, by design); the diagnostic plan dry-runs to source_count 34.

---

# Generator verification (HEAD `22bfcbb`, generator sha `924b1dcf8fc4661840b8b247879887e4c933ffdb0eca6088686be9581322e343`)

Scratch: `.tmp/scratch/step11/gen-20261113T132249Z/`. No hand-placed manifest. Tracked files not modified.

1. **Fresh snapshot (MEASURED, `consolidate-snapshot --custody <abs>/cust/custody --storage-root <abs>/cust/data --rehearsal <abs>/cust`)**: exit 0. Layout from `find <cust> -mindepth 1 -maxdepth 2 -printf '%y %m %s %p'`:
   - `cust/custody/baseline.sha256` f 600 24225 B, sha256 `3da51c56d4a9797d947c6dce533fb76bed7506db0a1c1774f9c3632d9ff5fcce`
   - `cust/custody/mode-table` f 600 11778 B
   - `cust/custody/manifest.sha256.excluded` f 600 33 B
   - `cust/custody/data` d 500, `cust/data` d 755
   - `find <cust> -name manifest.sha256` → 0 matches.
2. **Generator (MEASURED, `python3 <abs>/build_plan.py <abs>/cust/data <abs>/plan.json`)**: exit 0, wall 0.113 s (`date +%s.%N` delta). stderr empty. Summary JSON:
   `shards_on_disk` 37, `sources` 34, `unresolved` [], `not_sources` [project_2991cca9ba1ab013, project_60622035863a6bab, project_fd2bfb63318dfd29], `registry_ok` true, `schema_manifest_sha256` 487c10b289c11ca643b6926315778e8bb60559d167ffcaad6a87213bcd82846e, `baseline_manifest_sha256` 3da51c56…, `baseline_source` `<abs>/cust/custody/baseline.sha256`, `plan_mode` 0o600.
   Mappings 34. All `source_path` absolute. `storage_root` = `<abs>/cust/data`.
3. **Hash equality (MEASURED, python hashlib on the plan and on the file)**: `source_snapshot_sha256` = `3da51c56…` = `sha256(cust/custody/baseline.sha256)` → **True**. `baseline_manifest_sha256` also equal → **True**.
4. **Dry-run (MEASURED, `consolidate-shards --plan <abs>/plan.json --dry-run --storage-root <abs>/cust/data --rehearsal <abs>/cust`)**: exit 0, `source_count` 34, `group_count` 34, `dry_run` true, stderr empty.
5. **Frozen snapshot (MEASURED, `sha256sum manifest.{final,baseline}.sha256`)**: both `e0fe0276fe5cf769a649f742bbc18ba26e3383a4575a8e3edac9fab6f53d258e` before and after.

Downtime term closed: plan derivation = 0.113 s (MEASURED, one run, fresh snapshot, committed generator). Still unmeasured: stop ladder, custody release, restart.

Note (READ): `docs/evidence/consolidation-plan-generator-20261113/README.md` lines 46 and 70 still say the baseline sits "beside `data/`". That is stale against `22bfcbb`. Documentation only; not changed here (tracked file).

Caveat (MEASURED earlier, unchanged): `source_snapshot_sha256` is a hash of the custody baseline file, which is bound into the plan digest but never checked against the tree by the CLI. Its value depends on the snapshot taken.

GENERATOR: VERIFIED
