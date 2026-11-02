# 113 — the package's `./v2` export is dead

Status: needs-triage

Found while implementing issue 112 (the V2 post-compaction restore send). No code change is made by that
wave; this issue records the finding so removing a pinned packaging claim gets its own decision.

## Evidence

`packages/plugin/src/host.ts:25-65` resolves a plugin's server entry as `entry(["server", ""])` — i.e.
`exports["./server"]` then `exports["."]`. `./v2` is not in that list, at any version. The V2 adapter is
reached only by the in-process import at `plugin/src/plugin.js:1260`
(`const mod = await import("./v2/plugin.js")`), i.e. through the same `server()` the host does load, via
`default.setup`. MEASURED-adjacent: the wrapper's own tests drive `dist/v2/plugin.js` by import, never
through package resolution.

By contrast `./tui` **is** live, and its provenance is not resolution by the server: the server advertises
`features.tui` for a package whose `exports["./tui"]` resolves
(`packages/core/src/plugin/module.ts:147-152`, `packages/plugin/src/host.ts:58`), and the **TUI process**
then loads it (`packages/tui/src/plugin/context.tsx:95-104`, `:292-306`, `:670`). The server never
imports it.

## Why it is not removed here

`plugin/tests/v2-loader-contract.test.mjs:30` pins `pkg.exports["./v2"]`. Removing a pinned packaging
claim is its own decision with its own test change, not something to smuggle through a restore wave.

## Shape of the fix (not decided here)

Either (a) drop the `./v2` export and re-point `v2-loader-contract.test.mjs` at the in-process
`server().setup` route, or (b) keep it and change the contract test's name/claim to say it is inert.
Option (a) is smaller at the manifest; both require proving the host never resolves `./v2` for a live
package (source-read at a named commit plus a live load).

## Parent

Issue 112, D-112-8; ADR-0017 DECISION 5 amendment (2026-11-02).
