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
| `tests/turso-ready-gate.test.ts` | port | `crates/mem-store/tests/turso_ready_gate.rs` | W1.2 + W1.3 + W1.5; real legacy step wired W1.3; W1.5 adds the `build_ready_gate_with_encryption` ordering case (legacy → encryption → recovery → warmup); re-embed-swap case covered-by `turso_ready_gate.rs::test_ready_gate_recovers_interrupted_reembed_swap_before_engine_conversion`; libSQL-indexed startup cases covered-by `turso_engine_migrator.rs::test_refuses_diskann_indexed_shard_loudly_without_marker_or_backup` (in-place DiskANN rewrite refused — recorded divergence); live-lock case covered-by `turso_legacy_migrator.rs::test_migration_lock_retries_three_attempts_then_errors_locked_by_another_process` |
| `tests/shard-inventory.test.ts` | port | `crates/mem-store/tests/shard_inventory.rs` | W1.2; all 3 cases + status/sort/disk-only extras |
| `tests/shard-path-migrate.test.ts` | port | `crates/mem-store/tests/shard_path_migrate.rs` | W1.2; 8 of 9 cases — Windows-prompt case covered-by W3.1; + no-op/escape/prompt-failure extras |
| `tests/turso-auxiliary-db-upgrade.test.ts` | port | `crates/mem-store/tests/turso_auxiliary_db_upgrade.rs` | W1.2; case + idempotency + swallow-behavior |
| `tests/turso-vector-search.test.ts` | port | `crates/mem-store/tests/turso_vector_search.rs` | W1.3; 5 cases + zero-limit extra; exact cosine scans, hybrid ranking, session lookup, pins |
| `tests/turso-vector-utils.test.ts` | port | `crates/mem-store/tests/turso_vector_utils.rs` | W1.3; 3 upstream cases + 3 boundary extras |
| `tests/turso-exact-fallback.test.ts` | port | `crates/mem-store/tests/turso_exact_fallback.rs` | W1.3; single case |
| `tests/turso-reembed-migration.test.ts` | port | `crates/mem-store/tests/turso_reembed_migration.rs` | W1.3; injected `FakeReEmbedder` (mem-store stays embed-free) |
| `tests/turso-migrate-dims-preflight.test.ts` | port | `crates/mem-store/tests/turso_migrate_dims_preflight.rs` | W1.3; legacy dims preserved (4 ≠ config 768) |
| `tests/turso-engine-migrator.test.ts` | port | `crates/mem-store/tests/turso_engine_migrator.rs` | W1.3; marker/no-op/skip ported; rewrite case becomes loud refusal (turso cannot parse `libsql_vector_idx` — recorded divergence) |
| `tests/schema-migrations.test.ts` | port | `crates/mem-store/tests/schema_migrations.rs` | W1.3 completion of the W1.2 runner (`schema_migrations_runner.rs` stays a justified addition) |
| `tests/turso-legacy-migrator.test.ts` | port | `crates/mem-store/tests/turso_legacy_migrator.rs` | W1.3; 8 cases + lock/scope extras; batch-transaction divergence recorded |
| `tests/encryption-at-rest.test.ts` | port | `crates/mem-store/tests/encryption_at_rest.rs` | W1.5; 4 cases (encrypted open + missing-key rejection, 64-hex 0600 key file + reuse, marker fail-closed refusal, resolve-or-create off/on) |
| `tests/turso-multiprocess.test.ts` | port | `crates/mem-store/tests/turso_multiprocess.rs` | W1.5; `buildConnectOptions` encryption/multiprocess_wal cases + `wrapTursoOpenError` classification; the node-spawn serialization cases became the cross-process `lock_holder` example test; win32-only exclusive-open case N-A (no win32 runner) |
| `tests/embedding-model-presets.test.ts` | port | `crates/mem-core/tests/embedding_model_presets.rs` | W1.4; functions landed W0.2; 10 upstream cases + resolved-config preset integration |
| `tests/embedding-onnx-dtype.test.ts` | port | `crates/mem-embed/tests/embedding_onnx_dtype.rs` | W1.4; 6 upstream cases |
| `tests/embedding-remote-retry.test.ts` | port | `crates/mem-embed/tests/embedding_remote_retry.rs` | W1.4; 9 upstream cases + api-key-secrecy extra |
| `tests/embedding-task-prefixes.test.ts` | port | `crates/mem-embed/tests/embedding_task_prefixes.rs` | W1.4; 12 upstream cases |
| `tests/onnxruntime-resolve.test.ts` | covered-by | `crates/mem-embed/tests/runtime_init.rs` | W1.4; Node/Bun module-resolution shim cases are N-A (Bun/tooling-specific — `ort` statically links the runtime); runtime availability + actionable init error covered-by `test_ort_environment_initializes_once_per_process`, `test_runtime_init_error_message_names_cause_and_actionable_fallback`, plus every local-session test as liveness proof |
| `tests/memory-tool-warmup.test.ts` | port | `crates/mem-embed/tests/service_warmup_gate.rs` | W1.4; warmup-behavior subset (tool-gate wiring covered-by W5.x) |
| `tests/ai-provider-config.test.ts` | port | `crates/mem-ai/tests/provider/ai_provider_config.rs` | W2.1; factory/config resolution on the `AiProvider` seam; 90s hard ceiling + api-key-placeholder semantics ported |
| `tests/internal-capture-sessions.test.ts` | port | `crates/mem-ai/tests/provider/internal_capture_sessions.rs` | W2.1; capture-loop guard ported (60s grace window → still not re-captured) |
| `tests/opencode-provider.test.ts` | port | `crates/mem-ai/tests/provider/opencode_provider.rs` | W2.1; host provider over HTTP host-fetch (recorded divergence: no host token-count endpoint → `count_tokens` = ceil(chars/4)) |
| `tests/anthropic-provider.test.ts` | port | `crates/mem-ai/tests/provider/remote_providers.rs` | W2.2; exemplar adapter on shared plumbing, mock transport (no real keys/network); name/endpoint/auth/request + SSE + structured asserted through the W2.1 trait |
| `tests/openai-chat-completion-provider.test.ts` | port | `crates/mem-ai/tests/provider/remote_providers.rs` | W2.2; same seam; retry/timeout/structured uniformity via shared plumbing |
| `tests/minimax-provider.test.ts` | port | `crates/mem-ai/tests/provider/remote_providers.rs` | W2.2; same seam |
| `tests/atlas-cloud-provider.test.ts` | port | `crates/mem-ai/tests/provider/remote_providers.rs` | W2.2; same seam; conservative secret-shape rule keeps arbitrary keys usable |
| `tests/orcarouter-provider.test.ts` | port | `crates/mem-ai/tests/provider/remote_providers.rs` | W2.2; same seam; OpenAI-responses and Gemini adapters have no upstream test file — covered by the same file's trait-seam cases |
| `tests/privacy.test.ts` | port | `crates/mem-ai/tests/privacy/privacy_tags.rs` | W2.3; 12 upstream cases (strip + fully-private predicate) |
| `tests/privacy-malformed-tags.test.ts` | port | `crates/mem-ai/tests/privacy/malformed_tags.rs` | W2.3; 12 upstream + 3 parity-boundary guards + 2 tag-whitespace parity cases (U+FEFF recognized / U+0085 accepted — D4); companion `unicode_boundaries.rs` (5 justified additions, no upstream counterpart) pins char-boundary safety of the hand-written byte scanner |
| `tests/injected-prompt-filter.test.ts` | port | `crates/mem-ai/tests/privacy/injected_prompt_filter.rs` | W2.3; 23 upstream `it` cases port 1:1 + 1 re-export identity guard + 4 allowlist cases porting upstream `isStructuredSummaryPromptMessage` (D6, lives in `index.ts` upstream); `normalizeInjectionMarkers` is the re-exported `mem_core::config::normalize_injection_markers` (W0.2), first directly tested here |
| `tests/secret-resolver-permissions.test.ts` | port | `crates/mem-ai/tests/privacy/secret_resolver_permissions.rs` | W2.3; recorded divergence D2 — permissive modes REJECT where upstream warns-and-returns; mode tables re-partitioned: real-file `Ok` = readable owner-only (0600/0400/0700), owner-unreadable exposed modes proved by the pure `group_or_other_bits` predicate + read-denied `Err` (a non-root owner cannot `open` its own 0o000 file) |

