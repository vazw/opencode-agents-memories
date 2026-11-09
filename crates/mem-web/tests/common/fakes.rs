#![allow(dead_code)]

//! In-memory fakes for the eight W6.2 port seams.
//!
//! One [`FakeBackend`] implements every port; [`fake_services`] wraps it in
//! [`ApiServices`]. Every mutating call is recorded in [`FakeBackend::calls`]
//! so a test can prove a rejected request never reached the store
//! (upstream's `writeShardCalls.length === 0`).
//!
//! The seed is the W6.2 walk's canonical fixture (blueprint Step 14):
//! a project shard and a user shard (`c1c2c3d4e5f67890`), a memory
//! [`MEMORY_ID`], a user-scope memory [`USER_MEMORY_ID`], a prompt
//! [`PROMPT_ID`], the skill [`SKILL_ID`], a profile with one preference and
//! a pending cleanup, and the changelog [`CHANGELOG_ID`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use mem_core::scope::Scope;
use mem_core::tags::migration::{
    BatchOutcome, MemoryRecord, MigrationProgress, MigrationSink, TagGeneration, TagMigrator,
};
use mem_embed::embedder::EmbedTask;
use mem_episodic::milestone::{Contributor, Milestone};
use serde_json::{Value, json};

use mem_web::api::dto::{MilestoneItemDto, ProjectRowDto};
use mem_web::api::ports::{
    ApiServices, CatalogEntry, CleanupApplied, CleanupPreview, DedupReport, EmbedPort,
    MaintenancePort, MemoryPort, MemoryRow, MemoryStatsRow, MemoryUpdate, MigrationReport,
    MigrationRunReport, MilestonePort, NewMemory, NewSkill, PortError, ProfileChangelog,
    ProfileErasure, ProfilePort, ProfileRecord, PromptErasure, PromptPort, PromptRow, SearchHit,
    SessionPort, ShardRef, Skill, SkillPatch, SkillPort, TagMigrationReport, TagRow,
};

/// The seeded project scope hash (16 lowercase hex).
pub const PROJECT_HASH: &str = "c1c2c3d4e5f67890";
/// The seeded user scope hash.
pub const USER_HASH: &str = "c1c2c3d4e5f67890";
/// The seeded project container tag.
pub const PROJECT_TAG: &str = "mem_project_c1c2c3d4e5f67890";
/// The seeded user container tag.
pub const USER_TAG: &str = "opencode_user_c1c2c3d4e5f67890";
/// The seeded project memory id.
pub const MEMORY_ID: &str = "mem_probe_1";
/// The seeded user-scope memory id.
pub const USER_MEMORY_ID: &str = "mem_user_scope_1";
/// The seeded prompt id.
pub const PROMPT_ID: &str = "prompt_probe_1";
/// The seeded skill id (GET/PUT/DELETE and the catalog file).
pub const SKILL_ID: &str = "ship-it";
/// The fresh id `POST /api/skills` uses (the seed keeps `ship-it` intact).
pub const NEW_SKILL_ID: &str = "probe-skill";
/// The seeded changelog id / profile id (`GET …/changelog?profileId=probe`
/// and `GET …/snapshot?chlogId=probe`).
pub const CHANGELOG_ID: &str = "probe";
/// The seeded user identity.
pub const USER_ID: &str = "probe@example.com";
/// The second identity [`FakeBackend::seed_second_profile`] adds.
pub const SECOND_USER_ID: &str = "second@example.com";

const NOW: i64 = 1_700_000_000_000;

/// The seed profile payload (carries `centroid`/`anchor` on purpose — the
/// handlers must strip them).
fn seeded_profile_data() -> Value {
    json!({
        "preferences": [{
            "id": "pref_0",
            "category": "tools",
            "description": "prefer Rust",
            "confidence": 0.9,
            "centroid": [0.1, 0.2],
            "anchor": [0.3, 0.4]
        }],
        "patterns": [{
            "id": "pat_0",
            "description": "asks for diffs",
            "frequency": 4,
            "centroid": [0.5],
            "anchor": [0.6]
        }],
        "workflows": [{
            "id": "wf_0",
            "description": "review then commit",
            "frequency": 2,
            "steps": ["review", "commit"],
            "centroid": [0.7],
            "anchor": [0.8]
        }]
    })
}

struct FakeState {
    shards: Vec<ShardRef>,
    memories: Vec<(ShardRef, MemoryRow)>,
    prompts: Vec<PromptRow>,
    skills: HashMap<String, Skill>,
    /// Every stored `user_profiles` row (the store's `is_active = 1` set).
    profiles: Vec<ProfileRecord>,
    /// The project roster the `projects` port answers (issue 116).
    projects: Vec<ProjectRowDto>,
    changelogs: Vec<ProfileChangelog>,
    pending_cleanup: Option<CleanupPreview>,
    pending_user: Option<String>,
    /// The tier's `sweep_state` stamp (issue 43 W6c): `None` = never swept.
    sweep_stamp: Option<i64>,
    /// Makes `sweep_state` answer a store failure (the `sweepFailed` arm).
    sweep_read_fails: bool,
}

/// The shared fake backend.
pub struct FakeBackend {
    state: Mutex<FakeState>,
    migrator: Arc<Mutex<TagMigrator>>,
    calls: Mutex<Vec<String>>,
}

