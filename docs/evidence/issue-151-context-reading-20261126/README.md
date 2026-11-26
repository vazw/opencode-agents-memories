# Issue 151 — the `context.hook` reading, 2026-11-26

Premise under test: *a session reaches `context` with no prior `prompt` hook call, so the `context` path gives it
nothing.* The pre-registered, owner-approved rule (issue 151, "Instrument shipped") says the premise is witnessed
**only** by a session that has `context.hook` lines and **zero** `inject.requested` lines.

MEASURED by `read.py` against every `~/.opencode-mem/opencode-mem-2026-11-*.log`, read-only:

| quantity | value |
|---|---|
| log files scanned | 19 (`2026-11-01` … `2026-11-25`) |
| `context.hook` lines | **3,135** (the instrument shipped 2026-11-25, so these are one day's population) |
| `inject.requested` lines | 2,964 |
| sessions with `context.hook` | 62 |
| sessions with `inject.requested` | 337 |
| **sessions with `context.hook` and zero `inject.requested`** | **0** |
| never-prompted (also zero `Auto-capture memory persisted` rows) | **0** |

`context.hook` line shapes: `has_block=false, turn=0` — 252; `has_block=true, turn=1` — 2,514; `true,2` — 53;
`true,3` — 29; `true,4` — 287. There is **no** `has_block=false` at `turn_index >= 1`, so the "refresh answered empty
and then `context` fired" path (issue 155) is unobserved as well.

## Verdict

The premise is not witnessed in 3,135 `context.hook` lines across 62 sessions. Issue 151 closes as
**measurement, not a deliverable** — the fallback it proposed would be dead code on this population.

The 252 `has_block=false, turn=0` lines are a real population, but they belong to **issue 166**: every one of those
sessions has `inject.requested` lines, i.e. it was prompted, and the block is gone at the start of a re-instantiated
adapter (`turn_index` 0 with no cache). That is block loss across adapter re-activation, not a never-prompted session.

## Re-entry trigger for 151

Reopen if a session appears with `context.hook` lines and **zero** `inject.requested` lines — i.e. `read.py` reports a
non-empty "PREMISE CANDIDATES" list. The same script answers the question; re-run it on a later log.

## Reproduce

```sh
cd /home/opencode/Code/Personal/Rust/opencode-agents-memory
python3 docs/evidence/issue-151-context-reading-20261126/read.py
```

The script only reads `~/.opencode-mem/opencode-mem-*.log`; it writes nothing.
