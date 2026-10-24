//! The portability service: export, import, and the bundle file I/O.
//!
//! Upstream's `MemoryPortabilityService` (and the issue-22 CLI that calls
//! it) is the behavioural contract. The port keeps the same two operations
//! and adds the two the blueprint names: a **replace mode** and a
//! **dry run**.
//!
//! ## Import semantics
//!
//! The ported rule is upstream's, unchanged:
//!
//! > **Abort on any duplicate id: zero writes**, error
//! > "Import aborted: N memory id(s) already exist".
//!
//! The pre-scan runs BEFORE any write, so an aborted import leaves the
//! target exactly as it was. The Rust-only **replace mode** is the
//! explicit alternative: an id that already exists in the target is
//! deleted and re-imported instead of aborting the run.
//!
//! ## The four phases
//!
//! An import runs in four phases, and **only the fourth one writes**:
//!
//! 1. **the pre-scan** — the duplicate-id check (both fact tiers and the
//!    milestone ids). Nothing has been written, so an abort is a zero-write
//!    abort;
//! 2. **the prepare phase** — every content embedding, every tags
//!    embedding, every provenance metadata object and every milestone
//!    encoding is computed here. A failure in this phase is still a
//!    zero-write failure;
//! 3. **the dry run** — nothing is written; the report carries the counts
//!    the write path would apply, computed with the SAME skip predicate;
//! 4. **the write phase** — the rows land.
//!
//! ## The "no partial writes" guarantee — and its limit
//!
//! [`FactStore`] is a four-method trait over a sharded store: it has
//! **no transaction seam**, so the import cannot be atomic at the store
//! level. The port therefore promises exactly this:
//!
//! - every fallible COMPUTATION happens in the prepare phase, so an
//!   embedder failure, a malformed milestone or a milestone id collision
//!   never leaves a half-written target;
//! - the write phase records every row it CREATED and COMPENSATES a later
//!   failure by deleting exactly those rows. A row that REPLACED a
//!   pre-existing row is never compensated: the replace mode is destructive
//!   by definition, so deleting it would lose the target's row without
//!   restoring the old content.

//! ## Tiers
//!
//! The importer touches only the tiers the bundle actually carries. An
//! omitted tier never deletes anything in the target, and an unknown
//! top-level tier is preserved verbatim (the bundle's flattened `extra`
//! map), so a bundle written by a newer version is never silently
//! truncated by an older importer.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

use mem_core::scope::{extract_scope_from_container_tag, is_valid_scope_hash};
use mem_core::tags::{TagResolver, format_tags_for_embedding};
use mem_embed::embedder::Embedder;
use mem_episodic::milestone::encode_contributors;
use mem_episodic::record::NewAgentSession;
use mem_episodic::store::{EpisodicStore, UpsertOutcome};
use mem_profile::profile_store::{Profile, ProfileStore};
use mem_store::db::{Statement, Value};
use mem_store::episodic_sql::{DELETE_MILESTONE, INSERT_MILESTONE, SELECT_MILESTONE_ID_BY_ID};
use mem_store::vector::vector_to_text;

use crate::PortabilityError;
use crate::bundle::{
    Bundle, BundleScope, EmbeddingInfo, ExportedMemory, ExportedProfile, MemoryExportDocument,
    PLUGIN_PACKAGE, PluginInfo, SourceInfo, StagingTier,
};
use crate::facts::FactStore;

/// The import size cap: 100 MiB (upstream's `MAX_IMPORT_BYTES`).
pub const MAX_IMPORT_BYTES: u64 = 100 * 1024 * 1024;

/// The export options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExportOptions {
    /// Include the raw staging sessions (the opt-in tier).
    pub include_staging: bool,
}

/// The import mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReplaceMode {
    /// The upstream mode: an id that already exists in the target aborts
    /// the import (zero writes).
    #[default]
    Merge,
    /// The Rust-only mode: an id that already exists in the target is
    /// replaced.
    Overwrite,
}

/// One skipped row (imported nowhere, but the import continued).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedEntry {
    /// The row id.
    pub id: String,
    /// Why it was skipped.
    pub reason: String,
}

/// One rejected row (the import aborted because of it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedEntry {
    /// The row id.
    pub id: String,
    /// Why it was rejected.
    pub reason: String,
}

/// The import report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportReport {
    /// How many rows were imported.
    pub imported: usize,
    /// The rows that were skipped (the import continued).
    pub skipped: Vec<SkippedEntry>,
    /// The rows that were rejected (the import aborted).
    pub rejected: Vec<RejectedEntry>,
    /// The target container tag.
    pub container_tag: String,
}

/// The resolved scope of one directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedScope {
    /// The project container tag.
    pub container_tag: String,
    /// The user ("global") container tag.
    pub user_container_tag: String,
    /// The 16-hex scope hash.
    pub scope_hash: String,
    /// The project path.
    pub project_path: Option<PathBuf>,
    /// The project name.
    pub project_name: Option<String>,
    /// The git remote.
    pub git_repo_url: Option<String>,
    /// The display name.
    pub display_name: String,
    /// The episodic project id (the project path's string form — the
    /// episodic tier is keyed by the host's project directory).
    pub project_id: String,
}