impl FakeBackend {
    /// Builds the canonical W6.2 fixture.
    pub fn new() -> Self {
        let project = ShardRef {
            scope: Scope::Project,
            scope_hash: PROJECT_HASH.to_string(),
            shard_index: 0,
            db_path: std::path::PathBuf::from("/tmp/fake-project.db"),
            shard_id: Some(1),
            vector_count: 1,
            is_active: true,
        };
        let user = ShardRef {
            scope: Scope::User,
            scope_hash: USER_HASH.to_string(),
            shard_index: 0,
            db_path: std::path::PathBuf::from("/tmp/fake-user.db"),
            shard_id: Some(2),
            vector_count: 1,
            is_active: true,
        };

        let project_memory = MemoryRow {
            id: MEMORY_ID.to_string(),
            content: "probe memory content".to_string(),
            container_tag: PROJECT_TAG.to_string(),
            tags: Some("probe".to_string()),
            memory_type: Some("note".to_string()),
            created_at: NOW,
            updated_at: Some(NOW),
            metadata: Some(r#"{"source":"api"}"#.to_string()),
            session_id: None,
            display_name: Some("demo-app".to_string()),
            user_name: None,
            user_email: None,
            project_path: Some("/tmp/probe".to_string()),
            project_name: Some("demo-app".to_string()),
            git_repo_url: None,
            is_pinned: false,
            has_tags_vector: true,
        };
        let user_memory = MemoryRow {
            id: USER_MEMORY_ID.to_string(),
            content: "user scoped memory".to_string(),
            container_tag: USER_TAG.to_string(),
            tags: None,
            memory_type: Some("user".to_string()),
            created_at: NOW + 1,
            updated_at: Some(NOW + 1),
            metadata: None,
            session_id: None,
            display_name: Some("Ada".to_string()),
            user_name: Some("Ada".to_string()),
            user_email: Some(USER_ID.to_string()),
            project_path: None,
            project_name: None,
            git_repo_url: None,
            is_pinned: false,
            has_tags_vector: false,
        };

        let prompt = PromptRow {
            id: PROMPT_ID.to_string(),
            session_id: "session_probe".to_string(),
            content: "probe prompt content".to_string(),
            created_at: NOW,
            project_path: Some("/tmp/probe".to_string()),
            linked_memory_id: None,
        };

        let skill = Skill {
            id: SKILL_ID.to_string(),
            name: "Ship It".to_string(),
            description: "Release flow".to_string(),
            content: "## Ship\n\n1. Build\n2. Tag".to_string(),
            autoinvoke: true,
            enabled: true,
            container_tag: Some(PROJECT_TAG.to_string()),
            created_at: NOW,
            updated_at: NOW,
        };

        let profile = ProfileRecord {
            id: "profile_1".to_string(),
            user_id: USER_ID.to_string(),
            display_name: "Probe".to_string(),
            user_name: "probe".to_string(),
            user_email: USER_ID.to_string(),
            profile_data: seeded_profile_data().to_string(),
            version: 3,
            created_at: NOW,
            last_analyzed_at: NOW,
            total_prompts_analyzed: 12,
        };

        let changelog = ProfileChangelog {
            id: CHANGELOG_ID.to_string(),
            profile_id: CHANGELOG_ID.to_string(),
            version: 2,
            change_type: "update".to_string(),
            change_summary: "probe changelog".to_string(),
            profile_data_snapshot: seeded_profile_data().to_string(),
            created_at: NOW,
        };

        let mut skills = HashMap::new();
        skills.insert(skill.id.clone(), skill);

        // Issue 116: two projects, one of them path-shaped, with distinct
        // counts so a dropped or renamed field cannot pass unnoticed.
        let projects = vec![
            ProjectRowDto {
                project_id: "/home/vaz/Code/demo".to_string(),
                session_count: 4,
                milestone_count: 2,
                last_activity_at: 300,
            },
            ProjectRowDto {
                project_id: "/home/vaz/x".to_string(),
                session_count: 1,
                milestone_count: 0,
                last_activity_at: 100,
            },
        ];

        Self {
            state: Mutex::new(FakeState {
                shards: vec![project.clone(), user.clone()],
                memories: vec![(project, project_memory), (user, user_memory)],
                prompts: vec![prompt],
                skills,
                profiles: vec![profile],
                changelogs: vec![changelog],
                projects,
                pending_cleanup: None,
                pending_user: None,
                sweep_stamp: None,
                sweep_read_fails: false,
            }),
            migrator: Arc::new(Mutex::new(TagMigrator::new())),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// The recorded call log.
    pub fn calls(&self) -> Vec<String> {
        self.calls
            .lock()
            .map(|calls| calls.clone())
            .unwrap_or_default()
    }

    /// The seeded project shard.
    pub fn project_shard(&self) -> ShardRef {
        self.state
            .lock()
            .expect("fake state lock")
            .shards
            .iter()
            .find(|shard| shard.scope == Scope::Project)
            .cloned()
            .expect("the project shard is seeded")
    }

    /// The seeded user shard.
    pub fn user_shard(&self) -> ShardRef {
        self.state
            .lock()
            .expect("fake state lock")
            .shards
            .iter()
            .find(|shard| shard.scope == Scope::User)
            .cloned()
            .expect("the user shard is seeded")
    }

    /// A snapshot of one memory row.
    pub fn memory(&self, id: &str) -> Option<MemoryRow> {
        self.state
            .lock()
            .expect("fake state lock")
            .memories
            .iter()
            .find(|(_, row)| row.id == id)
            .map(|(_, row)| row.clone())
    }

    fn record(&self, call: impl Into<String>) {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(call.into());
        }
    }

    fn shard_for(state: &FakeState, scope: Scope, hash: &str) -> ShardRef {
        state
            .shards
            .iter()
            .find(|shard| shard.scope == scope && shard.scope_hash == hash)
            .cloned()
            .unwrap_or_else(|| ShardRef {
                scope,
                scope_hash: hash.to_string(),
                shard_index: 0,
                db_path: std::path::PathBuf::from(format!("/tmp/fake-{hash}.db")),
                shard_id: None,
                vector_count: 0,
                is_active: true,
            })
    }

    /// Adds a shard to the fixture (a test-only seed, never recorded).
    pub fn add_shard(&self, scope: Scope, hash: &str) -> ShardRef {
        let mut state = self.state.lock().expect("fake state lock");
        let shard = Self::shard_for(&state, scope, hash);
        if !state.shards.contains(&shard) {
            state.shards.push(shard.clone());
        }
        shard
    }

    /// Adds a memory row to the fixture (a test-only seed, never recorded).
    pub fn seed_memory(&self, shard: &ShardRef, row: MemoryRow) {
        let mut state = self.state.lock().expect("fake state lock");
        state.memories.push((shard.clone(), row));
    }

    /// Adds a prompt row to the fixture (a test-only seed, never recorded).
    pub fn seed_prompt(&self, prompt: PromptRow) {
        let mut state = self.state.lock().expect("fake state lock");
        state.prompts.push(prompt);
    }

    /// Seeds the tier's sweep stamp (issue 43 W6c): `Some` = a completed pass,
    /// `None` = never swept (the default).
    pub fn set_sweep_stamp(&self, stamp: Option<i64>) {
        self.state.lock().expect("fake state lock").sweep_stamp = stamp;
    }

    /// Makes `sweep_state` answer a store failure (the D-19-3 `sweepFailed` arm).
    pub fn set_sweep_read_failure(&self, fails: bool) {
        self.state.lock().expect("fake state lock").sweep_read_fails = fails;
    }

    /// Replaces the seeded profile's `profile_data` (a test-only seed, never
    /// recorded).
    pub fn set_profile_data(&self, data: Value) {
        let mut state = self.state.lock().expect("fake state lock");
        if let Some(profile) = state.profiles.first_mut() {
            profile.profile_data = data.to_string();
        }
    }

    /// Replaces the seeded profile's raw `profile_data` text (a test-only
    /// seed, never recorded). Unlike [`Self::set_profile_data`] this accepts
    /// any string, so a test can seed broken JSON.
    pub fn set_profile_raw(&self, raw: String) {
        let mut state = self.state.lock().expect("fake state lock");
        if let Some(profile) = state.profiles.first_mut() {
            profile.profile_data = raw;
        }
    }

    /// Removes the seeded profile so `active_profile` answers `Ok(None)`
    /// (a test-only seed, never recorded).
    pub fn clear_profile(&self) {
        self.state.lock().expect("fake state lock").profiles.clear();
    }

    /// Adds one stored profile row (a test-only seed, never recorded).
    pub fn seed_profile(&self, profile: ProfileRecord) {
        self.state
            .lock()
            .expect("fake state lock")
            .profiles
            .push(profile);
    }

    /// Adds the second canonical profile ([`SECOND_USER_ID`]) beside the
    /// seeded one (a test-only seed, never recorded).
    pub fn seed_second_profile(&self) {
        self.seed_profile(ProfileRecord {
            id: "profile_2".to_string(),
            user_id: SECOND_USER_ID.to_string(),
            display_name: "Second".to_string(),
            user_name: "second".to_string(),
            user_email: SECOND_USER_ID.to_string(),
            profile_data: seeded_profile_data().to_string(),
            version: 1,
            created_at: NOW,
            last_analyzed_at: NOW,
            total_prompts_analyzed: 4,
        });
    }

    /// Points one seeded shard's `db_path` at `path` (a test-only seed, never
    /// recorded) so a stats test can back it with a real (possibly missing)
    /// file under its own temp dir.
    pub fn set_shard_db_path(&self, scope: Scope, hash: &str, path: std::path::PathBuf) {
        let mut state = self.state.lock().expect("fake state lock");
        if let Some(shard) = state
            .shards
            .iter_mut()
            .find(|shard| shard.scope == scope && shard.scope_hash == hash)
        {
            shard.db_path = path;
        }
    }

    /// Sets one seeded shard's registered vector count and active flag (a
    /// test-only seed, never recorded).
    pub fn set_shard_meta(&self, scope: Scope, hash: &str, vector_count: u64, is_active: bool) {
        let mut state = self.state.lock().expect("fake state lock");
        if let Some(shard) = state
            .shards
            .iter_mut()
            .find(|shard| shard.scope == scope && shard.scope_hash == hash)
        {
            shard.vector_count = vector_count;
            shard.is_active = is_active;
        }
    }

    /// Queues a pending-cleanup session for [`USER_ID`] (a test-only seed,
    /// never recorded) so `POST /api/user-profile/ai-cleanup/apply` reaches
    /// its documented success without running a provider-backed cleanup first.
    pub fn seed_pending_cleanup(&self) {
        let mut state = self.state.lock().expect("fake state lock");
        let profile_data = state
            .profiles
            .first()
            .map(|profile| profile.profile_data.clone())
            .unwrap_or_else(|| "{}".to_string());
        let parsed: Value = serde_json::from_str(&profile_data).unwrap_or_else(|_| json!({}));
        state.pending_cleanup = Some(CleanupPreview {
            old: parsed.clone(),
            cleaned: mem_profile::profile_utils::to_public_profile_data(&parsed),
            changes: json!({"kept": [], "merged": [], "removed": []}),
        });
        state.pending_user = Some(USER_ID.to_string());
    }
}

/// A defaulted memory row for fixture seeds (`id`, `container_tag` only).
pub fn memory_row(id: &str, container_tag: &str) -> MemoryRow {
    MemoryRow {
        id: id.to_string(),
        content: format!("content of {id}"),
        container_tag: container_tag.to_string(),
        tags: None,
        memory_type: None,
        created_at: NOW,
        updated_at: None,
        metadata: None,
        session_id: None,
        display_name: None,
        user_name: None,
        user_email: None,
        project_path: None,
        project_name: None,
        git_repo_url: None,
        is_pinned: false,
        has_tags_vector: false,
    }
}

impl Default for FakeBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// The seeded milestone row: one contributor, no promoted facts (the fake
/// serves the roster only; the `facts.db` join is exercised over the real
/// stores in `api_milestones.rs`).
pub fn seeded_milestone() -> MilestoneItemDto {
    MilestoneItemDto {
        milestone: Milestone {
            id: "ms_probe_1".to_string(),
            project_id: "probe-project".to_string(),
            agent_id: "agent-alpha".to_string(),
            summary: "probe milestone".to_string(),
            contributors: vec![Contributor {
                session_row_id: "row_probe_1".to_string(),
                session_id: "session-probe-1".to_string(),
                parent_session_id: None,
                agent_id: "agent-alpha".to_string(),
                agent_name: "Alpha".to_string(),
                created_at: NOW,
                summary: "probe contribution".to_string(),
            }],
            source_count: 1,
            window_start: NOW,
            window_end: NOW,
            embedding: Vec::new(),
            created_at: NOW,
        },
        promoted_facts: Vec::new(),
    }
}

/// Wraps a fresh backend in the eight-port bundle.
pub fn fake_services() -> (Arc<FakeBackend>, ApiServices) {
    let backend = Arc::new(FakeBackend::new());
    let services = ApiServices {
        memory: backend.clone(),
        prompts: backend.clone(),
        profile: backend.clone(),
        skills: backend.clone(),
        embed: backend.clone(),
        maintenance: backend.clone(),
        sessions: backend.clone(),
        milestones: backend.clone(),
    };
    (backend, services)
}

#[async_trait]
impl MemoryPort for FakeBackend {
    async fn shards(
        &self,
        scope: Option<Scope>,
        scope_hash: Option<&str>,
    ) -> Result<Vec<ShardRef>, PortError> {
        self.record(format!("shards:{:?}:{}", scope, scope_hash.unwrap_or("")));
        let state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        Ok(state
            .shards
            .iter()
            .filter(|shard| scope.is_none_or(|scope| shard.scope == scope))
            .filter(|shard| match scope_hash {
                None | Some("") => true,
                Some(hash) => shard.scope_hash == hash,
            })
            .cloned()
            .collect())
    }

    /// The fake has no stored facts to judge; the authority is exercised over
    /// the production adapter in `shard_model_authority.rs`.
    async fn verify_shards(&self, _shards: &[ShardRef]) -> Result<(), PortError> {
        Ok(())
    }

    async fn all_memories(&self, shard: &ShardRef) -> Result<Vec<MemoryRow>, PortError> {
        self.record(format!("all_memories:{}", shard.scope_hash));
        let state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        Ok(state
            .memories
            .iter()
            .filter(|(owner, _)| owner == shard)
            .map(|(_, row)| row.clone())
            .collect())
    }

    async fn stats_memories(&self, shard: &ShardRef) -> Result<Vec<MemoryStatsRow>, PortError> {
        self.record(format!("stats_memories:{}", shard.scope_hash));
        let state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        Ok(state
            .memories
            .iter()
            .filter(|(owner, _)| owner == shard)
            .map(|(_, row)| MemoryStatsRow {
                container_tag: row.container_tag.clone(),
                memory_type: row.memory_type.clone(),
                created_at: row.created_at,
                tags: row.tags.clone(),
            })
            .collect())
    }

    async fn list_memories_by_container_tag(
        &self,
        shard: &ShardRef,
        container_tag: &str,
        limit: usize,
    ) -> Result<Vec<MemoryRow>, PortError> {
        self.record(format!(
            "list_memories_by_container_tag:{}:{}",
            shard.scope_hash, container_tag
        ));
        let state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        Ok(state
            .memories
            .iter()
            .filter(|(owner, row)| owner == shard && row.container_tag == container_tag)
            .take(limit)
            .map(|(_, row)| row.clone())
            .collect())
    }

    async fn memory_by_id(
        &self,
        shard: &ShardRef,
        id: &str,
    ) -> Result<Option<MemoryRow>, PortError> {
        self.record(format!("memory_by_id:{}:{id}", shard.scope_hash));
        let state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        Ok(state
            .memories
            .iter()
            .find(|(owner, row)| owner == shard && row.id == id)
            .map(|(_, row)| row.clone()))
    }

    async fn distinct_tags(&self, shard: &ShardRef) -> Result<Vec<TagRow>, PortError> {
        self.record(format!("distinct_tags:{}", shard.scope_hash));
        let state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        let mut seen = Vec::new();
        let mut rows = Vec::new();
        for (owner, row) in &state.memories {
            if owner != shard || seen.contains(&row.container_tag) {
                continue;
            }
            seen.push(row.container_tag.clone());
            rows.push(TagRow {
                container_tag: row.container_tag.clone(),
                display_name: row.display_name.clone(),
                user_name: row.user_name.clone(),
                user_email: row.user_email.clone(),
                project_path: row.project_path.clone(),
                project_name: row.project_name.clone(),
                git_repo_url: row.git_repo_url.clone(),
            });
        }
        Ok(rows)
    }

    async fn insert_memory(
        &self,
        scope: Scope,
        scope_hash: &str,
        memory: NewMemory,
    ) -> Result<(), PortError> {
        self.record(format!("insert_memory:{}:{scope_hash}", scope.as_str()));
        let mut state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        let shard = Self::shard_for(&state, scope, scope_hash);
        if !state.shards.contains(&shard) {
            state.shards.push(shard.clone());
        }
        state.memories.push((
            shard,
            MemoryRow {
                id: memory.id,
                content: memory.content,
                container_tag: memory.container_tag,
                tags: memory.tags,
                memory_type: memory.memory_type,
                created_at: memory.created_at,
                updated_at: Some(memory.updated_at),
                metadata: memory.metadata,
                session_id: memory.session_id,
                display_name: memory.display_name,
                user_name: memory.user_name,
                user_email: memory.user_email,
                project_path: memory.project_path,
                project_name: memory.project_name,
                git_repo_url: memory.git_repo_url,
                is_pinned: false,
                has_tags_vector: memory.tags_vector.is_some(),
            },
        ));
        Ok(())
    }

    async fn update_memory(&self, shard: &ShardRef, update: MemoryUpdate) -> Result<(), PortError> {
        self.record(format!("update_memory:{}", update.id));
        let mut state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        let Some((_, row)) = state
            .memories
            .iter_mut()
            .find(|(owner, row)| owner == shard && row.id == update.id)
        else {
            return Err(PortError::NotFound);
        };
        row.content = update.content;
        row.container_tag = update.container_tag;
        row.tags = update.tags;
        row.memory_type = update.memory_type;
        row.updated_at = Some(update.updated_at);
        row.metadata = update.metadata;
        row.has_tags_vector = update.tags_vector.is_some();
        Ok(())
    }

    async fn delete_memory(&self, shard: &ShardRef, id: &str) -> Result<(), PortError> {
        self.record(format!("delete_memory:{id}"));
        let mut state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        state
            .memories
            .retain(|(owner, row)| !(owner == shard && row.id == id));
        Ok(())
    }

    async fn pin_memory(&self, shard: &ShardRef, id: &str) -> Result<(), PortError> {
        self.record(format!("pin_memory:{id}"));
        self.set_pinned(shard, id, true)
    }

    async fn unpin_memory(&self, shard: &ShardRef, id: &str) -> Result<(), PortError> {
        self.record(format!("unpin_memory:{id}"));
        self.set_pinned(shard, id, false)
    }

    async fn search_in_shard(
        &self,
        shard: &ShardRef,
        _query_vector: &[f32],
        container_tag: &str,
        limit: usize,
        _query_text: Option<&str>,
    ) -> Result<Vec<SearchHit>, PortError> {
        self.record(format!(
            "search_in_shard:{}:{container_tag}",
            shard.scope_hash
        ));
        let state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        Ok(state
            .memories
            .iter()
            .filter(|(owner, row)| owner == shard && row.container_tag == container_tag)
            .take(limit)
            .enumerate()
            .map(|(index, (_, row))| SearchHit {
                id: row.id.clone(),
                content: row.content.clone(),
                similarity: 0.9 - (index as f64 * 0.01),
                created_at: row.created_at,
                tags: row
                    .tags
                    .as_deref()
                    .map(|tags| tags.split(',').map(|tag| tag.trim().to_string()).collect())
                    .unwrap_or_default(),
                metadata: row
                    .metadata
                    .as_deref()
                    .and_then(|raw| serde_json::from_str(raw).ok()),
                container_tag: row.container_tag.clone(),
                display_name: row.display_name.clone(),
                user_name: row.user_name.clone(),
                user_email: row.user_email.clone(),
                project_path: row.project_path.clone(),
                project_name: row.project_name.clone(),
                git_repo_url: row.git_repo_url.clone(),
                is_pinned: row.is_pinned,
            })
            .collect())
    }
}

impl FakeBackend {
    fn set_pinned(&self, shard: &ShardRef, id: &str, pinned: bool) -> Result<(), PortError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        let Some((_, row)) = state
            .memories
            .iter_mut()
            .find(|(owner, row)| owner == shard && row.id == id)
        else {
            return Err(PortError::NotFound);
        };
        row.is_pinned = pinned;
        Ok(())
    }
}

#[async_trait]
impl PromptPort for FakeBackend {
    async fn captured_prompts(
        &self,
        project_path: Option<&str>,
    ) -> Result<Vec<PromptRow>, PortError> {
        self.record(format!("captured_prompts:{}", project_path.unwrap_or("")));
        let state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        Ok(state
            .prompts
            .iter()
            .filter(|prompt| match project_path {
                None => true,
                Some(path) => prompt.project_path.as_deref() == Some(path),
            })
            .cloned()
            .collect())
    }

