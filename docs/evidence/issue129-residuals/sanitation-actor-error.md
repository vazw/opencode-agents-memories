# Issue 129 — startup audit: actor inventory (Step 2b) and captured error text (Step 3d-pre)

Tree: worktree `.tmp/worktrees/issue-129`, base `eda2efd`, branch `issue-129/wave-residuals`.
Everything below is **MEASURED** on the PRE-CHANGE tree unless marked READ.

## Step 2b — production DB actor inventory

Method: the real `eda2efd` binary (`--mode=standalone`, real dist shell, real token gate) run against
disposable roots; the storage root listed recursively before/after, and the child's structured stderr
events captured. Script: `.tmp/scratch/inventory-run.sh`; raw output
`.tmp/progress/r9-shard-A/inventory-run.txt`.

| # | Store / path class | Known actor (READ) | Measured at startup | Fixture disposition |
|---|---|---|---|---|
| 1 | `episodic.db` | `EpisodicStore::ready`, `composition.rs` (eager) | **created** on a fresh root; **found** on restart; composition fails on a poisoned directory | 3a Found, 3b Created, 3c Refused (real child fixtures) |
| 2 | `user-profiles.db` | `ProfileStore::new` (lazy) + `spawn_profile_decay_lifecycle` sweeps once at startup (`engine.rs`) | file appears during the run (decay tick, AFTER composition returned) | audit snapshot at composition time = Unknown `not_opened`; F1/F2 assert exactly that |
| 3 | `user-prompts.db` | `UserPromptManager::new` (lazy) | **not created** | Unknown `not_opened` |
| 4 | `facts.db` | `FactsStore::new`, feature-gated (lazy) | **not created**, flag OFF and flag ON (`promotion_pipeline=true`) | flag OFF → Unknown `disabled`; flag ON → Unknown `not_observed` (`not_opened` pre-fix, aligned 2026-11-06 — review-1 MEDIUM finding; RED `test_audit_facts_flag_on_reports_not_observed` measured rc=101 `left: "not_opened"`, GREEN rc=0 after the one-token fix in `startup_audit.rs` emit) |
| 5 | `skills.db` | `SkillsManager::new` (lazy, opens on request) | **not created** | Unknown `not_opened`; F5 pre-seeds one and asserts nothing opens it |
| 6 | `metadata.db` (shard registry) | `ShardManager` registry, `mem-store/src/ready.rs` readiness path | **not created**: startup never reaches the registry | Unknown `not_observed` |
| 7 | `vault.db` (per-scope `vault/<scope>/vault.db`) | `VaultStore::new` (lazy per scope) | **not created**, no startup scope | Unknown `not_opened` (token is slash-free by design) |
| 8 | registered `*_shard_*.db` | `ShardManager` / production memory port | registry not reached, so no shard is opened at startup | Unknown `not_observed`; **G4 and the shard-sanitizer claim DROPPED** (no per-shard entry, no `opaque_shard_id`) |

`N = entries.len() = 8`. The eighth token is `shard.db`: the dynamic shard file class has no single
file name, and emitting a real registered name would be exactly the leak the audit must not have.

### Measured file sets

Fresh root, default flags, after the port answers (M1 = M2, restart adds nothing):

```
.engine.lock
.wal-mode
decay-sweep.json
episodic.db
episodic.db-tshm
episodic.db-wal
user-profiles.db
user-profiles.db-tshm
user-profiles.db-wal
```

- No `metadata.db`, no `user-prompts.db`, no `facts.db`, no `skills.db`, no `vault/` — matches the
  table above: only `episodic.db` and `user-profiles.db` are opened by a real startup.
- Poisoned `episodic.db` (a directory, M3) after the run: `.engine.lock`, `episodic.db` (still a
  directory), and the one `engine.composition_failed` event; nothing is repaired and no lazy store
  appears.
- Flag-ON run (M4, `{"promotionPipeline":true}`) produced the SAME set: no `facts.db`, so the file-set
  census cannot tell the row apart (F4 dropped for the LISTING check) — but the emitted TOKEN is
  observable and now pinned by `test_audit_facts_flag_on_reports_not_observed` (`unknown`/`not_observed`,
  aligned 2026-11-06 per the Step 2b matrix wording; see row 4 above).
- The audit itself adds no file: the same sets hold on the GREEN tree (Step 3e F1/F2/F5).

F1 asserts the run's file set EQUALS the recorded set. Inside the test harness one more path appears,
`opencode-mem.log`, because `spawn_engine_inner` points `OPENCODE_MEM_LOG_FILE` into the root; the test's
expected set is the measured set plus that harness-owned sink, documented at the constant.

