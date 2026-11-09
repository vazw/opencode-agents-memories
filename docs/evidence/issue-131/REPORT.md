# Issue131 — ENOENT liveness corroboration and A4 parser coverage

## Scope and authority

READ: approved blueprint `blueprint-20261109-0858-adr25-issue131-r2`, body SHA prefix `e968d61cdbad`, SYS-APPROVED 2026-11-09T09:04Z. Starting HEAD `24552dbb5c659cfac85ca2ac266fcbaf8702d011`, branch `main`, tracked tree clean. No commit/push authorized by blueprint. Only production source changed: `crates/mem-store/src/platform.rs`; no dependencies, public API, schema, config, lock deletion, live storage action, mounts or restart.

READ: ADR-0025 remains Proposed. No explicit owner acceptance supplied. Issue43 unchanged. Issue131 criteria independent of ADR acceptance.

## A/B decision — losing design retained

REPORTED diagnosis: `.tmp/progress/issue131.debug-1.md`, captured below in `runs.txt` with original raw output. Diagnosis used copied production source with metadata expression replaced by fixture adapter, retaining gathering/decision/probe bodies. Its initial RED was **source-replica evidence, not shipping package regression evidence**.

| Design | Pros | Cons | Prediction / refutation |
|---|---|---|---|
| A: ENOENT requires checked signal0 ESRCH before Absent | Hidden-live safety plus automatic stale Linux holder recovery | One syscall on ENOENT; requires checked positive PID and immediate errno capture; unknown stays held | Success/EPERM Alive, ESRCH Dead. Trusted same-PID-namespace hidden-live ESRCH refutes safety assumption; investigate namespace/security policy first. |
| B: every ENOENT unknown-present, no syscall | Smaller conservative code, no syscall, hidden holder retained | Dead Linux holder never automatically reclaimed | Real reaped child remains Alive; loses existing automatic stale recovery. Viable only if owner withdraws recovery contract. |

REPORTED shared discriminator (diagnostic logs preserved in `runs.txt`): original combined selector 5 passed/1 failed; A 6 passed; B 4 passed/2 failed. Selector also matched parser tests: these are **not liveness-only or production regression counts**. ENOENT/success and EPERM: original Dead/calls0, A Alive/calls1, B Alive/calls0. ENOENT/ESRCH: original/A Dead, B Alive. ENOENT/EINVAL/EIO: original Dead, A/B Alive. EACCES/injected ESRCH: all Alive/calls0. Real self Alive for all; reaped-child Dead original/A, Alive B.

READ decision: syseng chose A over B given measured discriminator and existing automatic stale recovery contract. No new A/B design experiment required or claimed. Native cross-user trigger remains unmeasured.

## Production sequence

MEASURED: existing P tests passed before/after behavior-preserving seam extraction, 6 tests each. Mechanical extraction only substituted metadata/start-time closures; NotFound still Absent and signal closure unused. Non-Linux untouched. Added T1 then ran production package command: compiled successfully, 1 test ran, assertion failed `Dead` versus `Alive` for actual ENOENT error, pid42/token7/signal success; exit101. This is shipping regression RED **after extraction**, not replica or compile-error proof.

MEASURED: minimal ENOENT branch fix queries private signal adapter. Checked conversion rejects zero/unrepresentable PID before syscall; signal literal0; `last_os_error` read immediately after failed `libc::kill`. Only raw ESRCH yields Absent. Success, EPERM, EINVAL, EIO, invalid narrowing remain unknown-present/Alive; EACCES never calls signal. T1 GREEN exit0, 1 passed.

MEASURED: T2 initially GREEN because Step3 already wired safe checked adapter. Regression proof uses M4, not invented baseline failure. T3 initially GREEN; correct parser unchanged. Independent hard-comm oracle strips exact known `<tid> (a) b)c) ` prefix, parses numeric suffix starting field4, checks field21/23 differ from field22 and exact token; wrong recorded token gives Dead. Thread name restored by RAII. T4 passed with real self Alive and naturally exited/reaped child, absent proc, signal0 ESRCH, Dead. No new child-kill/nonzero-signal fixture.

Final gate and mutant results appended after actual completion; `runs.txt` holds exact commands, exits, timestamps and output. Pending results are not passes.

## Measurement limits and failed harness attempts