    async fn search_prompts(
        &self,
        query: &str,
        project_path: Option<&str>,
        limit: usize,
    ) -> Result<Vec<PromptRow>, PortError> {
        self.record(format!("search_prompts:{query}"));
        let state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        Ok(state
            .prompts
            .iter()
            .filter(|prompt| {
                prompt
                    .content
                    .to_lowercase()
                    .contains(&query.to_lowercase())
            })
            .filter(|prompt| match project_path {
                None => true,
                Some(path) => prompt.project_path.as_deref() == Some(path),
            })
            .take(limit)
            .cloned()
            .collect())
    }

    async fn prompts_by_ids(&self, ids: &[String]) -> Result<Vec<PromptRow>, PortError> {
        self.record(format!("prompts_by_ids:{}", ids.len()));
        let state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        Ok(state
            .prompts
            .iter()
            .filter(|prompt| ids.contains(&prompt.id))
            .cloned()
            .collect())
    }

    async fn prompt_by_id(&self, id: &str) -> Result<Option<PromptRow>, PortError> {
        self.record(format!("prompt_by_id:{id}"));
        let state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        Ok(state.prompts.iter().find(|prompt| prompt.id == id).cloned())
    }

    async fn delete_prompt(&self, id: &str) -> Result<PromptErasure, PortError> {
        self.record(format!("delete_prompt:{id}"));
        let mut state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        let existed = state.prompts.iter().any(|prompt| prompt.id == id);
        state.prompts.retain(|prompt| prompt.id != id);
        Ok(PromptErasure {
            existed,
            ..PromptErasure::default()
        })
    }

