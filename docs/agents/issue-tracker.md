# Issue tracker: Local Markdown

Issues and PRDs for this repo live as markdown files in `.scratch/` in this git repo.

## Conventions

- One feature per directory: `.scratch/<feature-slug>/`
- The PRD is `.scratch/<feature-slug>/PRD.md`
- Implementation issues are `.scratch/<feature-slug>/issues/<NN>-<slug>.md`, numbered from `01`
- **A number is unique across every feature directory, not just within one.** The staleness block below
  keys on `cut -d- -f1` and greps commits for `(issue NN)`, so a number shared by two features lets one
  feature's commit contradict another feature's status line — measured 2026-11-03, when `.scratch/rust-port/`
  and `.scratch/web-identity-and-project-picker/` both held 115, 116 and 117 (web `117-zero-milestones-diagnosis`
  versus rust-port's stderr drain, which `docs/adr/0013` cites). The web trio was renumbered to 120–122.
  Renumbering an existing issue means: `git mv`, a `*(Renumbered from NN …)*` note under the title naming
  the reason, and every citing prose reference updated. Take the next free number with:

  ```sh
  ls .scratch/*/issues/*.md | sed 's#.*/##' | cut -d- -f1 | grep -E '^[0-9]+$' | sort -n | tail -1
  ```

- Triage state is recorded as a `Status:` line near the top of each issue file (see `triage-labels.md` for the role strings)
- Comments and conversation history append to the bottom of the file under a `## Comments` heading

## When a skill says "publish to the issue tracker"

Create a new file under `.scratch/<feature-slug>/` (creating the directory if needed).

## When a skill says "fetch the relevant ticket"

Read the file at the referenced path. The user will normally pass the path or the issue number directly.

## Status lines are claims about the tree

A `Status:` line is read by the next agent as fact, so it must be checkable against the tree: a status that
claims readiness (`ready-for-agent`) **names the symbols or files the work will create**, and a status that
claims landing (`done`, or any line asserting merged work) **names the commit that landed it, or — for a
wave closed on a recorded review outcome — the outcome record under `## Comments`**. This rule is
**forward-looking**: the 2026-10-18 `done` backfill recorded in `docs/agents/triage-labels.md`
(issues 01–04) and the existing closings that rest on a recorded review outcome are conformant as they
stand and need no edit. When the deliverable landed but something remains, name the residual and its
owning issue in the line — `landed — residual (owned by issue NN)` — rather than choosing `done` (which
would hide the residual) or the readiness label (which would advert finished work as unstarted). Two
shapes stay non-findings: an open issue with some waves landed, whose line names what is still pending,
and a paused issue with a pause note.

Run the block below from the repository root after editing any `Status:` line. It prints nothing when no
`(issue NN)`-tagged commit contradicts a readiness claim — a commit can land an issue without that tag, so
**silence is a filter, not proof**: it never certifies that a status line is true, only that the tagged
subset is uncontradicted.

```sh
for f in .scratch/*/issues/*.md; do
  n=$(basename "$f" | cut -d- -f1)
  grep -q '^Status:.*ready-for-agent' "$f" || continue
  c=$(git log --format='%h %s' -F --grep="(issue $n)" -1)
  [ -n "$c" ] && printf 'STALE-CLAIM %s: %s\n' "$f" "$c"
done
```

**Anchor every count.** An unanchored `grep 'Status:.*ready-for-agent'` also matches a *quoted* status
line inside another issue's body — issues 51 and 67 each carry one — so a number taken that way is not a
measurement. Use the anchored form, `grep -l '^Status:.*ready-for-agent' .scratch/*/issues/*.md | wc -l`,
which is the pattern the loop above already greps.
