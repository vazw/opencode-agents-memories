# 109 — `AGENTS.md`'s counts are stale, and the file it misleads is the one every agent reads first

Status: ready-for-agent — 2026-11-01. MEASURED at `bab77cc`; the file's own "Verify this file" block gives the
commands that make this trivially checkable, which is why the drift is a defect rather than a surprise.

## What is stale

| `AGENTS.md` says | actual | how it is derived |
|---|---|---|
| 10 `plugin/tests/*.test.mjs` files | **28** | `ls plugin/tests/*.test.mjs \| wc -l` |
| 11 workspace-level root suites in `tests/` | **17** | `ls tests/*.rs \| wc -l` |
| 12 crates under `crates/*` | 12 ✅ | `ls -1d crates/*/ \| wc -l` |

Both wrong figures are in the "Current state & stack" section, and the plugin one is contradicted three lines
later by the file's own `Verify this file` block, which runs exactly the command that disproves the text
above it.

## Why it matters more than a typo

`AGENTS.md` is the first file every agent in this repo reads, and it is written as a snapshot with a
re-derivation appendix. An agent that trusts the snapshot and does not run the appendix inherits the number.

That happened, measured: a plan gate caught a blueprint stating "plugin 174 pass / **10 files**" as a
before-count. The blueprint author had taken the figure from `AGENTS.md` instead of running
`ls plugin/tests/*.test.mjs | wc -l`, so its before/after table was wrong before a single line of code was
written — and its own guard ("a mismatch is a blueprint defect to report, not to renumber") was tripped on
arrival, which is a guard that reports an error it exists to prevent.

## What lands

- Correct both figures, and date the correction ("counts re-derived 2026-11-01 at `<sha>`") so the next
  reader knows the snapshot's age rather than assuming it is live.
- Where a count is cheap to re-derive, prefer a phrase that cannot rot over a numeral: "the plugin test files"
  rather than "10 `*.test.mjs` files". Keep numerals only where the `Verify this file` block already prints
  the command beside them.

## Acceptance criteria

- [ ] Every numeral in "Current state & stack" matches a command in the "Verify this file" block, run at the
      commit the correction lands on.
- [ ] Any numeral that cannot be kept true cheaply is rewritten as a description rather than a count.
- [ ] The correction is dated and commit-stamped in the text.
- [ ] The `Verify this file` block itself is run as part of the change, and its output is recorded in the
      commit message or an evidence file — the block is the contract, so the check that it still works is
      part of the fix.

## Parent

No slice parent. Found by a plan gate that refused a before-count inherited from this file.

## Comments

**This is the same class as tonight's other doc defects, inverted.** Those were prose asserting something a
change had made false; this is prose asserting something that was never re-measured. Both are claims that
outran their artifact, and both were caught only because someone else ran the command. The difference is
that this file's whole design is supposed to prevent it — the appendix exists precisely so the snapshot is
checkable — so the failure is that the appendix is not run when the text is edited, not that the commands
are wrong.
