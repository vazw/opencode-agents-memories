# Architect A/B Draft: Lossless Consolidation of `/home/vaz` Shards into `/home/opencode` & Reconciliation of Phase C (`20261110-2337` Steps 9–13)

- **Date:** 2026-11-11
- **Status:** Draft (`NEEDS-USER` — competing designs unmeasured; operator policies unresolved)
- **Workspace / Host (`MEASURED`):** `uid=1002(opencode)`, repo `/home/opencode/Code/Personal/Rust/opencode-agents-memory`, live engine PID `18731` (ppid `17538`) running `plugin/engine/linux-x64/opencode-agents-memory --storage-root /home/opencode/.opencode-mem/data` holding `.engine.lock` (`operation="engine"`, `start_time=4386624`) and `.wal-mode` (`mode="legacy"`).
- **Repo Dirtiness (`MEASURED`):** `git status --short` shows only pre-existing `M .cargo/config.toml`, `M AGENTS.md`, `M crates/mem-web/src/api/deduplication/mod.rs`. Zero tracked files or live DBs were modified.
- **Upstream Seam Guard (`READ`):** Preserves `plugin/` `PluginModule` (`{ id?, server, tui?: never }`) and `mem-web` REST API contracts (`AGENTS.md`, ADR-0003, ADR-0007). Engine CLI (`src/main.rs`), stdio bridge, and Turso internals are repo-owned (`AGENTS.md`).

---

## 1. Verified Pre-Conditions & Why Existing Mechanisms Alone Are Insufficient

### 1.1 Live Storage Root State (`/home/opencode/.opencode-mem/data`)
- **Active shard inventory (`MEASURED` via live `memory list-shards` `all-projects`):** `37` project shard groups total (`current: 1`, `linked: 5`, `orphaned: 31`, `empty: 0`, `missingFile: 0`, `ambiguous: 0`):
  - `3` non-`/home/vaz` shards:
    - `fd2bfb63318dfd29` (`41` memories, `current`, `/home/opencode/Code/Personal/Rust/opencode-agents-memory`, remote `git@github.com:vazw/opencode-agents-memories.git`)
    - `60622035863a6bab` (`2` memories, `linked`, `/home/opencode/Code/Work/antwall`, remote `git@gitlab.bangmod.cloud:xdp/antwall.git`)
    - `2991cca9ba1ab013` (`4` memories, `linked`, `/tmp/opencode`, remote `null`)
  - `34` `/home/vaz` shards (`31` `orphaned`, `3` `linked`).
- **Uncheckpointed WAL & sidecar footprint (`MEASURED` via `ls -la`):**
  - `metadata.db` `28672 B` + `-wal` `3510272 B` + `-shm` + `-tshm`
  - `episodic.db` `11423744 B` + `-wal` `2962312 B` + `-shm` + `-tshm`
  - `user-profiles.db` `10375168 B` + `-wal` `2855192 B` + `-shm` + `-tshm` (plus `.schema-v1-*.bak*`)
  - `user-prompts.db` `1552384 B` + `-wal` `16512 B` + `-shm` + `-tshm`
  - `skills.db` `24576 B` + `-wal` `32 B` + `-shm` + `-tshm`
  - `ai-sessions.db` `45056 B` + `-wal` `0 B` + `-shm` (no `-tshm`)
  - `.profile-learning-coordination.db` `12288 B` + `-wal` `0 B` + `-shm`
  - Shard `.db` files under `projects/`: e.g. `fd2bfb63318dfd29` (`db` `4096 B` + `-wal` `2117712 B`, no `-shm`/`-tshm`), `60622035863a6bab` (`db` `4096 B` + `-wal` `189552 B`, no `-shm`/`-tshm`), `4182e0858e0c93a1` (`db` `19357696 B` + `-wal` `1170112 B` + `-shm` + `-tshm`), `e266c5689d45bee3` (`db` `10010624 B` + `-wal` `3563832 B` + `-shm` + `-tshm`), `3d5f981354135289` (`db` `4145152 B` + `-wal` `1870512 B` + `-shm` + `-tshm`).
  - **Absent tiers (`MEASURED`):** `facts.db`, `vault/`, and `vault-keys/` do **not** exist in `/home/opencode/.opencode-mem/data/`.
- **Why live `sqlite3` or naive `cp` is unsafe (`READ`):** `crates/mem-store/src/db.rs` opens every DB in legacy WAL mode (`open_guarded` probing `-tshm`); live PID `18731` holds uncheckpointed `-wal` state across `metadata.db`, `episodic.db`, `user-profiles.db`, and shards. Running `sqlite3` against live DBs or copying `.db` without its `-wal`/`-shm`/`-tshm` corrupts or loses recent writes.

### 1.2 Why Existing `PathMigrationService` and `PortabilityService` Both Fail Lossless Consolidation
1. **`PathMigrationService` (`crates/mem-store/src/shard_path_migration.rs`, `READ`):**
   - Refuses whenever the target already has any shard (`"Target project already has memories; aborting path migration to prevent overwrite."`), blocking `4182e0858e0c93a1 -> fd2bfb63318dfd29` (`41` rows), `e266c5689d45bee3 -> 60622035863a6bab` (`2` rows), and the 2-to-1 `.config/opencode` merge (`3d5f981354135289` + `b7a8895b5cdfe1b9` -> `6e64d9c620f21fbf`).
   - Refuses `path_exists && !allow_linked_source` (blocks `/home/vaz`, `/home/vaz/.config/kitty`, `/home/vaz/.config/nvim` unless opted in).
   - Updates `user_prompts.project_path` (`PromptStorePathUpdater` in `crates/mem-bridge/src/tools/inventory.rs:54,78-108` -> `mem-profile/src/prompt.rs:773-797`), **but does not rewrite** `episodic.db` (`project_id`) or `skills.db` (`container_tag`), nor check/guard `facts.db` / `vault/`.
   - Uses `TagResolver::project_tag_info(target_path)` (`crates/mem-core/src/tags/mod.rs:147-176`), which falls back to `path:<dir>` when `<dir>` does not exist on disk — **quietly minting a wrong `path:` hash instead of `git-common:` for uninstalled git repos**.