REPORTED debugger setup failures retained: first synthetic signal adapter accidentally answered real-child control with default success; repaired to use real syscall for real filesystem controls. Initial parser-mutant replacement matched wrong statement; narrowed to production parser expression. Neither setup failure counted as mutant kill. Original diagnostic scripts, logs, environment, notes and report preserved in durable `runs.txt`; original scratch left untouched.

Native hidepid mount/cross-user EPERM, actual lock theft and cross-user storage sharing were **not** measured. Synthetic gathering seam establishes conditional classification defect, not native deployment exposure. Real self/reaped child establish ordinary same-namespace wiring only. Success may include zombie; Alive until reap is conservative. No atomic ownership guarantee or elimination of exit/reuse race. Windows/other-Unix tests are decision-table data on Linux, not native runners. ADR25 platform/vacuum/no-probe limitations unchanged.

READ correction to original A4 triage: `None` is conservative; wrong `Some` can disagree with recorded token and falsely return Dead. Blanket “all misparsing is safe” claim withdrawn. Parser itself required no fix.

Inverted world: trusted hidden-live same-namespace ESRCH rejects design pending diagnosis. Missing production verdict RED, surviving required mutant, or unavailable real control rejects proof, not permission to lower criteria. Owner withholding ADR acceptance does not block independent issue131 fix.

## Paused gate result

MEASURED: P-final 10 passed, exit0. fmt and both required clippy commands exit0. Package/workspace both exit101 at existing `turso_operation_lock_xproc::test_stale_lock_from_dead_pid_is_reclaimed`: fixture `DEAD_PID = u32::MAX` expects reclaim while approved checked-narrowing contract requires unknown-present/Alive. `MigrationLocked { pid: 4294967295, operation: Some("stale") }`. No fixture edit authorized by original scope; requested planner delta, paused without weakening adapter. Full green package/workspace claim withheld.

MEASURED: M1/M2/M3/M5/M6 each named assertion failure exit101, one failed test each. M4 first run fails syscall count for PID0, not required u32::MAX: this is NOT sufficient discrimination proof. Production table reordered to put u32::MAX first before pause; fresh isolated M4-max prepared but not run. Step4 remains open for exact-input proof. No new removed/disabled/weakened assertions.

REPORTED orchestrator gate update: initial static opsec READ-only approval superseded by REJECTED/blocked artifact permission, runtime validators not run by reviewer. Step7 remains open, issue131 not closed. Separate docs-only wave now owns contingent Step9 after explicit owner ADR approval; coder did not edit ADR0025, issue43 or W6 REPORT. Initial Proposed statements above describe authority at this wave start, not later docs-wave result.

STOP: awaiting revised approved scope for existing stale-recovery fixture. No commit/push.

## r4 scope correction in progress

READ authority: r4 body SHA `8910b3830c27`, SYS-APPROVED 2026-11-09T09:48Z; starting HEAD unchanged; unrelated workflow.md diff protected. Fixture A selected: real naturally exited/reaped positive representable PID with pre/post proc NotFound and signal0 ESRCH. Pros: proves actual dead recovery; cons: lifecycle/reuse checks. Losing B: constant positive representable presumed-absent number, smaller but can name live holder; rejected because range is not death proof. Prediction: genuine dead recovery and invalid-holder retention coexist; corroborated dead recovery refusal would refute fixture-only diagnosis.

MEASURED R GREEN0 one passed: child26443, successful wait status0, pre/post NotFound+ESRCH, replaced lock with current process. Historical impossible-PID RED reused because test byte-equal HEAD and platform SHA matched prior evidence, no replay. New N initially GREEN0 one passed: tokenless u32::MAX holder retained through read/acquire, byte-identical file, typed MigrationLocked. Mutant proof pending. Linux cfg explicitly excludes unsupported non-Linux dead recovery; invalid-holder test stays cross-platform. Old constant/comment/test body and operand replacement recorded verbatim in r4 ledger. No production adapter changes.

READ deterministic reuse timing/exhaustion and injected EACCES/EPERM fixtures are testable:no here: no new abstraction solely for injection; strict branch/error checks reviewed, actual absence/ESRCH measured, no atomic PID-reuse claim. Every spawn immediately waited before assertions; bound3 explicit fail, never skip/pass on unavailable.

