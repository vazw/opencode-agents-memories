# Issue 43 W6a — flag drop + open-guard inversion (evidence)

Blueprint: `.tmp/blueprints/blueprint-20261108-w6-erasure-r1.md` (`sha256=1c8ab8d40e11`),
W6a only (Steps 0–7). Branch `issue-43/w6a-substrate`, base
`0e40770920a1f542a09e2fbae14d176e0bc96a09` (issue-130 wave landed; no rebase).

Every figure below was produced by the command shown, in this worktree, on
2026-11-09.

## 1. The atomic pair

| leg | change |
|---|---|
| flag drop | `ConnectOptions::for_config` returns `[Encryption]` on **every** platform (`crates/mem-store/src/db.rs`) |
| table inversion | `open_guard::decide` refuses `Multiprocess` (`RefuseUnsupportedMode`), probes the `-tshm` sensor for a writable `Legacy` open, and records `Divergence::NoProbeOnPlatform` where no probe exists |

Neither alone is correct: the flag drop alone refuses every open
(`UnsupportedOpenMode { mode: "legacy" }` — the brick), and the inversion alone
leaves a `Multiprocess` open that ignores a pre-drop holder (the silent no-op).
The pairing is pinned by
`crates/mem-store/tests/open_guard.rs::test_post_flip_open_succeeds_refuses_a_flag_on_holder_and_clears_when_free`.

## 2. RED (measured before the inversion)

Three commits, and the red→green window is verified at the commits themselves,
not only in the working tree: a throwaway worktree at `3db778d` (C1) was run
with `CARGO_TARGET_DIR` pointed at this worktree's target dir, then checked out
to `8b44e56` (C2) and re-run.

| commit | `--test open_guard` | `--lib` |
|---|---|---|
| `3db778d` test(mem-store) — C1 | 16 passed / **11 failed** / 1 ignored | 47 passed / 0 failed (pre-inversion suite) |
| `8b44e56` feat(mem-store) — C2 | **27 passed** / 0 failed / 1 ignored | **48 passed** / 0 failed |

C1's `--lib` run is green because the crate-internal test modules live inside
`db.rs`/`open_guard.rs` and therefore ship in C2: the 4 lib REDs recorded below
are an **intra-commit** red→green (they cannot be staged apart from the code
they test without splitting single files), while the 11 integration REDs are
the inter-commit window above.

`cargo test -p mem-store --test open_guard` at C1 → **16 passed / 11 failed / 1 ignored**

| victim | reason (pre-inversion world) |
|---|---|
| `test_multiprocess_connect_options_are_refused_with_unsupported_open_mode` | a multiprocess set opened successfully |
| `test_production_connect_options_open_and_write_without_the_multiprocess_token` | `for_config` classified `Multiprocess`, want `Legacy` |
| `test_purge_session_option_set_is_the_legacy_production_mode` | same classifier |
| `test_config_key_bypasses_the_guard_for_a_multiprocess_open` | the multiprocess set opened with no override |
| `test_mode_marker_is_written_in_the_storage_root_and_absent_from_the_database` | marker body carried `"mode":"multiprocess"` |
| `test_guarded_open_leaves_a_stale_tshm_byte_identical` | a legacy open created a `-tshm` |
| `test_the_tshm_sensor_does_not_see_this_process_own_legacy_handle` | probe against the `.db` read `Held` |
| `test_connection_manager_surfaces_the_typed_refusal_unchanged` | the open succeeded, want `OpenBlocked` |
| `test_factory_open_refuses_a_live_multiprocess_holder` | the multiprocess open ignored the `-tshm` holder |
| `test_post_flip_open_succeeds_refuses_a_flag_on_holder_and_clears_when_free` | same (the no-op mutant) |
| `test_a_second_process_legacy_open_is_refused_by_the_engine_file_lock` | the guard refused; post-drop the engine's own `.db` lock must |

`cargo test -p mem-store --lib` → **43 passed / 4 failed**