2. **`PortabilityService` + `ProductionFacts` (`crates/mem-portability/src/{bundle,service}.rs`, `crates/mem-web/src/api/store_adapter.rs:1433-1544`, `src/portability_backend.rs:88-99`, `READ`):**
   - `exported_memory` (`store_adapter.rs:1433-1454`) calls `strip_private_content` (destructive redaction of `<private>...</private>`) and drops `is_fully_private` rows completely.
   - `ExportedMemory` (`bundle.rs:121-168`) omits `vector`, `tags_vector`, `session_id`, and `source_prompt_created_at`; `ProductionFacts::insert_memory` (`store_adapter.rs:1521-1544`) hardcodes `session_id: None`, `source_prompt_created_at: None`, and forces re-embedding through `embedder`.
   - `ProductionPortabilityBackend::import` (`src/portability_backend.rs:96`) hardcodes `ReplaceMode::Merge` (aborts on any duplicate ID), and `PortabilityService::export` takes a live `Path` rather than a `scope_hash`.
   - **Conclusion (`READ`):** `20261110-2337` Phase C `Step 10`'s `Portability` forget+reimport plan is **lossy** and cannot be used for consolidation.

---

## 2. Source-Read Storage Schemas & Cross-DB Touchpoints (`READ`; not live-schema verification)

| Store / File | Exact Table(s) & Key Columns (`READ`) | Scope / Path Binding | Required Consolidation Action |
|---|---|---|---|
| Project Shard DBs (`projects/project_{16hex}_shard_{n}.db`) | `shard_metadata(key PK, value)`; `shard_stats(id=1, vector_count)`; `memories(id PK, content, vector F32_BLOB(dims), tags_vector F32_BLOB(dims), container_tag, tags, type, created_at, updated_at, metadata, display_name, user_name, user_email, project_path, project_name, git_repo_url, is_pinned, session_id, source_prompt_created_at)` (`sql.rs:14-103`, `shard_facts.rs:40`, `shard_retention_sql.rs:5`) | Filename `project_{scope_hash}_shard_{n}.db`; row columns `container_tag` (`{prefix}_project_{hash}`), `project_path`, `project_name`, `display_name`, `git_repo_url` | Copy/merge all 20 columns verbatim (no vector re-embed, no `<private>` strip), rewrite 5 association columns (`container_tag`, `project_path`, `project_name`, `display_name`, `git_repo_url`), sync `shard_stats`, enforce schema `v4` (`MEMORY_SHARD_MAX_KNOWN=4`) |
| `metadata.db` | `shards(id PK, scope, scope_hash, shard_index, db_path, vector_count, is_active, created_at, UNIQUE(scope, scope_hash, shard_index))` + `idx_active_shards` + partial unique active index (`sql.rs:23-40`, `shard_manager.rs:520`) | `scope='project'`, `scope_hash`, `db_path='projects/...'`, `is_active`, `vector_count` | Upsert target shard row(s) with exact `COUNT(*)` (`vector_count`), ensure exactly 1 `is_active=1` per `(scope, scope_hash)`, delete/archive retired source registry entries atomically |
| `user-prompts.db` | `user_prompts(id PK, session_id, message_id, project_path, content, created_at, captured, user_learning_captured, linked_memory_id, capture_attempts, provider_id, model_id)` (`sql.rs:44-57`, `user_prompts_sql.rs:17-21`) | `project_path` normalized with `/` (`mem-profile/src/prompt.rs:789`) | Rewrite `project_path` from normalized source path to target path (`WHERE REPLACE(project_path, '\', '/') = ?`) — **note:** handle 2-to-1 sources (`/home/vaz/.config/opencode` once covers both hashes) and any sub-paths policy |
| `episodic.db` | `agent_sessions(id PK, project_id, session_id, agent_id, parent_session_id, summary, metadata, embedding F32_BLOB(dims), created_at, updated_at, deleted_at, orphaned, capture_id, agent_role, UNIQUE(project_id, session_id, agent_id))`; `episodic_milestones(id PK, project_id, agent_id, ...)`; `episodic_raw_context(PK(project_id, session_id, agent_id))`; `episodic_segments(PK(project_id, session_id, agent_id, segment_index))`; `capture_inbox(capture_id PK, project_id, session_id, agent_id, ...)` (`episodic_sql.rs:20-219`) | Keyed by `project_id` (**encoding unverified**; do not assume hash, tag, or path. Settle using caller tracing plus isolated-copy values before constructing any remap) | Remap `project_id` across all 5 tables in one `BatchMode::Write` tx, aborting before mutation on divergent `UNIQUE(project_id, session_id, agent_id)` / PK collisions, without deleting `episodic_raw_context` (`episodic_sql.rs:95`) |
| `skills.db` | `skills(id PK, name, description, content, autoinvoke, enabled, container_tag, created_at, updated_at)` (`mem-skills/src/schema.rs:32-42`) | Nullable `container_tag` (`{prefix}_project_{hash}` or global `NULL`) | `UPDATE skills SET container_tag = ? WHERE container_tag = ?` |
| `user-profiles.db` & `.profile-learning-coordination.db` | `user_profiles(id PK, user_id UNIQUE, ...)`; `user_profile_changelogs(...)`; `profile_learning_lock` (`sql.rs:143-165`, `learning_lock.rs:58`) | Keyed by `user_id` (user email/name hash, **not** project `scope_hash`) | Read-only check; do **not** rewrite keys. (Inspect whether `profile_data` JSON contains stale `/home/vaz` paths only if user-requested; coordination lock untouched) |
| `ai-sessions.db` | Legacy filename referenced by `legacy_migrator.rs:961`; **exact tables/columns unverified** (the new episodic store is separate) | Legacy `45056 B` file on disk (`MEASURED`) | Snapshot unchanged; inspect exact schema and scope bindings on an isolated copy before declaring completeness. Abort cutover if an affected binding is unsupported; do not silently omit this DB |
| `facts.db` (`mem-facts/src/schema.rs:30-63`) | `facts(id PK, project_id, tier, content, key_normalized, embedding, ..., UNIQUE(project_id, tier, key_normalized))`; `fact_provenance(PK(fact_id, project_id, session_id, agent_id))` | `id = "fact-" + sha256(project_id \| tier \| key)[:32]` (content-keyed on `project_id`!) | **GUARD / FAIL-CLOSED:** Currently absent (`MEASURED`). If `facts.db` exists and holds rows for source `project_id`, **REFUSE** automatic consolidation unless explicit fact re-keying/collision policy is implemented (`fact_id` changes when `project_id` changes) |
| `vault/{scope}/vault.db` & `vault-keys/` (`mem-vault/src/schema.rs:39-65`) | `vault_entries(content_hash PK, content BLOB, mime_type, source_ref, created_at, created_by)` (`HMAC-SHA256` keyed per scope) | Per-scope DB + external key file in `vault-keys/` | **GUARD / FAIL-CLOSED:** Currently absent (`MEASURED`). `content_hash` is HMAC-keyed per scope key; naive copy breaks decryption/MACs. **REFUSE** if source or target has `vault/{scope}/` or `vault-keys/` entry |

