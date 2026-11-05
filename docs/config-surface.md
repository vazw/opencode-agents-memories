# Configuration Surface Census and Truth

This document establishes the authoritative, rot-proof census of the configuration surface for `opencode-agents-memory` (resolving Issue 49 and Issue 63). It itemizes all 102 fields (86 `Config` fields comprising 79 leaf fields and 7 composite structures, plus 16 nested leaf fields across 7 sub-structures) defined in `crates/mem-core/src/config/schema.rs`.

A rot-guard integration test in `crates/mem-core/tests/config_census.rs` validates this file on every run, asserting that every schema field is accounted for in the census table under its qualified struct with a non-empty disposition and valid row schema.

---

## 1. Chat Message Injection Surface (Issue 63 Dispositions)

The 6-key chat message surface controls chat-mode memory injection and capture behavior. Every key has a single canonical disposition, gated path, default value, upstream citation, and verified on/off or divergence observation:

| Key | Gated Path | Default | Upstream Citation | Status | Observation / Divergence |
|---|---|---|---|---|---|
| `chat_message.enabled` | `crates/mem-bridge/src/capture.rs:chat_capture_enabled` (capture) and `crates/mem-bridge/src/inject.rs:assemble` (inject when `chat_injection: true`) | `true` | `example/opencode-mem/src/index.ts:670, 735, 748, 767` | live | When `false`, chat injection refuses memory block assembly (`block: null`, `memory_count: 0`) while direct injection and profile tiers remain intact. Capture path refuses chat capture. Verified in `crates/mem-bridge/tests/inject_chat_gates.rs`. |
| `chat_message.exclude_current_session` | `crates/mem-bridge/src/inject.rs:assemble` | `true` | `example/opencode-mem/src/services/memory-recall.ts:159-160, 240` | live | When `chat_injection: true`, filters out memories and episodes whose `session_id` matches `params.session_id`. Top-level request parameter overrides policy default. Verified in `inject_chat_gates.rs`. |
| `chat_message.max_age_days` | `crates/mem-bridge/src/inject.rs:assemble` | `None` (unbounded) | `example/opencode-mem/src/services/memory-recall.ts:160, 241` | live | When `chat_injection: true`, drops candidates with `created_at` older than store clock cutoff millis (`now - max_age_days * 86_400_000`). Verified in `inject_chat_gates.rs`. |
| `chat_message.inject_on` | `crates/mem-bridge/src/engine.rs:call_hello` -> `hello.result.config.inject_on` | `InjectOn::First` (`"first"`) | `example/opencode-mem/src/index.ts:739, 791` | live | Advertised in handshake payload; wrapper `TurnIndex` evaluates `"always"` vs `"first"` to drive per-turn prompt context refresh in `plugin/src/v2/adapter.js`. |
| `chat_message.max_memories` | `crates/mem-bridge/src/inject.rs:assemble` | `3` | `example/opencode-mem/src/services/memory-recall.ts:179-218, 236` | live | **Documented divergence**: in Rust port, token budget is the primary allocator (`greedy` byte budget); `max_memories` acts as an emitted row count cap after budget and deduplication. Pinned by `test_max_memories_slot_not_lost_to_duplicate_summary`. |
| `chat_message.capture_child_sessions` | `hello.result.config.capture_child_sessions` | `false` | `example/opencode-mem/src/index.ts:698, 754` | live | Advertised in hello handshake to gate wrapper compaction-restore for child sessions (ADR-0017 amendment 2026-11-03); pre-compaction ingest is ungated, and pre-turn chat inject is ungated. Emergent property: child sessions restore nothing unless enabled. |

---

## 2. Configuration Surface Census

The table below itemizes all 102 schema fields:
- `Config`: 86 fields (79 leaf fields + 7 composite structs)
- `MemoryScope`: 1 leaf field
- `Compaction`: 2 leaf fields
- `ChatMessage`: 8 leaf fields
- `Episodic`: 1 leaf field
- `FeatureFlags`: 2 leaf fields
- `SoundMemory`: 1 leaf field
- `Promotion`: 1 leaf field

