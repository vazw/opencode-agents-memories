# Testing conventions

The upstream suite (`example/opencode-mem/tests/`, 110 files, v2.31.3 @ 906038e) is the behavioral contract for this port: every upstream test receives exactly one of three dispositions, making coverage explicit and auditable. Each ported mirror's mapping is recorded in the [mapping registry](#mapping-registry) below; W7.2 audits the union into the full parity matrix.

## Where tests live

- Integration tests live at `crates/<crate>/tests/<snake_case_of_upstream_name>.rs` — e.g. `context-limit.test.ts` → `crates/mem-core/tests/context_limit.rs`. Each test file doc-comments the upstream file it ports and traces its upstream cases.
- Shared helpers live in `crates/<crate>/tests/common/mod.rs` (std-only, headed by `#![allow(dead_code)]`; no dev-dependencies).
- Test names are `test_<behavior>_<condition>`.
- Crate-internal tests (`#[cfg(test)]` in `src/`) are only for private items — integration tests assert through the crate's public interface.
- Focused command: `cargo test -p <crate> --test <file>`. Full battery (all exit 0): `cargo fmt --all --check`, `cargo build --workspace` (0 warnings), `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.

## Test-first slices

Every behavior lands RED→GREEN: the failing test is written first, its failure reason recorded, then the minimal green implementation. The RED reason is what proves the test can fail — a test that cannot fail is not a test.

## Upstream-test dispositions

Exactly three strings, one per upstream test file:

- **port** — a Rust test file mirrors the upstream case (test-for-test; every upstream `it` case traced by name in the doc comment). The registry row names both the Rust file and the traceable tests.
- **covered-by** — the behavior is already asserted by a named Rust test with equivalent or stronger coverage. Example: upstream's node `path.join`/`win32.dirname` assertions are covered-by the `std::path` tests `test_path_join_child_segment_has_no_doubled_separators` and `test_parent_dir_extracts_shards_directory` in `windows_path.rs` on the native platform, plus `normalize_separators` for foreign-separator strings.
- **N-A** — not applicable; the row must state the reason class: Bun/tooling/packaging-specific (e.g. `plugin-bundle-boundary.test.ts`, `package-dependencies.test.ts`) or a deliberate port non-goal.

Helper modules (`turso-test-utils.ts`) and fixture files are not dispositioned — they translate mechanically per the recipe below.

## Fixture-translation recipe

Upstream worker-script fixtures become in-process or child-process tests in the consuming crate.

| Upstream fixture form | Translation |
|---|---|
| Worker script needing process isolation (e.g. `profile-learning-lock-worker.mts`) | Child entry in the consuming crate's integration test binary |
| `preload` monkey-patch fixtures (e.g. `profile-cold-buffer-worker-preload.ts`) | Fault injection at the same write seam, in-process (bounded chunks / failing writer) |
| Pure data fixtures | Files under `crates/<crate>/tests/fixtures/`, read via `include_str!` + `CARGO_MANIFEST_DIR` |

Child mechanics (proven executable by `crates/mem-core/tests/fixture_recipe.rs` + `tests/common/mod.rs::run_fixture_child`):

- A dedicated unreachable-by-default child test named exactly `fixture_child_entry` (`#[test] #[ignore = ...]`), harmless no-op when the mode env is unset.
- Spawned via self re-exec: `std::env::current_exe()` + args `--exact fixture_child_entry --ignored`.
- Env-var mode protocol: `MEM_FIXTURE_CHILD` selects the mode, `MEM_FIXTURE_OUT` carries the output path; configure children via env or files, never the real user storage.
- Payloads travel through files; stdout/stderr are redirected to files, never pipes (a chatty child cannot deadlock the parent).
- The parent polls `try_wait()` with a bounded deadline, then `kill()` + `wait()` — no orphan processes, no unbounded waits. Multi-step handshakes use barrier files with bounded waits — never sleep-synchronized.
- Per-crate helper sharing is by copying the helper into that crate's `tests/common/mod.rs` (a shared test-support crate or feature was rejected: it violates no-dev-deps and would put test code in the shipped lib).

## Hermeticity rules

- Temp dirs under `std::env::temp_dir()` only.
- Never `std::env::set_var` (parallel tests race) — inject values via the `EnvSource`/`ConfigPaths` seams.
- No network, no user-home reads/writes, no dev-dependencies, no sleeps for synchronization.

## Mapping registry