---

## 3. Lossless Merge, Dedup & Collision Semantics (Shared Across Designs)

### 3.1 Full 20-Column `memories` Preservation
Every surviving row in target `memories` MUST preserve verbatim (`READ` `sql.rs:64-103`, `shard_retention_sql.rs:5`, `vector.rs:28-47`):
- Identity & timestamps: `id`, `created_at`, `updated_at`, `session_id`, `source_prompt_created_at` (including `NULL` on pre-v4 rows).
- Content & privacy: `content` verbatim **including** `<private>...</private>` spans and rows where `is_fully_private(content) == true` (never pass through `strip_private_content` or drop).
- Vectors & flags: raw `vector` (`F32_BLOB(dims)`), raw `tags_vector` (`NULL` or `F32_BLOB(dims)`), `tags`, `type`, `metadata`, `is_pinned`, `user_name`, `user_email`.
- **Association rewrite (5 columns, matching `UPDATE_MEMORIES_PROJECT_ASSOCIATION`, `sql.rs:479-482`):**
  - `container_tag` → target `{prefix}_project_{target_hash}` (preserving workspace `container_tag_prefix`, e.g., `opencode`).
  - `project_path` → target root path (string).
  - `project_name` → target `get_project_name(project_root)`.
  - `display_name` → target root `display()`.
  - `git_repo_url` → target canonical origin (`None` when pinned by `.opencode-mem-project`, else target `git.remote_origin_url`; note for `e266c5689d45bee3 -> 60622035863a6bab`, `bangmod:xdp/antwall.git` normalizes to `git@gitlab.bangmod.cloud:xdp/antwall.git`; for `4182e0858e0c93a1 -> fd2bfb63318dfd29`, `null` updates to `git@github.com:vazw/opencode-agents-memories.git`).

### 3.2 `id` Collision & Vector Serialization Rules
1. **Primary-key (`id`) collisions:** Target row and source row(s) share `id` (possible in `4182e0858e0c93a1 + fd2bfb63318dfd29`, `e266c5689d45bee3 + 60622035863a6bab`, and 2-to-1 `3d5f981354135289 + b7a8895b5cdfe1b9`):
   - **Rule:** Normalize only the five approved association columns, then compare every other column, including raw `vector` and `tags_vector` bytes, `NULL` values, metadata, privacy-bearing content, flags, and all timestamps.
     - Exactly identical non-association values → retain one row and record every source occurrence in the private manifest. No flag union or timestamp changes.
     - Any divergent non-association value with the same `id` → **abort the entire plan before mutation**. No timestamp winner, target preference, or dropping a version. Preserving both versions requires a separately approved identity/reference-remapping design; a conflict log alone is not lossless.

2. **Shard capacity & rotation (`ShardManager`):** Target `shard_0` holds `41` (`fd2bfb63318dfd29`) + `2283` (`4182e0858e0c93a1`) = `<= 2324` rows, or `867` (`3d5f981354135289` + `b7a8895b5cdfe1b9`). If combined `COUNT(*)` exceeds `max_vectors_per_shard` (config-dependent), either write into `shard_0, shard_1, ...` respecting `max_vectors_per_shard` with only highest `shard_index` marked `is_active = 1` in `metadata.db` (`ADR-0022`), or write single shard if below threshold. Every touched shard gets `shard_stats` synced to exact `COUNT(*)` in the same write tx (`shard_facts::sync_count_statement`).
3. **Vector preservation (`INFERRED` risk to settle in Discriminating Measurement):** Upstream Turso `F32_BLOB` columns must transfer without losing dimensions or precision and remain queryable via `vector_distance_cos(vector, vector32(?))` (`vector.rs:340-371`). Note: `vector_extract(vector)` -> JSON text -> `vector32(?)` vs raw `BLOB` copy (`INSERT INTO ... VALUES (..., ?)` where bound `Value::Blob` vs `vector32`) **must be measured on actual `turso 0.8` in this repo** before implementation!

