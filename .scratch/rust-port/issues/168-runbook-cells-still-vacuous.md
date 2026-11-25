# 168 — the rehearsal runbook's §8 and §6b can still pass while measuring nothing

Status: done — §6b, §8 and the pid scan fixed; AC1–AC4 met with cited proofs; five non-blocking debts recorded below
Filed: 2026-11-25, from issue 164's AC1 run (`.tmp/progress/issue-164-ac1-20261125T062617Z`)

Issue 164 fixed four cells that measured nothing. Its own acceptance run then found three more in the same
family, in the two cells 164 did not touch. Each is a way for a cell to report success while the thing it
claims to have done did not happen.

### 1. `§6b` pipes the smoke script through `tee`, so its exit status is `tee`'s

MEASURED (`docs/runbooks/parity-drop-in-rehearsal.md:335-338`):

```sh
scripts/smoke-rest-surface.sh --live-base-url … --evidence-dir "$REH/evidence/rest" | tee "$REH/evidence/rest-summary.log"
```

`grep -c pipefail` over the runbook returns **0**, so nothing sets `pipefail` and the pipeline's rc is `tee`'s.
Measured in isolation: `false | tee /dev/null` → rc 0; with `set -o pipefail` → rc 1. A smoke run that fails
every row therefore reports rc 0, and the step reads as a pass. The AC1 run only caught it because it recorded
`smoke_rc=${PIPESTATUS[0]}` beside `tee_rc` instead of trusting the pipeline.

Fix shape: capture the script's own status — `set -o pipefail` for the fence, or
`smoke_rc=${PIPESTATUS[0]}` recorded and required — and make the cell fail on a non-zero script rc, not on a
non-zero `tee` rc.

### 2. `§8`'s kill silently succeeds when it matches nothing

MEASURED by issue 164's AC1 harness (run `20261125T062039Z`, disclosed in the report's "Harness iterations"):
the kill matched no process, `kill -TERM ""` failed, and the following `pgrep … || echo rehearsal_engine_gone`
printed `rehearsal_engine_gone` — a pass proving the engine was never there, not that it was stopped.

`docs/runbooks/parity-drop-in-rehearsal.md:422-424`:

```sh
kill -TERM "$(pgrep -f -- "--storage-root $REH/store" | head -1)"
sleep 2; pgrep -af -- "--storage-root $REH/store" || echo rehearsal_engine_gone
! ss -ltn | grep -q '127.0.0.1:3749' && echo port_3749_free_again
```

An empty match, an already-dead engine and a never-started engine all produce the same green output.

Fix shape: require the match first — `[ -n "$RPID" ] || { echo "REHEARSAL-PID: no engine on $REH/store" >&2; exit 3; }` —
then kill that pid, then assert it is gone. The port line has the same shape: it passes when nothing was ever
listening, so gate it on the engine having existed.

### 3. The same `pgrep -f … | head -1` can kill the operator's own shell

MEASURED twice while verifying this issue: a `bash -c` wrapper whose command text carries the **expanded**
storage root is itself a `pgrep -f` match, and `head -1` may select it. `kill -TERM` then terminated the
reading shell (`exit 143 SIGTERM`) instead of the engine. Issue 164 fixed exactly this for the live pid by
introducing an argv-exact `live_pid()` (runbook `:57`) because a substring scan of `/proc/*/cmdline`
self-matches through the `bash -c` wrapper; the rehearsal-engine side of §8 was left on the substring form.

This is not hypothetical for a transcript harness, which builds each command with its values expanded — the
case that produced it. It is also unsafe for an operator pasting a fence into a wrapper.

Fix shape: a `rehearsal_pid()` with an **argv-pair** scan, not the three-element form this issue first proposed.
MEASURED (gate round 1, 2026-11-25T06:52:33Z): the live engine's `argc` is 3 (`--storage-root`,
`/home/opencode/.opencode-mem/data`), but a rehearsal engine started from `plugin/engine/linux-x64/` has
`argc=6` because `--mode=standalone` comes first — so `argv[1] == --storage-root` matches nothing there and a
three-element mirror of `live_pid()` would have reproduced defect 2 exactly. The predicate must scan argv for
the adjacent pair `--storage-root` `$REH/store`, reading `/proc/<pid>/cmdline` NUL-split, and never `head -1` on
a substring scan. Both the kill and the gone-check use it, so "no match" and "stopped" stay distinguishable.

## Acceptance criteria

1. A smoke run whose script exits non-zero makes `§6b` report a failure, with the script's own rc in the output
   and not `tee`'s. **MET** — shipped fence rc 1 with `smoke_rc=2` equal to the script's direct rc; the pre-change
   form exited 0 with zero `smoke_rc` lines while its own log said `rc=2`.
