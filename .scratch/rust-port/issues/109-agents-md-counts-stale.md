# 109 — `AGENTS.md`'s counts had drifted from the tree, and the orchestrator is who let them

Status: closed — fixed in the commit that carries this file. This is a correction record, not a work item.

## What was wrong

| `AGENTS.md` said | actual | command |
|---|---|---|
| 10 `plugin/tests/*.test.mjs` | **28** | `ls plugin/tests/*.test.mjs \| wc -l` |
| 11 root suites in `tests/` | **17** | `ls tests/*.rs \| wc -l` |

Six other figures in the same section were re-derived and are correct: the vendored clone `1ddfdad` /
v2.33.0 / 117 `*.test.ts`, 12 crates, 16 pre-commit behavioural cases, `edition = "2024"` only, and zero
`jsonrpc`/`"hello"` hits under the reference clone's `src`.

## Whose defect this is

The orchestrator's. `AGENTS.md` is the fleet's own operating document — agents maintain it, not the owner —
so a stale snapshot is something I failed to re-derive, and filing it initially as a defect in "the owner's
file" was the wrong framing as well as the wrong fix. The owner said so plainly; that correction is recorded
here rather than repaired quietly.

The file already ships the commands that disprove both figures ("Verify this file"), so the failure is not
missing tooling — it is that the appendix is not run when the text changes.

## What changed

- Both figures corrected, and a dated line added at the top of the section: counts are a snapshot, re-run the
  appendix before quoting, prefer a description over a numeral when the numeral cannot be kept cheaply true.
- The whole "Verify this file" block was run at the commit that carries this file, not just the two broken
  commands; six clean figures above are its output.

## Why it was worth chasing rather than shrugging at

It propagated. A plan gate rejected a blueprint whose before-count read "plugin 174 pass / 10 files" — taken
from this file instead of measured — so the wave's before/after table was wrong before a line of code was
written, and the blueprint's own guard ("a mismatch is a blueprint defect to report, not to renumber") was
tripped on arrival. A stale numeral in the fleet's operating document is not a cosmetic defect; it is an
instruction that makes the next agent assert something false.

## Acceptance criteria

- [x] Every numeral in "Current state & stack" matches a command in "Verify this file", run at the commit the
      correction lands on.
- [x] The section states that the counts are a snapshot and that the appendix is the authority.
- [x] The correction is dated.
- [x] The appendix itself was run as part of the change.