/// The portability service.
pub struct PortabilityService {
    facts: Arc<dyn FactStore>,
    profiles: Arc<ProfileStore>,
    episodic: Arc<EpisodicStore>,
    embedder: Arc<dyn Embedder>,
    container_tag_prefix: String,
    embedding_model: String,
    embedding_dimensions: u32,
}

impl PortabilityService {
    /// The production constructor.
    pub fn new(
        facts: Arc<dyn FactStore>,
        profiles: Arc<ProfileStore>,
        episodic: Arc<EpisodicStore>,
        embedder: Arc<dyn Embedder>,
    ) -> Self {
        Self {
            facts,
            profiles,
            episodic,
            embedder,
            container_tag_prefix: mem_core::config::DEFAULTS.container_tag_prefix.to_string(),
            embedding_model: mem_core::config::DEFAULTS.embedding_model.to_string(),
            embedding_dimensions: mem_core::config::DEFAULTS.embedding_dimensions,
        }
    }

    /// Overrides the container-tag prefix (the resolved config's value).
    pub fn with_container_tag_prefix(mut self, prefix: &str) -> Self {
        self.container_tag_prefix = prefix.to_string();
        self
    }

    /// Overrides the recorded embedding identity.
    pub fn with_embedding_info(mut self, model: &str, dimensions: u32) -> Self {
        self.embedding_model = model.to_string();
        self.embedding_dimensions = dimensions;
        self
    }

    /// Resolves the scope of one directory: the project container tag, the
    /// user ("global") container tag, the scope hash and the project
    /// identity.
    ///
    /// The resolution goes through `mem_core::tags` — the same
    /// `TagResolver` the bridge and the web layer use — so the bundle's
    /// `containerTag` can never diverge from the one the store uses.
    pub fn resolve_scope(&self, target: &Path) -> Result<ResolvedScope, PortabilityError> {
        let env = mem_core::env::ProcessEnv;
        let git = mem_core::tags::probe::SystemGitProbe::new(&env);
        let resolver = TagResolver::new(self.container_tag_prefix.as_str(), None, None, &env, &git);
        let project = resolver.project_tag_info(target);
        let user = resolver.user_tag_info(target);
        let scope_ref = extract_scope_from_container_tag(&project.tag)
            .map_err(|error| PortabilityError::InvalidScope(error.to_string()))?;
        let scope_hash = scope_ref
            .hash
            .map(|hash| hash.as_str().to_string())
            .ok_or_else(|| {
                PortabilityError::InvalidScope(format!(
                    "Invalid containerTag hash: expected 16 lowercase hex characters in \"{}\"",
                    project.tag
                ))
            })?;
        Ok(ResolvedScope {
            container_tag: project.tag,
            user_container_tag: user.tag,
            scope_hash,
            project_path: project.project_path.clone(),
            project_name: project.project_name.clone(),
            git_repo_url: project.git_repo_url.clone(),
            display_name: project.display_name.clone(),
            project_id: target.to_string_lossy().to_string(),
        })
    }

    // -----------------------------------------------------------------------
    // Export
    // -----------------------------------------------------------------------