2. `§8` with no engine on `$REH/store` refuses with a distinct code and message, and does not print
   `rehearsal_engine_gone` or `port_3749_free_again`. **MET** — rc 3 never-started, rc 4 already-gone, rc 5
   mismatch, rc 6 survived, rc 0 stopped; only rc 0 prints `rehearsal_engine_stopped` and the port line.
3. `§8` run from inside a `bash -c` wrapper whose cmdline carries the expanded storage root kills the engine and
   not the wrapper — proven by the wrapper surviving and the engine's pid disappearing. **MET** — shipped block:
   `wrapper_rc=0`, `wrapper_alive=yes`, engine dead; pre-change mutant: `wrapper_rc=143`, engine still alive.
4. A stopped engine and a never-started engine remain distinguishable in the transcript, so "the engine was
   already gone" cannot be recorded as "the rollback stopped it". **MET** — rc 4 vs rc 3, distinct messages, and a
   sentinel naming a reused pid yields rc 4 or rc 5, never rc 0, because rc 0 requires the recomputed argv-exact
   reader to equal the sentinel.

## Inverted world

Closed as *measurement, not a deliverable* if a run shows `§6b` already propagates the script's status (the
runbook sets `pipefail` somewhere, or the script is invoked without `tee`) and `§8`'s kill already requires a
non-empty match — in that case the AC1 run's output was wrong, not the runbook.

## Landed 2026-11-25 — blueprint `8ea2e6a2f4d6`, review-1 APPROVED

One tracked file: `docs/runbooks/parity-drop-in-rehearsal.md` (+122/−16). `scripts/smoke-rest-surface.sh` untouched
(byte-identical, `2f3dcef9…`) — it already sets `set -u -o pipefail` at `:27`, so the defect was the runbook's pipe.

Plan history: gate round 1 rejected on four items — three line ranges one line off, one of which began on a fence
opener (`bash -n` rc 2 `unexpected EOF`), and a decoy test that could not fail because bash exec-optimises
`bash -c 'sleep 30; echo …'` to a cmdline of plain `sleep 30`. Round 2 rejected on one item that was the
orchestrator's, not the plan's: a tracker edit left the tree dirty, so the plan's own single-file row was
unreachable. Round 3 approved the unchanged stamp once the baseline was committed.

**The pid fix shape in this issue was wrong when filed and is corrected above.** Measured: the live engine's argv is
3 elements, a rehearsal engine's is 6 (`--mode=standalone` first), so the filed three-element mirror of `live_pid()`
would have matched nothing and reproduced defect 2 under the fix's name. Shipped: an argv-pair scan over NUL-split
`/proc/<pid>/cmdline`, verified at pair=1 vs `pgrep -f`=2 on a discriminating decoy.

Fifteen rows executed (14 mechanical plus the §8 stale-sentinel arm), every mutant applied to the real file with
`trap … EXIT` + in-chain `cp` + `cmp -s` and restored byte-identical; 17 `sh` fences `bash -n` clean;
`SMOKE_SELFTEST: PASS`; no scratch engine left on 3749; live engine 8953 and host 8878 never signalled.

### Non-blocking debts, recorded and not fixed

1. **Row 7's plan numeral predicted rc 0; measured rc 1.** A SIGSTOPped engine still holds 3749, so the port line
   reds by itself. The shipped row requires rc == 6 **and** the `still matches after SIGTERM` message **and** no
   `rehearsal_engine_stopped`, so the oracle is not rc-only and not vacuous. The plan text stays stale.
2. **`tee` numerals moved** to 167/215/221/282/388; the assertion is the count 5, verified by content. The plan's
   numeral list is stale.
3. **The plan's row-11 note is imprecise.** The awk excludes comments, so a comment naming `head -1` yields
   `hits=1` rc 0; the discriminating mutants are a second executable `head -1` (`hits=2`) and removal of the `REFDB`
   selector (`hits=0`). The shipped check is correct.
4. **§8's outcome list omits rc 1** — engine gone but something else binds 3749 gives `rehearsal_engine_stopped`
   without the port line, and the fence's last command fails. The cell still fails, never a green refusal; add rc 1
   to the prose on the next touch.
5. **A deviation was kept deliberately:** `kill -TERM "$STARTED_PID" || printf 'note: …'` in §5's rc-3 branch, which
   the approved Step 3 did not prescribe. Measured harmless (the `note:` line prints only if the kill fails, on
   stderr, in the start cell; §8's five-code contract untouched) and reverting it would strand an engine holding
   3749 and poison every later cell.
