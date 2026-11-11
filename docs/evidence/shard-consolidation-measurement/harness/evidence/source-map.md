# Partial source map — bootstrap blocked

READ revision: df34c522cd349ddb59f021fc82eae64190123e2f. This is not a completed schema/caller inventory.

- `Cargo.toml:23`: workspace Turso requirement 0.8.3; standalone manifest pins =0.8.3.
- `crates/mem-store/src/db.rs:1-8`: TursoDb owns Database and Connection; final handle drop releases. No close API. Manager close removes/drops. Caller drain and stale handles NOT EXERCISED.
- `crates/mem-store/src/db.rs:24-31`: typed values Null/Integer/Real/Text/Blob. No JSON normalization oracle implemented.
- `crates/mem-store/src/db.rs:114-140`: row names and typed value access available publicly.
- `crates/mem-store/src/schema_migrations.rs:65-80`: actual maxima metadata=2, prompts=3, profiles=2, memory shard=4, episodic=8. Context glossary numerals are stale. Complete table/column/index inventory NOT RUN.
- `crates/mem-store/src/schema_migrations.rs:13-26`: restoration belongs to owning caller after handle closure; backup requires exclusivity before checkpoint/copy.
- `crates/mem-store/src/lib.rs`: public db/schema/lock/shard modules accessible without production changes.
- Necessary-symbol index retained at `seam-index.txt`; it is a READ search artifact, not verified schema coverage.

UNKNOWN: actual episodic project_id writer semantics; complete prompts/skills/metadata/episodic identity column and unique-key inventories; ai-sessions.db schema; facts/vault support. No live file opened to resolve these.
UNKNOWN/NOT EXERCISED: online reads, memory writes, prompt capture, episodic inbox, skills writes, retention/promotion, shared admission/drain, stale-handle lifecycle. No barrier sufficiency conclusion.

Bootstrap stopped at offline dependency resolution failure before compilation. No fixture database exists. Resume only after explicit blocker resolution and ledger rework authorization.

## Continuation d90461a51f51 — source trace (READ, same revision)

Fetch exit 0 and locked offline metadata exit 0; effective target_directory is exact scratch target; workspace_root is scratch package, Turso 0.8.3. Full tracked/manifest/lock baseline 1399 files unchanged. See fetch.log and metadata-2.json.

### Actual schema sets and binding/key lists
- memories: sql.rs:61-119 + schema_migrations.rs:295-336; id PK; content/vector/tags_vector/container_tag/tags/type/created_at/updated_at/metadata/display_name/user_name/user_email/project_path/project_name/git_repo_url/is_pinned; v2 session_id; v3 separate shard_stats(id PK,vector_count); v4 source_prompt_created_at. shard_metadata(key PK,value). Actual runtime PRAGMA still required by S04.
- registry: sql.rs:22-39 and schema_migrations.rs:347-368: shards(id PK,scope,scope_hash,shard_index,db_path,vector_count,is_active,created_at); UNIQUE(scope,scope_hash,shard_index); partial one-active unique index. Association fields scope_hash/db_path; scope remains unchanged.
- prompts: sql.rs:44-57; user_prompts_sql.rs:17-21; schema_migrations apply_user_prompts_schema: id PK,session_id,message_id,project_path,content,created_at,captured,user_learning_captured,linked_memory_id,capture_attempts,provider_id,model_id; sweep_state(tier PK,last_swept_unix_ms) is unrelated. Only project_path maps.
- episodic_sql.rs:18-37,65-84,120-145,209-222: agent_sessions(id PK,project_id,session_id,agent_id,parent_session_id,summary,metadata,embedding,created_at,updated_at,deleted_at,orphaned,capture_id,agent_role), UNIQUE(project_id,session_id,agent_id), FK parent_session_id->id; episodic_milestones(id PK,project_id,agent_id,summary,contributors,source_count,window_start,window_end,embedding,created_at); episodic_raw_context PK(project_id,session_id,agent_id), raw_context/byte_len/captured_at; episodic_segments PK(project_id,session_id,agent_id,segment_index), raw_content/byte_offset/byte_len/created_at; capture_inbox(capture_id PK,project_id,session_id,agent_id,payload,created_at,attempts,last_attempt_at,status,not_before); sweep_state unrelated. Only project_id maps in these five tables. Historical JSON payload is not edited.
- skills schema.rs:32-49: id PK,name,description,content,autoinvoke,enabled,container_tag nullable,created_at,updated_at; only container_tag maps. No new crate allowed, so verbatim source DDL required if this fixture is reached, explicitly not public manager coverage.
- ai-sessions.db remains UNKNOWN (no live inspection). facts/vault conservative refusal required, not modeled support.

