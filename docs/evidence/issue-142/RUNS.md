# Issue 142 — run record (retire `legacyPeerLive`)

Blueprint: `.tmp/blueprints/blueprint-20261118-0740-issue-142-retire-legacy-peer-live-r4.md` (sha256=45c7c40bf4a3).
Every cell is verbatim output with its exit code, or `NOT RUN — <reason>`.
Scratch target: `CARGO_TARGET_DIR=.tmp/scratch/target-issue-142`. Tree base: `07bd9b2` (issue 143 committed).

## Step 1 — RED (guard test, pre-change tree)

`cargo test -p mem-web --test sweep_state_vocabulary` exits 101; cargo does not echo harness output on failure,
so the harness was run from the built binary.

```
running 3 tests
test test_the_dto_declares_no_legacy_peer_live ... FAILED
test test_the_retired_label_is_recorded_as_a_historical_note ... FAILED
test test_the_dto_and_the_adr_state_the_same_four_labels ... FAILED
thread 'test_the_dto_declares_no_legacy_peer_live' panicked at crates/mem-web/tests/sweep_state_vocabulary.rs:56:5:
the retired label must not reappear in dto.rs; history lives in ADR-0019 (issue 142)
thread 'test_the_retired_label_is_recorded_as_a_historical_note' panicked at crates/mem-web/tests/sweep_state_vocabulary.rs:89:49:
ADR-0019 must record the retirement as an `Amended (issue 142` note, not a silent deletion
thread 'test_the_dto_and_the_adr_state_the_same_four_labels' panicked at crates/mem-web/tests/sweep_state_vocabulary.rs:66:5:
assertion `left == right` failed: dto.rs must carry exactly one backticked `sweep_state: ` statement, on one line: []
  left: 0
 right: 1
test result: FAILED. 0 passed; 3 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

## Step 2 — GREEN dto (`dto.rs` doc comment)

`test_the_dto_declares_no_legacy_peer_live` → `ok`, 1 passed. Other two still FAILED (ADR not yet amended).

## Step 3 — GREEN adr (D-19-3 sentence + `Amended (issue 142, 2026-11-18)` note)

```
test test_the_dto_declares_no_legacy_peer_live ... ok
test test_the_retired_label_is_recorded_as_a_historical_note ... ok
test test_the_dto_and_the_adr_state_the_same_four_labels ... ok
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

## Step 4 — mutants (one at a time; restored by `cp` from `.tmp/scratch/issue-142/saved/`, `cmp -s` = 0)

| Mutant | Intended test | Observed (exit 101 each) |
| --- | --- | --- |
| M1 — `legacyPeerLive` in DTO vocabulary line | `test_the_dto_declares_no_legacy_peer_live` | FAILED, panic `:56` retired-label message; agreement test FAILED at DTO set assertion `:76`, `left` contains `legacyPeerLive` |
| M2 — `legacyPeerLive` in ADR line only | agreement test, ADR set | DTO test ok; agreement FAILED `:81` (`ADR-0019 D-19-3 publishes the same four labels…`), `left` contains `legacyPeerLive` |
| M3 — `Amended (issue 142` note deleted | `test_the_retired_label_is_recorded_as_a_historical_note` | FAILED `:89:49`, `expect` message. Other two ok |
| M4 — `swept` dropped from ADR line | agreement test, ADR set | FAILED `:81`, `left: {"noSweepForTier", "notYetSwept", "sweepFailed"}`, `right: {…, "swept"}` |

Build lines: M1 `Compiling mem-web v0.1.0`; M2 `Compiling mem-web v0.1.0`; M3 and M4 `Compiling mem-profile`,
`Compiling mem-portability`, `Compiling mem-web`.
Restore: `restore_m1_cmp=0`, `restore_m2_cmp=0`, `restore_m3_cmp=0`, `restore_m4_cmp=0`. Rebuilt green after each:
`test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s`, exit 0.

## Step 5 — gates

```
cargo fmt --all --check                                      → exit 0
cargo clippy --workspace --lib -- -A clippy::all -D clippy::unwrap_used -D clippy::expect_used   → exit 0
cargo clippy --workspace --all-targets -- -D warnings -A clippy::await_holding_lock -A clippy::assertions_on_constants -A clippy::doc_lazy_continuation   → exit 0
cargo test -p mem-web                                        → exit 0
```

`cargo test -p mem-web` per-target results (verbatim `test result` lines, all exit 0): lib 22 passed;
`api_erasure_disclosure` 14 passed; `api_endpoint_contract` 20 passed; `sweep_state_vocabulary` 3 passed; every other
`--test` target ok (0 failed across the run).

## Step 6 — record, then verify

6a snapshots: `.tmp/scratch/issue-142/saved/{issue-142.pre.md, issue-142.green.md, dto.rs, 0019.md, prompts.rs (stale, see 6g note), prompts.head.rs}`.

6c issue `## Comments` written: `Chosen: direction 2 — retire the value`.

6d five-path check:
```
$ git status --short --untracked-files=all
 M .scratch/rust-port/issues/142-legacy-peer-live-unreachable.md
 M crates/mem-web/src/api/dto.rs
 M docs/adr/0019-erasure-posture.md
?? crates/mem-web/tests/sweep_state_vocabulary.rs
?? docs/evidence/issue-142/RUNS.md
status_exit=0
```
Matches the planned five, in git's order.

6e AC4 quiet diffs:
```
git diff --quiet -- crates/mem-web/src/api/prompts.rs;  echo "prompts=$?"   → prompts=0
git diff --quiet -- crates/mem-web/src/api/ports.rs;    echo "ports=$?"     → ports=0
git diff --quiet -- crates/mem-web/src/api/store_adapter.rs; echo "store=$?" → store=0
git diff --quiet -- crates/mem-web/src/api/memories.rs; echo "memories=$?"  → memories=0
```

6f AC1 greps:
```
grep -c 'direction 2' .scratch/rust-port/issues/142-legacy-peer-live-unreachable.md; echo "exit=$?"   → 1, exit=0
grep -c 'ADR-0025' .scratch/rust-port/issues/142-legacy-peer-live-unreachable.md; echo "exit=$?"     → 1, exit=0
```

6g-AC1 mutant (restore pre-edit issue, expect 0/exit 1; restore green, prove by `cmp -s`):
```
cp .tmp/scratch/issue-142/saved/issue-142.pre.md <issue>
grep -c 'direction 2' <issue>; echo "mutant_exit=$?"   → 0, mutant_exit=1
grep -c 'ADR-0025' <issue>;    echo "mutant_exit=$?"   → 0, mutant_exit=1
cp .tmp/scratch/issue-142/saved/issue-142.green.md <issue>
cmp -s .tmp/scratch/issue-142/saved/issue-142.green.md <issue>; echo "restored_issue_cmp=$?"   → restored_issue_cmp=0
sha256sum (green snapshot, issue)   → a081379fd84e32a6 (both lines)
grep -c 'direction 2' <issue>; echo "restored_exit=$?"   → 1, restored_exit=0
grep -c 'ADR-0025' <issue>;    echo "restored_exit=$?"   → 1, restored_exit=0
```

6g-AC4 mutant (append comment to `prompts.rs`, expect `prompts=1`; restore from HEAD, prove by `cmp -s`):
```
printf '\n// issue-142 ac4 mutant\n' >> prompts.rs
git diff --quiet -- prompts.rs; echo "mutant_prompts=$?"                 → mutant_prompts=1
git show HEAD:prompts.rs > prompts.rs
cmp -s .tmp/scratch/issue-142/saved/prompts.head.rs prompts.rs; echo "restored_prompts_cmp=$?"  → 0
git diff --quiet -- prompts.rs; echo "restored_prompts=$?"              → restored_prompts=0
```
Restore-source note: a first attempt restored from `saved/prompts.rs`, which predates the committed issue-143 content.
That reverted issue 143's uncommitted `prompts.rs` edits in the working tree (`git diff --stat` showed the 410 arm
removed). It was caught, repaired by `git show HEAD:` (`quiet=0`), and the AC4 mutant was re-run against HEAD. The
stale snapshot is not a valid restore source for this wave.

6h repeats after the mutants:
```
git status --short --untracked-files=all   → the five planned lines, status_exit=0
cmp -s green snapshot vs issue             → issue_cmp=0
grep -c 'direction 2' <issue>; exit=0 → 1 ; grep -c 'ADR-0025' <issue>; exit=0 → 1
```