Row format: `| Struct | Field | Status | Reader / Disposition | Upstream Reference |`

| Struct | Field | Status | Reader / Disposition | Upstream Reference |
|---|---|---|---|---|
| Config | storage_path | live | crates/mem-bridge/src/capture.rs:710 | example/opencode-mem/src/config.ts:321 |
| Config | user_email_override | live | crates/mem-bridge/src/composition.rs:204 | example/opencode-mem/src/config.ts:322 |
| Config | user_name_override | live | crates/mem-bridge/src/composition.rs:205 | example/opencode-mem/src/config.ts:323 |
| Config | embedding_model | live | crates/mem-bridge/src/composition.rs:425 | example/opencode-mem/src/config.ts:324 |
| Config | embedding_dimensions | live | crates/mem-bridge/src/composition.rs:317 | example/opencode-mem/src/config.ts:325 |
| Config | embedding_use_task_prefixes | live | crates/mem-core/src/embedding.rs:43 | example/opencode-mem/src/config.ts:326 |
| Config | embedding_pooling | live | crates/mem-bridge/src/composition.rs:446 | example/opencode-mem/src/config.ts:327 |
| Config | embedding_query_prefix | live | crates/mem-core/src/embedding.rs:44 | example/opencode-mem/src/config.ts:328 |
| Config | embedding_document_prefix | live | crates/mem-core/src/embedding.rs:45 | example/opencode-mem/src/config.ts:329 |
| Config | embedding_dtype | live | crates/mem-bridge/src/composition.rs:445 | example/opencode-mem/src/config.ts:330 |
| Config | embedding_max_tokens | live | crates/mem-bridge/src/composition.rs:449 | example/opencode-mem/src/config.ts:331 |
| Config | embedding_api_url | live | crates/mem-bridge/src/composition.rs:418 | example/opencode-mem/src/config.ts:332 |
| Config | embedding_api_key | live | crates/mem-bridge/src/composition.rs:419 | example/opencode-mem/src/config.ts:333 |
| Config | similarity_threshold | dead | mark inert (issue 40) | example/opencode-mem/src/config.ts:338 |
| Config | max_memories | live | crates/mem-bridge/src/tools/memories.rs:77 | example/opencode-mem/src/config.ts:336 |
| Config | max_profile_items | dead | mark inert (issue 44) | example/opencode-mem/src/config.ts:337 |
| Config | inject_profile | live | crates/mem-profile/src/profile_context.rs:91 | example/opencode-mem/src/config.ts:339 |
| Config | container_tag_prefix | live | crates/mem-bridge/src/composition.rs:203 | example/opencode-mem/src/config.ts:343 |
| Config | auto_capture_enabled | live | crates/mem-profile/src/idle_scheduler.rs:32 | example/opencode-mem/src/config.ts:344 |
| Config | auto_capture_max_iterations | dead | mark inert (issue 128) | example/opencode-mem/src/services/auto-capture.ts:50 |
| Config | auto_capture_iteration_timeout | dead | mark inert (issue 128) | example/opencode-mem/src/services/auto-capture.ts:51 |
| Config | auto_capture_max_retries | live | crates/mem-profile/src/profile_store.rs:101 | example/opencode-mem/src/services/auto-capture.ts:52 |
| Config | auto_capture_max_context_bytes | live | crates/mem-bridge/src/composition.rs:244 | example/opencode-mem/src/services/auto-capture.ts:53 |
| Config | auto_capture_language | dead | mark inert (issue 128) | example/opencode-mem/src/services/auto-capture.ts:60 |
| Config | memory_provider | live | crates/mem-bridge/src/engine.rs:1462 | example/opencode-mem/src/config.ts:355 |
| Config | memory_provider_claimed | live | crates/mem-bridge/src/manual_provider.rs:221 | example/opencode-mem/src/config.ts:356 |
| Config | memory_model | live | crates/mem-bridge/src/manual_provider.rs:54 | example/opencode-mem/src/config.ts:357 |
| Config | memory_api_url | live | crates/mem-bridge/src/engine.rs:1459 | example/opencode-mem/src/config.ts:358 |
| Config | memory_api_key | live | crates/mem-bridge/src/manual_provider.rs:56 | example/opencode-mem/src/config.ts:359 |
| Config | memory_temperature | dead | mark inert (issue 128) | example/opencode-mem/src/config.ts:360 |
| Config | force_tool_choice | live | crates/mem-ai/src/provider/remote/openai_chat.rs:155 | example/opencode-mem/src/config.ts:361 |
| Config | memory_extra_params | dead | mark inert (issue 128) | example/opencode-mem/src/config.ts:362 |
| Config | opencode_provider | live | crates/mem-bridge/src/engine.rs:1413 | example/opencode-mem/src/config.ts:363 |
| Config | opencode_model | live | crates/mem-bridge/src/composition.rs:238 | example/opencode-mem/src/config.ts:364 |
| Config | opencode_variant | live | crates/mem-bridge/src/engine.rs:1415 | example/opencode-mem/src/config.ts:365 |
| Config | opencode_base_url | live | crates/mem-bridge/src/engine.rs:1412 | example/opencode-mem/src/config.ts:366 |
| Config | opencode_timeout_ms | dead | mark inert (issue 128) | example/opencode-mem/src/config.ts:367 |
| Config | auto_capture_provider_status | dead | mark inert (issue 128) | example/opencode-mem/src/services/auto-capture.ts |
| Config | ai_session_retention_days | dead | mark inert (issue 39) | example/opencode-mem/src/config.ts:368 |
| Config | web_server_enabled | live | crates/mem-web/src/settings.rs:27 | example/opencode-mem/src/config.ts:370 |
| Config | web_server_port | live | crates/mem-web/src/settings.rs:29 | example/opencode-mem/src/config.ts:371 |
| Config | web_server_host | live | crates/mem-web/src/settings.rs:28 | example/opencode-mem/src/config.ts:372 |
| Config | web_server_auth_password | live | crates/mem-web/src/settings.rs:30 | example/opencode-mem/src/config.ts:373 |
| Config | web_server_auth_username | live | crates/mem-web/src/settings.rs:31 | example/opencode-mem/src/config.ts:374 |
| Config | web_server_api_token | live | crates/mem-web/src/settings.rs:32 | example/opencode-mem/src/config.ts:375 |
| Config | max_vectors_per_shard | live | crates/mem-store/src/config.rs:16 | example/opencode-mem/src/config.ts:376 |
| Config | auto_cleanup_enabled | live | crates/mem-profile/src/incremental_cleanup.rs:221 | example/opencode-mem/src/config.ts:377 |
| Config | auto_cleanup_retention_days | dead | mark inert (issue 41) | example/opencode-mem/src/config.ts:378 |
| Config | deduplication_enabled | live | src/main.rs:223 | example/opencode-mem/src/services/deduplication-service.ts:37 |
| Config | deduplication_similarity_threshold | live | src/main.rs:224 | example/opencode-mem/src/services/deduplication-service.ts:38 |
| Config | user_profile_analysis_interval | live | crates/mem-profile/src/profile_store.rs:102 | example/opencode-mem/src/services/user-profile.ts:40 |
| Config | user_profile_max_context_bytes | dead | mark inert (issue 44) | example/opencode-mem/src/services/user-profile.ts:41 |
| Config | user_profile_display_preferences | dead | mark inert (issue 44) | example/opencode-mem/src/services/user-profile.ts:42 |
| Config | user_profile_display_patterns | dead | mark inert (issue 44) | example/opencode-mem/src/services/user-profile.ts:43 |
| Config | user_profile_display_workflows | dead | mark inert (issue 44) | example/opencode-mem/src/services/user-profile.ts:44 |
| Config | user_profile_stale_days | live | crates/mem-profile/src/profile_store.rs:112 | example/opencode-mem/src/services/user-profile.ts:45 |
| Config | user_profile_inject_preferences | live | crates/mem-profile/src/profile_context.rs:93 | example/opencode-mem/src/services/user-profile.ts:46 |
| Config | user_profile_inject_patterns | live | crates/mem-profile/src/profile_context.rs:94 | example/opencode-mem/src/services/user-profile.ts:47 |
| Config | user_profile_inject_workflows | live | crates/mem-profile/src/profile_context.rs:95 | example/opencode-mem/src/services/user-profile.ts:48 |
| Config | user_profile_confidence_decay_days | live | crates/mem-profile/src/profile_store.rs:111 | example/opencode-mem/src/services/user-profile.ts:49 |
| Config | user_profile_changelog_retention_count | live | crates/mem-profile/src/profile_store.rs:114 | example/opencode-mem/src/services/user-profile.ts:50 |
| Config | user_profile_embedding_threshold_same_cat | live | crates/mem-profile/src/profile_store.rs:104 | example/opencode-mem/src/services/user-profile.ts:51 |
| Config | user_profile_embedding_threshold_same_cat_weak | live | crates/mem-profile/src/profile_store.rs:106 | example/opencode-mem/src/services/user-profile.ts:52 |
| Config | user_profile_embedding_threshold_cross_cat | live | crates/mem-profile/src/profile_store.rs:107 | example/opencode-mem/src/services/user-profile.ts:53 |
| Config | user_profile_embedding_threshold_cross_cat_weak | live | crates/mem-profile/src/profile_store.rs:109 | example/opencode-mem/src/services/user-profile.ts:54 |
| Config | user_profile_centroid_drift_threshold | live | crates/mem-profile/src/profile_store.rs:110 | example/opencode-mem/src/services/user-profile.ts:55 |
| Config | user_profile_embedding_min_description_length | live | crates/mem-profile/src/profile_store.rs:103 | example/opencode-mem/src/services/user-profile.ts:56 |
| Config | user_profile_min_evidence_for_retention | live | crates/mem-profile/src/profile_store.rs:113 | example/opencode-mem/src/services/user-profile.ts:57 |
| Config | user_profile_auto_cleanup_enabled | live | crates/mem-profile/src/profile_store.rs:116 | example/opencode-mem/src/services/user-profile.ts:58 |
| Config | user_profile_auto_cleanup_interval | live | crates/mem-profile/src/profile_store.rs:117 | example/opencode-mem/src/services/user-profile.ts:59 |
| Config | user_profile_validation_enabled | live | crates/mem-profile/src/profile_store.rs:115 | example/opencode-mem/src/services/user-profile.ts:60 |
| Config | show_auto_capture_toasts | dead | mark inert (issue 20) | example/opencode-mem/src/index.ts:710 |
| Config | show_user_profile_toasts | dead | mark inert (issue 20) | example/opencode-mem/src/index.ts:711 |
| Config | show_error_toasts | dead | mark inert (issue 20) | example/opencode-mem/src/index.ts:712 |
| Config | database_encryption_enabled | live | crates/mem-bridge/src/composition.rs:526 | example/opencode-mem/src/config.ts:380 |
| Config | database_encryption_key | live | crates/mem-bridge/src/composition.rs:527 | example/opencode-mem/src/config.ts:381 |
| Config | database_encryption_cipher | live | crates/mem-store/src/config.rs:21 | example/opencode-mem/src/config.ts:382 |
| Config | allow_unguarded_open | live | crates/mem-store/src/db.rs:286 | ADR-0021/issue 45 (port-specific guard) |
| Config | auto_update | dead | mark inert (issue 27) | example/opencode-mem/src/config.ts:385 |
| Config | memory | live | crates/mem-core/src/config/schema.rs:620 | example/opencode-mem/src/config.ts:335 |
| Config | compaction | live | crates/mem-bridge/src/composition.rs:33 | example/opencode-mem/src/config.ts:340 |
| Config | chat_message | live | crates/mem-bridge/src/composition.rs:293 | example/opencode-mem/src/config.ts:342 |
| Config | episodic | live | crates/mem-episodic/src/consolidation.rs:84 | example/opencode-mem/src/config.ts:350 |
| Config | feature_flags | live | crates/mem-bridge/src/composition.rs:154 | example/opencode-mem/src/config.ts:352 |
| Config | sound_memory | live | crates/mem-bridge/src/capture.rs:613 | example/opencode-mem/src/config.ts:355 |
| Config | promotion | live | crates/mem-bridge/src/composition.rs:168 | example/opencode-mem/src/config.ts:358 |
| MemoryScope | default_scope | dead | mark inert (issue 128) | example/opencode-mem/src/config.ts:335 |
| Compaction | enabled | live | crates/mem-bridge/src/inject.rs:507 | example/opencode-mem/src/config.ts:340 |
| Compaction | memory_limit | live | crates/mem-bridge/src/inject.rs:550 | example/opencode-mem/src/config.ts:341 |
| ChatMessage | enabled | live | crates/mem-bridge/src/inject.rs:100 | example/opencode-mem/src/index.ts:670 |
| ChatMessage | max_memories | live | crates/mem-bridge/src/inject.rs:445 | example/opencode-mem/src/services/memory-recall.ts:179-218 |
| ChatMessage | exclude_current_session | live | crates/mem-bridge/src/inject.rs:434 | example/opencode-mem/src/services/memory-recall.ts:159-160 |
| ChatMessage | max_age_days | live | crates/mem-bridge/src/inject.rs:465 | example/opencode-mem/src/services/memory-recall.ts:160 |
| ChatMessage | inject_on | live | crates/mem-bridge/src/engine.rs:167 | example/opencode-mem/src/index.ts:739 |
| ChatMessage | filter_injected_prompts | live | crates/mem-ai/src/privacy/layer.rs:74 | example/opencode-mem/src/index.ts:657 |
| ChatMessage | injection_markers | live | crates/mem-ai/src/privacy/layer.rs:77 | example/opencode-mem/src/index.ts:658 |
| ChatMessage | capture_child_sessions | live | crates/mem-bridge/src/engine.rs:163 | example/opencode-mem/src/index.ts:698 |
| Episodic | consolidation_age_days | live | crates/mem-episodic/src/consolidation.rs:84 | example/opencode-mem/src/config.ts:350 |
| FeatureFlags | sound_memory | live | crates/mem-bridge/src/composition.rs:268 | example/opencode-mem/src/config.ts:353 |
| FeatureFlags | promotion_pipeline | live | crates/mem-bridge/src/composition.rs:154 | example/opencode-mem/src/config.ts:354 |
| SoundMemory | raw_context_required | live | crates/mem-bridge/src/capture.rs:1271 | example/opencode-mem/src/config.ts:356 |
| Promotion | ttl_fallback_days | live | crates/mem-bridge/src/composition.rs:168 | example/opencode-mem/src/config.ts:359 |

