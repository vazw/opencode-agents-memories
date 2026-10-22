# 36 — W6.8 follow-up · Package the web UI beside the engine and forward the dist setting

Status: needs-triage

**Scheduling:** its own wave (packaging + wrapper lane) — sequenced AFTER W6.8 (issue 35). Named
here because W6.8's Plan Gate found that the wave it gates leaves a net regression for every
plugin-launched engine, and the remedy was prose in a blueprint rather than a tracked obligation.

**Why this exists.** W6.8 (issue 35) wires the composition root to hand mem-web the built Leptos app.
It resolves the location from `OPENCODE_MEM_WEB_DIST` / `--web-dist`, and otherwise probes two
exe-relative candidates. It also refuses to launch the web server when no build resolves, so the
failure is loud rather than a silent 404. But the setting and the default cannot actually reach a
plugin-launched engine:

- `plugin/scripts/stage-engine.sh:54-56` stages ONLY the binary to
  `plugin/engine/<plat>-<arch>/opencode-agents-memory`; no UI bundle is staged beside it, so the
  packaged default candidate (`exe_dir/web-ui/dist`) never exists for an installed package.
- The checkout default candidate (`exe_dir/../../web-ui/dist`) is produced by nothing either — under
  the mandated `CARGO_TARGET_DIR=/var/tmp/opencode/target-w53` it resolves outside the checkout.
- `plugin/src/plugin.js:143-151` builds the child environment from a fixed whitelist
  (`PATH`, `HOME`, `TMPDIR`, `LANG`, `LC_ALL`), so `OPENCODE_MEM_WEB_DIST` is never forwarded;
  `plugin/src/plugin.js:119-130` (`engineArguments`) forwards only `--storage-root`, `--mode`,
  `--web-host` and `--web-port`, so `--web-dist` is never passed. `plugin/dist/plugin.js` mirrors both.

**Consequence until this lands.** Every plugin-launched engine emits `web.dist_missing` plus
`web.launch_failed` at every start and runs with the ENTIRE axum surface disabled — the REST API
and `/api/health` as well as the UI, not merely a 404 on `/`. That is a net regression against
today, where the server binds and `/api/health` answers 200 while only `/` 404s. The W6.7 browser
smoke and the W7.2 "web UI functional" rehearsal cannot pass before this issue closes.

## What to build

Two changes, both in the packaging and wrapper lanes:

1. **Stage the bundle.** `plugin/scripts/stage-engine.sh` stages `web-ui/dist` beside the engine
   binary (the packaged default candidate's location), so the default resolves for an installed
   package with no setting and no flag.
2. **Forward the setting.** `plugin/src/plugin.js` whitelists `OPENCODE_MEM_WEB_DIST` in
   `childEnvironment` and pushes `--web-dist` in `engineArguments` when the host provides it;
   `plugin/dist/plugin.js` mirrors the change byte-for-byte, and
   `plugin/tests/wrapper-glue.test.mjs` pins both behaviours.

Until (1) lands, the explicit setting is the only way to point a plugin-launched engine at a build,
which is exactly why (2) matters as much as (1).

## Acceptance criteria

- [ ] `plugin/scripts/stage-engine.sh` stages the built UI beside the engine binary, and a packaged
      engine resolves the default candidate with no setting and no flag.
- [ ] The wrapper forwards `OPENCODE_MEM_WEB_DIST` to the child engine when the host sets it, and
      forwards `--web-dist` when the host provides it; `plugin/dist/plugin.js` matches `plugin/src/plugin.js`.
- [ ] `plugin/tests/wrapper-glue.test.mjs` pins both forwarding behaviours, and the plugin battery
      stays green.
- [ ] A packaged engine started with no UI build present still degrades LOUDLY through the existing
      structured path (`web.launch_failed` plus `web.dist_missing`) and never panics — the
      degradation W6.8 established is preserved, not softened into a silent 404.
- [ ] `docs/bridge-schema.md` §9 and the `docs/web-skeleton-smoke.md` D-7 note no longer state that
      the whole web surface is unavailable for plugin-launched engines, or are updated to reflect
      what actually holds after this issue lands.

## Blocked by

- 35 · W6.8 (the composition-root wire and the `web.dist_missing` event this issue depends on)
