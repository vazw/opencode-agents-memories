# 128 — At startup, engine logs and warns on inert config keys set in JSONC

Status: done — 2026-11-05 (deferred AC 49.6 from Issue 49)

## What happened

In Issue 49 (`docs/config-surface.md`), a full census of 102 config schema fields was performed. Dead/unwired fields in `FileConfig` were identified and cataloged with dispositions `mark inert (issue <N>)`.

However, if a user explicitly sets an inert configuration key (e.g. `similarity_threshold`, `auto_capture_max_iterations`, `memory_temperature`) in their `opencode-mem.jsonc` file, the engine previously parsed it silently into `FileConfig` without warning the user that the setting has no runtime effect in `opencode-mem`.

Issue 49 AC 49.6 was deferred to Issue 128:
> "At startup, the engine logs once the inert keys the user explicitly set in a jsonc file. Explicitly deferred to issue 128 per approved blueprint r2."

## Why it matters

- **Silent failure to apply configuration**: Users configure parameters expecting them to adjust thresholds, temperatures, or iteration counts, but the port does not wire these parameters. Without explicit diagnostics, users assume their settings are taking effect.
- **Truth in configuration**: Directly supports Issue 49's objective that the configuration surface must be truthful and transparent.

## What would close it

- [x] 1. Define `macro_rules! define_file_config` in `crates/mem-core/src/config/schema.rs` declaring `FileConfig` fields and tagging inert fields (`#[inert]`), exposing compile-time `FileConfig::inert_field_names()`.
- [x] 2. Update `crates/mem-core/tests/config_census.rs` to verify that `FileConfig::inert_field_names()` matches exactly 1:1 against all `docs/config-surface.md` census rows with `status == "dead"`. Zero drift.
- [x] 3. Track explicitly present inert keys in `crates/mem-core/src/config/mod.rs` during `Config::load` from parsed `merged` map, storing them in `pub inert_keys_present: Vec<String>` on `Config`.
- [x] 4. In `src/main.rs`, emit structured startup warning (`tracing::warn!` and `structured_stderr("config.inert_keys", ...)`) if `inert_keys_present` is non-empty.
- [x] 5. Include `inert_keys` in `hello` response (`crates/mem-bridge/src/engine.rs::handshake_result`).
- [x] 6. In plugin wrapper (`plugin/src/plugin.js`), when `advertisedConfig.inert_keys` is non-empty, call `notifyHost` with warning toast formatted with named keys (if <= 3) or count summary.
- [x] 7. Add integration test verifying:
  - Startup warning logged when inert keys set.
  - No warning logged when config contains only live keys or defaults.
  - Toast payload rendered properly.
- [x] 8. Verify non-vacuous mutant: omit inert key detection or report empty vector, proving test fails.

## Grilling Decisions (2026-11-05)

1. **Emission seam**: `pub inert_keys_present: Vec<String>` on `Config` + `tracing::warn!` and `structured_stderr` at startup in `src/main.rs`, plus Notification Toast.
2. **Toast delivery**: Plugin wrapper via `hello` handshake (`hello.result.config.inert_keys`). When non-empty, wrapper triggers `notifyHost` with warning toast.
3. **Inert key definition**: Declarative `macro_rules! define_file_config` in `crates/mem-core` tagging inert fields, generating `FileConfig::inert_field_names()`. Tested 1:1 against `docs/config-surface.md` census.
4. **Zero new dependencies**: Pure stdlib `macro_rules!`, no new crates.
5. **Toast message format**: Named keys summary: lists keys if <= 3 (e.g. `Inert config keys set: similarityThreshold, maxProfileItems. These have no effect.`), or count if > 3. Variant: `warning`.
