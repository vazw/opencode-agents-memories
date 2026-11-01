# 89 — four tracker-state defects found by the 2026-11-01 audit: an untracked PRD, an unowned carry-over, a contradicted premise, and a quoted `Status:` token that corrupts the readiness count

Status: ready-for-agent — 2026-11-01 (all four MEASURED by the `opsec` audit §3 "Tracker defects".
Work will edit `.scratch/overview-roadmap-reconciliation/PRD.md`, `.scratch/rust-port/issues/64-*.md`,
`.scratch/rust-port/issues/51-tracker-status-truth.md` and `.scratch/rust-port/issues/67-v2-compaction-hook.md`;
no source changes. `docs/agents/issue-tracker.md` already carries the anchored-grep prose as of this
batch — see T4.)

## What is missing

The audit found the tracker disagreeing with the tree in four places. None is a code bug; all four make
the next agent believe something false.

**T1 — the reconciliation PRD is untracked and stale-open.** `.scratch/overview-roadmap-reconciliation/PRD.md`
has no issue file, although its own story 8 asks for one. Its amendments D1–D5 **have** landed in
`OVERVIEW.md` (verified by reading: `turso` only, the Svelte AC removed, the Epic 1 scope note present),
so the artifact reads as pending work that is in fact done.

**T2 — issue 14's R13 carry-over has no owning issue.** `sweep_decay_daily` has no production caller. A
`grep -rn sweep_decay_daily` over the tree finds the **definition** at
`crates/mem-profile/src/profile_store.rs:1062`, the two `operation: "sweep_decay_daily"` strings inside
that same function (`:1070`, `:1118`), a test at `:1595`, and three call sites in
`crates/mem-profile/tests/user_profile_decay.rs:253,317,323` — and no caller outside tests. An earlier
revision of this issue enumerated only `:1062` and `:1595`; the omission matters because the
`:1070`/`:1118` hits are what makes a naive `grep -c` look like a production caller. Issue 14 records the
gap as "carry-over to W5/W6"; a grep across the issue set finds `sweep_decay_daily` only in issue 14 (as
of 2026-11-01 the set held 97 files — `01`–`96` plus `54b-dedup-scan-bounding.md` — and 102 after this
batch; the number grows with every issue filed, so re-derive it rather than quoting it). See issue
90, filed to own it.

**T3 — issue 64's conclusion is contradicted by the live log; its premises are not.** 64 asserts the
profile tier renders on no production path. `~/.opencode-mem/opencode-mem.log` carries **119** `profile
inject` events on 2026-11-01 (as of 2026-11-01T11:00Z; it was 94 at the audit's read a few hours earlier
— a point-in-time counter, not a durable figure). What 64 measured still holds: `src/main.rs:243` is still
`user_id: "unknown".to_owned()`, the only assignment. What is falsified is only the conclusion drawn from
it.

The settling observation is named, not a request for "more counter-evidence": the `profile inject` lines
carry **no fields** — no user id, no profile id, no row count — so nothing in the log says *which* profile
key the renderer looked up. One instrumented render that logs the `user_id`/profile id it resolved (or a
`GET /api/profile` response from the running server) settles it in one reading: if that key is `"unknown"`,
64's story stands and the render is an empty-tier no-op; if it is a real identity, 64's identity claim is
wrong and only the store-write half survives.

**T4 — issue 51's own subject is no longer live; one quoted token still is.** Issue 51 is `done`
(`ce9e0b5` and `5e743cc`, plus `b16fb4d` citing both in its status line) and
`40-dedup-service-memories.md:3` already reads "landed — residual owned by issue 54". Nothing in 51 needs
fixing. What is live is one line **inside** it: `51-tracker-status-truth.md:11` quotes issue 40's old
status, and because the quote begins at the start of the line, an unanchored
`grep 'Status:.*ready-for-agent'` matches it. The same is true of `67-v2-compaction-hook.md:117`. That is
what made the audit's first readiness count 13 instead of 12.

MEASURED (2026-11-01T11:00Z, before this batch's files were rewritten):
`grep -l 'Status:.*ready-for-agent' .scratch/*/issues/*.md | wc -l` = **27**;
`grep -l '^Status:.*ready-for-agent' …` = **25**; the set difference was exactly
`51-tracker-status-truth.md` and `67-v2-compaction-hook.md`.

## What lands

- T1: an issue file (or an explicit `wontfix` note in the PRD) so the reconciliation PRD is tracked; if
  its amendments are landed, say so and close it.
- T2: issue 90 owns the missing production caller; 14's R13 line points at it.
- T3: issue 64's **conclusion** corrected in place, with the measured counter-evidence, a date, and the
  settled-by observation above — the specific premises stay, because they are still true.
- T4: reword `51-tracker-status-truth.md:11` — and `67-v2-compaction-hook.md:117` — so neither contains a
  bare `Status:` token (indent the quote, or move the quoted value inside a fenced block). The
  anchored-grep **snippet** already lives in `docs/agents/issue-tracker.md` at `:40-47` — its loop greps
  `'^Status:.*ready-for-agent'` at `:43` — so the mechanism was half-written; the missing prose sentence
  ("an unanchored grep also matches a *quoted* status line, so that count is not a measurement") was
  **added by this batch's correction**, immediately after that block at `:49-52`. Only the reword remains
  as work.

## Acceptance criteria

- [ ] `.scratch/overview-roadmap-reconciliation/PRD.md` is either tracked by an issue or marked
      superseded with a pointer to where its content landed.
- [ ] `grep -l '^Status:.*ready-for-agent' .scratch/*/issues/*.md | wc -l` is the count quoted anywhere
      in the tracker. (The prose rule itself landed with this correction at
      `docs/agents/issue-tracker.md:49-52`; the loop's anchored pattern is at `:43`.)
- [ ] `grep -l 'Status:.*ready-for-agent' .scratch/*/issues/*.md` returns the same file set as the
      anchored form — i.e. the two quoted-token sites (51, 67) are reworded.
- [ ] Issue 64 carries the correction with the settled-by observation named, or is closed.
- [ ] Issue 14's R13 line names issue 90.

## Parent

Slice issue 96 (`96-slice5-tracker-truth.md`).

## Comments

**Why this matters more than it looks:** the AGENTS.md contract says a `Status:` line is read by the next
agent as fact. Three of these four are a claim in the tracker that the tree contradicts — the same class
as issues 83/84 (a value one layer asserts and another never reads), one level up. An audit that spends
its effort on code while the tracker lies about what is open will keep re-finding the same work.

**Correction of an earlier revision:** this issue used to claim "issue 51's stale line" was broken and
demand it be fixed. It is not broken, and demanding a fix to finished work is the exact error this issue
is about. Only the quoted token remains.