Maintenance rule: every wave appends/updates registry rows for the upstream tests it touches.

### Async tests (mem-store)

`#[tokio::test]` current-thread default; `tokio::sync::oneshot` barriers instead of sleeps; `tokio::join!` for races; real-turso tests own temp dirs and assert release structurally (Drop flags, rename-after-drop, `/proc/self/fd` baseline with tolerance); fakes implement the boxed-future traits (`Db`, `ConnectFactory`, `ShardRegistry`, `ProjectTagSource`, `PromptPathUpdater`, `ProcessProbe`); `golden_sql.rs`/`schema_migrations_runner.rs`/`vector_store_subset.rs` are justified additions not registry-mapped (W1.1 `sha256.rs` precedent; W1.3 completes `schema-migrations.test.ts`). W1.5 adds three more justified additions — `turso_encryption_migrator.rs`, `turso_encryption_restore.rs` and `turso_operation_lock_xproc.rs` (upstream has no `encryption-migrator.test.ts` / `operation-lock.test.ts`, so those modules get direct coverage; `turso_encryption_restore.rs` is a regression for the restore-on-verify-failure path and sits in its own binary because its `set_force_verify_fail` fault seam is process-global); their cross-process cases spawn the `examples/lock_holder.rs` helper binary (built by `cargo test`/`--example`, located via `tests/common/mod.rs::spawn_lock_holder`). Fixture note: pure-data `tests/fixtures/legacy_diskann_shard.db` read via `CARGO_MANIFEST_DIR`; recipe in the committed script.


## Untested upstream modules

Modules with no upstream test (e.g. `src/utils/math.ts`) get direct unit coverage. The registry row is intentionally omitted; the wave's test plan names the tests instead (W0.3: `crates/mem-core/tests/math.rs`). Additions in this class from W2.3: `crates/mem-ai/tests/privacy/secret_shape.rs` (`SecretString` hygiene / shape validation — upstream tests neither) and `crates/mem-ai/tests/privacy/privacy_layer.rs` (the `PrivacyFilterLayer` middleware — upstream filters inline in `index.ts`, no dedicated test file), plus 3 in-crate lib tests in `src/privacy/layer.rs` (the structured path is unreachable from integration tests — sealed trait).
