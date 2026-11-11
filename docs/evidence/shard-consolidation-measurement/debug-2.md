# Debug 2 — review-5 R1/R2/R3 diagnosis (S09 admission oracle + S10 settlement)

BLUEPRINT: `.tmp/blueprints/blueprint-20261111-0310-shard-measurement.md`
BLUEPRINT-ACK: sha256=d90461a51f51 (SYS-APPROVED 2026-11-11T03:13Z; body re-read this run)
LEDGER: `.tmp/progress/20261111-0252-shard-measurement.ledger.md` (S09/S10 rows still `[x]` DONE; review-5
requires same-row REWORK — not done by this report)
R = `/home/opencode/Code/Personal/Rust/opencode-agents-memory`
S = `R/.tmp/scratch/shard-measurement`
Role: diagnosis only. No source, manifest, lock, tracked, config, vault or live-store change. Nothing in `S`
was mutated by this run; the two reads that recompute bytes (`sha256sum`) and four `stat` calls are the whole
footprint.

Read for this report: `review-5.md`, `handoff-5.md`, `handoff-4.md`, `review-4.md`, `S/tests/measurement.rs`
(1100-1688), `S/evidence/{results.json,admission.json,s09-pristine.sha256,s08-pristine.sha256,commands.log,
review-5-m3.log,s09-final-gates.log,update-results-rework.py}`, `R/crates/mem-store/src/shard_manager.rs`
(240-300), `.tmp/drafts/20261111-shard-measurement-report.md` (36-73), `ledger.md`.

---

## [REPRO]

Environment: offline, `--offline --locked`, canonical scratch manifest, no network, no new dependency.
All three defects are reproducible with read-only commands except R1's mutant, which needs a scratch copy.