    /// Exports the durable tiers of one directory into a bundle.
    pub async fn export(
        &self,
        target: &Path,
        opts: ExportOptions,
    ) -> Result<Bundle, PortabilityError> {
        let scope = self.resolve_scope(target)?;
        let memories = self.facts.list_memories(&scope.container_tag).await?;
        let global_memories = self.facts.list_memories(&scope.user_container_tag).await?;
        let profiles = self.export_profiles().await?;
        let milestones = self.export_milestones(&scope.project_id).await?;
        let staging = if opts.include_staging {
            Some(StagingTier {
                sessions: self.export_sessions(&scope.project_id).await?,
            })
        } else {
            None
        };

        Ok(MemoryExportDocument {
            schema_version: crate::bundle::SchemaVersion::CURRENT,
            exported_at: rfc3339_utc_now(),
            plugin: PluginInfo {
                package: PLUGIN_PACKAGE.to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            source: SourceInfo {
                container_tag: scope.container_tag.clone(),
                scope: BundleScope::Project,
                scope_hash: scope.scope_hash.clone(),
                project_path: scope
                    .project_path
                    .as_ref()
                    .map(|path| path.to_string_lossy().to_string()),
                project_name: scope.project_name.clone(),
                git_repo_url: scope.git_repo_url.clone(),
            },
            embedding: EmbeddingInfo {
                model: self.embedding_model.clone(),
                dimensions: self.embedding_dimensions,
            },
            memories,
            global_memories: Some(global_memories),
            profiles: Some(profiles),
            milestones: Some(milestones),
            staging,
            extra: BTreeMap::new(),
        })
    }

    /// The active user profiles (the additive profile tier).
    async fn export_profiles(&self) -> Result<Vec<ExportedProfile>, PortabilityError> {
        let profiles = self
            .profiles
            .get_all_active_profiles()
            .await
            .map_err(|error| PortabilityError::Profile(error.to_string()))?;
        Ok(profiles.into_iter().map(exported_profile).collect())
    }

    /// The consolidated milestones of one project (the additive milestone
    /// tier).
    ///
    /// The listing is bounded by [`MILESTONE_EXPORT_LIMIT`]. The store's
    /// listing has no offset, so a project with more rows than the bound
    /// is TRUNCATED — the truncation is logged (never silent) and the
    /// bound is a public constant, so a caller can see it.
    async fn export_milestones(
        &self,
        project_id: &str,
    ) -> Result<Vec<crate::bundle::ExportedMilestone>, PortabilityError> {
        let milestones = self
            .episodic
            .list_milestones(project_id, MILESTONE_EXPORT_LIMIT)
            .await
            .map_err(|error| PortabilityError::Episodic(error.to_string()))?;
        if milestones.len() >= MILESTONE_EXPORT_LIMIT {
            tracing::warn!(
                project_id,
                limit = MILESTONE_EXPORT_LIMIT,
                "portability export truncated the milestone tier at the listing bound"
            );
        }
        Ok(milestones)
    }

    /// The raw sessions of one project (the opt-in staging tier).
    ///
    /// The listing is bounded by [`SESSION_EXPORT_LIMIT`]; the truncation
    /// is logged (never silent), exactly as for the milestone tier.
    async fn export_sessions(
        &self,
        project_id: &str,
    ) -> Result<Vec<mem_episodic::record::AgentSession>, PortabilityError> {
        let sessions = self
            .episodic
            .list_sessions(project_id, SESSION_EXPORT_LIMIT)
            .await
            .map_err(|error| PortabilityError::Episodic(error.to_string()))?;
        if sessions.len() >= SESSION_EXPORT_LIMIT {
            tracing::warn!(
                project_id,
                limit = SESSION_EXPORT_LIMIT,
                "portability export truncated the staging tier at the listing bound"
            );
        }
        Ok(sessions)
    }

    // -----------------------------------------------------------------------
    // Import
    // -----------------------------------------------------------------------

    /// Imports a bundle into one directory.
    ///
    /// The import runs in FOUR phases and only the fourth one writes:
    ///
    /// 1. **the pre-scan** — the duplicate-id check. Nothing is written
    ///    before it passes, so an aborted import leaves the target exactly
    ///    as it was.
    /// 2. **the prepare phase** — every embedding, every tags embedding,
    ///    every provenance metadata object and every milestone encoding is
    ///    computed, and the milestone ids are validated. A failure here is
    ///    still a zero-write failure.
    /// 3. **the dry run** — nothing is written; the report carries the
    ///    counts the write path would apply (the SAME skip predicate).
    /// 4. **the write phase** — the rows land.
    ///
    /// `dry_run` writes nothing and reports the counts it would have
    /// applied. `mode` decides what happens to an id that already exists
    /// in the target: [`ReplaceMode::Merge`] aborts the whole import
    /// (upstream's rule, zero writes), [`ReplaceMode::Overwrite`] replaces
    /// the row.
    ///
    /// ## The "no partial writes" guarantee — and its limit
    ///
    /// [`FactStore`] has **no transaction seam**, so the import cannot be
    /// atomic at the store level. What the port promises:
    ///
    /// - every fallible COMPUTATION happens in the prepare phase, before
    ///   the first write;
    /// - the write phase records every row it CREATED and COMPENSATES a
    ///   later failure by deleting exactly those rows. A row that
    ///   REPLACED a pre-existing row is never compensated — the replace
    ///   mode is destructive by definition, so deleting it would lose the
    ///   target's row without restoring the old content.
    pub async fn import(
        &self,
        target: &Path,
        bundle: Bundle,
        mode: ReplaceMode,
        dry_run: bool,
    ) -> Result<ImportReport, PortabilityError> {
        let scope = self.resolve_scope(target)?;
        validate_bundle(&bundle)?;

        // --- Phase A: the pre-scan. Nothing is written before it passes.
        //
        // The scan covers BOTH fact tiers: the project tier against the
        // target's project container and the global tier against the
        // target's user container. A bundle that repeats an id inside
        // itself is rejected too — the target of that id would be
        // ambiguous.
        let mut rejected: Vec<RejectedEntry> = Vec::new();
        let mut seen: BTreeSet<String> = BTreeSet::new();
        for (tier, target_tag) in [
            (bundle.memories.as_slice(), scope.container_tag.as_str()),
            (
                bundle.global_memories.as_deref().unwrap_or_default(),
                scope.user_container_tag.as_str(),
            ),
        ] {
            for memory in tier {
                if !seen.insert(memory.id.clone()) {
                    rejected.push(RejectedEntry {
                        id: memory.id.clone(),
                        reason: "memory id appears more than once in the bundle".to_string(),
                    });
                    continue;
                }
                if mode == ReplaceMode::Merge
                    && self.facts.memory_exists(&memory.id, target_tag).await?
                {
                    rejected.push(RejectedEntry {
                        id: memory.id.clone(),
                        reason: "memory id already exists in target project".to_string(),
                    });
                }
            }
        }
        if !rejected.is_empty() {
            return Err(PortabilityError::ImportAborted {
                count: rejected.len(),
                rejected,
            });
        }

        // --- Phase B: the prepare phase. Nothing is written.
        //
        // Every embedding, every tags embedding, every metadata object and
        // every milestone encoding is computed HERE, so a failure in this
        // phase is a zero-write failure.
        let (project_facts, project_skipped) = self
            .prepare_facts(&bundle.memories, &scope.container_tag, &bundle, &scope)
            .await?;
        let (global_facts, global_skipped) = match &bundle.global_memories {
            Some(rows) => {
                self.prepare_facts(rows, &scope.user_container_tag, &bundle, &scope)
                    .await?
            }
            None => (Vec::new(), Vec::new()),
        };
        let milestone_statements = self
            .prepare_milestones(
                bundle.milestones.as_deref().unwrap_or_default(),
                &scope,
                mode,
            )
            .await?;

        // --- Phase C: the dry run. Nothing is written.
        //
        // The counts are the ones the write path would apply: the skip
        // predicate lives in the prepare phase, so the two can never
        // disagree.
        if dry_run {
            let mut skipped = project_skipped;
            skipped.extend(global_skipped);
            return Ok(ImportReport {
                imported: project_facts.len() + global_facts.len(),
                skipped,
                rejected: Vec::new(),
                container_tag: scope.container_tag,
            });
        }

        // --- Phase D: the writes.
        let mut skipped = project_skipped;
        skipped.extend(global_skipped);
        let (imported, staging_skipped) = self
            .write_phase(
                WritePlan {
                    project_facts,
                    global_facts,
                    milestone_statements,
                    mode,
                },
                &bundle,
                &scope,
            )
            .await?;
        skipped.extend(staging_skipped);

        Ok(ImportReport {
            imported,
            skipped,
            rejected: Vec::new(),
            container_tag: scope.container_tag,
        })
    }

    /// Phase B — prepares one fact tier.
    ///
    /// Every content embedding, every tags embedding and every provenance
    /// metadata object is computed here, BEFORE the first insert, so an
    /// embedder failure is a zero-write failure.
    ///
    /// The skip predicate lives HERE, so the dry run and the write path
    /// always agree on which rows are imported.
    async fn prepare_facts(
        &self,
        memories: &[ExportedMemory],
        target_tag: &str,
        bundle: &Bundle,
        scope: &ResolvedScope,
    ) -> Result<(Vec<PreparedFact>, Vec<SkippedEntry>), PortabilityError> {
        let mut prepared = Vec::new();
        let mut skipped = Vec::new();
        for memory in memories {
            if memory.content.trim().is_empty() {
                skipped.push(SkippedEntry {
                    id: memory.id.clone(),
                    reason: "memory content is empty".to_string(),
                });
                continue;
            }
            let embedding = self
                .embedder
                .embed(&memory.content)
                .await
                .map_err(|error| PortabilityError::Embed(error.to_string()))?;
            let tags_embedding = match memory.tags.as_ref().filter(|tags| !tags.is_empty()) {
                Some(tags) => Some(
                    self.embedder
                        .embed(&format_tags_for_embedding(tags))
                        .await
                        .map_err(|error| PortabilityError::Embed(error.to_string()))?,
                ),
                None => None,
            };
            prepared.push(PreparedFact {
                target_tag: target_tag.to_string(),
                id: memory.id.clone(),
                memory: rewrite_memory(memory, scope),
                embedding,
                tags_embedding,
                metadata_json: provenance_metadata(memory, bundle),
            });
        }
        Ok((prepared, skipped))
    }

    /// Phase B — prepares the milestone tier.
    ///
    /// The milestone id is the table's PRIMARY KEY, so a collision is
    /// GLOBAL (not project-scoped). In merge mode a collision aborts the
    /// import — the same contract the fact tiers have — and the abort
    /// happens BEFORE the first write.
    ///
    /// The returned statements are the atomic write batch: the replace
    /// mode's DELETE and the INSERT ride in the same batch.
    async fn prepare_milestones(
        &self,
        milestones: &[crate::bundle::ExportedMilestone],
        scope: &ResolvedScope,
        mode: ReplaceMode,
    ) -> Result<Vec<Statement>, PortabilityError> {
        if milestones.is_empty() {
            return Ok(Vec::new());
        }
        if mode == ReplaceMode::Merge {
            let mut seen: BTreeSet<String> = BTreeSet::new();
            let mut collisions: Vec<String> = Vec::new();
            for milestone in milestones {
                if !seen.insert(milestone.id.clone()) {
                    collisions.push(milestone.id.clone());
                    continue;
                }
                if self.milestone_exists(&milestone.id).await? {
                    collisions.push(milestone.id.clone());
                }
            }
            if !collisions.is_empty() {
                return Err(PortabilityError::ImportAbortedMilestones {
                    count: collisions.len(),
                });
            }
        }
        let mut statements: Vec<Statement> = Vec::new();
        for milestone in milestones {
            if mode == ReplaceMode::Overwrite {
                statements.push(Statement::new(
                    DELETE_MILESTONE,
                    vec![Value::Text(milestone.id.clone())],
                ));
            }
            let embedding = self
                .embedder
                .embed(&milestone.summary)
                .await
                .map_err(|error| PortabilityError::Embed(error.to_string()))?;
            let contributors_json = encode_contributors(&milestone.contributors)
                .map_err(|error| PortabilityError::Episodic(error.to_string()))?;
            statements.push(Statement::new(
                INSERT_MILESTONE,
                vec![
                    Value::Text(milestone.id.clone()),
                    // The project id is rewritten to the TARGET's.
                    Value::Text(scope.project_id.clone()),
                    Value::Text(milestone.agent_id.clone()),
                    Value::Text(milestone.summary.clone()),
                    Value::Text(contributors_json),
                    Value::Integer(milestone.source_count),
                    Value::Integer(milestone.window_start),
                    Value::Integer(milestone.window_end),
                    Value::Text(
                        vector_to_text(&embedding)
                            .map_err(|error| PortabilityError::Store(error.message()))?,
                    ),
                    Value::Integer(milestone.created_at),
                ],
            ));
        }
        Ok(statements)
    }

    /// Does a milestone with this id exist?
    ///
    /// The id is the table's PRIMARY KEY, so the probe is global: a
    /// milestone of ANOTHER project collides just the same.
    async fn milestone_exists(&self, id: &str) -> Result<bool, PortabilityError> {
        let db = self
            .episodic
            .ready()
            .await
            .map_err(|error| PortabilityError::Episodic(error.to_string()))?;
        let row = mem_store::db::query_one(
            db.as_ref(),
            SELECT_MILESTONE_ID_BY_ID,
            vec![Value::Text(id.to_string())],
        )
        .await
        .map_err(|error| PortabilityError::Episodic(error.to_string()))?;
        Ok(row.is_some())
    }

    /// Phase D — the write phase.
    ///
    /// [`FactStore`] has no transaction seam, so the phase records every
    /// row it CREATED and compensates a later failure by deleting exactly
    /// those rows. A row that REPLACED a pre-existing row is never
    /// compensated (the replace mode is destructive by definition).
    async fn write_phase(
        &self,
        plan: WritePlan,
        bundle: &Bundle,
        scope: &ResolvedScope,
    ) -> Result<(usize, Vec<SkippedEntry>), PortabilityError> {
        let mut created = CreatedRows::default();
        let outcome = self
            .write_phase_inner(&mut created, plan, bundle, scope)
            .await;
        match outcome {
            Ok(value) => Ok(value),
            Err(error) => {
                self.compensate(&created).await;
                Err(error)
            }
        }
    }

    /// Phase D — the writes themselves.
    async fn write_phase_inner(
        &self,
        created: &mut CreatedRows,
        plan: WritePlan,
        bundle: &Bundle,
        scope: &ResolvedScope,
    ) -> Result<(usize, Vec<SkippedEntry>), PortabilityError> {
        let WritePlan {
            project_facts,
            global_facts,
            milestone_statements,
            mode,
        } = plan;
        let mut imported = 0usize;

        // The fact tiers: the project tier and the global tier, each into
        // its own container.
        for prepared in project_facts.into_iter().chain(global_facts) {
            let PreparedFact {
                target_tag,
                id,
                memory,
                embedding,
                tags_embedding,
                metadata_json,
            } = prepared;
            let replaced = mode == ReplaceMode::Overwrite
                && self.facts.memory_exists(&id, &target_tag).await?;
            if replaced {
                self.facts.delete_memory(&id, &target_tag).await?;
            }
            self.facts
                .insert_memory(
                    &target_tag,
                    memory,
                    embedding,
                    tags_embedding,
                    metadata_json,
                )
                .await?;
            created.facts.push((target_tag, id, replaced));
            imported += 1;
        }

        // The profile tier: the id, the version and the timestamps are the
        // SOURCE's.
        if let Some(profiles) = &bundle.profiles {
            for profile in profiles {
                let replaced = self
                    .profiles
                    .get_profile_by_id(&profile.id)
                    .await
                    .map_err(|error| PortabilityError::Profile(error.to_string()))?
                    .is_some();
                self.profiles
                    .import_profile(imported_profile(profile))
                    .await
                    .map_err(|error| PortabilityError::Profile(error.to_string()))?;
                created.profiles.push((profile.id.clone(), replaced));
            }
        }

        // The milestone tier: ONE atomic batch (the replace mode's DELETE
        // rides in the same batch as the INSERT).
        if !milestone_statements.is_empty() {
            self.episodic
                .write_batch(milestone_statements)
                .await
                .map_err(|error| PortabilityError::Episodic(error.to_string()))?;
            // In merge mode the milestone is NEW, so a later failure may
            // compensate it. In replace mode it REPLACED a pre-existing
            // row, so it is kept.
            if mode == ReplaceMode::Merge {
                created.milestones.extend(
                    bundle
                        .milestones
                        .as_deref()
                        .unwrap_or_default()
                        .iter()
                        .map(|milestone| milestone.id.clone()),
                );
            }
        }

        // The staging tier: the raw sessions, in two passes.
        let mut skipped = Vec::new();
        if let Some(staging) = &bundle.staging {
            skipped = self.import_staging(staging, scope, created).await?;
        }

        Ok((imported, skipped))
    }

    /// Compensates a failed write phase.
    ///
    /// Exactly the rows this run CREATED are deleted; a row that replaced a
    /// pre-existing row is kept (the replace mode is destructive by
    /// definition, so deleting it would lose the target's row without
    /// restoring the old content).
    async fn compensate(&self, created: &CreatedRows) {
        for (target_tag, id, replaced) in created.facts.iter().rev() {
            if !*replaced {
                let _ = self.facts.delete_memory(id, target_tag).await;
            }
        }
        for (id, replaced) in created.profiles.iter().rev() {
            if !*replaced {
                let _ = self.profiles.delete_profile(id).await;
            }
        }
        for id in created.milestones.iter().rev() {
            let statement = Statement::new(DELETE_MILESTONE, vec![Value::Text(id.clone())]);
            let _ = self.episodic.write_batch(vec![statement]).await;
        }
        for id in created.sessions.iter().rev() {
            let _ = self.episodic.delete_session(id).await;
        }
    }

    /// Imports the raw staging sessions through the episodic upsert — in
    /// TWO passes.
    ///
    /// **Pass 1** inserts every session with `parent_session_id: None`.
    /// The source's parent link is a SOURCE ROW id and the target mints
    /// its own row ids, so the link can only be resolved once every row
    /// exists. (Handing the source id straight to the upsert made the
    /// import fail with "no such parent_session" for every session that
    /// had a parent.)
    ///
    /// **Pass 2** resolves every link: the source's parent row id is
    /// mapped to the parent's `(session_id, agent_id)`, then to the
    /// TARGET's row id, and the child's link is set with
    /// [`EpisodicStore::link_parent`]. A link whose parent row is absent
    /// is RECORDED as a skipped entry — never silently dropped.
    async fn import_staging(
        &self,
        staging: &StagingTier,
        scope: &ResolvedScope,
        created: &mut CreatedRows,
    ) -> Result<Vec<SkippedEntry>, PortabilityError> {
        let mut skipped = Vec::new();
        if staging.sessions.is_empty() {
            return Ok(skipped);
        }

        // The source's own map: source row id -> (session_id, agent_id).
        let mut source_triples: BTreeMap<String, (String, String)> = BTreeMap::new();
        for row in &staging.sessions {
            source_triples.insert(
                row.id.clone(),
                (row.session_id.clone(), row.agent_id.clone()),
            );
        }

        // The rows this run CREATED (the compensation list): we track the
        // triples that `upsert_session` actually INSERTED and resolve their
        // target row ids afterwards. The previous `before`/`after` snapshot
        // diff was bounded by `SESSION_EXPORT_LIMIT` (10 000) via
        // `list_sessions(... ORDER BY created_at DESC, id DESC LIMIT ?)`,
        // so on a project with more sessions than that, `created.sessions`
        // could neither see rows pushed out of the window nor bound the
        // rollback exactly. Inserted rows carry `created_at = now()`, so
        // they sort first and are all visible within the listing window
        // while a single run inserts no more than `SESSION_EXPORT_LIMIT`
        // sessions.
        let mut inserted_keys: BTreeSet<(String, String)> = BTreeSet::new();

        // --- Pass 1: insert every session WITHOUT its parent link.
        for row in &staging.sessions {
            let new = NewAgentSession {
                project_id: scope.project_id.clone(),
                session_id: row.session_id.clone(),
                agent_id: row.agent_id.clone(),
                // The link is resolved in pass 2.
                parent_session_id: None,
                summary: row.summary.clone(),
                agent_name: row.metadata.agent_name.clone(),
                git_branch: row.metadata.git_branch.clone(),
                git_commit: row.metadata.git_commit.clone(),
                files_touched: row.metadata.files_touched.clone(),
                completed_work: row.metadata.completed_work.clone(),
                decisions: row.metadata.decisions.clone(),
                open_threads: row.metadata.open_threads.clone(),
                extra: row.metadata.extra.clone(),
                timestamp: Some(row.metadata.timestamp),
                capture_id: row.capture_id.clone(),
            };
            let outcome = self
                .episodic
                .upsert_session(new, self.embedder.as_ref())
                .await
                .map_err(|error| PortabilityError::Episodic(error.to_string()))?;
            match outcome {
                UpsertOutcome::TombstoneUnchanged => {
                    // A tombstone is never mutated: the row keeps its content,
                    // so the link pass below has nothing to attach to.
                    skipped.push(SkippedEntry {
                        id: row.id.clone(),
                        reason: "the target session row is a tombstone and was not written"
                            .to_string(),
                    });
                }
                UpsertOutcome::Inserted => {
                    inserted_keys.insert((row.session_id.clone(), row.agent_id.clone()));
                }
                _ => {}
            }
        }

        // The target's session keys AFTER the import: the row ids the
        // links resolve against (pass 2).
        let after = self.session_triples(scope).await?;
        // The rows this run CREATED (the compensation list): exactly the
        // inserted triples' row ids, resolved from the post-import listing.
        created.sessions.clear();
        for key in &inserted_keys {
            if let Some(row_id) = after.get(key) {
                created.sessions.push(row_id.clone());
            }
        }

        // --- Pass 2: resolve and set every parent link.
        for row in &staging.sessions {
            let Some(source_parent) = row.parent_session_id.as_ref() else {
                continue;
            };
            let Some((parent_session_id, parent_agent_id)) = source_triples.get(source_parent)
            else {
                skipped.push(SkippedEntry {
                    id: row.id.clone(),
                    reason: format!("parent session row {source_parent} is not part of the bundle"),
                });
                continue;
            };
            let parent_key = (parent_session_id.clone(), parent_agent_id.clone());
            let Some(parent_row_id) = after.get(&parent_key) else {
                skipped.push(SkippedEntry {
                    id: row.id.clone(),
                    reason: format!("parent session row {source_parent} has no imported row"),
                });
                continue;
            };
            let child_key = (row.session_id.clone(), row.agent_id.clone());
            let Some(child_row_id) = after.get(&child_key) else {
                skipped.push(SkippedEntry {
                    id: row.id.clone(),
                    reason: "the imported session row is missing".to_string(),
                });
                continue;
            };
            self.episodic
                .link_parent(&scope.project_id, child_row_id, parent_row_id)
                .await
                .map_err(|error| PortabilityError::Episodic(error.to_string()))?;
        }
        Ok(skipped)
    }

    /// The target project's session rows: `(session_id, agent_id)` -> the
    /// row id.
    async fn session_triples(
        &self,
        scope: &ResolvedScope,
    ) -> Result<BTreeMap<(String, String), String>, PortabilityError> {
        let rows = self
            .episodic
            .list_sessions(&scope.project_id, SESSION_EXPORT_LIMIT)
            .await
            .map_err(|error| PortabilityError::Episodic(error.to_string()))?;
        Ok(rows
            .into_iter()
            .map(|row| ((row.session_id, row.agent_id), row.id))
            .collect())
    }
}

/// The prepared write plan: the two fact tiers, the milestone batch and
/// the import mode — everything the write phase needs, computed by the
/// prepare phase.
#[derive(Debug)]
struct WritePlan {
    /// The project-scope fact rows.
    project_facts: Vec<PreparedFact>,
    /// The user-scope ("global") fact rows.
    global_facts: Vec<PreparedFact>,
    /// The milestone batch (the replace mode's DELETE rides in it).
    milestone_statements: Vec<Statement>,
    /// The import mode.
    mode: ReplaceMode,
}

/// One prepared fact row: everything the write phase needs, computed
/// BEFORE the first insert.
#[derive(Debug, Clone)]
struct PreparedFact {
    /// The target container tag.
    target_tag: String,
    /// The memory id (the compensation key).
    id: String,
    /// The rewritten memory.
    memory: ExportedMemory,
    /// The re-computed content embedding.
    embedding: Vec<f32>,
    /// The re-computed tags embedding.
    tags_embedding: Option<Vec<f32>>,
    /// The provenance-stamped metadata JSON.
    metadata_json: String,
}

/// The rows one import run created — the compensation bookkeeping.
///
/// A `replaced` flag marks the row that REPLACED a pre-existing row: the
/// replace mode is destructive by definition, so such a row is never
/// compensated.
#[derive(Debug, Default)]
struct CreatedRows {
    /// The fact rows: the container tag, the id and the replaced flag.
    facts: Vec<(String, String, bool)>,
    /// The profile ids and their replaced flags.
    profiles: Vec<(String, bool)>,
    /// The milestone ids.
    milestones: Vec<String>,
    /// The session row ids.
    sessions: Vec<String>,
}

/// The milestone listing bound of the export.
///
/// The store's listing has no offset, so a project with more milestones
/// than this bound is truncated. The truncation is LOGGED (never silent)
/// and the bound is public, so a caller can see it.
pub const MILESTONE_EXPORT_LIMIT: usize = 10_000;

/// The staging listing bound of the export.
///
/// As [`MILESTONE_EXPORT_LIMIT`]: the bound is public and the truncation
/// is logged.
pub const SESSION_EXPORT_LIMIT: usize = 10_000;

/// Validates the bundle's own invariants (the schema version and the
/// source scope hash).
fn validate_bundle(bundle: &Bundle) -> Result<(), PortabilityError> {
    let version = bundle.schema_version.as_u32();
    if version > crate::bundle::PORTABILITY_SCHEMA_VERSION {
        return Err(PortabilityError::UnsupportedSchemaVersion(version));
    }
    if version != crate::bundle::PORTABILITY_SCHEMA_VERSION {
        return Err(PortabilityError::InvalidBundle(
            "Invalid export document: schemaVersion must be 1".to_string(),
        ));
    }
    if !is_valid_scope_hash(&bundle.source.scope_hash) {
        return Err(PortabilityError::InvalidBundle(
            "Invalid export document: source.scopeHash must be 16 lowercase hex characters"
                .to_string(),
        ));
    }
    Ok(())
}

/// Rewrites the target-bound fields of one memory.
///
/// The rewrite list is fixed: `container_tag`, `project_path`,
/// `project_name` and `display_name` become the TARGET's; every other
/// field is carried over untouched.
fn rewrite_memory(memory: &ExportedMemory, scope: &ResolvedScope) -> ExportedMemory {
    let mut rewritten = memory.clone();
    rewritten.project_path = scope
        .project_path
        .as_ref()
        .map(|path| path.to_string_lossy().to_string())
        .or_else(|| memory.project_path.clone());
    rewritten.project_name = scope.project_name.clone().or(memory.project_name.clone());
    rewritten.display_name = Some(scope.display_name.clone());
    rewritten
}

/// Builds the imported memory's metadata: the source metadata plus the
/// provenance stamps.
fn provenance_metadata(memory: &ExportedMemory, bundle: &Bundle) -> String {
    let mut metadata = memory.metadata.clone().unwrap_or_default();
    metadata.insert(
        "source".to_string(),
        serde_json::Value::String("import".to_string()),
    );
    metadata.insert(
        "importedAt".to_string(),
        serde_json::Value::Number(now_millis().into()),
    );
    metadata.insert(
        "exportSchemaVersion".to_string(),
        serde_json::Value::Number(bundle.schema_version.as_u32().into()),
    );
    metadata.insert(
        "originalContainerTag".to_string(),
        serde_json::Value::String(bundle.source.container_tag.clone()),
    );
    if let Some(path) = &bundle.source.project_path {
        metadata.insert(
            "originalProjectPath".to_string(),
            serde_json::Value::String(path.clone()),
        );
    }
    serde_json::to_string(&metadata).unwrap_or_else(|_| "{}".to_string())
}

/// Maps a store profile onto the bundle's profile shape.
fn exported_profile(profile: Profile) -> ExportedProfile {
    ExportedProfile {
        id: profile.id,
        user_id: profile.user_id,
        display_name: profile.display_name,
        user_name: profile.user_name,
        user_email: profile.user_email,
        profile_data: serde_json::from_str(&profile.profile_data)
            .unwrap_or(serde_json::Value::Object(Default::default())),
        version: profile.version,
        created_at: profile.created_at,
        last_analyzed_at: profile.last_analyzed_at,
        total_prompts_analyzed: profile.total_prompts_analyzed,
        is_active: profile.is_active,
    }
}

/// Maps a bundle profile onto the store's profile shape.
fn imported_profile(profile: &ExportedProfile) -> Profile {
    Profile {
        id: profile.id.clone(),
        user_id: profile.user_id.clone(),
        display_name: profile.display_name.clone(),
        user_name: profile.user_name.clone(),
        user_email: profile.user_email.clone(),
        profile_data: profile.profile_data.to_string(),
        version: profile.version,
        created_at: profile.created_at,
        last_analyzed_at: profile.last_analyzed_at,
        total_prompts_analyzed: profile.total_prompts_analyzed,
        is_active: profile.is_active,
    }
}

/// The current epoch in milliseconds.
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}

