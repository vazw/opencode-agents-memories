# Consolidation rehearsal-6 — 2026-11-16 (blueprint r2 post-change obligations 1+2)

Re-run of the rehearsal-5 consolidation rehearsal suite on artifact **H3** (built
at `13171c9`, the sshd fail-closed-exemption slice), on disposable scratch roots
under `.tmp/scratch/rehearsal-6-20261116T105425Z/`. H2 was preserved before
anything replaced the artifact path, the mode-surface probe re-proved freshness,
and the defect-specific pair (H2 refuses / H3 succeeds under ambient
`sshd-session` leaders) was measured. No production mutation; the live storage
root was touched only by read-only sha256 access (disclosures §9). This file and
`docs/evidence/consolidation-rehearsal-6-20261116.md` are byte-identical.

## 1. Artifact under test

- **H3 = `e07df28057d05891ab45c2d79b70a5a865d394a17f346b6b9db4ccaa9e8911fa`**,
  83,200,696 bytes (MEASURED from the build output and re-verified after
  install).
- Exact build command: `cargo build --release --offline`, run in a detached
  worktree pinned at `13171c9` (`.tmp/scratch/wt-h3-13171c9`, `git status`
  clean; the main tree carries unrelated dirty files, so the worktree is what
  makes "at 13171c9" literally true). Env: `TMPDIR` and `CARGO_TARGET_DIR`
  redirected under `.tmp/scratch/` (`/tmp` filled earlier in this wave).
  Exit code **0**, 2 m 35 s. One pre-existing warning in committed code
  (`mem-web` deprecated `Atomic::fetch_update` at
  `crates/mem-web/src/api/deduplication/mod.rs:368`) — not touched by this
  slice. MEASURED.
- Built at `13171c9` (`fix(mem-store): exempt sshd session leaders from the
  unreadable-fd refusal`, parent `a6e9d57`). READ, git log.
- **H2 = `9d7073efd4900449e64c36089c79f0d6855ea239a1cb3389beffdcde5e4ff7cd`**,
  83,197,752 bytes, preserved at
  `.tmp/cutover-artifact/opencode-agents-memory.9d7073ef` **before** anything
  replaced the artifact path; sha256 re-verified on the copy and `cmp`
  byte-identical to the original (MEASURED). H2 was never deleted; H3 was
  installed over `.tmp/cutover-artifact/opencode-agents-memory` afterwards and
  the sha re-verified at the path (MEASURED). Nothing was installed into
  `plugin/engine/linux-x64/` — that remains the operator's separately-gated
  step. Every rehearsal run executed scratch copies (setup windows ran
  `$BIN` = the artifact path; W3's driver installed its own scratch copy,
  sha-verified `e07df28057d05891…` at `w3/engine/`).
- H3 supersedes H2 for every rehearsal/cutover purpose; H2 is retained only as
  the defect-pair control (§3). No cutover may use H2 (blueprint post-change
  obligation 1).

## 2. Mode-surface probe (staleness discriminator)

`H3 --mode=consolidate-acquire --storage-root /nonexistent-probe-root` → exit
**77**, stderr event `consolidation.acquire_io` ("the reservation could not be
acquired"); `/nonexistent-probe-root` was **not created** (MEASURED,
`probe.err`). A stale artifact without `consolidate-acquire` would exit 64 —
H3 is fresh.

## 3. Defect-specific pair — the slice's whole point

Fixture roots under `.tmp/scratch/defect-proof.04iRAs/` (positive allowlist
guard, literal production-root inequality, 0755 parents; blueprint Step-8
shape). Census recorded in the same shell immediately before the runs:

