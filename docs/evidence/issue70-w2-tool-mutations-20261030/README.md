# Issue 70 wave 2 — mutation battery (2026-10-30)

One mutant per claimed guard of the blueprint's 20-row battery (`bp-20261030-0740-issue70-w2-memory-tool-r4`,
step 12). Each log records the command, the full output and the exit code. A mutant whose oracle stayed
green would be a vacuous guard; every row below exited non-zero.

Driver (gitignored, not shipped): `.tmp/scripts/issue70_w2_mutants.mjs`.

| id | mutant | oracle | files | exit |
|---|---|---|---|---|
| M01 | `mode-method-swapped` | `test_memory_tool_routes_add_to_the_add_method` | plugin/src/tools.js | 1 |
| M02 | `scope-default-dropped` | `test_memory_tool_routes_search_with_the_resolved_scope` | plugin/src/tools.js | 1 |
| M03 | `scope-always-project` | `test_memory_tool_routes_search_with_the_resolved_scope` | plugin/src/tools.js | 1 |
| M04 | `forget-scope-dropped` | `test_memory_tool_routes_forget_with_the_scope_rule` | plugin/src/tools.js | 1 |
| M05 | `project-from-model` | `test_memory_tool_compares_the_model_project_and_sends_the_host_directory_raw` | plugin/src/tools.js | 1 |
| M06 | `project-resolved` | `test_memory_tool_compares_the_model_project_and_sends_the_host_directory_raw` | plugin/src/tools.js | 1 |
| M07 | `scope-check-order-swapped` | `test_memory_tool_fails_closed_without_a_host_project` | plugin/src/tools.js | 1 |
| M08 | `session-from-args` | `test_memory_tool_reads_the_session_from_the_call_context` | plugin/src/tools.js | 1 |
| M09 | `session-check-dropped` | `test_memory_tool_requires_a_call_session` | plugin/src/tools.js | 1 |
| M10 | `capability-gate-dropped` | `test_tool_request_never_sends_without_the_capability` | plugin/src/tools.js | 1 |
| M11 | `capability-gate-inverted` | `test_tool_request_never_sends_without_the_capability` | plugin/src/tools.js | 1 |
| M12 | `handshake-gate-dropped` | `test_tool_request_gates_on_the_handshake_before_the_capability` | plugin/src/tools.js | 1 |
| M13 | `wire-reason-dropped` | `test_memory_tool_maps_request_failures_to_values` | plugin/src/tools.js | 1 |
| M14 | `profile-empty-content-is-read` | `test_memory_tool_reads_and_writes_the_profile_by_the_presence_of_content` | plugin/src/tools.js | 1 |
| M15 | `deleted-zero-is-failure` | `test_memory_tool_routes_forget_with_the_scope_rule` | plugin/src/tools.js | 1 |
| M16 | `portability-mode-routed` | `test_memory_tool_returns_the_capability_no_op_for_the_four_portability_modes` | plugin/src/tools.js | 1 |
| M17 | `help-requires-a-session` | `test_memory_tool_help_lists_the_routed_and_unavailable_modes` | plugin/src/tools.js | 1 |
| M18 | `unknown-mode-falls-through` | `test_memory_tool_rejects_an_unknown_mode` | plugin/src/tools.js | 1 |
| M19 | `wire-data-dropped` | `test_memory_tool_surfaces_the_wire_reason_over_the_bridge` | plugin/src/plugin.js, plugin/dist/plugin.js | 1 |
| M20 | `malformed-result-accepted` | `test_memory_tool_maps_a_malformed_result_to_engine_unavailable` | plugin/src/tools.js | 1 |

**Totals:** 20 mutants, 20/20 RED (exit non-zero), 0 vacuous.

Restore discipline: every mutated file was snapshotted by content (never `git checkout --`, the wave is
uncommitted) under `.tmp/scratch/issue70_w2_mutants/snapshots/`, copied back after each mutant, and verified
against `.tmp/scratch/issue70_w2_mutants/pre.sha256`:

```
pre.sha256:
plugin/src/tools.js: OK
plugin/src/plugin.js: OK
plugin/dist/plugin.js: OK
```

M19 patches **both** `plugin/src/plugin.js` and `plugin/dist/plugin.js`: the e2e harness stages `dist/` and
the driver imports the staged `dist/plugin.js`, so a src-only revert would leave T22 green and the guard
vacuous. M07 is a two-replacement mutant (the host-scope guard moved after the `scope_rejected` check); its
single log carries one exit code.