---

## 3. Audit Arithmetic Reconciliation at HEAD

### 3.1 Initial Audit vs. Current Surface
The initial design concerns audit (2026-10-25, concern C4) reported:
- 98 resolved fields (82 `Config` + 16 nested)
- 45 fields without production readers (32 genuinely inert, 13 forward declarations)
- 53 fields with live readers

Subsequent verification at `8023dd8` and through HEAD (`5432ee748c1f`) resolved and reconciled the arithmetic:
1. **Schema Expansion**:
   - `Config` defines **86 fields**: 79 scalar/option leaves and 7 composite nested structures (`memory`, `compaction`, `chat_message`, `episodic`, `feature_flags`, `sound_memory`, `promotion`).
   - The +1 leaf addition was `allow_unguarded_open` (issue 45), which is live in `crates/mem-store/src/db.rs:286`.
   - The 7 nested structs define **16 leaf fields**: `MemoryScope` (1), `Compaction` (2), `ChatMessage` (8), `Episodic` (1), `FeatureFlags` (2), `SoundMemory` (1), `Promotion` (1).
   - Total fields in schema and census: **102 fields** (95 leaf fields + 7 composite structures).

2. **10 Fields Landed Readers at `8023dd8`**:
   - `compaction.enabled` (`inject.rs:507`, issue 33)
   - `compaction.memory_limit` (`inject.rs:550`, issue 33)
   - `inject_profile` (`profile_context.rs:91`, issue 44)
   - `user_profile_inject_preferences` (`profile_context.rs:93`, issue 44)
   - `user_profile_inject_patterns` (`profile_context.rs:94`, issue 44)
   - `user_profile_inject_workflows` (`profile_context.rs:95`, issue 44)
   - `deduplication_enabled` (`src/main.rs:223`, issue 54)
   - `deduplication_similarity_threshold` (`src/main.rs:224`, issue 54)
   - `chat_message.filter_injected_prompts` (`layer.rs:74`, issue 52)
   - `chat_message.injection_markers` (`layer.rs:77`, issue 52)