    async fn sweep_state(&self, tier: &str) -> Result<Option<i64>, PortError> {
        self.record(format!("sweep_state:{tier}"));
        let state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        if state.sweep_read_fails {
            return Err(PortError::Store("sweep_state read failed".into()));
        }
        Ok(state.sweep_stamp)
    }
}

#[async_trait]
impl ProfilePort for FakeBackend {
    async fn active_profile(&self, user_id: &str) -> Result<Option<ProfileRecord>, PortError> {
        self.record(format!("active_profile:{user_id}"));
        let state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        Ok(state
            .profiles
            .iter()
            .find(|profile| profile.user_id == user_id)
            .cloned())
    }

    async fn active_profiles(&self) -> Result<Vec<ProfileRecord>, PortError> {
        self.record("active_profiles".to_string());
        let state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        Ok(state.profiles.clone())
    }

    async fn delete_profile(&self, profile_id: &str) -> Result<ProfileErasure, PortError> {
        self.record(format!("delete_profile:{profile_id}"));
        let mut state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        let before = state.changelogs.len();
        state
            .changelogs
            .retain(|changelog| changelog.profile_id != profile_id);
        let changelogs_deleted = (before - state.changelogs.len()) as u64;
        let before_profiles = state.profiles.len();
        state.profiles.retain(|profile| profile.id != profile_id);
        let profiles_deleted = (before_profiles - state.profiles.len()) as u64;
        Ok(ProfileErasure {
            profiles_deleted,
            changelogs_deleted,
        })
    }

