# 144 — issue 43's residual 8 names F4–F7 but no tracked document defines them

Status: needs-info — filed 2026-11-18 from issue 43 residual 8 (`docs/evidence/issue-43-w6/REPORT.md:153`).

*(Parent: issue 43.)*

## The defect this records

Issue 43's REPORT closes with:

> `F4/F5/F6/F7 stay P3 as ranked; F4 (Windows) needs an owner decision, not code.`

**None of F4, F5, F6 or F7 is defined anywhere tracked.** A repo-wide search finds the identifiers only in that
sentence, plus unrelated same-named findings in other waves' documents (the consolidation rehearsal's F4, the
`memoryApiUrl` userinfo F6 in `crates/mem-bridge`), which are different findings sharing a letter. The ranking those
four were assigned ranks them against nothing a reader can see, and "as ranked" points at a table the tracked tree
does not contain.

This is the residual-8 defect: not that four findings are unactioned, but that the durable record cannot say what
they are. Issue 43's own `## Ordering` and `## P3` sections list `43.10` and `43.11` and do not mention F4–F7.

F4 is described only as "(Windows)" and "needs an owner decision, not code", which is enough to know it is a platform
question and not enough to act on.

## What this issue must produce

1. **Recover the ranking** — locate the review or blueprint the four letters came from and transcribe each finding
   with its evidence into a tracked document under `docs/evidence/issue-43-w6/`, or
2. **Retire the reference** — if the source cannot be recovered, rewrite `REPORT.md:153` to say the ranking was lost
   with an uncommitted artifact and that residual 8 is closed as unrecoverable rather than left implying a table
   exists.

Either way, `REPORT.md:153` stops being a pointer to nothing.

## Acceptance criteria

- [ ] Each of F4, F5, F6, F7 either defined in a tracked document with its evidence, or explicitly retired as
      unrecoverable in `docs/evidence/issue-43-w6/REPORT.md`.
- [ ] `REPORT.md:153` rewritten so it no longer claims a ranking a reader cannot find.
- [ ] If F4 is a Windows decision, state the decision that is needed — the sentence "needs an owner decision, not
      code" is not actionable without what the decision is.
- [ ] The same-named findings in other waves are disambiguated, so a reader searching `F4` does not land on the
      consolidation rehearsal's unrelated finding.

## Blocked by

Nothing for the transcription; the F4 Windows decision itself is `ready-for-human` once it is defined.
