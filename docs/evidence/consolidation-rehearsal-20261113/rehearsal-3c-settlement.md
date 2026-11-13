# Rehearsal-3c: final settlement, lossless shard consolidation on `eacb66d`

Scope: settlement only. No new measurements. Evidence is the rehearsal-1, -2, -3 and -3b reports and their scratch artifacts. Scratch root for this attempt: `.tmp/scratch/rehearsal4-20261113T113418Z/` (`g/`, `rb/`, `run/`, `f5/`, `pristine/`, `oracle3/`).

## Provenance
- HEAD `eacb66dd25edeac7d1d41b3b366756a3d380b006`. Binary sha256 `9fa3dfdf1db05a2888c1535f5e7eeb80c7ff48f369cffac8542fc92d4decdb7d`.
- Frozen snapshot `/home/opencode/.opencode-mem/recovery/consolidation-20261112T030050Z-452b0f5d/`: `manifest.final.sha256`, `manifest.baseline.sha256`, `manifest.sha256` all `e0fe0276fe5cf769a649f742bbc18ba26e3383a4575a8e3edac9fab6f53d258e`. `data` mode 555. `recovery/` 1 entry. Before and after: the rehearsal-3 pre-run check and the rehearsal-3b post-run check (`3b-frozen-after.txt`). No command in this pass touched the snapshot; the last frozen check is rehearsal-3b's post-run check.
- Live `S` never opened. No commit, push, cutover, or production mutation. Steps 11–13 not run.

## F1' ruling (owner, option a)
Repeat-apply via `consolidate-shards --apply` at `Verified` refuses 77 `window_not_applyable`. Settlement re-run (`consolidate-verify`) at `Verified` is the no-op: exit 0, custody bytes and journal mtime unchanged.

Basis for the ruling:
- `src/shard_consolidation.rs:3572-3585` carries the rev-13 pin with its reason: the sidecar is not hashed into any manifest, so refusing 77 stops a re-apply from silently rewriting the audit record from the installed tree.
- Rehearsal-1 F1 was repeat-apply returning 72 instead of 77. The fix made it 77.
- Design line 333 is pre-rev-13 body text. The later amendment in the same approved document supersedes it.
- `test_verified_repeat_apply_is_noop` (`tests/shard_consolidation.rs:4447-4510`) exercises settlement, not apply. It does not contradict the 77.

F1' is therefore NOT a defect and NOT a blocker. It is a documentation supersession, recorded as a named documentation defect carried like C30–C32: design line 333 wording and the blueprint's `test_verified_repeat_apply_is_noop` row to be corrected at the next revision. Not amended now, to keep the dual approval valid for the body.

## 1. Item table — final disposition

