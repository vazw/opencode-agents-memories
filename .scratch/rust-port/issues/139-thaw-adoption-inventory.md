# 139 — Thaw adoption inventory: census every raw element and rule it

Status: done — 2026-11-18. Ruling landed: `docs/evidence/thaw-adoption-census/` holds the ruling, the probe report, the route walk and the README; `web-ui/scripts/thaw-adoption-census.mjs` and `thaw-census-browser.cjs` are the gates. Probe landed at HEAD `d58a94f`; replace set 6 kinds / 20 occurrences, all `core_screens.rs`. No UI code changed.

*(Parent: issue 118.)*

## Parent

118 — the web UI draws 1 of 58 Thaw components.

## What to build

A measurement, not a migration: enumerate every raw HTML element the web UI still renders, decide for each whether a Thaw component exists and whether replacing it is an improvement, and publish the ruling the migration will follow.

The app currently draws a small number of Thaw components and a large number of raw elements — tables, a textarea, details/summary pairs, inputs, lists, and bulk `div`/`span`/`p`/heading markup — while Thaw ships table, popover, tooltip, dialog, textarea, switch, checkbox, menu, drawer, divider and layout primitives that are unused. But "replace all raw HTML" is not automatically right: two native `<select>`s are load-bearing contracts, `ul`/`li` carry list semantics Thaw has no equivalent for, and replacing semantic markup with `div`s would reduce accessibility while raising the Thaw count.

Each element kind gets exactly one classification: **replace** (a Thaw component exists and is strictly better), **keep-native** (a contract or a semantic the framework cannot express), or **no-equivalent**. The census script fails when a *replace* kind appears outside the ruling, so the list cannot silently grow back.

## Acceptance criteria

- [ ] Every raw element kind in the rendered app is counted per route, with the route and the count stated.
- [ ] Each kind carries exactly one classification, with the reason: replace / keep-native / no-equivalent.
- [ ] Contrived or semantic markup is explicitly ruled `no-equivalent` where a Thaw swap would lose semantics (`ul`/`li` is the expected case) — a ruling that inflates the Thaw count at accessibility's expense is a defect, not progress.
- [ ] The two `data-testid` selects are ruled `keep-native` with the contract named.
- [ ] The census script exits non-zero when a `replace`-classified kind is found outside the ruling.
- [ ] The ruling is a durable artifact (tracked under `docs/evidence/`), not scratch.

## Blocked by

None — can start immediately.