    async fn changelogs(
        &self,
        profile_id: &str,
        limit: usize,
    ) -> Result<Vec<ProfileChangelog>, PortError> {
        self.record(format!("changelogs:{profile_id}"));
        let state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        Ok(state
            .changelogs
            .iter()
            .filter(|changelog| changelog.profile_id == profile_id)
            .take(limit)
            .cloned()
            .collect())
    }

    async fn changelog_by_id(&self, id: &str) -> Result<Option<ProfileChangelog>, PortError> {
        self.record(format!("changelog_by_id:{id}"));
        let state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        Ok(state
            .changelogs
            .iter()
            .find(|changelog| changelog.id == id)
            .cloned())
    }

    async fn update_profile(
        &self,
        profile_id: &str,
        profile_data: &Value,
        _change_summary: &str,
    ) -> Result<bool, PortError> {
        self.record(format!("update_profile:{profile_id}"));
        let mut state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        let Some(profile) = state
            .profiles
            .iter_mut()
            .find(|profile| profile.id == profile_id)
        else {
            return Ok(false);
        };
        profile.profile_data = profile_data.to_string();
        profile.version += 1;
        Ok(true)
    }

    async fn decay(&self, profile_data: &Value) -> Result<(Value, bool), PortError> {
        Ok((profile_data.clone(), false))
    }

