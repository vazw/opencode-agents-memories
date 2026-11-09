# Opsec Audit Report — opsec-issue131-r5

## Run Metadata
- **Run ID**: opsec-issue131-r5
- **Repository**: opencode-agents-memory
- **Target**: crates/mem-store/src/platform.rs
- **Commit**: 24552dbb5c659cfac85ca2ac266fcbaf8702d011 (dirty)
- **Profile**: standard
- **Scope**: crates/mem-store/src/platform.rs
- **Budget**: 50 units
- **Output Directory**: /home/vaz/Code/Personal/Rust/opencode-agents-memory/.tmp/progress/opsec-issue131-r5
- **Started**: 2026-11-09T10:15:00Z
- **Completed**: 2026-11-09T10:35:00Z
- **Run Status**: audit_complete

## Posture Confirmation
- Report-only: `edit: deny` enforced — no target modifications
- Spawn-proof: `subagent: deny` with `explore: allow` only — serial execution
- No target-controlled execution — source-only analysis per fleet policy
- Validators: both pass (findings.json, coverage-ledger.json)

## Coverage Summary
| Unit | Surface | Boundary | Attack Class | Status |
|------|---------|----------|--------------|--------|
| 1 | ProcObservability::current | host-classification | config-plane | covered |
| 2 | SystemProcessProbe::liveness | production-probe-integration | resource-exhaustion | covered |
| 3 | decide_liveness | lock-reclamation-decision | policy-matching | covered |
| 4 | observe_process_with | host-observability | state-tracking | covered |
| 5 | read_proc_start_time | procfs-parser | parser-bounds | covered |
| 6 | signal_zero | unsafe-libc-boundary | memory-safety | covered |
| 7 | signal_zero_with | unsafe-libc-boundary | memory-safety | covered |

**Total units**: 7 planned, 7 covered, 0 incomplete

## Findings Summary
| Verdict | Count |
|---------|-------|
| confirmed | 0 |
| needs_validation | 0 |
| rejected | 0 |

**No security vulnerabilities confirmed.** The audited code correctly implements fail-closed liveness semantics for stale lock detection with PID reuse protection via start-time tokens.

## Key Audit Observations

### 1. Unsafe Libc Boundary (signal_zero_with, signal_zero)
- **PID conversion**: `libc::pid_t::try_from(pid)` with `.filter(|target| *target > 0)` correctly rejects PID 0 and unrepresentable values (u32::MAX on platforms where pid_t is i32), returning `InvalidInput`.
- **Unsafe block**: `libc::kill(target, 0)` called only with validated positive PID and signal 0. SAFETY comment accurately describes adapter guarantees.
- **Errno handling**: `std::io::Error::last_os_error()` captured immediately after failed syscall — correct.

### 2. Procfs Parser (read_proc_start_time)
- **Comm field parsing**: Uses `rsplit_once(')')` to find last `)` in comm field, then token index 19 (field 22) for starttime. Validated by `test_start_time_parser_handles_comm_with_spaces_and_right_parens` with real comm containing `)`.
- **Error propagation**: Returns `Option<u64>` — `None` on any read/parse failure, treated conservatively as "alive" by decision logic.

### 3. Host Observability (observe_process_with)
- **PID 0**: Short-circuits to `Absent` → `Dead` (null ID never a holder).
- **Metadata Ok**: Reads start_time token → `Present{Some(_)}` or `Present{None}`.
- **NotFound + signal0**: Only `ESRCH` yields `Absent` (Dead); other errors → `Present{None}` (Alive) — hidepid conservative.
- **Other errors (EACCES, EIO, EPERM)**: Conservative `Present{None}` (Alive) — never steal a possibly-live lock.

### 4. Liveness Decision (decide_liveness)
Complete decision table implemented:
| Observability | PID | Recorded Token | Observed | Verdict |
|---------------|-----|----------------|----------|---------|
| any | 0 | any | any | Dead |
| Unobservable | >0 | any | any | Alive (fail-closed) |
| ProcFs | >0 | any | Absent | Dead |
| ProcFs | >0 | absent | Present{Some(_)} | Alive |
| ProcFs | >0 | Some(_) | Present{None} | Alive |
| ProcFs | >0 | Some(t) | Present{Some(t)} | Alive |
| ProcFs | >0 | Some(t) | Present{Some(other)} | Dead (PID reuse) |

All 7 cells covered by tests including cross-platform (`test_decide_liveness_both_directions_per_platform`) and PID reuse (`test_decide_liveness_pid_reuse_is_dead_on_mismatch`).

### 5. Host Classification (ProcObservability::current)
- Linux + accessible `/proc/self/stat` → `ProcFs`
- Stripped container (no `/proc/self/stat`) or non-Linux → `Unobservable` (fail-closed)
- Correctly prevents lock theft in containers without procfs

### 6. Production Integration (SystemProcessProbe)
- `is_alive`: Delegates to `decide_liveness` with `recorded_start: None` (existence only)
- `liveness`: Full token-aware verdict via `decide_liveness` + `observe_process` + `ProcObservability::current`
- Consumed by `read_live_lock` (operation_lock.rs:126) and `acquire_turso_operation_lock` (records own start_time via `LockRecord::for_this_process`)
- Validated by cross-process tests: `test_stale_lock_from_dead_pid_is_reclaimed` (reaped child + ESRCH → reclaimed) and `test_unrepresentable_holder_pid_is_not_reclaimed` (u32::MAX → retained)

## Mutation Testing Evidence (from prior wave)
Six distinct exact-input mutation kills accepted:
- M1: old NotFound Absent mutant (historical)
- M3: any signal error Absent mutant (historical)
- M5: first-delimiter parser mutant (historical)
- M6: wrong index18 parser mutant (historical)
- M2-ESRCH: fresh ESRCH-first mutant (isolated)
- M4-max: fresh u32::MAX-first mutant (isolated)

Plus invalid-holder mutant (separate seam) killed. All mutations target exact required inputs and fail as expected.

## Disclosure: Confirmed Definition
Per fleet policy, **`confirmed` = a static source property** (complete trace + no visible preventing layer + named lower-trust principal + named resource + concrete impact). Never a runtime-demonstrated result. Runtime-decisive facts default to `needs_validation` with paste-pack plan. This run found no confirmed vulnerabilities.

## Validators
- `validate-findings.cjs`: PASS (0 findings)
- `validate-coverage-ledger.cjs`: PASS (7 units)

## Verdict
**VERDICT: APPROVED**

No confirmed findings with overall_severity high or critical. Run complete with all validators passing.