| ID | item | disposition | basis |
|---|---|---|---|
| F1 | repeat-apply at `InstalledUnverified` returned 72 instead of 77 | **CLOSED** | 77 `window_not_applyable`, S and sidecar unchanged (rehearsal-3b §2, `g/repeat-installed.exit`) |
| F1' | repeat-apply at `Verified` vs design line 333 no-op | **CLOSED — not a defect.** Apply refuses 77 by rev-13 pin; settlement is the no-op. Design-text supersession carried as documentation defect | ruling above; rehearsal-3b §2 (`g/repeat-verified.exit`=77; `g/repeat-verify.exit`=0, journal mtime and bytes unchanged) |
| F2 | 300 `agent_sessions` FK orphans | **CLOSED** | reparent 300, independent oracle 34/34 exact, `agent_sessions` 888→887 rows_equal true, sidecar agreement, FK dangling 0 (rehearsal-3 §5–§6) |
| F3 | `shard_stats` wrong in 34/35 | **CLOSED** for 34 targets (34/34 match `COUNT(*)`). `2991cca9…` carried-legacy: source and staged have no `shard_stats` table | rehearsal-3 §4 |
| F4 | `--apply` writes `S/.wal-mode` | **CLOSED** | `.wal-mode` sha identical pre and post apply (rehearsal-2 §A) |
| F5 | settlement must refuse on a writable-open marker | **CLOSED, both halves.** Planted `.wal-mode` before settlement: exit 77 `post-install source edit`, journal to `RollingBack`. Clean settle: exit 0. `M1_EXCLUDED` still `["status","engine.lock"]` | rehearsal-3 §3. Caveat: marker is a byte copy of the engine's `.wal-mode` (sha `98a1b243…`), not a live writable open; live open not measured |
| F6 | install without custody baseline | **CLOSED on install.** Refused 77 `baseline unreadable`, S and staged intact | rehearsal-3 §1 |
| Rollback | whole-root restore from snapshot | **CLOSED.** Kill after L4 exit 134, recover 0, rollback 0. 209/209/209 files. Digest `0ceba1b6…` equal to custody baseline and pre-apply, 0 diff lines. Restored `(mode, kind, path)` equals `custody/mode-table`, 222/222, 0 diff | rehearsal-3b §1 (`rb/`) |
| Repeat settlement | `consolidate-verify` at `Verified` is no-op | **CLOSED** | rehearsal-3b §2 |
| Oracle §C.5a | reparent rule validated from spec, independent of implementation | **CLOSED.** 34/34 memory targets exact; `agent_sessions` 888→887; reparent 300; sidecar agrees on set and rows | rehearsal-3 §5 (`oracle3/oracle3.py`, written from §C.5a) |
| Integrity / FK | clean-copy integrity and FK | **CLOSED.** 41/41 ok, FK dangling 0 | rehearsal-3 §6 |
| Registry | 35 rows, shape, targets, sources, orphans | **CLOSED.** 35 rows; 0 shape violations; 34/34 targets registered; 0 sources registered; 0 registered-missing; 0 unregistered on disk | rehearsal-3b §3 (`g/registry-counts.txt`) |
| Cosine smoke | blob equality plus non-collinear query smoke | **CLOSED as smoke.** 5899 rows: 5827 equal source union, 72 pre-existing equal own values, 0 unexplained. 3 queries pairwise 0.58–0.72, top-5 identical, top-1 distinct. Not a recall measurement | rehearsal-3b §4 |
| Frozen snapshot | byte-identical before and after | **CLOSED.** Manifests `e0fe0276…` before and after; `data` 555; `recovery/` 1 entry | rehearsal-3 pre, rehearsal-3b post |
| F7 | blueprint step-10 text stale | non-blocking, documentation, carried | rehearsal-1 §1 |
| F8 | `-shm` volatile | non-blocking, carried | rehearsal-1 §4 |
| F9 | determinism / nonce | non-blocking, carried. Its no-op wording is the F1' documentation defect | rehearsal-1 §4 |
| F10 | apply and staged-verify accept a baseline-less custody dir | **non-blocking residual**, carried to Step 11, not fixed | rehearsal-3 §1, owner ruling |
| Doc defect | design line 333 and blueprint `test_verified_repeat_apply_is_noop` row describe apply repeat as no-op | **non-blocking documentation defect**, carried like C30–C32, correct at next revision | ruling above |

## 2. Settlement check against the brief's PASSED condition
- F1–F6 each have a clean-copy measurement closing them: yes.
- Independent oracle validates the reparent rule from the spec: yes (§C.5a, `oracle3.py`).
- Frozen snapshot byte-identical before and after: yes, per the rehearsal-3 and rehearsal-3b checks.
- Only open items are named non-blocking residuals (F10, F7–F9, the documentation defect): yes.

## Settlement

SETTLEMENT: PASSED — F1–F6 closed on the clean copy; F1' is a documentation supersession under the owner's option-(a) ruling, not a defect; the oracle validates the reparent rule from §C.5a; the frozen snapshot is unchanged. Open items are non-blocking: F10 (carry to Step 11), F7–F9, and the design line 333 / blueprint test-row documentation defect (correct at next revision).

Steps 11–13 are not run. No commit, push, or cutover.