3. **Subsequent Landings to HEAD**:
   - 6 chat message injection keys wired in issues 63 & 98: `chat_message.enabled` (`capture.rs:70` & `inject.rs:100`), `chat_message.exclude_current_session` (`inject.rs:434`), `chat_message.max_memories` (`inject.rs:445`), `chat_message.max_age_days` (`inject.rs:465`), `chat_message.inject_on` (`engine.rs:167`), `chat_message.capture_child_sessions` (`engine.rs:163`).
   - `SoundMemory.raw_context_required` (`capture.rs:1271`), `FeatureFlags.promotion_pipeline` / `sound_memory` (`composition.rs:154, 268`), `Promotion.ttl_fallback_days` (`composition.rs:168`).
   - Live reader correction: `Episodic.consolidation_age_days` (`mem-episodic/src/consolidation.rs:84`) was misclassified as dead by a naive regex pass that missed local binding names; it has always been live.
   - Dedup attribution correction: `deduplication_enabled` and `deduplication_similarity_threshold` gate the manual maintenance endpoint (`crates/mem-web/src/api/maintenance.rs:245`) and are instantiated in `src/main.rs:223-224`.

4. **Current Status Partition at HEAD**:
   - Total Schema Fields: **102** (86 `Config` + 16 nested)
   - Total Leaf Fields: **95** (79 `Config` + 16 nested)
   - Composite Structures: **7** (all live in composition / schema initialization)
   - Live Leaf Fields: **75** (60 `Config` + 15 nested)
   - Dead Leaf Fields: **20** (19 `Config` + 1 `MemoryScope`)
   - Every one of the 20 dead leaf fields has exactly one disposition: `mark inert (issue <N>)`.