CORRECTION during required historical verification: old M2.log fails calls0/1 on first ENOENT/success cell BEFORE reaching ESRCH verdict. Prior reports claiming M2 ESRCH verdict kill were inaccurate. Only four historical exact-input kills M1/M3/M5/M6 currently accepted; original M2 and original PID0-first M4 preserved as insufficient. r4 inherited-five assumption refuted by original output. Planner notified; no M2 rerun or further integration without scoped ruling. Already-started M4-max and invalid-holder isolated proof independent. Opsec still REJECTED/blocked permission, no completed report; no closeout.

## Fresh M4-max proof measured

MEASURED M4-max-production exit0: u32::MAX checked conversion InvalidInput/calls0; PID0 InvalidInput/calls0; test ok. Source SHA recorded .tmp/scratch/issue131-fixture-r4/M4-max-production.source.sha256.
MEASURED M4-max-mutant exit101: unchecked pid_t cast target=-1/signal0/calls1 before assertion; exact named assertion invalid pid=4294967295 must not reach syscall fails left1/right0; diff recorded. Fresh isolated source/target paths, live timestamps, exact command, verbatim output preserved.
MEASURED invalid-holder-mutant exit101: ENOENT+InvalidInput treated Absent mutation causes unrepresentable holder incorrectly reclaimed; read_live_lock assertion fails. Proof that checked adapter preserves unknown-present Alive; mutant loses it. No real invalid syscall invoked. Exact fresh source/target captured.

CORRECTED accepted count: four historical exact-input kills M1/M3/M5/M6. M2 original PID0-first failure not exact ESRCH proof; old PID0-first M4 preserved insufficient. Fresh M4-max accepted as sixth exact-input kill. Invalid-holder mutant accepted as seventh but on different seam. No acceptance claim for pending M2 rerun or unresolved opsec/final gates.

R GREEN0 reported pid26443 pre/post ESRCH/Dead; N GREEN0 reported u32::MAX retained MigrationLocked. Not independently rerun by this wave; recorded as reported artifacts. Integration gates withheld per orchestrator ruling until M2 delta and opsec resolution. No commit/push.

## r5 exact M2-ESRCH proof and full gates

READ authority: r5 body SHA `121de636d3bb`, SYS-APPROVED 2026-11-09T10:12Z; HEAD unchanged; r4 work preserved, no reset. Fixture A retained; B rejected. M2-ESRCH proof isolated: ESRCH cell first, diagnostics printed, verdict asserted before call-count. Production GREEN0 exit0: ENOENT/ESRCH verdict Dead calls=1; subsequent cells Alive/calls=1; EACCES/ESRCH Alive/calls=0. Mutant RED101 exit101: same ESRCH fixture, every-ENOENT-unknown-present mutant skips signal adapter (calls=0), yields unknown-present/Alive; verdict assertion fails `expected=Dead actual=Alive` BEFORE call-count check. Exact named-input proof achieved; old M2 insufficient log preserved with superseded label. Six distinct exact-input kills: M1/M3/M5/M6 historical + fresh M2-ESRCH + fresh M4-max. Invalid-holder mutant seventh on different seam. No root adapter change, no real invalid syscall.

MEASURED R/N/X/P/package exit0; fmt/clippy-lib/clippy-all exit0. Workspace flaky mem-core test_probe_refuses_git_executable_inside_untrusted_project_root fails in full suite but passes in isolation — pre-existing, not issue131 scope, not a regression from platform.rs or turso_operation_lock_xproc.rs changes.

Opsec REJECTED/blocked artifact permission remains OPEN; no durable runtime report. Step7 incomplete; issue131 not closed. No commit/push authorization. Step9 ADR/issue43/W6 docs wave separate; coder leaves untouched.


## Current disposition — r5 evidence reconciliation iteration 2 (supersedes historical gate state above)

READ: `.tmp/progress/issue131.debug-3.md` FIX PRESCRIPTION and r5 review-1 require evidence-only repair. Original r5 ledger archived byte-for-byte at `.tmp/progress/blueprint-20261109-0955-issue131-fixture-r5.ledger.pre-reconciliation-2.md`, SHA256 `503f20b45c58846783c09252dd3bc03164edced530501a37d1f3e3bd59d1b791`; original handoff-1, r2/r4 ledgers and raw logs preserved. Active ledger now has explicit fixture/invalid-holder/proof/inherited dispositions rather than duplicate STARTED cursors. Reconciliation times are live documentation actions; artifact run times remain qualified historical metadata. No source edits, tests/build replay, permission changes, commit or push in this repair.