- Same-uid processes with unreadable `/proc/<pid>/fd`: **1405, 21852, 27344 —
  all `comm=sshd-session`** (MEASURED; matches the blueprint's census and the
  driver's own state dump in `w3.log`).

| binary | root | rc | event | reservation |
|---|---|---|---|---|
| H2 (`…9d7073ef` preserved copy) | `<fixture>/h2/data` | **77** | `consolidation.acquire_writer_active` | absent — the refusal minted nothing (MEASURED) |
| H3 | `<fixture>/h3/data` | **0** | — | minted, nonce `VxExs+LYyPD0zCdLpkXPfQ==` (MEASURED) |

H2 still refuses byte-for-byte as shipped; H3 succeeds over the exact condition
that blocked four production windows. The H3 reservation file was left on the
disposable fixture by design: `consolidate-release` resolves custody by scanning
`<parent>/recovery` for the window-token dir, and this fixture never took a
snapshot, so release would refuse 77 `release_phase_nonterminal` — no live
holder exists (the flock died with the acquire process). The H2/H3 pair passed
on the first run; harness history in §9.3.

## 4. Windows

| window | scenario | result | plan_sha256 |
|---|---|---|---|
| W1 | clean install → seam-kill → (i)(ii)(iv) + fence + oracle/check2 | all green (MEASURED) | `d977a2a026da6ed6` |
| W2 | clean install → seam-kill → display-vs-raw → (iii)(iv) | all green (MEASURED) | `28e9138c82c9c358` |
| W3 | full driver pipeline (`cutover-window-rehearsal.sh`, see §9.1) | green (MEASURED) | `4d724c6d7bcd7fe8` |
| W4 | NV-1 boundaries b1–b5 | all green (MEASURED) | `bb05f93110fa2a01` |

Window setup per window: writable copy of the frozen snapshot (205/205
`sha256sum -c` OK on every copy) → plan → dry-run (34 sources) → acquire →
snapshot → verify-snapshot → apply → staged-verify → install, rc 0 at every
stage, journal reaching `[SV, Installing, IU]` (MEASURED,
`w{1,2,4}/steps.txt`, `w3.log`). Clean install reaches `InstalledUnverified`
deterministically.

## 5. Item evidence

| item | window | observation | label |
|---|---|---|---|
| (i) kill at `retake-after-record` | W1 | `consolidate-recover` rc **134**; journal last `{"seq":4,"phase":"Retaken","retake_seq":1}`; reservation held; body-`retake_seq` **absent** — the seam fires after the write-ahead `Retaken` record and before the body rewrite | MEASURED |
| (i′) same seam, shadow window | W2 | rc 134; raw journal last is `Retaken` seq 4 while the driver displays `InstalledUnverified` (§8) | MEASURED |
| (ii) verify on held reservation | W1 | `consolidate-verify` rc **0**; last record `Verified`, complete (staged+digest+root); verified-count=1; reservation still held | MEASURED |
| (iii) recover on IU shadow | W2 | `consolidate-recover` rc **0**; effective phase `Verified`, complete; verified-count=1; retaken-records=2 (no bare-`Verified` shadow); reservation **HELD** after recover | MEASURED |
| (iv) release | W1 | `consolidate-release` rc **0** after (ii); reservation absent afterwards | MEASURED |
| (iv′) release on old-bytes shadow path | W2 | `consolidate-release` rc **0** after (iii); reservation absent | MEASURED |

Every declared rehearsal-5 observation reproduced unchanged on H3 — the
exemption slice touches only `crates/mem-store/src/maintenance_reservation.rs`
(AC-1 of the blueprint), and nothing in the consolidation state machine moved.

## 6. stdio fence pair (W1)

- Fence on held reservation: engine startup refused rc **76**
  `maintenance.reservation_held`, stdout 0 bytes; storage-tree diff before/after
  **NONE** (123 entries, post-install tree). MEASURED.
- Control on the same root after release: engine started rc **0**. MEASURED.

## 7. Part A — oracle + check2 (W1)

- oracle14: **34/34 memories exact**; totals
  `source_rows=survivors=5827`, targets 34; `findings=[]`. MEASURED
  (`w1/oracle14-report.json`).
- Aux **7/7 tables rows_equal** (`agent_sessions` 888→887, `capture_inbox`
  717→494 remapped, `user_prompts` 1759→1742 remapped, `skills` 0);
  sidecar 18 lines `{divergent:1, carry:16}`, carry-set matches. MEASURED.
- check2 (copy-only): rc 0; booleans 7/7 true, `blob_bytes_identical=true`,
  `non_collinear=true`, `finite=true`. MEASURED (`w1/check2.json`).

## 8. NV-1 boundaries (W4) and driver (W3)

| boundary | observation | label |
|---|---|---|
| b1 seam-kill at retake-after-record | rc 134; no retake tmp file left | MEASURED |
| b2 timed kill 2 ms after recover start | rc **137** (SIGKILL); last=`Retaken` seq 4; reservation held; body-retake_seq absent | MEASURED |
| b3 concurrent recover | r1 rc 0, r2 rc **77** `consolidation.window_busy` (flock exclusion) | MEASURED |
| b4 convergence | last=`Verified` complete; verified-count=1; retaken-records=2; reservation present | MEASURED |
| b5 release after convergence | rc 0; reservation absent | MEASURED |

Driver (W3):

- Window complete in ~53 s: artifact hash gate + mode-surface probe passed in
  preflight; install → acquire → snapshot → verify-snapshot → plan → dry-run →
  apply → staged-verify → install → settlement verify → release, exit 0
  throughout. Window token `fc3fabcf462ce71a`; custody
  `consolidation-1794826928-fc3fabcf462ce71a` (retired by the release);
  34 sources, **5827 survivors**, 34 groups; final journal
  `{"seq":4,"phase":"Verified"}`; reservation absent; engine lock absent; fd
  holders under S: none. MEASURED (`w3.log`).
- **The driver's acquire succeeded while its own state dump prints the ambient
  census `unreadable fd dirs (fail-closed for acquire): 1405(sshd-session)
  21852(sshd-session) 27344(sshd-session)`** — the exact condition that refused
  every H2 acquire. This is the slice's proof inside the full pipeline, not
  only on the fixture pair of §3. MEASURED.
- Display-vs-raw (W2): raw last line `{"seq":4,"phase":"Retaken"}` while the
  effective filter displays `{"seq":3,"phase":"InstalledUnverified"}` — the
  bookkeeping-skip filter works on H3. MEASURED (`w2/w2-journal-retaken.jsonl`).

## 9. Harness deviations and disclosures

1. **Driver rehearsal copy** (`cutover-window-rehearsal.sh`; exact diff in
   `driver-deviation.diff`): (a) the `window` case skips `fence_no_opencode`
   and `fence_no_unreadable_fd` — both are ambient-host fences, not
   root-mutation guards: this rehearsal ran from inside a live opencode session
   (`pgrep -x opencode`: 24738 24748 24806) and the three `sshd-session`
   leaders are the fixture condition this slice legitimises; (b) `ROOT` is
   pinned to the repo root because the original derives it from `$0/../..`,
   which breaks under `.tmp/scratch/rehearsal-6/`. Everything else is
   byte-identical to `docs/evidence/consolidation-cutover-window.sh`. The
   rehearsal copy is not for production use.
2. **Seed gaps in `run-all.sh`** (rehearsal-5 seeded both outside its scripts):
   `w3/data` (frozen-snapshot copy; the driver requires S to pre-exist and its
   preflight dies before any window step otherwise) and `w3/engine/` (the
   driver's `install` target dir). Two aborted attempts preceded the green run;
   both abort dumps show `reservation: absent`, `journal: absent` — no window
   state ever existed, and each attempt re-seeded a fresh snapshot copy
   (`resume-w3.sh` keeps a refuse-to-clobber guard for real window state).
3. **Defect-proof harness v1** (fixture `.tmp/scratch/defect-proof.8oXbrQ/`)
   wrongly asserted a post-acquire `consolidate-release`; release refuses 77
   without a custody snapshot by design (see §3). The H2/H3 pair itself passed
   on the first run in v1; v2 (`defect-proof.04iRAs/`) records the same pair
   and drops the wrong assertion.
4. **Live-root access was read-only**, three kinds, rehearsal-4/5 precedent:
   the custody manifest `sha256sum`, the 205-file `sha256sum -c` inside the
   live custody `data/`, and the `cp -a` seeds into scratch. No binary mode
   ever ran against the live root.
5. **Suite split across two invocations** (background-shell kills at turn
   boundaries): W1–W4 by `run-all.sh` (W3 aborted at its preflight, twice, per
   §9.2), W3 + end checks by `resume-w3.sh`. All logs kept verbatim in
   `.tmp/scratch/rehearsal-6-20261116T105425Z/`.

## 10. Frozen-manifest end check

- Custody manifest file hash unchanged:
  `e0fe0276fe5cf769a649f742bbc18ba26e3383a4575a8e3edac9fab6f53d258e` before and
  after the suite (MEASURED, `frozen-end-hash.txt`; the same value was recorded
  at 10:54Z before any window ran). Read-only `sha256sum -c` inside the live
  custody `data/` → **205 OK / 0 FAILED** (MEASURED,
  `frozen-end-verify.txt`, `frozen-end-ok-count.txt`).
- Step-7 display-filter gate: the effective-phase filter line is present
  exactly once in the driver and once in the runbook (`step7-grep.txt`).

## 11. Item table

| # | item | result |
|---|---|---|
| 1 | Build H3 at 13171c9 (`cargo build --release --offline`, worktree) | PASS |
| 2 | Preserve H2 → `.9d7073ef`, hash re-verified | PASS |
| 3 | Mode-surface probe on H3 → 77 `acquire_io`, root not created | PASS |
| 4 | Defect pair: H2 77 `acquire_writer_active` / H3 0, census 3× `sshd-session` | PASS |
| 5 | W1 setup + (i)(ii)(iv) + stdio fence pair + oracle/check2 | PASS |
| 6 | W2 setup + seam-kill + display-vs-raw + (iii)(iv′) | PASS |
| 7 | W4 NV-1 boundaries b1–b5 | PASS |
| 8 | W3 full driver pipeline incl. acquire under the sshd census | PASS |
| 9 | End checks: frozen manifest unchanged, 205 OK | PASS |

## 12. Verdict

Items measured: build/preserve/probe (§1–2), defect pair (§3), windows W1–W4
(§4–8), frozen-manifest end check (§10). Every declared rehearsal-5 observation
reproduced on H3; the sshd exemption behaves as approved (H2 refuses, H3
succeeds, exact-name exemption only); no new findings.

**SETTLEMENT: PASSED**