**R1 — the `blocked` row survives a degenerate poll (submitter's M3).**

```sh
cd $R
cp -a .tmp/scratch/shard-measurement .tmp/scratch/debug2-m3     # scratch copy, own target dir
sed -i '1508s/from_secs(1)/from_secs(0)/' .tmp/scratch/debug2-m3/tests/measurement.rs
env -u CARGO_TARGET_DIR -u CARGO_BUILD_TARGET_DIR -u CARGO_BUILD_BUILD_DIR \
  CARGO_TARGET_DIR=$R/.tmp/scratch/debug2-m3/target \
  cargo test --offline --locked --manifest-path .tmp/scratch/debug2-m3/Cargo.toml \
  --test measurement test_existing_barriers_report_observed_coverage -- --exact --nocapture
# expected defect signal: exit 0, 1 passed  <- the mutant lives
```

Already measured by review-5, re-read here, not re-run (budget discipline): `S/evidence/review-5-m3.log:279-281`
`test ... ok`, `1 passed`, **0.03 s**. Pristine: `s09-green.log` 1.04 s, review-5 re-run 1.05 s. The 1.0 s gap
*is* the negative poll; the mutant removes exactly that and nothing else — and every truthfulness gate still
passes.

**R2 — settlement artifacts contradict each other, same file.**

```sh
python3 - <<'P'
import json; d=json.load(open('.tmp/scratch/shard-measurement/evidence/results.json'))
print(d['online_admission_executed'], '|', d['restore_executed'], '|', d['s09']['green_exit'], '|', d['unknowns'])
P
# False | True | 0 | ['ai-sessions schema', 'production auxiliary completeness',
#                     'admission/drain/stale handles', 'whole-root interrupted restore']
```

**R3 — false OK claim, measurable today.**

```sh
cd $R/.tmp/scratch/shard-measurement
sha256sum -c evidence/s09-pristine.sha256; echo "EXITCODE:$?"
# tests/measurement.rs: FAILED
# src/lib.rs: FAILED
# sha256sum: WARNING: 2 computed checksums did NOT match
# EXITCODE:1
sha256sum src/lib.rs src/main.rs tests/measurement.rs
# e755acc5… src/lib.rs / 75f3d936… src/main.rs / 37a5bca6… tests/measurement.rs
```

## [MINIMAL CASE]

- **R1 minimal case** = one `tokio::time::timeout(window, &mut contender_rx).is_err()` with no assertion about
  the window or the channel's state. Removed elements, each verified non-load-bearing for the survivor: the
  distinct-scope branch, the aux write, the drain, the persisted artifact — M3 stays green with any of them
  gone, because the only witness for the `blocked` row is `bool`.
- **R2 minimal case** = two hand-maintained top-level fields (`online_admission_executed`, `unknowns[]`) that
  no command derives and no gate reads. `restore_executed` was flipped by hand for S08; its S09 twin never was.
- **R3 minimal case** = one `sha256sum -c` executed at 06:05:49Z, quoted at 06:12Z as if it applied to the
  06:06:29Z tree. Nothing else about the evidence is wrong.

---

## [FINDINGS]

### R1 — vacuous oracle on `scope-lock/same-scope-while-held`

Hypotheses, ranked; measurement that settled each.

**H1 (CONFIRMED) — the row's only witness is "a timeout returned Err", which a zero-length window satisfies.**
`tests/measurement.rs:1507-1514` computes `contender_blocked = timeout(1s, &mut contender_rx).is_err()` and
asserts only that it is `true`. `admission.json.phases.timings_us` records the row as
`contender_blocked_poll_1s: true` — a bool, no elapsed. `results.json.s09.key_observations.scope_barrier` is
prose. So the artifact cannot distinguish "waited 1 s on a live, queued contender" from "returned instantly".
M3 (window → 0) is the falsifying input: `review-5-m3.log:279-281` GREEN in 0.03 s. Absence of a positive
control (no uncontended same-key completion anywhere in the S09 test) means the `blocked` status is never
contrasted with a completed run of the same key.

**H2 (FALSIFIED) — the lock itself is broken, so no assertion could pin it.** Falsified by M1
(`review-5-m1-latch.log`, RED 101 at `:1511` when the holder stops awaiting `release`) and by
`scope-lock/drain-after-release` completing. The mutex at `shard_manager.rs:283-288` does hold its guard across
`op().await`. The defect is observational only.

**H3 (FALSIFIED) — a persisted-lock-state or held-token discriminator exists at this seam.** Read:
`scope_write_locks: Arc<Mutex<HashMap<String, Arc<AsyncMutex<()>>>>>` (`shard_manager.rs:143`), minted per
`format!("{}:{}", scope, hash)` at `:265-283`. It is process memory; `with_scope_write_lock` returns
`Result<T, StoreError>`, never a guard/token; nothing is written to disk. The persisted
`.turso-operation.lock` (`operation_lock.rs`) belongs to the other seam and cannot witness scope admission.
So the review's suggested "persisted lock state" option is **not available here** — that is a finding, not a
missed shortcut.

**H4 (FALSIFIED) — a positive control alone kills M3.** Tokio's `Timeout` polls the inner future *before* the
delay, so an uncontended same-key op is Ready on its first poll and completes even inside a zero window.
A positive control is required for the *meaning* of the row (the same key does admit completions in this
harness) but cannot be the M3 killer.

**Answer to "which observable distinguishes them":** the **elapsed duration of the bounded wait on the live
contender channel**, lower-bounded (a 1 s monotone timer cannot fire early, so a lower bound is load-safe
where an upper bound would be flaky), corroborated by the channel's own state — `try_recv() == Empty` means
"still queued, sender alive", `Closed` means the contender died without signalling. There is no persisted
lock state and no held-token API at this seam to use instead.

Smallest killing assertion:

```rust
assert!(poll_elapsed >= S09_SCOPE_POLL / 2, "negative poll returned after {poll_elapsed:?}");
```

One line. M3 yields ~0 s; pristine yields >= 1 s; the 500 ms margin cannot be eaten by scheduler load.
(`tokio::sync::oneshot::Receiver::try_recv` is present in the locked tokio 1.53.3 — registry source
`src/sync/oneshot.rs:1171`, `TryRecvError` at `:352` — and needs no feature beyond the manifest's `sync`.)

### R2 — settlement contradiction

**H1 (CONFIRMED) — stale pre-execution text, mechanically carried forward.** `evidence/update-results-rework.py:49-54`
writes the four `unknowns` entries verbatim during the S05/S07 rework cycle, when S08/S09 were still blocked.
S10's reconciliation (handoff-5:42, commands.log:45) replaced `limits[0]/[1]` — review-4 advisory #1 — and
never re-derived `unknowns`. `grep -rl online_admission_executed` finds the string **only** in `results.json`:
no script, no test and no log writes it, so the field is hand-authored from the S08 era and was never revisited.

**H2 (FALSIFIED) — the fields are a deliberate production-scope claim.** If `online_admission_executed: false`
meant "the production online pipeline was not exercised", the field would need that name and the report would
say so; instead `report:47-59` says S09 executed, the same file's `s09.green_exit` is `0`, and `limits[1]`
("S09 executed: … 12 completed / 2 refused / 1 blocked") directly contradicts `unknowns[2]`. Two fields of one
file assert incompatible things; that is staleness, not scope.

Which of the `unknowns` entries survive execution (`admission.json.phases.paths`, 15 rows):
- `admission/drain/stale handles` — **false at the executed scope**: drain is `scope-lock/drain-after-release`
  completed, stale handle is `operation-lock/stale-dead-pid-reclaim` + `…-acquire-after-reclaim` completed.
  Surviving truth: the 5 `not_exercised` paths + connection drain of cloned `Arc` handles.
- `whole-root interrupted restore` — **false as written**: S08 restored a whole *closed synthetic* root twice,
  byte-idempotent. Surviving truth: power-loss / production-filesystem atomicity, which `limits` and
  `results.s08.limitations` already state.
- `ai-sessions schema`, `production auxiliary completeness` — true, keep.

**What must read what:** no human. The S10 gate ("report and results JSON agree") must be a script (see
prescription), because three consecutive reviews have now found the same drift class surviving prose review.

### R3 — false pristine-hash claim

**H1 (CONFIRMED) — ordering: the snapshot was taken before the last source edits.** MEASURED, mtimes
(+0700 → UTC):

| file | mtime | UTC |
|---|---|---|
| `evidence/s09-pristine.sha256` | 13:05:49 | 06:05:49Z |
| `src/lib.rs` | 13:06:10 | 06:06:10Z |
| `tests/measurement.rs` | 13:06:29 | 06:06:29Z |
| `evidence/s09-final-gates.log` | 13:06:51 | 06:06:51Z |

`commands.log:43` (mutant revert + `sha256sum -c` → OK) is the pre-`:44` state; `commands.log:44` is the same
gate burst that then ran `rustfmt --edition 2024 src/lib.rs src/main.rs tests/measurement.rs` in **write mode**
and the three `bool_assert_comparison` fixes. READ: the fixes are test-only; the `lib.rs` byte change is
therefore the rustfmt write pass (INFERRED — no pre-format copy survives: no `lib.rs` under `.tmp/scratch/`
hashes to `fe603303…`, so the old bytes are gone). The `-c` verdict was true at 06:05:49Z and was quoted as a
06:12Z final-state claim in `handoff-5:20`, `report:49`, `ledger.md:28`, `ledger.md:15` ("pristine sha256 OK").

**H2 (FALSIFIED) — real tree drift after the review.** Current live hashes equal `results.json:173-177`
`final_tree_sha256` exactly (`e755acc5…`, `75f3d936…`, `37a5bca6…`), and review-5 independently recomputed
them (`review-5.md:19`). The tree is the reviewed tree. Only the manifest is stale.

**H3 (FALSIFIED) — the manifest was written after the test fix but lib.rs drifted separately.** Refuted by the
mtime table: the manifest precedes *both* files.

**Secondary defect in the same artifact:** `s09-pristine.sha256` lists 2 files; `final_tree_sha256` lists 3
(`src/main.rs` is missing from the manifest). Even freshly regenerated, a 2-file manifest cannot pin the tree
the settlement file claims.

**Why the review-4 fix did not hold.** Review-4 advisory #2 (`review-4.md:43`) said `s08-pristine.sha256` was a
pre-rustfmt snapshot; the response was to *label* it (`results.json.s08.pristine_snapshot_superseded`,
handoff-4:23 honest) and to mint a **new** manifest inside the S09 loop at the mutant-revert moment — i.e. the
same ordering mistake one artifact later. Root cause of the class: a hash snapshot was used as a **step-level**
assertion (did the mutant revert cleanly?) while its verdict was consumed as a **settlement-level** claim (is
the final tree pinned?), and no machine gate re-read the tree at settlement. Labelling fixes the instance, not
the class; the class is closed only by a settlement-time check (R2/R3 prescription item 3).

---

## [ROOT CAUSE]

- **R1 — CONFIRMED.** The `blocked` row is observed as `bool` ("timeout elapsed") instead of as a measured
  wait over a live channel. `tests/measurement.rs:1507-1514`, persisted at `:1537` /
  `admission.json.phases.timings_us`. No positive control, no elapsed, no channel-state check ⇒ M3 survives.
- **R2 — CONFIRMED.** Two hand-maintained top-level fields (`results.json:13`, `:37-42`) are not derived from
  the blocks that contradict them; the S10 "report and results JSON agree" gate was performed by prose.
- **R3 — CONFIRMED.** `evidence/s09-pristine.sha256` was captured at 06:05:49Z, before the 06:06:10–06:06:29Z
  rustfmt/clippy writes, then quoted as a final claim. Not tree drift. Recurrence cause: no settlement-time
  hash gate; the review-4 fix relabelled one artifact instead of gating the class.

---

## [FIX PRESCRIPTION]

Order matters: **source edits first, hash snapshot last-but-one, settlement checker last.** Every gate log
records a literal `EXITCODE:<n>` (R4).

### 1. R1 — pin the `blocked` row (`S/tests/measurement.rs`, S09 test only)

1a. Add one constant above `s09_signal_op` (`:1124`):

```rust
/// Bounded scope-barrier window. The negative poll and the positive control share
/// it: a degenerate window must be visible to both, not only to the negative one.
const S09_SCOPE_POLL: std::time::Duration = std::time::Duration::from_secs(1);
```

1b. Replace `:1507-1514` (the bare `contender_blocked` computation + assert) with:

```rust
        let poll_started = std::time::Instant::now();
        let poll = tokio::time::timeout(S09_SCOPE_POLL, &mut contender_rx).await;
        let poll_elapsed = poll_started.elapsed();
        assert!(
            poll.is_err(),
            "same-scope contender must not complete while the holder is latched"
        );
        // Structural: the contender is still queued with a live sender. `Closed`
        // would mean the spawned op died without signalling; `Ok` would mean it ran.
        assert!(
            matches!(
                contender_rx.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ),
            "same-scope contender must still be pending with a live sender while the holder is latched"
        );
        // The wait itself: a 1s monotone timer cannot fire early, so a LOWER bound is
        // load-safe (an upper bound would be flaky). This is what kills the degenerate poll.
        assert!(
            poll_elapsed >= S09_SCOPE_POLL / 2,
            "negative poll returned after {poll_elapsed:?}; a degenerate window proves nothing"
        );
        let contender_blocked = true; // kept for the json! field below, now proven above
```

1c. Positive control (blueprint `blocked` vs `completed` contrast on the *same* key), inserted after the drain
assert at `:1584-1587`, still inside the async block where `manager` is alive:

```rust
        // Positive control: the same key `user:scope-a` must admit a completion in this
        // harness, so `blocked` is a measured contrast and not an inert path.
        let control_started = std::time::Instant::now();
        let (control_tx, control_rx) = tokio::sync::oneshot::channel::<&'static str>();
        let control_manager = manager.clone();
        let control = tokio::time::timeout(
            S09_SCOPE_POLL,
            control_manager.with_scope_write_lock(Scope::User, "scope-a", move || {
                s09_signal_op(control_tx)
            }),
        )
        .await;
        let control_us = control_started.elapsed().as_micros();
        assert!(
            control.is_ok() && control.unwrap().is_ok(),
            "uncontended same-scope control must complete inside the poll window"
        );
        assert!(
            tokio::time::timeout(S09_SCOPE_POLL, control_rx).await.is_ok(),
            "uncontended same-scope control must deliver its completion signal"
        );
```

1d. Replace the bool with measured microseconds in the `json!` at `:1534-1541`:
`"contender_blocked_poll_1s": contender_blocked` → `"scope_same_scope_negative_poll_us": poll_elapsed.as_micros() as u64`
and add `"scope_positive_control_us": control_us as u64`.

1e. One persisted-artifact gate beside the others (`:1627+`), so the evidence file itself cannot carry a
vacuous row:

```rust
    let poll_us = persisted["timings_us"]["scope_same_scope_negative_poll_us"]
        .as_u64()
        .unwrap();
    assert!(
        poll_us >= 500_000,
        "the persisted negative poll must show the wait on the live contender, got {poll_us}us"
    );
```

Only positive control for M3-killing: 1b. 1c is the reviewer's requested control; 1d/1e make the artifact
falsifiable. Nothing above weakens an existing assertion; M1 and M2 must still go RED after this change
(re-run both spot-checks, not just M3).

**Regression test first (RED on the buggy tree):** apply M3 to a scratch copy of the *current* tree
(`from_secs(1)`→`from_secs(0)` at `:1508`), confirm it is still GREEN, then apply 1a–1e and confirm the same
mutant is RED at the `poll_elapsed` assert while the pristine tree is GREEN. Evidence:
`evidence/s09-m3-after-fix.log` + `evidence/s09-green-after-fix.log`, `EXITCODE:` captured.

### 2. R2 — make settlement derived, not hand-written

2a. `S/evidence/results.json`: **delete** `restore_executed` (`:12`) and `online_admission_executed` (`:13`).
They duplicate `s08.green_exit` / `s09.green_exit` and are exactly the fields that drifted twice. If a
top-level summary is wanted, the checker prints it; it must not be hand-maintained.

2b. `S/evidence/results.json:37-42` `unknowns` — keep only facts that remain unmeasured, each prefixed with
its scope so the pre-execution drift class cannot recur silently:

```json
"unknowns": [
  "production: ai-sessions schema (never opened by this wave)",
  "production: auxiliary completeness beyond the synthetic fixtures",
  "production: power-loss / filesystem atomicity for interrupted restore (process-kill harness only)",
  "not_exercised: skills-manager/list-get-create",
  "not_exercised: mem-episodic/inbox-store-direct-sql",
  "not_exercised: mem-bridge/capture-promotion-real-callers",
  "not_exercised: connection/close-drain-stale-cloned-handle",
  "not_exercised: power-loss-durability"
]
```

The `not_exercised:*` entries must be **generated from** `admission.json.phases.not_exercised[].path`, not
retyped.

2c. New `S/evidence/check-settlement.py` (python3, no new dependency; same placement style as
`ledger-check.py`), run from `R` at the end of S10, writing `evidence/s10-settlement.log` with a literal
`EXITCODE:<n>`. It asserts, and exits nonzero on any failure:

1. `results.s08.green_exit == 0 and results.s09.green_exit == 0`; no top-level key matching
   `*_executed` exists at all (2a);
2. every `unknowns[]` entry starts with `production: ` or `not_exercised: `;
3. `sorted(unknowns[not_exercised:])` == `sorted(admission.not_exercised[].path)`, and
   `results.s09.measured_outcomes` == `admission.status_counts` (with `not_exercised` = entry count);
4. `results.final_tree_sha256` == live `sha256sum` of the three files, **recomputed**;
5. `sha256sum -c evidence/s09-pristine.sha256` exit 0 **and** the manifest's mapping == `final_tree_sha256`;
6. every claim `limits[i]` starting with `S0x executed` has a matching block with `green_exit == 0`;
7. prints `settlement: PASS`.

`handoff-6` / `report` cite that log and its exit code — never restate a check result from memory.

### 3. R3 — evidence construction (do this **after** 1 and 2 are written and formatted)

3a. Sequence, in one gate burst: source edits → `rustfmt --edition 2024 <3 files>` → `--check` → clippy →
named test → full serial suite → **then**:

```sh
cd $R/.tmp/scratch/shard-measurement
sha256sum src/lib.rs src/main.rs tests/measurement.rs > evidence/s09-pristine.sha256
sha256sum -c evidence/s09-pristine.sha256; echo "EXITCODE:$?"
```

captured into `evidence/s09-pristine-check.log`. Three files, matching `final_tree_sha256` keys (fixes the
missing `src/main.rs`).

3b. Record the supersession rather than erasing history, same shape as `s08.pristine_snapshot_superseded`:

```json
"s09": { "pristine_snapshot_superseded":
  "evidence/s09-pristine.sha256 first captured 2026-11-11T06:05:49Z, before the final rustfmt write pass (src/lib.rs 06:06:10Z) and the three bool_assert_comparison fixes (tests/measurement.rs 06:06:29Z); superseded values tests a745c97da91f5994217a695a27c9cbc75977c67e73be9f153f4ddc1362716e4a / lib fe6033033b4eda55c5a4b76fe96bcc8224b58c18e317ea4a3752f78f2a5cf227; regenerated after the final edit, three files, verified by check-settlement.py" }
```

3c. Correct the prose claims — do not rewrite retained history:
- `handoff-5.md:20`: supersede in handoff-6 (handoff-5 is retained verbatim): state the pre-format manifest,
  the 06:06:10/06:06:29Z edits, the regenerated three-file manifest and `s09-pristine-check.log` `EXITCODE:0`.
- `report:49`: replace the "`evidence/s09-pristine.sha256` OK (supersedes …)" clause with the post-format
  manifest + checker reference and the superseded values.
- `ledger.md:28` ("three advisories … all addressed"): **append** a correcting `WAVE HISTORY:` record for the
  review-5 reopening — "pristine-snapshot advisory re-opened as review-5 R3; closed at S10 settlement with a
  post-format three-file manifest and check-settlement.py" — and leave line 28 intact as the historical claim,
  per the ledger's preserve-history rule.

### 4. Cheap cleanups review-5 also requires

- R4: re-capture the S09/S10 gate logs with literal `EXITCODE:<n>` (`s09-final-gates.log:19,22` currently carry
  `SUITECODE:$?` / `CLIPPYCODE:$?`); note it in commands.log the way `s09-mutant-unknown-blocked.log:22` does.
- R5: give the S09 ledger row the explicit authorization/release note the S08 row has (`ledger.md:14` vs `:15`),
  and de-duplicate the repeated `prior dependency blocker -` block in `ledger.md:16`.

### 5. Constraints that must survive the rework

Scratch-only; `Cargo.toml`/`Cargo.lock`/tracked/live/config/vault untouched; no network; no new dependency;
`runs/`, mutant copies and prior evidence retained; `ruling: INCONCLUSIVE`, policy-only status of
recommendation A, `phase_c: blocked` unchanged; S08 not re-litigated.

### Memory relay (blueprint §Security forbids memory adds from this provisional wave — for orchestrator recording)

1. *bug-pattern* — symptom: a `blocked`/timeout observation survives a zero-length-window mutant; cause: the
   row is persisted as a bool ("timeout elapsed") with no elapsed and no channel-state check; fix: lower-bound
   the elapsed wait (`>= window/2`, monotone-safe) + `oneshot::try_recv() == Empty`; a positive control alone
   cannot kill it because tokio's `Timeout` polls the inner future before the delay.
2. *bug-pattern* — symptom: a checksum manifest that reported OK now fails; cause: snapshot captured
   mid-sequence inside a loop (mutant revert) and its verdict consumed as a settlement claim after the final
   format pass; fix: regenerate **after** the last source edit, list every file the settlement file claims,
   and gate settlement with a checker that recomputes hashes rather than quoting a past `-c`.

---

## [STATUS]

DIAGNOSED-NEEDS-CODER