### Errata and proof provenance

- Historical paragraph at line68 incorrectly says old M2 was PID0-first and calls M4-max sixth while M2 pending. Old M2 failed success-cell call-count0/1 before ESRCH; old M4 failed PID0-first. Four historical exact-input kills M1/M3/M5/M6 plus M4-max made FIVE, not six. Final M2-ESRCH supplies SIXTH. Invalid-holder mutant is separate seventh seam proof, not seventh of required six. Earlier generic exit101 claims are not named-input acceptance.
- READ r4 runs.jsonl:2-3: M4-max production exit0 InvalidInput/calls0 for u32::MAX; mutant101 recording closure target=-1/signal0/calls1 before named assertion `invalid pid=4294967295 must not reach syscall` left1/right0. No real invalid-target syscall. r4 runs:1,4 N baseline0 / invalid-holder mutant101 retained-lock assertion; r5 runs:8 N0. Fixture R inherited r4 child26443 wait success/pre-post NotFound+ESRCH and r5 runs:7 R0; Linux cfg excludes unsupported non-Linux stale recovery. Removed constant/comment/test and assertion operand retained in r4 ledger:23-53.
- READ r5 runs.jsonl:5-6 final M2 pair: production artifact10:24:55Z–10:26:17Z exit0; mutant10:26:17Z–10:27:44Z exit101. ESRCH FIRST, pid42/token7; production Dead/calls1; every-ENOENT-unknown-present mutant Alive/calls0 fails verdict FIRST. Configured ESRCH is NOT observed syscall errno when adapter skipped. Earlier pairs at :1-4 include setup failures; not extra kills. Root test was not reordered ESRCH-first: handoff-1's current-source ESRCH-first claim was wrong; instrumentation belongs to isolated copies only.
- Existing source/target paths, exact commands, artifact timestamps, hashes, diffs and verbatim assertion excerpts copied to durable `runs.txt` reconciliation section. Six coder proofs are not three independent reviewer probes.

### Integration and audit disposition

READ coder r5 runs.jsonl:7-15: R/N/X/P/package/fmt/clippy0, workspace101. The isolated pass does not establish that workspace failure was pre-existing, flaky or unrelated; historical causal assertions at line76 and handoff-1 are withdrawn. Cause UNCONFIRMED. Reviewer later workspace0 is distinct evidence, not cause proof.

REPORTED by independent reviewer in durable `reconciliation-r5/review-1.md:9-12`: R/N GREEN (reaped PID27666), cross-process6/platform10, package passed; final workspace/fmt/clippy-lib/clippy-all/cargo audit exit0. Exact logs copied verbatim to `reconciliation-r5/reviewer-gate-0.log` through `reviewer-gate-4.log`; shell provenance sh_bdpehtq5. Earlier timeout and head-truncated attempt excluded. No gate rerun during evidence repair.

READ completed audit: durable `reconciliation-r5/opsec-REPORT.md:17-20,97-107` states source-only analysis, audit_complete, **VERDICT: APPROVED**, validators PASS0 findings/PASS7 coverage units. Audit metadata start10:15Z/completed10:35Z are report claims, not newly measured timestamps. Reviewer personally executed both validators per `reconciliation-r5/review-1.md:12`; raw validator stdout not supplied there. This supersedes historical audit-permission-block assertions, including prior final paragraph; it does NOT claim runtime opsec execution or independent mutations.

### Remaining gate and excluded work

Independent mutation gate OPEN/BLOCKED: reviewer scratch edit `.tmp/scratch/issue131-review/mutations.py` denied `Permission denied: edit` per durable review-1:6. Three required independent probes have no results; selections not named there. Reviewer must recover/record selections and gain authorized narrow access before execution. No permission-layer diagnosis or bypass inferred; coder historical kills cannot substitute. Final review remains REJECTED/pending these probes and artifact recheck; no issue closure, commit or push authorized.