    async fn ai_cleanup(
        &self,
        user_id: &str,
        _profile_version: i64,
        profile_data: &Value,
        _include_ids: Option<Vec<String>>,
    ) -> Result<CleanupPreview, PortError> {
        self.record(format!("ai_cleanup:{user_id}"));
        let mut state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        let preview = CleanupPreview {
            old: profile_data.clone(),
            cleaned: mem_profile::profile_utils::to_public_profile_data(profile_data),
            changes: json!({"merged": [], "removed": []}),
        };
        state.pending_cleanup = Some(preview.clone());
        state.pending_user = Some(user_id.to_string());
        Ok(preview)
    }

    async fn apply_cleanup(
        &self,
        user_id: &str,
        _body: &Value,
    ) -> Result<CleanupApplied, PortError> {
        self.record(format!("apply_cleanup:{user_id}"));
        let mut state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        let Some(preview) = state.pending_cleanup.take() else {
            return Err(PortError::Invalid(
                "No pending cleanup found. Run AI cleanup first.".to_string(),
            ));
        };
        let Some(profile) = state
            .profiles
            .iter_mut()
            .find(|profile| profile.user_id == user_id)
        else {
            return Err(PortError::Invalid("Profile not found".to_string()));
        };
        profile.profile_data = preview.cleaned.to_string();
        profile.version += 1;
        Ok(CleanupApplied {
            message: "Cleanup applied successfully".to_string(),
            version: profile.version,
        })
    }
}

#[async_trait]
impl SkillPort for FakeBackend {
    async fn list(
        &self,
        container_tag: Option<&str>,
        enabled_only: bool,
    ) -> Result<Vec<Skill>, PortError> {
        self.record(format!(
            "list_skills:{}:{enabled_only}",
            container_tag.unwrap_or("")
        ));
        let state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        let mut skills: Vec<Skill> = state.skills.values().cloned().collect();
        skills.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(skills
            .into_iter()
            .filter(|skill| match container_tag {
                None => true,
                Some(tag) => skill.container_tag.as_deref() == Some(tag),
            })
            .filter(|skill| !enabled_only || skill.enabled)
            .collect())
    }