---

## 4. Complete 34 `/home/vaz` Shard Inventory & Dispositions (`MEASURED`)

All `34` `/home/vaz` shards verified from live `memory list-shards` (`all-projects`) + filesystem/git probes under `/home/opencode`:

### Category A — Existing `/home/opencode/*` Targets (`11` source shards → `10` distinct target hashes; `4,792` total source memories)
| # | Source Hash | Rows | Live Status | Source Identity (`MEASURED`) | Target Path (`/home/opencode/...`) | Target Hash & Existing Rows (`MEASURED`) | Collision / Special Handling |
|---|---|---|---|---|---|---|---|
| 1 | `4182e0858e0c93a1` | 2283 | `orphaned` | `git-common:/home/vaz/Code/Personal/Rust/opencode-agents-memory/.git` (`remote=null`) | `/home/opencode/Code/Personal/Rust/opencode-agents-memory` | `fd2bfb63318dfd29` (**41**, `current`, `git@github.com:vazw/opencode-agents-memories.git`) | **Populated-target merge**; backfill `git_repo_url` (`null`→origin) |
| 2 | `e266c5689d45bee3` | 1255 | `orphaned` | `git-common:/home/vaz/Code/Work/antwall/.git` (`bangmod:xdp/antwall.git`) | `/home/opencode/Code/Work/antwall` | `60622035863a6bab` (**2**, `linked`, `git@gitlab.bangmod.cloud:xdp/antwall.git`) | **Populated-target merge**; normalize `git_repo_url` |
| 3 | `3d5f981354135289` | 520 | `orphaned` | `git-common:/home/vaz/.config/opencode/.git` | `/home/opencode/.config/opencode` | `6e64d9c620f21fbf` (0 — unsharded) | **2-to-1 merge** (with #4) into `6e64d9c620f21fbf` |
| 4 | `b7a8895b5cdfe1b9` | 347 | `orphaned` | `path:/home/vaz/.config/opencode` (pre-git) | `/home/opencode/.config/opencode` | `6e64d9c620f21fbf` (0 — unsharded) | **2-to-1 merge** (with #3) into `6e64d9c620f21fbf` |
| 5 | `8464b82a892b16cf` | 90 | `orphaned` | `git-common:/home/vaz/Documents/AgentsBrain/.git` | `/home/opencode/Documents/AgentsBrain` | `f28b4a74af055334` (0 — unsharded) | Track 4b (`Steps 12–13`); direct move/retarget + cross-DB sync |
| 6 | `d5784a7283268f72` | 66 | `orphaned` | `path:/home/vaz/Code/Personal/JS/session-talk` | `/home/opencode/Code/Personal/JS/session-talk` | `2b347e22819619db` (0 — unsharded) | Retarget (`path:` → `path:`) |
| 7 | `03b7931d97d0837c` | 63 | `orphaned` | `path:/home/vaz/Code/Personal/VPN-Auto-ReConnect` | `/home/opencode/Code/Personal/VPN-Auto-ReConnect` | `da4d52176999ad20` (0 — unsharded) | Retarget (`path:` → `path:`) |
| 8 | `f14065d44cdb2ed9` | 26 | `orphaned` | `git-common:/home/vaz/Code/Personal/JS/9router/.git` | `/home/opencode/Code/Personal/JS/9router` | `4c8b445c6111c05e` (0 — unsharded) | Retarget (`git-common:` → `git-common:`) |
| 9 | `610dab7607f3fa4e` | 19 | `orphaned` | `path:/home/vaz/Code/Personal` | `/home/opencode/Code/Personal` | `3ddacb8e07061af7` (0 — unsharded) | Retarget (`path:` → `path:`) |
| 10 | `76688b45006c2dc8` | 19 | **`linked`** | `path:/home/vaz` (readable `drwx-----x`) | `/home/opencode` | `26e50c7b19bbf9cf` (0 — unsharded) | **Policy Q1:** linked home dir — requires explicit opt-in vs keep separate |
| 11 | `e9d07245057e86c6` | 4 | `orphaned` | `path:/home/vaz/.config/opencode-cockpit` | `/home/opencode/.config/opencode-cockpit` | `2416a5f68e61f821` (0 — unsharded) | Retarget (`path:` → `path:`) |

### Category B — `/home/opencode/*` Target Missing on Disk, `old = git-common` (`13` shards; `990` rows)
`a004f9ff6f25bf3f` (`267`, `inquisitor`), `56c3912b7371f31b` (`179`, `Code/Personal/JS/opencode`, `anomalyco/opencode.git`), `f72ad766d333a57c` (`172`, `void-packages`), `c4cfb5149f636c54` (`165`, `leninguard`), `4e0bd88bb61068fc` (`157`, `usbgurad-tray`), `ceb74de5e6f0f59b` (`13`, `cleanyourtv`), `1579634fb04f6a2d` (`11`, `infinity`), `4c25e692f265bfe8` (`10`, `hex`), `ff6f33bcd6d45f24` (`9`, `opencode-share`), `21267416e8713a4f` (`4`, `dotfiles/files/opencode`, `git-common:/home/vaz/Code/dotfiles/files/opencode/.git`), `87e13638a743d4ed` (`1`, `antministrator`), `9be94f01845a1c8c` (`1`, `ZCode`), `d43861d6ee66ce05` (`1`, `siamstr`).
- **Critical hazard (`READ` + `MEASURED`):** Because none of these `13` `/home/opencode/...` paths exist yet, calling `TagResolver::project_tag_info` computes `sha256("path:/home/opencode/...")` **instead of** `sha256("git-common:/home/opencode/.../.git")`! When the repo is later cloned/checked out, `git-common:` activates and **re-splits the shard**.
- **Also note #2 (`56c3912b7371f31b`):** `/home/opencode/Code/opencode` **does exist** (`git@github.com:vazw/opencode.git`) while `56c3912b7371f31b` points to `/home/vaz/Code/Personal/JS/opencode` (`https://github.com/anomalyco/opencode.git`). Requires **Policy Q2** (merge into `/home/opencode/Code/opencode` vs keep archived).

### Category C — `/home/opencode/*` Target Missing on Disk, `old = path` (`10` shards; `145` rows: `8` orphaned + `2` linked)
- **`8` orphaned (`125` rows):** `12f3367ad73f86b8` (`57`, `open-kernel-challenge`), `d3bf85216530166c` (`30`, `Scipts`), `cd507a4d1702b687` (`21`, `opencode-askjev`), `5413c71df716ef8b` (`12`, `Documents/OPSEC`), `b6a9ddc693aba750` (`2`, `Note/cv-resume`), `6183486a14210f83` (`1`, `new-siamstr`), `8fbd9ef97319c0dc` (`1`, `.agents`), `fd1a5bd594cec6d2` (`1`, `Note/Note`).
- **`2` linked on `/home/vaz` (`20` rows):** `f0c274e00a11b8eb` (`13`, `/home/vaz/.config/kitty`, `/home/opencode/.config/kitty` absent), `692651b6e29bfb95` (`7`, `/home/vaz/.config/nvim`, `/home/opencode/.config/nvim` absent).

---

## 5. Competing Candidate Designs (`Design A` vs `Design B`)

### Design A — Offline Turso-Native Engine Subcommand (`opencode-agents-memory --mode=consolidate-shards`)
- **Architecture:** Add an offline CLI mode to `src/main.rs` backed by a new `mem_store::shard_consolidation` module + root/bridge cross-DB orchestrator (`mem-store` + `mem-episodic` + `mem-profile` + `mem-skills` + fail-closed `mem-facts`/`mem-vault` presence checks). Requires the live engine (PID `18731`) to be cleanly shut down first.
- **Execution flow:**
  1. **Quiesce & Lock:** Verify live engine absent (`ExclusiveRoot` acquires `.engine.lock`, `OperationLock` acquires `.turso-operation.lock`, verify zero foreign PIDs hold `-tshm` / open FDs on storage root).
  2. **Pre-flight Checkpoint & Backup:** Run `PRAGMA wal_checkpoint(TRUNCATE)` on every touched DB (`metadata.db`, `user-prompts.db`, `episodic.db`, `skills.db`, source & target shard DBs), close handles, copy full `.db` + `-wal`/`-shm`/`-tshm` tuples (`sqlite_files.rs`) + `.wal-mode`/`.turso-migrated` into timestamped `recovery/consolidate-<ts>/` with SHA-256 + row-count manifest. Refuse if `facts.db` or `vault/`/`vault-keys/` has matching scope data.
  3. **Plan & Dry-Run:** Evaluate explicit mapping manifest (`(source_hash -> target_hash, target_path, identity_mode)` — never guessing `path:` for `git-common:` missing repos unless explicit `--synthetic-git-common`), enumerate `memories.id` collisions, `episodic` triple collisions, `user_prompts` matches, `skills` matches, and print JSON plan.
  4. **Atomic Staging & Commit:** Build consolidated target shard file(s) (`project_{target}_shard_{k}.db.tmp`), write v4 schema + copy all 20 columns verbatim with rewritten 5 association columns + synced `shard_stats`, checkpoint & close, swap into place under `.consolidate-journal.json`, then apply `BatchMode::Write` updates to `metadata.db` (`shards`), `user-prompts.db`, `episodic.db`, `skills.db`. Retire/move source shard `.db`+sidecars to `recovery/consolidate-<ts>/retired-shards/`.
  5. **Verify & Rollback:** Re-open via `ShardManager` + `EpisodicStore` + `UserPromptManager`, verify row sums (`target_after == target_before + source - resolved_collisions`), `PRAGMA integrity_check`, `<private>`/pin/`source_prompt_created_at`/vector similarity spot-queries; on mismatch, restore all DB+sidecar tuples from `recovery/consolidate-<ts>/`.
- **Pros:**
  - Zero concurrent writers or in-memory handle-cache races (`ConnectionManager::connections` / `model_facts` cache / `ShardManager::metadata` cannot serve stale handles while DB files swap).
  - Clean `PRAGMA wal_checkpoint(TRUNCATE)` guarantees single-file self-contained backups before mutation; aligns with `--mode=purge-session` / `--mode=reembed-scope` CLI pattern (`src/main.rs:93-95`).
  - Simple crash recovery: offline journal + immutable pre-flight snapshot restores byte-identical DB + sidecars with zero live-session side effects.
- **Cons & Failure Modes:**
  - Requires engine downtime window (stopping PID `18731` / active OpenCode sessions), interrupting ongoing `mem-bridge` auto-capture during consolidation.
  - **Failure mode 1 (Orphaned `ExclusiveRoot` / unclean PID kill):** If an operator `SIGKILL`s PID `18731` instead of graceful shutdown, uncheckpointed `-wal` pages (`3.5 MB` `metadata.db-wal`, `2.9 MB` `episodic.db-wal`) must be replayed safely by Turso on first offline open — if an uncleaned `-tshm` mismatch causes `open_guarded` refusal, recovery stalls until locks/sidecars are inspected.
  - **Failure mode 2 (Split cross-DB partial commit):** Turso has no multi-DB 2PC across `shard.db`, `metadata.db`, `user-prompts.db`, `episodic.db`, `skills.db`; crash mid-apply requires idempotent replay or full `recovery/` restore from `.consolidate-journal.json`.
- **Predicted outcome:** Consolidates all approved `/home/vaz` shards with 0 dropped `<private>` rows, 0 lost `source_prompt_created_at`/`session_id`/`is_pinned`, 0 re-embedding latency, exact `shard_stats == vector_count`, and full cross-DB alignment in a maintenance window whose duration must be measured (`INFERRED` prediction, not a timing result).
- **Refutation:** Falsified if (a) `turso 0.8` cannot copy `F32_BLOB` columns losslessly without re-embedding (`vector_distance_cos` drift > `1e-6` or rejected `F32_BLOB` bind), OR (b) shutting down PID `18731` leaves `.engine.lock` / `-tshm` in a state that blocks offline open or loses `-wal` transactions, OR (c) offline maintenance downtime is operationally rejected for this host.

### Design B — Live Bridge-Integrated `PathMigrationService` + Multi-DB Merge Extension (`memory migrate --merge`)
- **Architecture:** Extend `mem_store::shard_path_migration::PathMigrationService` and `mem-bridge`'s `InventoryService` (`tools/inventory.rs`) + `PortabilityDeps`-style composed seams (`mem-episodic`, `mem-skills`) so the **live running engine** (PID `18731`) performs populated-target merge + multi-DB remap online under `OperationLock` + dual `with_scope_write_lock` (sorted hash order to prevent deadlock). Explicitly rejects `PortabilityService` `ExportedMemory` as the transfer mechanism, using Turso-level row merge inside `mem-store`.
- **Execution flow:**
  1. Engine stays online (`PID 18731` stays alive); client invokes `memory migrate` (or REST/stdio `consolidate` command) with explicit `source_hash` + `target_path` + `--merge` + `--allow-linked-source`.
  2. Engine acquires `.turso-operation.lock` (`OperationLock`) and acquires `ShardManager::with_scope_write_lock` for `(source_hash, target_hash)` in lexicographical lock order.
  3. Flushes `ConnectionManager::close_connection` for source and target shard DBs, snapshots shard `.db`+sidecars + auxiliary DBs into `recovery/`, builds `.tmp` merged shard copying all 20 columns, checkpoints & swaps via extended `.path-migrate-swap.json` journal, revalidates `ModelFactsCache`, and runs `UPDATE`s on `metadata.db`, `user-prompts.db` (`PromptStorePathUpdater`), `episodic.db`, `skills.db`.
- **Pros:**
  - Zero engine downtime; active OpenCode sessions keep running and queue/serialize behind `with_scope_write_lock`.
  - Directly extends `PathMigrationService` (`mem-store`) and `PromptStorePathUpdater` (`mem-bridge/src/tools/inventory.rs`), preserving the single-process `.engine.lock` invariant (`ADR-0007`) without asking the user to stop/restart the host daemon.
  - Naturally reusable afterward whenever any user moves a repo directory on disk via the `memory migrate` tool / Web UI.
- **Cons & Failure Modes:**
  - **Higher blast radius in live engine memory:** `EpisodicStore`, `UserPromptManager`, `SkillsManager`, and `ShardManager` hold live cached handles (`ConnectionManager`) and background loops (e.g., `capture_inbox` worker, `decay-sweep` timer). Mutating `episodic.db` `project_id` or deleting source shards while background capture tasks read/write `episodic.db` risks `SQLITE_BUSY` / lock contention or stale in-flight `ContainerTag` captures for a directory being merged.
  - **Failure mode 1 (Lock-order / pool deadlock):** Acquiring two `with_scope_write_lock`s + `OperationLock` + `ConnectionManager::close_connection` while `mem-web` HTTP routes or `mem-bridge` hooks issue reads against the same `ConnectionManager` can deadlock or fail `open_guarded` (`-tshm` collision) if any background reader re-opens the shard mid-swap.
  - **Failure mode 2 (Cross-crate dependency ceiling, `ADR-0001`):** `mem-store` cannot depend on `mem-episodic`/`mem-skills`/`mem-profile`, requiring trait ports (`PromptPathUpdater`-style traits for `EpisodicPathUpdater`, `SkillTagUpdater`) wired through `mem-bridge` — larger API/test surface across 4 crates for a largely one-time `/home/vaz` migration.
  - **Failure mode 3 (Missing-on-disk targets still fail `resolve_scope`):** Without an explicit override bypassing `TagResolver::project_tag_info`, live `memory migrate` will still mis-hash Category B (`old=git-common`) missing directories.
- **Predicted outcome:** Achieves lossless row merge without restarting PID `18731`, with predicted higher implementation/trait-seam complexity (`INFERRED`; measure changed seams and code size) (`mem-store` + `mem-bridge` + `mem-episodic` + `mem-skills`) and non-zero risk of live-worker contention on `episodic.db` (`2.9 MB` active WAL) during batch migration of 11–34 shards.
- **Refutation:** Falsified if (a) live `ConnectionManager::close_connection` + `capture_inbox` / `mem-web` concurrent reads produce zero `Busy`/`OpenGuard` errors across all 34 migrations under load, OR (b) extending `PathMigrationService` requires fewer lines/seams than adding a standalone offline CLI mode while keeping `workspace_deps` green.

---

## 6. One Shared Discriminating Measurement (Isolated Fixture, Runnable Next)

**Principle (`docs/agents/workflow.md`):** Do not touch live `/home/opencode/.opencode-mem/data/`. Build an **isolated fixture** by quiescing or snapshotting a consistent test copy (or synthetic fixture matching exact live schemas + live `-wal` conditions + real `/home/vaz` edge cases: `4182e0858e0c93a1` + `fd2bfb63318dfd29`, 2-to-1 `3d5f981354135289` + `b7a8895b5cdfe1b9` -> `6e64d9c620f21fbf`, `<private>` + `is_fully_private` rows, `is_pinned=1`, non-null `session_id` + `source_prompt_created_at`, colliding `memories.id`, matching `user_prompts`, `episodic.db` `agent_sessions` + `episodic_milestones` + `episodic_raw_context`, and `skills.db` rows), plus a second fixture containing `facts.db` and `vault/` to verify fail-closed refusal.

### Measurement Protocol & Pass/Fail Gates
1. **Vector & 20-Column Losslessness Gate:**
   - Measure pre- vs post-merge across all surviving rows: `COUNT(*)` (`target_after == target_before + src_rows - exact_dups`), zero `<private>` stripping, `is_fully_private` row count unchanged, `SUM(is_pinned)` preserved, non-null `session_id` & `source_prompt_created_at` byte-identical, `created_at`/`updated_at` preserved, and cosine similarity delta `|cos_before - cos_after| <= 1e-6` on raw copied `vector`/`tags_vector` blobs via `search_in_shard` (zero calls to embedder).
2. **Cross-DB & Guard Gate:**
   - `metadata.db`: `shards.vector_count == shard_stats.vector_count == COUNT(memories)`, exactly `1` `is_active=1` per `(scope, scope_hash)`.
   - `user-prompts.db`, `episodic.db`, `skills.db`: 0 remaining rows referencing migrated source `project_path`/`project_id`/`container_tag`; `episodic_raw_context` byte lengths unchanged.
   - Guard fixture: immediate non-mutating error if `facts.db` or `vault/{scope}/` contains source/target scope rows.
3. **Concurrency, Crash-Rollback & Seam Gate:**
   - **Design A test:** Verify WAL checkpoint drains `-wal` to `0 B` cleanly on shut-down fixture; simulate mid-swap `SIGKILL` after shard write before `metadata.db` update and prove rollback restores identical SHA-256 checksums across all DB + sidecar files.
   - **Design B test:** Run migration while concurrent `mem-web` read + `capture_inbox` episodic write tasks run against the same fixture `ConnectionManager` — measure error/retry rate (`OpenGuard`, `Busy`, stale handle reads).
   - **Decision rule:** **A wins** if A passes all losslessness/rollback gates and Turso blob-copy preserves vectors within `1e-6` while B exhibits any `Busy`/`OpenGuard` contention or requires significantly larger cross-crate boilerplate. **B wins** if B achieves 0 errors under concurrent load with minimal seam delta and downtime of PID `18731` is unacceptable. **NEITHER** if Turso `0.8` `F32_BLOB` copy requires re-embedding (forcing a vector-extract/re-encode fallback design).

---

## 7. Engine Quiesce, Backup, Verify & Rollback Protocol (Live Execution Safety)

For live cutover (Design A — or pre-cutover snapshot even under Design B):
1. **Quiesce (`MEASURED` target PID `18731`):** Stop active OpenCode client/plugin processes cleanly (`SIGTERM`, await exit of PID `18731` and removal/release of `.engine.lock`); verify `ps -eo pid,args | grep opencode-agents-memory` empty and `lsof /home/opencode/.opencode-mem/data/*` empty. **Never `SIGKILL` unless hung after timeout**, and if killed, open via Turso once offline to replay `-wal` before copying.
2. **Full Tuple Backup (`sqlite_files.rs` contract):** Copy entire `/home/opencode/.opencode-mem/data/` (including `.db` + `-wal` + `-shm` + `-tshm`, `.wal-mode`, `.turso-migrated`, `.auth-token`) to a timestamped sibling/backup root with `0700`/`0600` permissions + SHA-256 manifest **before** running any checkpoint or write.
3. **Verify Post-Run:**
   - `PRAGMA integrity_check` on `metadata.db`, `user-prompts.db`, `episodic.db`, `skills.db`, and every touched shard.
   - Re-launch engine (`--storage-root /home/opencode/.opencode-mem/data`) and compare `memory list-shards` (`all-projects`) counts, `memory search` spot-queries (including pinned & `<private>` preservation in store vs web), and `mem-web` HTTP routes.
4. **Rollback:** Quiesce engine -> replace `/home/opencode/.opencode-mem/data` DB+sidecar tuples atomically from pre-run SHA-256-verified backup -> restart engine -> verify `list-shards` matches baseline `37`-shard counts.

---

## 8. Reconciliation with `20261110-2337` Phase C (`Steps 9–13`) & `20261111-0000`

From `.tmp/progress/20261110-2337-post-migration-all-tracks.ledger.md` (`READ` lines 15–25):
- **`Step 9` (Export/back up both scopes):** Retain pre-flight full storage-root backup (with `-wal`/`-shm`/`-tshm` + checksum/row-count manifest). Drop JSON `Portability` export as the restoration source because it is lossy (`§1.2`).
- **`Step 10` (`10.1`–`10.6`, Consolidate `4182e0858e0c93a1` `2283` → `fd2bfb63318dfd29` `41`):**
  - **Supersede `Steps 10.3–10.5`:** Replace destructive "forget A IDs → empty target → import B → re-import A" (`Portability` `ReplaceMode::Merge` workaround) with the chosen lossless Turso consolidation design (`Design A` or `Design B`), preserving `<private>`, `is_fully_private`, `is_pinned`, `session_id`, `source_prompt_created_at`, timestamps, and raw vectors, plus `user_prompts`, `episodic.db`, and `skills.db`.
  - **Scope expansion vs `20261111-0000`:** Reconcile `Steps 10` & `20261111-0000` (orphan rename) into one unified manifest covering Category A (`11` shards → `10` targets, including `2-to-1` `.config/opencode`), while gating Category B (`13` missing `git-common`) and Category C (`10` missing `path`) + the `3` `linked` `/home/vaz` shards on operator policy (`§9`).
- **`Step 11` (Verify repo-shard consolidation):** Verify `fd2bfb63318dfd29` count `<= 2324` (`41 + 2283 - exact_id_dups`), zero orphan `4182e0858e0c93a1` in active `list-shards`, `git_repo_url = git@github.com:vazw/opencode-agents-memories.git`, and spot search + `user_prompts`/`episodic` continuity.
- **`Steps 12–13` (`AgentsBrain` `8464b82a892b16cf` `90` → `f28b4a74af055334`):** Note `/home/opencode/Documents/AgentsBrain` currently has **no** existing target shard (`0` rows), so it is a 1-to-1 target creation (`git-common:/home/opencode/Documents/AgentsBrain/.git`), still requiring cross-DB `episodic.db` + `user-prompts.db` + `skills.db` updates and cross-session verification inside `AgentsBrain` (`Step 13`).

---

## 9. Open Operator Policy Questions (For User / `syseng` Plan Gate)

1. **Scope of Consolidation Wave 1:** Consolidate only Category A `orphaned` shards with existing `/home/opencode/*` directories (`10` shards → `9` targets, omitting `#10` `/home/vaz` `linked`), include `/home/vaz` → `/home/opencode` (`#10`), or also process Categories B & C?
2. **Category B (`13` missing-on-disk `git-common:` repos, `990` rows):** Keep archived/untouched as `/home/vaz` orphans until cloned under `/home/opencode`, OR migrate using explicit synthetic `git-common:/home/opencode/.../.git` identity hashes? (Never fall back to `path:` for these 13.)
3. **`#2` `opencode` repo ambiguity (`56c3912b7371f31b`):** Should `/home/vaz/Code/Personal/JS/opencode` (`179` rows, `https://github.com/anomalyco/opencode.git`) merge into existing `/home/opencode/Code/opencode` (`git@github.com:vazw/opencode.git`, `0` rows yet), or stay separate/archived?
4. **3 `linked` `/home/vaz` shards (`76688b45006c2dc8` `/home/vaz` `19`, `f0c274e00a11b8eb` `.config/kitty` `13`, `692651b6e29bfb95` `.config/nvim` `7`):** Consolidate `/home/vaz` → `/home/opencode` (`--allow-linked-source`) and archive `.config/{kitty,nvim}`, or leave all 3 untouched while `/home/vaz` exists?
5. **Divergent `memories.id` collisions:** Default is whole-plan abort. If any exist, should the wave remain blocked, or should a separate identity/reference-remapping design preserve both versions? Dropping either version is outside lossless scope.
6. **Legacy `/home/vaz` text inside `user_profiles.profile_data` / `memories.content` / `memories.metadata`:** Preserve verbatim (recommended for lossless provenance), or rewrite path strings inside JSON/text?
7. **Design selection & maintenance window:** Is brief quiesce of engine PID `18731` acceptable for **Design A** (`--mode=consolidate-shards`), or is online **Design B** strictly required?

## 10. Evidence Limits & Gate

- `READ`: Schema descriptions above come from the cited source, not live DB introspection. `ai-sessions.db` exact schema, full auxiliary column inventories, and episodic `project_id` encoding remain unverified. A generated schema manifest from the isolated fixture is a mandatory pre-cutover artifact; unknown tables/columns/bindings abort rather than being omitted.
- `INFERRED`: Both designs and their performance/complexity predictions remain hypotheses. No prototype, fixture measurement, migration, or rollback exercise ran. Neither design is approved.
- Both candidates must use the same immutable baseline and exact field/blob equality oracle; cosine tolerance is supplemental, never a substitute for byte preservation. Duplicate accounting must explain any reduced pin/private counts by exact duplicate occurrences, not assert raw sums stay unchanged after deduplication.
- Safety ordering: acquire exclusive custody, create and verify the full immutable pre-open DB/sidecar snapshot, then perform Turso recovery/checkpoint on the working copy. The earlier Design A checkpoint-before-backup ordering is superseded by this rule. Never copy a changing live root. Snapshot and rollback cover the whole root, not only touched files; never restore stale lock ownership as an active lock.
- Design B must establish a root-wide write/read admission barrier covering every auxiliary writer and cached handle before snapshot/swap. Scope locks alone do not establish this. Failure to establish it rejects B; zero downtime is a prediction, not evidence.
- Shared decision measurement is **specified, not executable yet**: coder must supply an isolated-fixture harness before the gate. Record both candidates' invariants, interruption results, duration, and changed seams. If both pass, choose using measured locality/maintenance cost and operator downtime policy. If both fail, choose NEITHER. If no approved mappings or required consolidation remain, close as measurement, not a deliverable (inverted world).
- Phase C Steps 9–13 remain unfinished. This draft proposes changes only; no approved blueprint or ledger has been edited.

**STATUS: NEEDS-USER**
