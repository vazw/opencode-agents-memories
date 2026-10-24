# 36 — W6.8 follow-up · Package the web UI beside the engine and forward the dist setting

Status: done — 2026-10-23 (reviewer APPROVED; node glob 39/39, enumerated 34/34, `web_dist_packaging` 2/2, `web_dist_wiring` 20/20, crate-scoped `fmt`/`clippy` exit 0, `cmp` exit 0 both sha256 `7d34d2e3…`; live packaged smokes on fresh ports 35457 / 35657 → health 200, title match, index 1,238 B, both stderr counters 0; real stage exit 0)

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

- [x] `plugin/scripts/stage-engine.sh` stages the built UI beside the engine binary, and a packaged
      engine resolves the default candidate with no setting and no flag.
      **Evidence:** A's three hermetic tests 3/3 (`test_stage_engine_copies_web_ui_bundle_beside_binary`,
      `test_stage_engine_refuses_when_web_ui_build_is_absent`, `test_stage_engine_refuses_to_replace_an_existing_bundle`);
      C1 `test_packaged_engine_serves_bundle_from_exe_relative_default_without_setting_or_flag`;
      E3 real stage exit 0 → `plugin/engine/linux-x64/` = 75,312,488 B binary + `web-ui/dist`
      (4 files, `index.html` 1,125 B, no `dist/dist`), smoke A on a fresh port 35457 → health 200,
      title match, served index 1,238 B.
- [x] The wrapper forwards `OPENCODE_MEM_WEB_DIST` to the child engine when the host sets it, and
      forwards `--web-dist` when the host provides it; `plugin/dist/plugin.js` matches `plugin/src/plugin.js`.
      **Evidence:** B1 RED (`manifest.env.OPENCODE_MEM_WEB_DIST` was `undefined`, no `--web-dist` in argv)
      → B2 green; `test_wrapper_forwards_web_dist_env_and_flag_to_child_engine` passes with the
      space-bearing path `/opt/opencode mem/web ui/dist` carried verbatim as one env value and as two
      adjacent argv elements; `test_dist_plugin_byte_identical_to_src` passes;
      `cmp plugin/src/plugin.js plugin/dist/plugin.js` exit 0, both files sha256 `7d34d2e3…`.
