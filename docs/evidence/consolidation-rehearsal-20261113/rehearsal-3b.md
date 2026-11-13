# Rehearsal-3b — lossless shard consolidation, STEP 10 gaps closed on the clean copy, on `eacb66d`

Scratch root: `.tmp/scratch/rehearsal4-20261113T113418Z/` (same attempt as rehearsal-3). Clean-copy runs: `g/` (apply → install → repeat-apply → settle), `rb/` (kill → recover → rollback), `run/` and `f5/` from rehearsal-3.

## Provenance
- HEAD: `eacb66dd25edeac7d1d41b3b366756a3d380b006` (MEASURED, `3b-head.txt`).
- Binary sha256 `9fa3dfdf1db05a2888c1535f5e7eeb80c7ff48f369cffac8542fc92d4decdb7d`, unchanged from rehearsal-3.
- Frozen snapshot, before this pass and after (MEASURED, `3b-frozen-after.txt`): `manifest.final.sha256`, `manifest.baseline.sha256`, `manifest.sha256` all `e0fe0276fe5cf769a649f742bbc18ba26e3383a4575a8e3edac9fab6f53d258e`; `data` mode 555; `recovery/` 1 entry.

## Correction recorded
Brief rule 2 conflated `apply` with `install`; the apply-without-baseline behaviour is the non-blocking residual **F10**, not a blocker. Not fixed here.

## Deviation from the brief, stated
Gap 1 asked for rollback on `run/`. `run/` is already settled at `Verified`, and rollback is refused there by design (R1-i, test `test_verified_repeat_apply_is_noop` line 40–45: "a verified install refuses a rollback"). Rollback was therefore measured on a fresh copy `rb/`, taken through apply → kill-after-L4 → recover → rollback. The rollback path itself is identical.