Every listing entry is classified: `episodic.db` (row 1), `user-profiles.db` (row 2), their
`-wal`/`-tshm` sidecars (store infrastructure of rows 1–2), `decay-sweep.json` (mem-profile decay stamp),
`.engine.lock` (engine lifetime lock), `.wal-mode` (WAL-mode marker), `opencode-mem.log` (harness log
sink). No entry is an unclassified store.

## Step 3d-pre — real captured composition error (branch O1)

Why in-process: the raw text goes to `tracing::warn!(detail, …)` (`engine.rs`, target `mem_bridge::engine`)
and the only global subscriber is `mem_core::logging::SinkLayer`, whose `on_event` returns early unless
`target() == "mem_core::logging"`. The child's stderr therefore carries only
`{"component":"engine","detail_short":true,"event":"engine.composition_failed","level":"warn"}` —
observed in the 3c/3d RED transcripts. Outcome: **error text not observable by child** (always true).

Capture: helper `captured_composition_error(root, home)` in `tests/production_api_wiring.rs` calls
`mem_bridge::composition::build_composed_runtime(&root, &config, Arc::new(NoopEmbedder), None)` on the
same deterministic hostile root the child is later launched on, and returns the `Err(String)` verbatim.

Fixture root: `<scratch>/HOSTILE_MARK_7f3a/root-<real newline><real tab>-x`, `episodic.db` a directory.

Captured text (Linux, one run, verbatim, `\n`/`\t` shown as escapes):

```
episodic store init failed: Refusing to open database <scratch>/HOSTILE_MARK_7f3a/root-\n\t-x/episodic.db: the WAL-mode probe or the `.wal-mode` marker could not determine whether another opener holds this database (errno Some(21)); the guard refuses rather than guess. Set OPENCODE_MEM_ALLOW_UNGUARDED_OPEN=1 (or the config key allowUnguardedOpen) to bypass the mixed-version guard
```

Contains (recorded by inspection of the value above):

| part | present |
|---|---|
| marker `HOSTILE_MARK_7f3a` | yes |
| full parent path (from the harness scratch root) | yes |
| basename `root-<NL><TAB>-x` | yes |
| a real newline | yes |
| a real tab | yes |

⇒ **O1**: the captured text contains the path and the marker, so it IS the G2 oracle. G2 asserts that no
12-byte window of the text after the fixed prefix `episodic store init failed: ` appears in any audit
string. The fixed prefix is excluded because it is our own text, not the store's. O2/O3 do not apply
(the capture succeeded and carries distinctive bytes). Gate item 5 holds as well: even a generic OS
error would discriminate as long as its bytes share no 12-byte run with the hostile marker — here the
text also shares long path runs with the G1 mutant, which is why G1 and G2 overlap on the path.

The capture is read-only for the target: the poisoned directory is still an empty directory afterwards
(asserted in S1).

## RED/GREEN

Transcripts: `.tmp/progress/r9-shard-A/red/` (pre-change) and `.../green/` (post-change); mutation
transcripts in `sanitation-mutations.md`. All five REDs are "event absent" with their prerequisite
proven in the same transcript:

| test | RED (pre-change) | GREEN |
|---|---|---|
| `test_audit_classifies_found_missing_and_refused` | rc=101, no `storage.root_audit`; stderr shows `consolidation.startup_tick` = composition succeeded | rc=0 |
| `test_audit_is_total_over_expected_list` | rc=101, same prerequisite | rc=0 |
| `test_audit_refuses_when_root_unreadable` | rc=101, no event; stderr shows `engine.composition_failed` | rc=0 |
| `test_startup_audit_sanitizes_names` | rc=101 at S2, no event; `engine.composition_failed` present; S1 printed the captured text | rc=0 |
| `test_startup_audit_inventory` | rc=101, no event on F1 | rc=0 (after the expected set gained the harness log sink) |
| `test_audit_facts_flag_on_reports_not_observed` (rework) | rc=101, `left: String("not_opened") right: "not_observed"` (`.tmp/progress/r9b-rework/f5-red.log`) | rc=0 (`.tmp/progress/r9b-rework/f5-green.log`) |
| `test_audit_dangling_symlink_at_episodic_is_unknown` (rework) | GREEN-first (the tree's guard is correct); mutation proof = M2 rc=101 `left: "created" right: "unknown"` (`.tmp/progress/r9b-rework/m2-mutant.log`) | rc=0 (`.tmp/progress/r9b-rework/f1-dangling-green.log`) |
| `test_audit_escaping_symlink_at_episodic_is_refused` (rework) | explicitly NOT an M2 oracle (collinear — the store refuses through the link under both stat calls) | rc=0 (`.tmp/progress/r9b-rework/f1-escaping-green.log`) |