- [x] `plugin/tests/wrapper-glue.test.mjs` pins both forwarding behaviours, and the plugin battery
      stays green.
      **Evidence:** `test_wrapper_forwards_web_dist_env_and_flag_to_child_engine` +
      `test_wrapper_omits_web_dist_when_host_sets_nothing` (paired over-forwarding guard); glob battery
      `node --test plugin/tests/*.test.mjs` → **39/39 pass, 0 fail, 0 skipped** (34 baseline + A's 3 +
      B's 2); the enumerated `plugin/package.json` set → **34/34**.
- [x] A packaged engine started with no UI build present still degrades LOUDLY through the existing
      structured path (`web.launch_failed` plus `web.dist_missing`) and never panics — the
      degradation W6.8 established is preserved, not softened into a silent 404.
      **Evidence:** C2 `test_packaged_engine_without_bundle_degrades_loudly_and_never_panics`
      (`web_dist_packaging` 2/2); both structured events on stderr, no socket bound, stdio `hello`
      answered, no `panicked at`, exit 0 on EOF. Mutation proof: deleting the
      `structured_stderr("web.dist_missing", …)` call at `src/main.rs:121` produced 1 passed / 1 failed
      (stderr carried only `web.launch_failed`); restored byte-exact (`sha256sum -c` OK,
      `git diff src/main.rs` empty), back to 2 passed.
- [x] `docs/bridge-schema.md` §9 and the `docs/web-skeleton-smoke.md` D-7 note no longer state that
      the whole web surface is unavailable for plugin-launched engines, or are updated to reflect
      what actually holds after this issue lands.
      **Evidence:** E4 stale-claim greps all find nothing (`ENTIRE axum`, `Until issue 36 lands`,
      `engine-direct only`, `stages no UI` in `docs/`; `engine-direct surface only` in `src/`); the new
      anchors (`OPENCODE_MEM_WEB_DIST`, `plugin/scripts/stage-engine.sh`, the packaged path) and the
      `exit 5` / `exit 7` / line-sensitive `"--web-dist", dist` conformance greps all exit 0; the
      reviewer read all four doc hunks.

## Blocked by

- 35 · W6.8 (the composition-root wire and the `web.dist_missing` event this issue depends on)

## Comments

### 2026-10-23T10:08:10Z — close-out (`blueprint_id: 20261023-0836-w6-9-web-dist-packaging-r4`)

**Verdict:** reviewer `STATUS: APPROVED` on independent re-execution; all five ACs are ticked above
with inline evidence.

**Files changed (11):** `plugin/scripts/stage-engine.sh`, `plugin/tests/stage-engine.test.mjs` (new),
`plugin/src/plugin.js`, `plugin/dist/plugin.js`, `plugin/tests/wrapper-glue.test.mjs`,
`plugin/package.json`, `tests/web_dist_packaging.rs` (new), `src/web_dist.rs` (doc comment only),
`docs/bridge-schema.md`, `docs/web-skeleton-smoke.md`, and this issue file. The gitignored stage
output `plugin/engine/linux-x64/{opencode-agents-memory,web-ui/dist/**}` is build output, not a
tracked change (`git status --porcelain plugin/engine` = 0).

**Measured digest (reviewer, independent):** node glob **39/39**; enumerated battery **34/34**;
`web_dist_packaging` **2/2**; `web_dist_wiring` **20/20**; `cargo fmt -p opencode-agents-memory
--check` exit 0; `cargo clippy -p opencode-agents-memory --all-targets --no-deps -- -D warnings`
exit 0; `cmp plugin/src/plugin.js plugin/dist/plugin.js` exit 0 (both sha256 `7d34d2e3…`). E3 layout:
75,312,488 B binary + `web-ui/dist` 4 files, `index.html` 1,125 B, no `dist/dist`. Real stage
**exit 0**. Two live packaged smokes re-run on fresh ports **35457 / 35657**: health **200**, title
match, served index **1,238 B**, both stderr counters (`web.dist_missing`, `web.launch_failed`)
**0**. **Nothing in the wave was deleted**; `plugin/engine/**` stays gitignored.

**Mutation proofs:** C1 (i) liveness — renaming the fixture `index.html` away failed on the served
body (candidate 2's `CHECKOUT-DECOY` was served) rather than on readiness; restored, 2 passed.
C1 (ii) precedence — swapping `PACKAGED-FIXTURE` ↔ `CHECKOUT-DECOY` failed on the served body,
proving candidate-1 order; restored, 2 passed. C2 structured-event — deleting the
`structured_stderr("web.dist_missing", …)` call at `src/main.rs:121` produced 1 passed / 1 failed
(stderr carried only `web.launch_failed`); restored byte-exact (`sha256sum -c` OK,
`git diff src/main.rs` empty), back to 2 passed.

**Rulings folded from the review**

1. **`cargo test --workspace` exit 101 — accepted consequence, NOT a wave defect; mandatory
   follow-up before anyone claims a green workspace.** The sole failure is the pre-existing
   `engine_shutdown::test_package_tarball_contains_engine` (`tests/engine_shutdown.rs:565`), which
   asserts `stage-engine.sh` *always* exits 0; frozen contract 4 deliberately makes a second stage
   refuse with exit 7 once a bundle is already staged. Follow-up: update that oracle to treat exit 7
   as the already-staged success path and to decide the exit-5 / no-built-UI case for clean CI,
   **or** have it stage into a scratch copy like shard A's harness. **Measured timeline (evidence
   correction):** the blueprint's clean-tree baseline is **exit 0 at `9a523b5`**; the ledger's only
   post-stage measurement is **exit 101 at 09:41Z (post-E3)**. The earlier handoff's "exit 0 at E2,
   09:06Z pre-stage" claim is **withdrawn as unmeasured and implausible** — the stage landed at
   09:04:58Z, so any pre-E3 workspace run would itself have run that oracle and staged the bundle.
   Do not cite it.
2. **Two writers on `tests/web_dist_packaging.rs` — declared rework, process defect, artifact
   trustworthy.** Canonical token: **`rework: respawn-1`** (shard C; both stamp pairs and the finding
   were declared, not hidden). Same shard, so not an out-of-ownership edit. Artifact verified stable
   since **09:02:03Z**, sha256 `7babdc4c…`, exactly 2 `#[test]` fns, candidate 1 `PACKAGED-FIXTURE`,
   decoy `CHECKOUT-DECOY`, no `.bak` / `.orig` / `MUTATION` residue. Process finding: a future
   respawn must confirm the prior session is dead **and** the output path absent before dispatch.
3. **Shard B's START/END stamps were never relayed — recorded as *"stamps not relayed —
   substitute evidence"*, a protocol defect.** The substitute (controlled mutation of both wrapper
   files → 7 pass / 1 fail on the expected assertion; byte-exact restore proven by sha256 + `cmp` 0;
   suite back to 8/0; mtimes inside the wave-1 window) is adequate for the gate but **is not a
   stamp**, and is never recorded as a compliant one.

**Carried forward (non-blocking)**

- **Security residual, pre-existing, no delta from this wave:** `cargo audit` reports 3
  vulnerabilities — `rustls-webpki 0.103.15` (RUSTSEC-2026-0382), `webpki-root-certs 1.0.9`
  (RUSTSEC-2026-0383), `webpki-roots 1.0.9` (RUSTSEC-2026-0381), reachable via `mem-ai`/reqwest and
  `ort` — plus 3 allowed warnings (fxhash, paste, lru). `Cargo.toml` / `Cargo.lock` are unchanged by
  this wave; carry to a dependency wave.
- **Advisory, exit-6 invariant:** the refusal keys on `$WEB_UI_DEST/index.html`, so a dest left
  without `index.html` by exit 6 (or an interrupted copy) would be merged into on a re-run. The
  blueprint mandates this exact code and documents exit 6 as an accepted leftover, so it is not a
  defect; if "never merge" is ever wanted as a hard invariant, key the refusal on
  `[ -e "$WEB_UI_DEST" ]`.
- **Minor evidence note:** `crates/mem-vault/src/store.rs:141` is the accurate span for the
  `unnecessary_lazy_evaluations` baseline owner (the `InvalidKeyHex` literal sits at `:145`); the
  14 (`mem-skills`) / 1 (`mem-vault`) workspace-clippy split stands.
- **Count reconciliation:** `plugin/package.json:18` still omits `tests/e2e-shutdown.test.mjs`
  (5 tests) — pre-existing and out of scope; enumerated 29 → 34 and glob 34 → 39 both move by +5.

**Tracker:** ledger row Step F flipped to DONE at 2026-10-23T10:08:10Z; the ledger now carries
0 unchecked rows and 0 open `[~]` cursors.