Inherited Step7 audit disposition reconciled APPROVED (source-only); inherited Step8 final independent review OPEN. Step9 DONE disposition only: transferred/excluded per blueprint:24,42,52. ADR acceptance owner-granted per task; historical ungranted sentence is not current authority. ADR25/issue43/W6 docs execution remains separate; workflow.md untouched. Native cross-user hidepid/EPERM, live lock theft and atomic PID-reuse proof remain unmeasured.


## Final independent disposition — r5 closeout transcription iteration 3

READ final independent `final-review-r5/review-3.md`: **STATUS: APPROVED**. This supersedes iteration-2 OPEN/BLOCKED independent gate and pending review statements above; historical records remain intact. Reviewer reports normal narrow scratch write admission succeeded, without permission bypass/configuration change. Approval covers unchanged Issue131 implementation/evidence, not ADR25/issue43/W6 or commit/push.

REPORTED independently measured by reviewer (shell `sh_gcrcovbx` exit0): exactly three semantic mutants M2/M4/M5, each with separate production control, six source trees and six distinct targets. Durable `final-review-r5/independent-runs.jsonl` retains exact absolute cwd/target/commands/start/end/exit; complete logs, instrumentation-only diffs, mutant-only diffs, source hashes, predetermined plan and runner copied beside it. `COPY-PROVENANCE.json` records original paths and byte hashes. No target or source-tree bulk copied. These are transcribed existing reviewer measurements, not new coder execution.

| Independent discriminator | Production control | Mutant exact behavioral failure |
| --- | --- | --- |
| M2, ENOENT/configured ESRCH FIRST, pid42/token7 | artifact13:07:16–13:09:40Z exit0, Dead/calls1 | artifact13:09:41–13:12:07Z exit101, every ENOENT unknown-present, Alive/calls0, verdict fails FIRST left Alive/right Dead. Configured ESRCH is not observed when adapter skipped. |
| M4, u32::MAX FIRST | artifact13:12:08–13:14:37Z exit0, InvalidInput/calls0 | artifact13:14:37–13:17:07Z exit101, recording closure target=-1/signal0/calls1, named invalid-pid assertion left1/right0. No real invalid-target syscall. |
| M5, comm `a) b)c`, independent exact-prefix field22 oracle | artifact13:17:07–13:19:37Z exit0, field22=60594629/Some(60594629) | artifact13:19:37–13:22:07Z exit101, first-`)` parser returns Some(0) against field22=60609663; named exact field22 assertion. Separate process tokens correctly differ between runs. |

All six runs collected exactly one named test. All mutants compiled and reached distinguishing assertion, not setup/earlier unrelated failure. Six distinct coder proofs remain M1/M3/M5/M6 historical + exact M2/M4; three independent probes supplement these, not nine distinct required mutants. Invalid-holder proof remains separate seam.

READ review-3:24-30: reviewer verified source hashes before/after, integrity58 entries and prior17 state checks. Prior checker intentionally checked then-current OPEN state; not final acceptance oracle. Completed earlier suite/fmt/clippy/audit/validators retained from review-1, not rerun by final reviewer or this transcription. Historical coder workspace101 cause stays unconfirmed; later reviewer workspace0 is separate evidence. Source-only opsec APPROVED remains distinct from these runtime probes; native cross-user hidepid and non-Linux recovery remain unmeasured.

Approved hashes preserved: platform.rs `78c6f73bddd26158913b98dd077542047dae483548a0623acb7a84a759a3e1b8`; turso_operation_lock_xproc.rs `b6cbe2d605a72cf4fdf2ffaab4f12528383e1ac767e798959a0152d4839a0044`. No new source changes or gate replay. Tracker left unchanged; no inferred tracker transition, commit or push authorization. Workflow/ADR/issue43/W6 excluded.

### Retained scratch; cleanup not authorized

MEASURED `du -sk` from repository root during transcription (KiB allocated, directory totals, not exclusive target reclaim estimates): issue131-production16872668; issue131-fixture-r4 8386100; issue131-fixture-r5 5509868; issue131-review276; issue131-review-final16854932. Total47623844KiB (~45.4GiB) across these five scratch roots. Source copies, targets and logs retained. Potential reclaim requires user-scoped approval; totals include evidence/source as well as build outputs, so not all bytes classified disposable. No deletion performed. Existing SHA256SUMS still references scratch evidence; before later cleanup, preserve/migrate referenced evidence and revise manifest under explicit scope.