| Upstream test | Disposition | Rust counterpart | Notes |
|---|---|---|---|
| `tests/config.test.ts` | port | `crates/mem-core/tests/config_defaults.rs`, `provider_status.rs` | W0.2 |
| `tests/config-resolution.test.ts` | port | `crates/mem-core/tests/config_resolution.rs` | W0.2 |
| `tests/jsonc.test.ts` | port | `crates/mem-core/tests/jsonc_parse.rs` | W0.2 |
| `tests/context-limit.test.ts` | port | `crates/mem-core/tests/context_limit.rs` | W0.3, 4 upstream `it` cases + justified boundary extras |
| `tests/windows-path.test.ts` | port | `crates/mem-core/tests/windows_path.rs` | W0.3; `join`/`dirname` sections covered-by `test_path_join_child_segment_has_no_doubled_separators`, `test_parent_dir_extracts_shards_directory` |
| `tests/memory-scope.test.ts` | port | `crates/mem-core/tests/memory_scope.rs` | W1.1; scope-defaulting + ref-ordering ported (`test_default_scope_walks_only_the_tag_project_scope`, `test_all_projects_selection_walks_user_and_project_scopes`, `test_explicit_scope_argument_overrides_config_default`, `test_all_projects_ref_order_places_user_scope_first`); cross-shard walking (result counts, shard-id order) covered-by W1.2/W1.3 store tests |
| `tests/memory-scope-helper.test.ts` | port | `crates/mem-core/tests/memory_scope_helper.rs` | W1.1; 9 upstream cases + justified boundary extras |
| `tests/client-scope-traversal.test.ts` | port | `crates/mem-core/tests/client_scope_traversal.rs` | W1.1; the upstream `getAllShards` call sequences are `resolve_memory_scope` output |
| `tests/tags.test.ts` | covered-by | `crates/mem-core/tests/windows_path.rs` | W0.3 ported all `getProjectName` cases; W1.1 adds container-tag construction coverage in `tags.rs` + `project_scope.rs` |
| `tests/project-scope.test.ts` | port | `crates/mem-core/tests/project_scope.rs` | W1.1; 8 Linux-running cases ported; the 3 win32-only shim cases are N-A (upstream skips them on non-win32; no win32 runner) — Linux-analogous containment covered by `test_probe_refuses_git_executable_inside_the_untrusted_project_root` |
| `tests/tool-scope.test.ts` | port | `crates/mem-core/tests/tool_scope.rs` | W1.1; scope precedence ported as `scope::resolve_scope_selection`; plugin/tool wiring covered-by W5.4 |
| `tests/tag-migration-skip-tagged.test.ts` | port | `crates/mem-core/tests/tag_migration_skip_tagged.rs` | W1.1; migration core extracted from `api-handlers.ts`; HTTP handler wiring covered-by W6.2 |
| `tests/tag-migration-soft-failure.test.ts` | port | `crates/mem-core/tests/tag_migration_soft_failure.rs` | W1.1; same extracted seam; reset/retry semantics pinned |
| `tests/turso-connection-manager.test.ts` | port | `crates/mem-store/tests/turso_connection_manager.rs` | W1.2; 5 cases + encryption-flag/key + `..`-filename extras |
| `tests/turso-connection-lifecycle.test.ts` | port | `crates/mem-store/tests/turso_connection_lifecycle.rs` | W1.2; 2 cases + gate-invalidation extra |
| `tests/sqlite-handle-release.test.ts` | port | `crates/mem-store/tests/sqlite_handle_release.rs` | W1.2; retry/sidecar/Drop-fd batteries; invalid-`maxRetries` type-enforced; win32 cleanup case N-A |
| `tests/turso-shard-rotation.test.ts` | port | `crates/mem-store/tests/turso_shard_rotation.rs` | W1.2; both cases + lock/registry boundary extras |
| `tests/turso-shard-recreate.test.ts` | port | `crates/mem-store/tests/turso_shard_recreate.rs` | W1.2; both cases + missing-file/no-table extras |
| `tests/turso-ready-gate.test.ts` | port | `crates/mem-store/tests/turso_ready_gate.rs` | W1.2; injected legacy/encryption probe steps (real migrators W1.3/W1.5); re-embed-swap case covered-by W1.3; live-lock store half in W1.2 |
| `tests/shard-inventory.test.ts` | port | `crates/mem-store/tests/shard_inventory.rs` | W1.2; all 3 cases + status/sort/disk-only extras |
| `tests/shard-path-migrate.test.ts` | port | `crates/mem-store/tests/shard_path_migrate.rs` | W1.2; 8 of 9 cases — Windows-prompt case covered-by W3.1; + no-op/escape/prompt-failure extras |
| `tests/turso-auxiliary-db-upgrade.test.ts` | port | `crates/mem-store/tests/turso_auxiliary_db_upgrade.rs` | W1.2; case + idempotency + swallow-behavior |

Maintenance rule: every wave appends/updates registry rows for the upstream tests it touches.

### Async tests (mem-store)

`#[tokio::test]` current-thread default; `tokio::sync::oneshot` barriers instead of sleeps; `tokio::join!` for races; real-turso tests own temp dirs and assert release structurally (Drop flags, rename-after-drop, `/proc/self/fd` baseline with tolerance); fakes implement the boxed-future traits (`Db`, `ConnectFactory`, `ShardRegistry`, `ProjectTagSource`, `PromptPathUpdater`, `ProcessProbe`); `golden_sql.rs`/`schema_migrations_runner.rs`/`vector_store_subset.rs` are justified additions not registry-mapped (W1.1 `sha256.rs` precedent; W1.3 completes `schema-migrations.test.ts`).


## Untested upstream modules

Modules with no upstream test (e.g. `src/utils/math.ts`) get direct unit coverage. The registry row is intentionally omitted; the wave's test plan names the tests instead (W0.3: `crates/mem-core/tests/math.rs`).