    async fn get(&self, id: &str) -> Result<Option<Skill>, PortError> {
        self.record(format!("get_skill:{id}"));
        let state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        Ok(state.skills.get(id).cloned())
    }

    async fn create(&self, skill: NewSkill) -> Result<Skill, PortError> {
        self.record(format!("create_skill:{}", skill.id));
        let mut state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        let created = Skill {
            id: skill.id,
            name: skill.name,
            description: skill.description.unwrap_or_default(),
            content: skill.content,
            autoinvoke: skill.autoinvoke.unwrap_or(false),
            enabled: skill.enabled.unwrap_or(true),
            container_tag: skill.container_tag,
            created_at: NOW,
            updated_at: NOW,
        };
        state.skills.insert(created.id.clone(), created.clone());
        Ok(created)
    }

    async fn update(&self, id: &str, patch: SkillPatch) -> Result<Skill, PortError> {
        self.record(format!("update_skill:{id}"));
        let mut state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        let Some(skill) = state.skills.get_mut(id) else {
            return Err(PortError::Invalid("Skill not found".to_string()));
        };
        if let Some(name) = patch.name {
            skill.name = name;
        }
        if let Some(description) = patch.description {
            skill.description = description;
        }
        if let Some(content) = patch.content {
            skill.content = content;
        }
        if let Some(autoinvoke) = patch.autoinvoke {
            skill.autoinvoke = autoinvoke;
        }
        if let Some(enabled) = patch.enabled {
            skill.enabled = enabled;
        }
        if let Some(container_tag) = patch.container_tag {
            skill.container_tag = Some(container_tag);
        }
        skill.updated_at = NOW + 1;
        Ok(skill.clone())
    }

    async fn delete(&self, id: &str) -> Result<bool, PortError> {
        self.record(format!("delete_skill:{id}"));
        let mut state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        Ok(state.skills.remove(id).is_some())
    }

    async fn catalog_index(&self) -> Result<Vec<CatalogEntry>, PortError> {
        let state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        let mut skills: Vec<&Skill> = state.skills.values().collect();
        skills.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(skills
            .into_iter()
            .map(|skill| CatalogEntry {
                name: skill.id.clone(),
                version: "1.0.0".to_string(),
                files: vec![format!("{}.md", skill.id)],
            })
            .collect())
    }

    async fn catalog_markdown(&self, id: &str) -> Result<Option<String>, PortError> {
        let state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        Ok(state
            .skills
            .get(id)
            .map(|skill| format!("# {}\n\n{}", skill.name, skill.content)))
    }
}

#[async_trait]
impl EmbedPort for FakeBackend {
    async fn warmup(&self) -> Result<(), PortError> {
        Ok(())
    }

    async fn embed(&self, _text: &str, _task: EmbedTask) -> Result<Vec<f32>, PortError> {
        let mut vector = vec![0.0_f32; 768];
        vector[0] = 1.0;
        Ok(vector)
    }
}

#[async_trait]
impl MaintenancePort for FakeBackend {
    async fn detect_migration(&self) -> Result<MigrationReport, PortError> {
        Ok(MigrationReport {
            needs_migration: false,
            config_dimensions: 768,
            config_model: "fake-model".to_string(),
            shard_mismatches: Vec::new(),
        })
    }

