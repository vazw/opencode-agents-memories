# Issue 70 wave 3 — mutation battery (2026-10-30)

One mutant per claimed guard of the blueprint's 20-row battery
(`bp-20261030-1053-issue70-w3-portability-tool-r8`, step 10). Each log records the
command(s), the full output and the exit code. A mutant whose oracle stayed green would be
a vacuous guard; every row below exited non-zero. M19 runs two oracles (T15 and T22) and
both exited non-zero.

Driver (gitignored, not shipped): `.tmp/scripts/issue70_w3_mutants.mjs`.

| id | mutant | oracle(s) | files | exit |
|---|---|---|---|---|
| M01 | `list-shards-sends-project-id` | `test_memory_tool_routes_list_shards_with_the_host_project_path` | plugin/src/tools.js, plugin/dist/tools.js | 1 |
| M02 | `migrate-sends-project-id` | `test_memory_tool_routes_migrate_with_present_key_selection` | plugin/src/tools.js, plugin/dist/tools.js | 1 |
| M03 | `migrate-source-selectors-dropped` | `test_memory_tool_routes_migrate_with_present_key_selection` | plugin/src/tools.js, plugin/dist/tools.js | 1 |
| M04 | `migrate-dry-run-dropped` | `test_memory_tool_routes_migrate_with_present_key_selection` | plugin/src/tools.js, plugin/dist/tools.js | 1 |
| M05 | `migrate-allow-linked-source-dropped` | `test_memory_tool_routes_migrate_with_present_key_selection` | plugin/src/tools.js, plugin/dist/tools.js | 1 |
| M06 | `export-path-dropped` | `test_memory_tool_routes_export_with_the_path_verbatim` | plugin/src/tools.js, plugin/dist/tools.js | 1 |
| M07 | `import-path-dropped` | `test_memory_tool_routes_import_with_the_dry_run_flag` | plugin/src/tools.js, plugin/dist/tools.js | 1 |
| M08 | `import-dry-run-dropped` | `test_memory_tool_routes_import_with_the_dry_run_flag` | plugin/src/tools.js, plugin/dist/tools.js | 1 |
| M09 | `export-local-path-guard-added` | `test_memory_tool_routes_export_with_the_path_verbatim` | plugin/src/tools.js, plugin/dist/tools.js | 1 |
| M10 | `portability-local-no-op-restored` | `test_memory_tool_routes_export_with_the_path_verbatim` | plugin/src/tools.js, plugin/dist/tools.js | 1 |
| M11 | `session-fallback-for-portability` | `test_memory_tool_requires_a_call_session_for_portability_modes` | plugin/src/tools.js, plugin/dist/tools.js | 1 |
| M12 | `export-scope-hash-dropped` | `test_memory_tool_routes_export_with_the_path_verbatim` | plugin/src/tools.js, plugin/dist/tools.js | 1 |
| M13 | `import-notes-stripped` | `test_memory_tool_routes_import_with_the_dry_run_flag` | plugin/src/tools.js, plugin/dist/tools.js | 1 |
| M14 | `migrate-actions-dropped` | `test_memory_tool_routes_migrate_with_present_key_selection` | plugin/src/tools.js, plugin/dist/tools.js | 1 |
| M15 | `list-shards-camel-renamed` | `test_memory_tool_routes_list_shards_with_the_host_project_path` | plugin/src/tools.js, plugin/dist/tools.js | 1 |
| M16 | `list-shards-malformed-accepted` | `test_memory_tool_maps_a_malformed_result_to_engine_unavailable` | plugin/src/tools.js, plugin/dist/tools.js | 1 |
| M17 | `help-unavailable-stale` | `test_memory_tool_help_lists_the_routed_and_unavailable_modes` | plugin/src/tools.js, plugin/dist/tools.js | 1 |
| M18 | `capability-noop-third-key-dropped` | `test_memory_tool_call_reaches_the_engine_as_the_routed_methods` | plugin/src/tools.js, plugin/dist/tools.js | 1 |
| M19 | `reason-whitelist` | `test_memory_tool_maps_request_failures_to_values`, `test_memory_tool_surfaces_the_wire_reason_over_the_bridge` | plugin/src/tools.js, plugin/dist/tools.js | 1/1 |
| M20 | `args-spread` | `test_memory_tool_routes_import_with_the_dry_run_flag` | plugin/src/tools.js, plugin/dist/tools.js | 1 |

**Totals:** 20 mutants, 20/20 RED (every oracle exit non-zero), 0 vacuous.

Restore discipline: both twins were snapshotted by content (never `git checkout --`, the
wave is uncommitted) under `.tmp/scratch/issue70_w3_mutants/snapshots/`, copied back after
each mutant, and verified against `.tmp/scratch/issue70_w3_mutants/pre.sha256`:

```
/home/vaz/Code/Personal/Rust/opencode-agents-memory/.tmp/worktrees/issue-70-w3-portability-tool/plugin/src/tools.js: OK
/home/vaz/Code/Personal/Rust/opencode-agents-memory/.tmp/worktrees/issue-70-w3-portability-tool/plugin/dist/tools.js: OK
```

Every mutant patches **both** `plugin/src/tools.js` and `plugin/dist/tools.js`: the e2e
harness stages `dist/` and the driver imports the staged `dist/plugin.js`, so a src-only
mutant would leave T21/T22 green and the guard vacuous.
