# 28 — W7.2 · Parity matrix & drop-in rehearsal

Status: needs-triage

## What to build

The final gate: a coverage matrix for **all 110 upstream `*.test.ts` files** with disposition (ported / covered-by / N-A) and zero unexplained red cells; and the end-to-end rehearsal — TS version uninstalled, Rust version installed into the same project, existing memories readable, UI functional: the port's Leptos UI works against the Rust server; REST parity additionally keeps the upstream Svelte dist usable. Closes (or explicitly confirms) the §0b D3 sqlite-vec experimental hatch. Wave detail: `.scratch/rust-port/roadmap.md` §W7.2.

## Acceptance criteria

- [ ] Parity matrix document: every upstream test file has a disposition; all ported tests green.
- [ ] Drop-in rehearsal transcript: same project dir, TS removed, Rust installed → memories intact, UI functional, capture/injection working.
- [ ] §0b D3 hatch closed or expressly reconfirmed as deferred.

## Blocked by

- 01–27 (all prior waves)
