# 28 — W7.2 · Parity matrix & drop-in rehearsal

Status: ready-for-human — 2026-10-29 (split: the 23 missing matrix rows are **issue 71**; this issue is the
manual drop-in rehearsal and the §0b D3 hatch, which is now **ruled: reconfirmed deferred**. The rehearsal plan
is fixed — copy-first, re-derivable transcript, live swap as a separate owner-executed repeat — under
`## Owner rulings 2026-10-29`.)

## What to build

The final gate: a coverage matrix for **all 110 upstream `*.test.ts` files** with disposition (ported / covered-by / N-A) and zero unexplained red cells; and the end-to-end rehearsal — TS version uninstalled, Rust version installed into the same project, existing memories readable, UI functional: the port's Leptos UI works against the Rust server; REST parity additionally keeps the upstream Svelte dist usable. Closes (or explicitly confirms) the §0b D3 sqlite-vec experimental hatch. Wave detail: `.scratch/rust-port/roadmap.md` §W7.2.

## Acceptance criteria

- [ ] Parity matrix document — **moved to issue 71** (matrix completed by issue 71: 110/110; 15 rows were still missing at r1).
- [ ] Drop-in rehearsal transcript: same project dir, TS removed, Rust installed → memories intact, UI functional, capture/injection working.
- [ ] §0b D3 hatch closed or expressly reconfirmed as deferred.

## Blocked by

- 01–27 (all prior waves)

- **2026-10-27 — triaged `ready-for-human`.** The registry is real but behind: `docs/agents/testing.md`
  holds 87 upstream rows (83 port / 3 covered-by / 1 N-A) against a pinned 110-file suite, and 23 pinned files
  have no row — several with live counterparts (`plugin/tests/*.test.mjs`, the portability and skills crates),
  others genuinely unported (`v2-*`, `tui-companion`, `auto-update`, `i18n`, `runtime-require`). No rehearsal
  transcript exists under `docs/evidence/`; the hatch has a PRD reconfirmation but no closure record. Why human:
  a drop-in rehearsal is a manual install/uninstall property no test can settle, and closing the §0b D3 hatch is
  an owner ruling.

## Owner rulings 2026-10-29

**Target = copy first.** Copy the real storage root to a scratch path and run the migration against the copy
with a scratch config; the live swap is a **separate, owner-executed repeat**, and this issue closes only after
that step is recorded. Reason: the port's `legacy_migrator` renames sources to `.legacy.bak`, so the live root
is the only copy of the user's memory and must not be the test fixture.

**Evidence = a re-derivable text transcript** under `docs/evidence/issue-28-dropin-rehearsal/`, containing:

- the TS store's memory count **before**;
- the swap (`opencode-mem@latest` removed from `opencode.jsonc:171–173`, the local `plugin/` installed);
- the count **after**, plus **one recall by content** (a count alone proves a database opened, not that the data
  is the same);
- the `.legacy.bak` files, proving the migration ran;
- `curl` responses from the UI's own API (index plus a listing endpoint);
- one capture round-trip (a new prompt becomes a memory);
- one injection (the `memory_context` frame appears in a live turn).

No screenshots as primary evidence; no scripted suite — 28 is a manual install property by definition, which is
why the matrix half is issue 71.

**§0b D3 hatch = reconfirmed deferred.** Trigger table:

- **(a) SQLCipher at rest** — not triggered: the vault uses turso's experimental encryption, and issue 53
  measured it across processes.
- **(b) multi-process DB access** — **not suppressed**: the engine documents *"No multi-process access, No
  multi-threading"* and this port depends on concurrent processes (engine, host plugin, CLI). Issue 53 measured
  the unlisted combination working; that is evidence it works, **not** that it is supported.
- **(c) int8/bit vectors, ANN** — not triggered; exact scans at our row counts.

Close condition (roadmap §0b D3): *"If neither switch trigger materializes by Phase 7, close it as
considered-and-deferred."* Trigger (b) is material, so the hatch stays named. **The observation that would open
it:** observed corruption, or a functional need that requires multi-process access on a supported path. The
Phase-7 gate is "closed-or-confirmed"; this is the confirmed branch.

**Order:** after the W3 wave (issue 43) and the ADR-0019 text. The copy-first run may be executed by a subagent;
the live repeat is the owner's.