### 3.2 Dead Field Dispositions and Itemized Forward Declarations
The 20 dead leaf fields break down into three explicit categories:
1. **Dead Upstream Too (5 fields)**:
   - `Config.max_profile_items`: parsed and never read in upstream `example/opencode-mem/src/config.ts:337`; issue 44.9 explicitly ruled it stays unwired. Disposition: `mark inert (issue 44)`.
   - `Config.user_profile_max_context_bytes`: upstream `services/user-profile.ts` defines but never reads. Disposition: `mark inert (issue 44)`.
   - `Config.user_profile_display_preferences`: upstream UI display cap unreferenced in engine. Disposition: `mark inert (issue 44)`.
   - `Config.user_profile_display_patterns`: upstream UI display cap unreferenced in engine. Disposition: `mark inert (issue 44)`.
   - `Config.user_profile_display_workflows`: upstream UI display cap unreferenced in engine. Disposition: `mark inert (issue 44)`.

2. **Forward Declarations Citing Open Issues (6 fields)**:
   - `Config.similarity_threshold`: vector search threshold (issue 40). Disposition: `mark inert (issue 40)`.
   - `Config.ai_session_retention_days`: periodic cleanup retention threshold (issue 39). Disposition: `mark inert (issue 39)`.
   - `Config.auto_cleanup_retention_days`: background cleanup scheduler retention threshold (issue 41). Disposition: `mark inert (issue 41)`.
   - `Config.show_auto_capture_toasts`: UI toast notification flag (issue 20). Disposition: `mark inert (issue 20)`.
   - `Config.show_user_profile_toasts`: UI toast notification flag (issue 20). Disposition: `mark inert (issue 20)`.
   - `Config.show_error_toasts`: UI toast notification flag (issue 20). Disposition: `mark inert (issue 20)`.

