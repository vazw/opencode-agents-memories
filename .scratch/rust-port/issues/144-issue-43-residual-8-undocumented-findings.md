# 144 — issue 43's residual 8 names F4–F7 but no tracked document defines them

Status: done — 2026-11-18. Recovered what was recoverable (F4 = residual 3, the Windows
post-drop opens) and retired the rest as unrecoverable rather than leaving a pointer to a
table that was never committed.

*(Parent: issue 43.)*

## The defect this records

Issue 43's REPORT closed with:

> `F4/F5/F6/F7 stay P3 as ranked; F4 (Windows) needs an owner decision, not code.`

**None of F4, F5, F6 or F7 was defined anywhere tracked.** A repo-wide search found the identifiers only in that
sentence, plus unrelated same-named findings in other waves' documents (the consolidation rehearsal's F4, the
`memoryApiUrl` userinfo F6 in `crates/mem-bridge`), which are different findings sharing a letter. The ranking those
four were assigned ranked them against nothing a reader can see, and "as ranked" pointed at a table the tracked tree
does not contain.

This is the residual-8 defect: not that four findings are unactioned, but that the durable record could not say what
they are. Issue 43's own `## Ordering` and `## P3` sections list `43.10` and `43.11` and do not mention F4–F7.

F4 was described only as "(Windows)" and "needs an owner decision, not code", which is enough to know it is a platform
question and not enough to act on.

## What was searched, and where the trail ends

**MEASURED, 2026-11-18:**

| place | finding |
|---|---|
| tracked tree, `git grep 'F4/F5/F6/F7'` | only `docs/evidence/issue-43-w6/REPORT.md:153` and the commit that wrote it |
| `git log --all -S 'F4/F5/F6/F7 stay P3'` | `df6c744` only — the W6c rework whose message says "residuals 6-8 recorded (… F4/F5/F6/F7 P3)", naming no finding |
| `.tmp/progress/` (504 files) | no W6/erasure review; the only `F4`/`F5`/`F6`/`F7` hits belong to the consolidation wave and the rev14 review |
| `.tmp/blueprints/` (90 files) + `.tmp/blueprints/archive/` (6) | no `blueprint-20261108-w6-erasure-r1.md`; the file cited by `README.md` is gone, and its review with it |
| second-brain vault | no hit |

The ranking therefore died with an uncommitted `.tmp/` artifact. `.tmp/` is gitignored by design, so this is not a
recoverable regression — it is the documented cost of the repo's own rule that decisions live in tracked files.

## What was done

- **F4 identified from the document itself, not from memory (INFERRED, one reading available):** residual 3 is
  "Windows post-drop opens untested", the
  only Windows item in the same numbered list, and F4's own parenthetical says "(Windows)". The two are the same
  finding, and what it needs is the platform decision, not code.
- **F5, F6, F7 retired as unidentified.** They are *not* mapped onto residuals 1/2/6/7 by guesswork, and the same
  letters in other waves are named as different findings.
- `docs/evidence/issue-43-w6/REPORT.md:153` rewritten: it no longer claims a ranking a reader cannot find, it states
  the search that failed, and it says plainly that nothing follows from the row.

## Acceptance criteria

- [x] Each of F4, F5, F6, F7 either defined in a tracked document with its evidence, or explicitly retired as
      unrecoverable in `docs/evidence/issue-43-w6/REPORT.md`. — F4 identified; F5/F6/F7 retired with the search
      record.
- [x] `REPORT.md:153` rewritten so it no longer claims a ranking a reader cannot find.
- [x] If F4 is a Windows decision, state the decision that is needed. — It is residual 3: whether Windows post-drop
      opens are validated natively (needs a Windows runner this tree does not have) or accepted as unmeasured like
      macOS. The row now points at that.
- [x] The same-named findings in other waves are disambiguated, so a reader searching `F4` does not land on the
      consolidation rehearsal's unrelated finding.

## Comments

**Closed 2026-11-18.** Doc-only; no code, no test. The value here is negative: the record now says "unrecoverable"
where it used to imply a table existed, so nobody schedules work against a ranking that cannot be read.

Disposition of the sibling residuals, unchanged: 6 → issue 142 (`legacyPeerLive` unreachable from HTTP), 7 → issue
143 (status 404s for a deleted prompt), 1/2/3 stay open in issue 43 with residual 3's Windows decision now correctly
named as F4.
