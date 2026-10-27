# 28 — W7.2 · Parity matrix & drop-in rehearsal

Status: ready-for-human — 2026-10-28 (split: the 23 missing matrix rows are **issue 71**; this issue is the manual drop-in rehearsal and the §0b D3 hatch ruling.)

## What to build

The final gate: a coverage matrix for **all 110 upstream `*.test.ts` files** with disposition (ported / covered-by / N-A) and zero unexplained red cells; and the end-to-end rehearsal — TS version uninstalled, Rust version installed into the same project, existing memories readable, UI functional: the port's Leptos UI works against the Rust server; REST parity additionally keeps the upstream Svelte dist usable. Closes (or explicitly confirms) the §0b D3 sqlite-vec experimental hatch. Wave detail: `.scratch/rust-port/roadmap.md` §W7.2.

## Acceptance criteria

- [ ] Parity matrix document — **moved to issue 71** (23 missing rows; 87/110 today).
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