| victim | reason |
|---|---|
| `test_decision_table_covers_mode_and_writability` | `Multiprocess` + `None` → `Refuse(Unknown{None})`, want `RefuseUnsupportedMode` |
| `test_decision_table_read_only_skips_the_legacy_probe_and_records_the_divergence` | `Legacy` read-only → `RefuseUnsupportedMode`, want `Clear{ReadOnlySkipsLegacyProbe}` |
| `test_wal_mode_classifier_reads_the_multiprocess_feature` | `for_config` still carried the token on Unix |
| `encryption_migrator::tests::test_encryption_probe_propagates_a_guard_refusal` | `[Encryption]` → `UnsupportedOpenMode { mode: "legacy" }` |

`cargo test -p mem-bridge --test purge_cli` (before the `ready()` mapping) →
`test_purge_busy_names_the_engine_holding_the_storage_root` failed on exit 74 vs 75.

## 3. GREEN (measured after the inversion)

| command | result |
|---|---|
| `cargo test -p mem-store` | **51 suites, 431 passed, 0 failed, 16 ignored** |
| `cargo test -p mem-store --test open_guard` | 27 passed, 1 ignored (`fixture_child_entry`) |
| `cargo test -p mem-store --lib` | 48 passed |
| `cargo test -p mem-bridge --test purge_cli` | 29 passed |
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --lib -- -A clippy::all -D clippy::unwrap_used -D clippy::expect_used` | exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings …` | exit 0 |

The 16 ignored tests are the `#[ignore]` measurement harnesses
(`issue53_probe.rs`, `issue53_vault_probe.rs`) plus the re-exec'd fixture children —
compile-only, not part of the suite count.

## 4. Sensor facts pinned by the new tests

- `test_the_guard_queries_the_tshm_sensor_twice_and_never_the_database`
  (`crates/mem-store/src/db.rs`): a writable legacy open probes the `-tshm` path
  exactly twice (preflight + postflight) and never `db_path`.
- `test_the_tshm_sensor_separates_a_flag_on_holder_from_a_flag_off_one`: a flag-on
  child reads `Held`, a flag-off child reads not-`Held` (the two-arm rule).
- `test_a_pre_drop_multiprocess_database_opens_and_keeps_its_rows`: a pre-drop
  database plus a stale `-tshm` opens under the post-drop build with its rows.
- `test_guarded_open_leaves_a_stale_tshm_byte_identical`: 0-byte and 86 016-byte
  debris is untouched by the probe and by a guarded open; absent stays absent.

## 5. Consequences found while greening (code, not test patches)

- `crates/mem-store/src/migration_service.rs`: the staged re-embed handle stayed
  alive past `close_connection`, so the swap's exclusivity probe read `Held`
  against this process's own descriptor (OFD locks are per open file
  description). Fixed by dropping the `Arc` before the swap — the mutation guard
  was right, the handle was stale.
- `crates/mem-bridge/src/purge.rs`: `store.ready()` now maps a lock-shaped failure
  to `PurgeError::Busy` (exit 75); every other readiness failure stays exit 74.
- Test fixtures that required a `-tshm` from a shipping open now build the
  pre-drop option set explicitly with a raw `turso::Builder`
  (`turso_capabilities.rs` arm B, `sidecar_deletion_contract.rs` holder child).
- `tests/production_api_wiring.rs::test_startup_audit_inventory` pins the
  measured fresh-root file set of a default-flag run. Post-drop the set has no
  `-tshm` entries, so the pin was re-measured (10 → 8) rather than kept:

  ```
  left:  [".engine.lock", ".wal-mode", "decay-sweep.json", "episodic.db",
          "episodic.db-wal", "opencode-mem.log", "user-profiles.db",
          "user-profiles.db-wal"]
  right: [.., "episodic.db-tshm", .., "user-profiles.db-tshm", ..]
  ```

  The premise that died is "every default-flag run leaves a `-tshm` per
  database", which is exactly the flag drop; the test's own claim (the audit
  creates no file beyond the real actors) is unchanged and still asserted.

## 6. Ledger

`.tmp/progress/blueprint-20261108-w6-r1.ledger.md` (gitignored; the wave-close
summary in the handoff carries the same figures).