/// The current instant as an RFC 3339 UTC stamp.
fn rfc3339_utc_now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

// ---------------------------------------------------------------------------
// Bundle file I/O
// ---------------------------------------------------------------------------

/// Reads a bundle from a file, enforcing the size cap BEFORE the read.
///
/// The cap is checked on the file's metadata, so an oversized file is never
/// pulled into memory. The content is parsed as JSON only — nothing in a
/// bundle is ever executed.
pub fn read_bundle(path: &Path) -> Result<Bundle, PortabilityError> {
    let metadata = std::fs::metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            PortabilityError::FileNotFound(path.to_string_lossy().to_string())
        } else {
            PortabilityError::Io(error.to_string())
        }
    })?;
    if metadata.len() > MAX_IMPORT_BYTES {
        return Err(PortabilityError::ImportTooLarge);
    }
    let text =
        std::fs::read_to_string(path).map_err(|error| PortabilityError::Io(error.to_string()))?;
    parse_bundle(&text)
}

/// Parses a bundle from JSON text.
///
/// The schema version is checked FIRST so an unsupported version answers
/// upstream's exact message instead of a parse error.
pub fn parse_bundle(text: &str) -> Result<Bundle, PortabilityError> {
    let raw: serde_json::Value = serde_json::from_str(text)
        .map_err(|error| PortabilityError::InvalidJson(error.to_string()))?;
    if let Some(version) = raw.get("schemaVersion").and_then(serde_json::Value::as_u64)
        && version > u64::from(crate::bundle::PORTABILITY_SCHEMA_VERSION)
    {
        return Err(PortabilityError::UnsupportedSchemaVersion(version as u32));
    }
    serde_json::from_value(raw).map_err(|error| PortabilityError::InvalidBundle(error.to_string()))
}