    async fn run_migration(&self, strategy: &str) -> Result<MigrationRunReport, PortError> {
        self.record(format!("run_migration:{strategy}"));
        Ok(MigrationRunReport {
            success: true,
            strategy: strategy.to_string(),
            deleted_shards: 0,
            re_embedded_memories: 0,
            duration_ms: 5,
            error: None,
        })
    }

    async fn detect_tag_migration(&self) -> Result<TagMigrationReport, PortError> {
        let state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        let count = state
            .memories
            .iter()
            .filter(|(owner, _)| owner.scope == Scope::Project)
            .filter(|(_, row)| row.tags.as_deref().is_none_or(|tags| tags.is_empty()))
            .count() as u64;
        Ok(TagMigrationReport {
            needs_migration: count > 0,
            count,
        })
    }

    async fn tag_migration_progress(&self) -> Result<MigrationProgress, PortError> {
        let migrator = self
            .migrator
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        Ok(migrator.progress().clone())
    }

    async fn tag_migration_records(&self) -> Result<Vec<MemoryRecord>, PortError> {
        let state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        Ok(state
            .memories
            .iter()
            .filter(|(owner, _)| owner.scope == Scope::Project)
            .map(|(_, row)| MemoryRecord {
                id: row.id.clone(),
                content: row.content.clone(),
                tags: row.tags.clone(),
                has_tags_vector: row.has_tags_vector,
            })
            .collect())
    }

    async fn generate_tags(&self, memory_id: &str, _content: &str) -> Result<Vec<String>, String> {
        self.record(format!("generate_tags:{memory_id}"));
        Ok(vec!["probe".to_string(), "memory".to_string()])
    }

    async fn run_tag_migration_batch(
        &self,
        batch_size: usize,
        records: Vec<MemoryRecord>,
        tags: HashMap<String, Result<Vec<String>, String>>,
    ) -> Result<BatchOutcome, PortError> {
        self.record(format!("run_tag_migration_batch:{batch_size}"));
        let migrator = Arc::clone(&self.migrator);
        tokio::task::spawn_blocking(move || {
            let mut migrator = migrator
                .lock()
                .map_err(|_| PortError::Store("migrator lock poisoned".into()))?;
            let generation = PrecomputedGeneration { tags };
            let mut sink = NoopSink;
            Ok(migrator.run_batch(batch_size, &records, &generation, &mut sink))
        })
        .await
        .map_err(|error| PortError::Store(format!("tag migration task failed: {error}")))?
    }

    async fn cleanup(&self) -> Result<mem_web::api::ports::CleanupReport, PortError> {
        Err(PortError::Unavailable)
    }

    async fn deduplicate(&self) -> Result<DedupReport, PortError> {
        // The handler is real: the fake records the call and still answers
        // `Unavailable`, so the 501 test proves the port was reached.
        self.record("deduplicate".to_string());
        Err(PortError::Unavailable)
    }
}

#[async_trait]
impl SessionPort for FakeBackend {
    async fn list_sessions(&self, _project_id: &str) -> Result<Vec<Value>, PortError> {
        Err(PortError::Unavailable)
    }

    async fn delete_session(&self, _project_id: &str, _session_id: &str) -> Result<(), PortError> {
        Err(PortError::Unavailable)
    }

    async fn projects(&self, limit: usize) -> Result<Vec<ProjectRowDto>, PortError> {
        self.record(format!("projects:{limit}"));
        let state = self
            .state
            .lock()
            .map_err(|_| PortError::Store("lock".into()))?;
        Ok(state.projects.iter().take(limit).cloned().collect())
    }
}

#[async_trait]
impl MilestonePort for FakeBackend {
    async fn list_milestones(
        &self,
        project_id: &str,
        _limit: usize,
    ) -> Result<Vec<MilestoneItemDto>, PortError> {
        self.record(format!("list_milestones:{project_id}"));
        if project_id == "probe-project" {
            Ok(vec![seeded_milestone()])
        } else {
            Ok(Vec::new())
        }
    }
}

/// The fake tag generation over a pre-computed map (D-22's seam).
struct PrecomputedGeneration {
    tags: HashMap<String, Result<Vec<String>, String>>,
}

impl TagGeneration for PrecomputedGeneration {
    fn generate_tags(&self, memory_id: &str, _content: &str) -> Result<Vec<String>, String> {
        match self.tags.get(memory_id) {
            Some(Ok(tags)) => Ok(tags.clone()),
            Some(Err(error)) => Err(error.clone()),
            None => Err(format!(
                "tag generation was not precomputed for memory {memory_id}"
            )),
        }
    }
}

/// A no-op sink: the batch test asserts the counts, not the stored vectors.
struct NoopSink;

impl MigrationSink for NoopSink {
    fn write_tags_csv(&mut self, _memory_id: &str, _tags_csv: &str) -> Result<(), String> {
        Ok(())
    }

    fn embed_document(&mut self, _text: &str) -> Result<Vec<f32>, String> {
        Ok(vec![0.0; 768])
    }

    fn update_vector(
        &mut self,
        _memory_id: &str,
        _content_vector: &[f32],
        _tags_vector: Option<&[f32]>,
    ) -> Result<(), String> {
        Ok(())
    }
}
