# 113 — the package's `./v2` export is dead

Status: done — 2026-11-02 — export removed in 48c00a1

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

## Decision

**D-113-1 — shape (a): drop the `./v2` export and re-point the contract test at the in-process route.**
The export is dead at every host version (`packages/plugin/src/host.ts:57-59` lists `server`, `tui`,
`rpc`; `:61` `gui-experimental`), so keeping it ships a packaging claim that is false for every
consumer, and the only thing holding it in place was a test asserting the claim — a test that pins a
lie is worse than no test. The live route is already pinned by
`test_v2_loader_contract_setup_delegates_to_the_v2_entry` in the same file, which drives `main.setup`
into `dist/v2/plugin.js`; the exports row changed from asserting presence to asserting absence
(`test_v2_loader_contract_exports_map_declares_server_and_tui_only`). Shape (b), keeping the key and
renaming the test "inert", was rejected: it leaves the manifest misleading for the next reader and the
renamed test still has to assert the same dead key.

**D-113-2 — the proof obligation is split into MEASURED, REPORTED and INFERRED, and the live host is
NOT restarted.**

- MEASURED: the host source-read at `9af7aa2da8` — `const entry = (subpaths) …` at
  `packages/plugin/src/host.ts:31`, and the returned `{ server: entry(["server", ""]), tui:
  entry(["tui"]), rpc: entry(["rpc"]), gui: … }` at `:57-61`; no `v2` subpath anywhere. Plus a node
  self-reference probe from the package directory, which is the same resolver the host calls
  (`resolveModule(specifier, directory)`), showing `opencode-mem/server` still resolving
  (`server -> function function`) and `opencode-mem/v2` throwing `ERR_PACKAGE_PATH_NOT_EXPORTED` after
  the edit (it printed `v2 -> object` before it).
- REPORTED (dated, already in the record): the live restores at 14:09:08 and 14:10:19 +07 are V2-only
  rows, so the adapter is being reached on the live host today — through `server()` → `setup()` →
  relative `import("./v2/plugin.js")`, not through package resolution.
- INFERRED (honest, not re-verified): the running host keeps working after the manifest edit, because
  the file it already loaded (`dist/plugin.js`) is byte-unchanged by this item and the key removed is
  never consulted by `resolve()`. Settling it needs a host restart, which this wave forbids; the
  inference is recorded as INFERRED with exactly that reason. A watcher-driven reload is **not**
  assumed: no watcher behaviour is claimed anywhere in this repo.
- `./tui` is untouched and its mechanism is unrelated: the advertisement is `features.tui`
  (`packages/core/src/plugin/module.ts:149`, `...(entrypoints.tui ? { tui: true as const } : {})`),
  consumed by the desktop inventory (`packages/desktop/src/main/plugin/host.ts:452`) and by the TUI
  process (`packages/tui/src/plugin/context.tsx:100` filter, `:670` `Host.resolve(target).tui`). No
  exports key other than `./tui` feeds that path, and `./tui` stays.

`plugin/dist/v2/**` stays with the export gone: `dist/plugin.js` reaches the adapter by a relative
in-process import that no exports map governs, so deleting the tree would kill the restore path.

## Parent

Issue 112, D-112-8; ADR-0017 DECISION 5 amendment (2026-11-02).