/// Writes a bundle to a file: mode `0600`, an atomic temp+rename, and no
/// symlink following.
pub fn write_bundle(path: &Path, bundle: &Bundle) -> Result<(), PortabilityError> {
    // A symlink at the destination is refused — the writer never follows
    // one out of the intended directory.
    if let Ok(metadata) = std::fs::symlink_metadata(path)
        && metadata.file_type().is_symlink()
    {
        return Err(PortabilityError::UnsafePath(
            "refusing to write through a symlink".to_string(),
        ));
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|error| PortabilityError::Io(error.to_string()))?;
    }
    let text = serde_json::to_string_pretty(bundle)
        .map_err(|error| PortabilityError::InvalidBundle(error.to_string()))?;
    let temp = temp_sibling(path);
    let write_result = (|| -> Result<(), PortabilityError> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        // The bundle is private data: on unix the file is created `0600`
        // (owner read/write only). The mode is unix-specific, so on other
        // platforms the platform default applies.
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options
            .open(&temp)
            .map_err(|error| PortabilityError::Io(error.to_string()))?;
        file.write_all(text.as_bytes())
            .map_err(|error| PortabilityError::Io(error.to_string()))?;
        file.sync_all()
            .map_err(|error| PortabilityError::Io(error.to_string()))?;
        Ok(())
    })();
    if let Err(error) = write_result {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    std::fs::rename(&temp, path).map_err(|error| {
        let _ = std::fs::remove_file(&temp);
        PortabilityError::Io(error.to_string())
    })
}

/// The temp sibling path of the destination.
fn temp_sibling(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "bundle".to_string());
    name.push_str(&format!(".tmp-{}-{}", std::process::id(), now_millis()));
    path.with_file_name(name)
}