### Caller semantics and admission
- mem-bridge engine.rs:587-607 accepts nonblank request project_id and deserializes original capture params; project_scope.rs:125-147 resolves tags from Path::new(project_id) but retains original project_id string. inject.rs:400-424 retains same original string. episodic inbox.rs:110-139 and store.rs:289-305 bind entry/session.project_id verbatim. Thus path strings from caller, not encoded scope hashes, are supported synthetic inputs; arbitrary untraced encodings must refuse.
- shard_manager.rs:260-288 scope closure mutex is per-manager (scope,hash), awaits closure completion, distinct scope unblocked. It is not a root barrier.
- mem-bridge capture.rs:189-205 and episodic promotion.rs:344-355 acquire same manager scope lock, get_write_shard checks operation lock but own PID tolerance exists. No global drain proved.
- mem-profile prompt.rs:215-279 save_prompt privacy-strips then ready and direct execute; ready:164-198 caches handle, no operation-lock or scope-lock acquisition here. Cannot use save_prompt for lossless copy.
- mem-episodic inbox.rs:110-139 directly ready/execute; store.rs:190-215 caches handle and initializes schema, insert_session binds direct SQL. No shared barrier in traced methods.
- mem-skills manager.rs:281-370 ready caches handle; list/get/create directly query/execute, no shared scope/operation barrier at those methods. Create also writes content file. Actual manager not executable by this immutable dependency manifest; record not_exercised, not blocked.
- mem-bridge sweep.rs:336 acquires operation lock before open/checkpoint/vacuum/stamp; episodic consolidation.rs:283 also acquires it. Promotion/capture own-lock tolerance is READ, not tested yet.
- connection.rs:134-181 waits close signals and returns cached handles; close_connection:271+ removes manager ownership, cannot revoke outstanding cloned Arc handles. db.rs:491-505 last owner drop releases actual DB. Drain/stale handle correctness unexercised.
- operation_lock.rs:144-158 checks live PID files only when called; acquire:188+ atomic create_new and liveness. No automatic lock check on Db.execute/query (db.rs:560-583).

Two targeted passes used for auxiliary/caller seams; no further recursive expansion. Missing legacy auxiliary schema remains a production-completeness blocker, not grounds for guessing.

## S09 exercised seams (measured, test_existing_barriers_report_observed_coverage)

EXERCISED (public APIs, synthetic root, real Turso aux writes):
- operation_lock.rs:188+ `acquire_turso_operation_lock`: atomic create_new after live-lock assert; second acquire while held refused `MigrationLocked{pid=self}`; completes after release. Timings µs.
- operation_lock.rs:144-158 `assert_no_turso_migration_in_progress`: refuses while an operation lock is live; ok after release.
- operation_lock.rs:105+ `read_live_lock`: returns live record while held; unlinks dead-owner (stale PID) record on read — reclaim-before-acquire is the measured mechanism (acquire runs the assert first, so a stale lock never blocks the first acquire).
- shard_manager.rs:260-288 `with_scope_write_lock`: per-(scope,hash) tokio AsyncMutex measured — holder latched in closure, same-scope contender blocked (1s bounded negative poll), distinct scope completed while held, queued contender drained only after release. Scoped, not root-wide.
- delete_guard.rs:82-96 `ExclusiveRoot::current`: Some while a strong `Arc<EngineLock>` custody exists; None only after BOTH the `ExclusiveRoot` proof token and custody drop (the proof holds a strong Arc — measured drop-order requirement).
- Auxiliary: `db.execute`/`query` on aux-one.db completed under the operation lock, under the scope lock, and after release — no root-wide barrier over direct SQL.

NOT EXERCISED (honest ledger, each with reason in evidence/admission.json): skills-manager list/get/create (no shared barrier at those methods; crate outside manifest); mem-episodic inbox/store direct SQL (no shared barrier traced; crate outside manifest); mem-bridge capture/promotion real callers (same-manager scope lock traced READ-only; crate outside manifest); connection close-drain/stale cloned Arc handles (no public close seam reachable); power-loss durability (process-kill harness cannot establish it).

No admission-sufficiency conclusion follows: these observations cover only the seams reachable from this immutable dependency manifest.