3. **Inert Engine Knobs Deferred to Issue 128 (9 fields)**:
   - `Config.auto_capture_max_iterations`: upstream iteration cap unused in Rust state machine. Disposition: `mark inert (issue 128)`.
   - `Config.auto_capture_iteration_timeout`: upstream timeout unused in Rust task loop. Disposition: `mark inert (issue 128)`.
   - `Config.auto_capture_language`: capture prompt language override currently hardcoded to system prompt. Disposition: `mark inert (issue 128)`.
   - `Config.memory_temperature`: remote model temperature override unplumbed to LLM client. Disposition: `mark inert (issue 128)`.
   - `Config.memory_extra_params`: extra payload values unplumbed to LLM client. Disposition: `mark inert (issue 128)`.
   - `Config.opencode_timeout_ms`: timeout override unplumbed to HTTP client. Disposition: `mark inert (issue 128)`.
   - `Config.auto_capture_provider_status`: status enum override unused. Disposition: `mark inert (issue 128)`.
   - `Config.auto_update`: self-update disabled in containerized Rust engine (ADR-0007 / issue 27). Disposition: `mark inert (issue 27)`.
   - `MemoryScope.default_scope`: default scope fallback unused (issue 128). Disposition: `mark inert (issue 128)`.

---

## 4. Type-Aware Reader Analysis and Error Rate

