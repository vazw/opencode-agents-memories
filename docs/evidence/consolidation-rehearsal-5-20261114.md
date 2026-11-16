# Consolidation rehearsal-5 — 2026-11-14 (blueprint r12 Step 9)

Re-run of the rehearsal-4 consolidation rehearsal suite on artifact H2 (Step 6
rebind), on disposable scratch roots under
`.tmp/scratch/rehearsal-5-20261114T153119Z/`. No production mutation; the live
storage root was touched only by read-only sha256sum access (disclosure §7).
Evidence doc: `docs/evidence/consolidation-rehearsal-5-20261114.md`.

## 1. Artifact under test

- **H2 = `9d7073efd4900449e64c36089c79f0d6855ea239a1cb3389beffdcde5e4ff7cd`**,
  83197752 bytes (MEASURED, re-verified at env setup from
  `.tmp/cutover-artifact/opencode-agents-memory`).
- Built at commit `a6e9d57` (`fix(consolidation): skip bookkeeping records in
  effective phase and last-record reads`, parent `07d4440`). READ, ledger Step 5.
- Supersedes `edc5783306ead…0266` (rehearsal-4's binary) for every purpose. READ,
  ledger Step 6 + NV-1 pin section.
- H2 was never installed into `plugin/engine/`; every run in this suite executed
  the scratch copy under the rehearsal root.

## 2. Windows

| window | scenario | result | plan_sha256 |
|---|---|---|---|
| W1 | clean install → seam-kill → (i)(ii)(iv) | all green (MEASURED) | `8780fe7aace90e91` |
| W2 | clean install → seam-kill → display-vs-raw → (iii)(iv) | all green (MEASURED) | `904d4dc097537ecb` |
| W3 | full driver pipeline (`consolidation-cutover-window.sh`) | green after environmental retry (MEASURED) | `10d9087dd7619f48` |
| W4 | NV-1 boundaries b1–b5 | all green (MEASURED) | `01c964c4fe8fbad0` |

Window setup per window: writable copy of the frozen snapshot → plan → dry-run
(34 sources) → acquire → snapshot → verify-snapshot → apply → staged-verify →
install, rc 0 at every stage, journal reaching `[SV, Installing, IU]`
(MEASURED, `w{1,2,4}/steps.txt`, `w3.log`). Clean install reaches
`InstalledUnverified` deterministically — no timed kill needed for setup.

## 3. Item evidence

| item | window | observation | label |
|---|---|---|---|
| (i) kill at `retake-after-record` | W1 | `consolidate-recover` rc **134** (`kill_at` = `std::process::abort()`); journal last `{"seq":4,"phase":"Retaken","retake_seq":1}`; reservation held; body-`retake_seq` **absent** — the seam at `src/shard_consolidation.rs:5100` fires after the write-ahead `Retaken` record and before the body rewrite | MEASURED |
| (i′) same seam, shadow window | W2 | rc 134; raw journal last is `Retaken` seq 4 while the driver displays `InstalledUnverified` (§6) | MEASURED |
| (ii) verify on held reservation | W1 | `consolidate-verify` rc **0**; last record `Verified`, complete (staged+digest+root); verified-count=1; reservation still held | MEASURED |
| (iii) recover on IU shadow | W2 | `consolidate-recover` rc **0**; effective phase `Verified`, complete; verified-count=1; retaken-records=2 (no bare-`Verified` shadow — bookkeeping skip active); reservation **HELD** after recover (IU arm `:5185-5191` settles without releasing — the 2f fix) | MEASURED |
| (iv) release | W1 | `consolidate-release` rc **0** after (ii); reservation absent afterwards | MEASURED |
| (iv′) release on old-bytes shadow path | W2 | `consolidate-release` rc **0** after (iii); reservation absent — defect-2 measured on the exact `Retaken`-shadow path | MEASURED |

Source facts backing the seam/order claims: READ at `a6e9d57`,
`src/shard_consolidation.rs` — seam `:5100`, retake `:5182-5184`, IU settle
`:5185-5191`, terminal release `:5192-5196`.

## 4. stdio fence pair (W1)

- Fence on held reservation: engine startup refused rc **76**
  `maintenance.reservation_held`, stdout 0 bytes; storage-tree diff before/after
  **NONE** (123 entries, post-install tree). MEASURED.
- Control on the same root after release: engine started rc **0**. MEASURED.
  (Control ran after `consolidate-release`; its engine startup mutates S, which
  is why the check2 copy was taken before it.)

## 5. Part A — oracle + check2 (W1)

- oracle14 (argv: plan, src, staged, sidecar, out): **34/34 memories exact**;
  totals `source_rows=survivors=occurrences=5827`, targets 34; `findings=[]`.
  MEASURED (`w1/oracle14-report.json`).
- Aux **7/7 tables rows_equal**: `agent_sessions` 888→887 (1 discarded,
  875 remapped), `capture_inbox` 717→494 remapped, `user_prompts` 1759→1742
  remapped, `skills` 0; `byte_equal` divergence list empty. MEASURED.
- Tree: 1 carried shard (`project_2991cca9ba1ab013_shard_0.db`), no source shard
  files left in staged, no root drops. MEASURED.
- Sidecar aux-divergent journal: 18 lines `{divergent:1, carry:16}`, 8254 bytes,
  carry-set matches. MEASURED.
- check2 (copy-only; never touched S): rc 0; sqlite `integrity_check` ok on every
  DB; registry 35 rows / 34 targets, no shape violations, no rows-without-file or
  files-without-row; cosine harness 5903=5903 vector rows,
  `blob_bytes_identical=true`, width 768. MEASURED (`w1/check2.json`).

## 6. NV-1 boundaries (W4) and driver (W3)

| boundary | observation | label |
|---|---|---|
| b1 seam-kill at retake-after-record | rc 134; no retake tmp file left | MEASURED |
| b2 timed kill 2 ms after recover start | rc **137** (SIGKILL); last=`Retaken` seq 4; reservation held; body-retake_seq absent | MEASURED |
| b3 concurrent recover | r1 rc 0, r2 rc **77** `consolidation.window_busy` (flock exclusion) | MEASURED |
| b4 convergence | last=`Verified` complete; verified-count=1; retaken-records=2; reservation present | MEASURED |
| b5 release after convergence | rc 0; reservation absent | MEASURED |

NV-1's convergence invariant 3 previously **FAILED** on old bytes
(`edc57833…0266`); it **PASSES** on H2. MEASURED (this suite) against the
failure recorded in `.tmp/progress/20261114-nv1-crash-fixtures.md`.

Driver:

- W3 run 1 failed at apply: exit 74 `consolidation.staged_shard_failed …
  pwrite: no storage space`. Root-caused **environmental**: `/tmp` tmpfs at
  100% (`df`: 4.0K free) with `TMPDIR` not exported on that invocation; `/home`
  had 315G free. Proven by `df`, not inferred. MEASURED.
- W3 run 2 with `TMPDIR`/`SCCACHE_DIR` exported: driver exit **0** in 40 s;
  34 sources, **5827 survivors**; window complete; final journal
  `{"seq":4,"phase":"Verified"}`; reservation absent; engine lock absent; fd
  holders under S: none. MEASURED (`w3.log`, shell exit receipt).
- Display-vs-raw check (W2): raw last line
  `{"seq":4,"phase":"Retaken","retake_seq":1,…}` while the driver's
  `effective_journal_line` displays `{"seq":3,"phase":"InstalledUnverified"}` —
  the bookkeeping-skip filter works on H2. MEASURED
  (`w2/w2-journal-retaken.jsonl`).

## 7. Frozen-manifest end check and disclosures

- Custody manifest file hash unchanged:
  `e0fe0276fe5cf769a649f742bbc18ba26e3383a4575a8e3edac9fab6f53d258e` before and
  after the suite (MEASURED). Read-only `sha256sum -c` inside the live custody
  `data/` → **205 OK / 0 FAILED** → root-tree diff **NONE** (MEASURED).
- Disclosures:
  1. Live recovery custody was accessed **read-only** three times: manifest
     stat+sha256, the 205-file `sha256sum -c`, and the initial `cp -a` seed.
     Rehearsal-4/NV-1 precedent; the frozen-manifest diff is only reachable
     there. No binary mode ever ran against the live root.
  2. Scratch resets used `find <scratch> -mindepth 1 -delete`; no `rm -rf`.
  3. W3 run 1 ENOSPC was environmental (§6), not a binary defect.
- Deviation: (iii) and (iv) were measured on sibling windows, not one window —
  recover releases the reservation itself at terminal phase
  (`:5192-5196`), so a post-recover `consolidate-release` cannot run on the
  same window; the IU-arm recover keeps it (W2 measured HELD), and W2's release
  then ran on W2. Both release paths are therefore covered.
- First-measurement note: rehearsal-4's recover hit the `Installing` arm only;
  the IU-settle/terminal-recover path had never been exercised on any binary
  before this suite. Items (ii)–(iv) and boundaries b2–b5 are first
  measurements (INFERRED from rehearsal-4 notes + source reading; confirmed by
  H2's observed behaviour).

## 8. Verdict

Every declared observation reproduced on H2; no new findings (`findings=[]`,
integrity clean, manifest untouched). **SETTLEMENT: PASSED.**