## 1. Rollback (`rb/`) — restored vs baseline, the comparison, not only exit 0
| check | command / source | result | label |
|---|---|---|---|
| kill after L4 | `OPENCODE_MEM_CONSOLIDATE_KILL_AT=after-l4 consolidate-install` | exit **134** | MEASURED (`rb/kill.exit`) |
| recover | `consolidate-recover` | exit **0** | MEASURED (`rb/recover.exit`) |
| rollback | `consolidate-rollback` | exit **0** | MEASURED (`rb/rollback.exit`) |
| journal | `custody/journal.json` phases | `StagedVerified → Installing → InstalledUnverified → RollingBack → RolledBack` | MEASURED (`rb/journal-phases.txt`) |
| file count | restored, baseline, pre-apply | 209 / 209 / 209 | MEASURED (`rb/digest-counts.txt`) |
| tree digest | `sha256sum` over the sorted per-file list, restored vs custody baseline vs pre-apply | all `0ceba1b609e74d584b045fc56fe13650215da468eaf9e755b468130e0f25ce78` | MEASURED (`rb/digest-compare.txt`) |
| file diff | `diff custody-baseline.full.sha256 post-rollback.full.sha256` | **0 lines** | MEASURED (`rb/digest-compare.txt`, `DIFF_LINES=0`) |
| modes/kind vs clone-time mode table | restored `(mode, kind, path)` vs `custody/mode-table` (222 entries, the design's pinned oracle, NV-1(d)) | **0 diff lines** | MEASURED (`rb/table-result.txt`, `TABLE_DIFF_LINES=0`) |

Note on modes (MEASURED, honest): a first check against the custody `data` tree showed 437 diff lines (for example `400` in the snapshot becomes `600` or `644` after restore). That was my wrong oracle. The design (line 141, line 199, line 226) pins the restore target as the clone-time **mode table**, not the custody `data` modes, and the restored tree matches that table exactly. The `400` values in custody `data` are the snapshot's own read-only state, not the restore target.

Verdict: **CLOSED.**

## 2. Repeat-apply and the 77 refusal — one run, `g/`
| step | command | exit | event / stderr | S and custody | label |
|---|---|---|---|---|---|
| install | `consolidate-install` (background, ~3 min; a first foreground attempt was killed by the tool at 120 s and left the journal at `StagedVerified`, S and staged intact) | **0** | — | S present, staged moved | MEASURED (`g/install-result.txt`) |
| repeat-apply at `InstalledUnverified` | `consolidate-shards --plan plan-abs.json --custody` | **77** | `consolidation.window_not_applyable` "the journal already records an installed window; the audit record is never rewritten" | S digest unchanged; sidecar unchanged | MEASURED (`g/repeat-installed.exit`, `g/S_UNCHANGED`, `g/SIDECAR_UNCHANGED`) |
| settle | `consolidate-verify` | **0** | — | journal → `Verified` | MEASURED (`g/settle.exit`, `g/journal-settled.txt`) |
| repeat-**apply** at `Verified` | `consolidate-shards --plan plan-abs.json --custody` | **77** | `consolidation.window_not_applyable` | S digest unchanged; custody byte + mtime unchanged | MEASURED (`g/repeat-verified.exit`, `g/S_UNCHANGED=yes`, `g/CUSTODY_UNCHANGED=yes`) |
| repeat-**settlement** at `Verified` | `consolidate-verify` | **0** | — | journal mtime unchanged; custody bytes unchanged | MEASURED (`g/repeat-verify.exit`, `JOURNAL_MTIME_UNCHANGED=yes`, `CUSTODY_BYTES_UNCHANGED=yes`) |

Verdict on the 77 at `InstalledUnverified`: **CLOSED** (measured this run, matches rehearsal-1 F1 target).

**Open item — repeat-apply at `Verified`: NOT CLOSED, design divergence.** The brief and design line 333 say repeat apply on `Verified` with a matching plan hash is a no-op (no new backup, no journal rewrite), and rehearsal-1 F9 says "the no-op only exists at Verified". Measured: the repeat **apply** returns 77 `window_not_applyable` with no state change. The repeat **settlement** is the true no-op (exit 0, no rewrite), and the test `test_verified_repeat_apply_is_noop` exercises settlement (`consolidate-verify`), not apply. So the named test does not cover the apply repeat the design sentence describes. Two readings, and this rehearsal cannot choose between them:
- (a) the design sentence is wrong; the 77 at `Verified` is intended (consistent with "audit record never rewritten" and the R1-i terminal stance), and line 333 should be amended to say settlement is the no-op;
- (b) the code is wrong; apply at `Verified` with a matching plan hash should be a no-op.
Owner ruling needed. Not a pass.

## 3. Registry (`g/data/metadata.db`, the installed tree) — re-derived
Table `shards`; schema `id, scope, scope_hash, shard_index, db_path, vector_count, is_active, created_at` with `UNIQUE(scope, scope_hash, shard_index)` (MEASURED, `g/registry-schema.txt`).

| count | value | label |
|---|---|---|
| rows | **35** | MEASURED (`g/registry-counts.txt`) |
| shape violations (`scope_hash` 16 hex, `shard_index` ≥ 0 int, `created_at` int, `is_active` ∈ {0,1}, `db_path` ends `.db`) | **0** | MEASURED |
| plan targets registered | **34 / 34** | MEASURED |
| plan sources still registered | **0** | MEASURED |
| registered but missing on disk | **0** | MEASURED |
| on disk but not registered | **0** | MEASURED |

Verdict: **CLOSED.** Shape rules are the ones I chose from the schema; they are not a copy of the design's registry predicate, which I did not locate in this pass. Named as my own derivation.

## 4. Cosine smoke — what was sampled, the criterion
Source: the pristine frozen copy (not the installed tree), so expectations are not derived from the install. Installed tree `g/data`.
- **Blob equality, all rows**: 5899 installed target rows compared against the source-derived expectation: **5827 equal** to the source union; **72 unequal, all 72 explained** as the pre-existing target rows, each byte-equal to its own pre-existing value. **Unexplained: 0** (MEASURED, `g/cosine.txt`, `g/cosine-unequal.txt`).
- **Non-collinear queries**: 3 query vectors sampled from the source union with a fixed seed (7). Pairwise cosines **0.5817, 0.7217, 0.6039**. Criterion: pairwise < 0.9, so the queries are not near-collinear (`NONCOLLINEAR True`).
- **Top-5 ranking**: for each query, the top-5 over the source union equals the top-5 over the installed rows (`TOP5_IDENTICAL_PER_QUERY [True, True, True]`). Top-1 distinct across the three (MEASURED).
- Criterion as stated: blob equality is primary; the cosine rankings only supplement it, as the blueprint line 151 specifies.

Verdict: **CLOSED** on this sample. Sample size is 3 queries, which is the same as rehearsal-1. It is a smoke test, not a recall measurement.

## 5. Carried over from rehearsal-3 (not re-run here, stated as such)
- F5 both halves (alarm fires at 77 `post-install source edit`, not silenced): rehearsal-3 §3, MEASURED there.
- F3 `2991cca9…` carried-legacy absence: rehearsal-3 §4, MEASURED there.
- Oracle §C.5a 34/34, `agent_sessions` 888→887, reparent 300 with sidecar agreement: rehearsal-3 §5, MEASURED there.
- Clean-copy integrity 41/41 ok, FK dangling 0: rehearsal-3 §6, MEASURED there.

## 6. Item table — one disposition each

| ID | item | disposition | basis |
|---|---|---|---|
| F1 | repeat-apply exit 72 instead of 77 at `InstalledUnverified` | **CLOSED** | 77 `window_not_applyable` (§2) |
| F1' | repeat-apply at `Verified` no-op (F9 / design line 333) | **NOT CLOSED — design divergence, owner ruling** | 77 measured, no-op claimed (§2) |
| F2 | 300 `agent_sessions` FK orphans | **CLOSED** | reparent 300, oracle exact, dangling 0 (rehearsal-3 §5–§6; `g` integrity) |
| F3 | `shard_stats` wrong in 34/35 | **CLOSED** for the 34 targets (34/34 match); carried `2991cca9…` absence **CLOSED as carried-legacy** (source has no table) | rehearsal-3 §4 |
| F4 | `--apply` writes `S/.wal-mode` | **CLOSED** (rehearsal-2, `.wal-mode` sha identical pre/post apply) | rehearsal-2 §A |
| F5 | settlement refuses on marker | **CLOSED** — alarm fires (77) and is not silenced (clean settle 0; `M1_EXCLUDED` two-element, plain `manifest_of`) | rehearsal-3 §3; caveat: marker is a byte copy of the engine's, not a live open |
| F6 | install without custody baseline | **CLOSED on install** — refused 77 `baseline unreadable`, S and staged intact | rehearsal-3 §1 |
| F7 | blueprint step-10 text stale | non-blocking, documentation, carried | rehearsal-1 §1 |
| F8 | `-shm` volatile | non-blocking, carried | rehearsal-1 §4 |
| F9 | determinism / nonce | non-blocking, carried; its no-op claim is the F1' divergence | rehearsal-1 §4 |
| F10 | apply and staged-verify accept a baseline-less custody dir | **non-blocking residual** (per owner ruling; carried to Step 11) | rehearsal-3 §1 |
| Rollback | whole-root restore, digest and mode-table equal | **CLOSED** | §1 |
| Registry | 35 / 0 / 34 / 0 / 0 / 0 | **CLOSED** | §3 |
| Cosine | 5827 + 72 explained, 3 non-collinear, top-5 identical | **CLOSED** (smoke) | §4 |

## 7. Not measured, and what would close it
- **F1'** needs the owner to rule whether the design sentence or the code is correct. Then either the design text is amended, or the code path is changed and re-measured on a fresh copy.
- Cosine is a 3-query smoke. A recall measurement over many queries is not claimed.
- The F5 marker is a byte copy of the engine's `.wal-mode`. A live writable open producing the marker was not measured; no scoped entry point was located.

## Frozen snapshot
Before and after this pass: `e0fe0276fe5cf769a649f742bbc18ba26e3383a4575a8e3edac9fab6f53d258e` for all three manifests. `data` mode 555. `recovery/` 1 entry. No invocation referenced the live store (`grep` over outputs, 0 files, carried from rehearsal-3).

## Steps 11–13
Untouched. No commit, push, cutover, or live mutation. Live `S` never opened.

## Settlement

Every F1–F6 item is closed on the clean copy except F1', and F1' is a disagreement between the design text and the measured code, not a closed item. The brief's PASSED condition requires that every item is measured closed and that the only open items are named non-blocking residuals. F1' is neither closed nor a residual. It needs a ruling.

SETTLEMENT: BLOCKED — F1' open: repeat-apply at `Verified` returns 77 `window_not_applyable` (measured, `g/repeat-verified.exit`), while design line 333 and rehearsal-1 F9 state a no-op with test `test_verified_repeat_apply_is_noop`, which exercises settlement, not apply. The owner rules whether the design text or the code is authoritative. All other items are measured closed or are named non-blocking (F7, F8, F9, F10).
