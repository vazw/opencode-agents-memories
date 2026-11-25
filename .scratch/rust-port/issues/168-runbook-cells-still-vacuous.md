# 168 — the rehearsal runbook's §8 and §6b can still pass while measuring nothing

Status: ready-for-agent
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

Fix shape: a `rehearsal_pid()` with the same argv-exact predicate the live pid uses —
`argv[0]` suffix `plugin/engine/*/opencode-agents-memory`, then `--storage-root`, then `$REH/store` — reading
`/proc/<pid>/cmdline` for the exact three-element form, and never `head -1` on a substring scan. Both the
kill and the gone-check use it, so "no match" and "stopped" stay distinguishable.

## Acceptance criteria

1. A smoke run whose script exits non-zero makes `§6b` report a failure, with the script's own rc in the output
   and not `tee`'s.
2. `§8` with no engine on `$REH/store` refuses with a distinct code and message, and does not print
   `rehearsal_engine_gone` or `port_3749_free_again`.
3. `§8` run from inside a `bash -c` wrapper whose cmdline carries the expanded storage root kills the engine and
   not the wrapper — proven by the wrapper surviving and the engine's pid disappearing.
4. A stopped engine and a never-started engine remain distinguishable in the transcript, so "the engine was
   already gone" cannot be recorded as "the rollback stopped it".

## Inverted world

Closed as *measurement, not a deliverable* if a run shows `§6b` already propagates the script's status (the
runbook sets `pipefail` somewhere, or the script is invoked without `tee`) and `§8`'s kill already requires a
non-empty match — in that case the AC1 run's output was wrong, not the runbook.