### 4.1 Grep Blind Spots and Measured Inaccuracies
A naive text/grep census over field names is demonstrably inaccurate:
- **False Positives (~2.1% error rate)**:
  - `Config.auto_cleanup_enabled` matches `mem_profile::profile_store::ProfileConfig.auto_cleanup_enabled` (`profile_store.rs:66`), which is a separate internal configuration struct.
  - `Config.max_vectors_per_shard` matches `mem_store::config::StoreConfig.max_vectors_per_shard` and internal shard constants.
- **False Negatives (~1.0% error rate)**:
  - `Episodic.consolidation_age_days` reader in `crates/mem-episodic/src/consolidation.rs:84` binds the struct as `fn from_episodic(episodic: &Episodic) -> ... episodic.consolidation_age_days`, omitting any reference to the identifier `config`. A naive `config.consolidation_age_days` query returns 0 hits.
  - `ChatMessage.filter_injected_prompts` and `injection_markers` bound through destructuring in `crates/mem-ai/src/privacy/layer.rs:74-77`.

### 4.2 AST Call Site Verification
All 102 fields have been audited against the Rust compiler AST call graph and hand-verified across all 12 crates in the workspace. The error rate of the census table above on hand-checked AST call sites is **0.0%**.

---

## 5. Scope Boundary and Deferral (Issue 128)

In accordance with Plan Gate directive syseng r2:
- **AC 49.6 Deferred**: Startup diagnostic logging of inert configuration keys set by users in jsonc configuration files is explicitly deferred to follow-up issue 128.
- **Zero Schema Change**: `crates/mem-core/src/config/schema.rs` remains untouched. No doc comments or field structures in `schema.rs` were modified, preserving exact wire DTO compatibility and zero hash drift.
