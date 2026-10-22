//! The portable bundle's wire shape (upstream
//! `src/shared/api/portability-schemas.ts`).
//!
//! The bundle is a **versioned JSON document**. `schemaVersion` is the
//! literal `1`; the top-level `memories` array is the upstream payload and
//! stays the primary tier. The Rust port ADDS optional sibling tiers
//! (`globalMemories`, `profiles`, `milestones`, `staging`) under the same
//! `schemaVersion: 1` — they are additive, so an upstream reader ignores
//! them and a Rust reader tolerates their absence.
//!
//! Two properties are load-bearing:
//!
//! - **unknown top-level fields are preserved.** `extra` is a
//!   `#[serde(flatten)]` map, so a field this version does not know about
//!   survives a deserialize/serialize round trip and is re-emitted. That is
//!   what makes "extra tier" imports lossless.
//! - **a tier's absence never means "delete".** Every sibling tier is
//!   `Option`, and the importer only touches the tiers the bundle carries.

use std::collections::BTreeMap;

use mem_episodic::milestone::Milestone;
use mem_episodic::record::AgentSession;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Upstream `PORTABILITY_SCHEMA_VERSION`.
pub const PORTABILITY_SCHEMA_VERSION: u32 = 1;

/// The plugin identity the bundle records (upstream writes
/// `"opencode-mem"`; the Rust port writes its own package name).
pub const PLUGIN_PACKAGE: &str = "opencode-agents-memory";

/// The bundle's schema version.
///
/// The value is carried as a number so an UNSUPPORTED version can be
/// reported with upstream's exact message ("Unsupported export schemaVersion
/// N") instead of a parse error; the importer still refuses every value
/// other than [`PORTABILITY_SCHEMA_VERSION`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SchemaVersion(pub u32);

impl SchemaVersion {
    /// The one supported version.
    pub const CURRENT: SchemaVersion = SchemaVersion(PORTABILITY_SCHEMA_VERSION);

    /// The numeric value.
    pub fn as_u32(self) -> u32 {
        self.0
    }
}

impl Default for SchemaVersion {
    fn default() -> Self {
        Self::CURRENT
    }
}

/// The bundle's scope segment (upstream `z.literal("project")`).
///
/// The enum has exactly one variant, so only `"project"` deserializes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BundleScope {
    /// The project scope.
    Project,
}

/// The plugin that produced the bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginInfo {
    /// The package name.
    pub package: String,
    /// The package version.
    pub version: String,
}

/// Where the bundle came from.
///
/// The identity is **advisory provenance only**: the importer never trusts
/// it for routing, it re-resolves the target scope from the target
/// directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceInfo {
    /// The source container tag.
    pub container_tag: String,
    /// The scope segment (always `project`).
    pub scope: BundleScope,
    /// The 16-hex scope hash of the source container tag.
    pub scope_hash: String,
    /// The source project path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
    /// The source project name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
    /// The source git remote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_repo_url: Option<String>,
}

/// The embedding the bundle was written with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingInfo {
    /// The embedding model name.
    pub model: String,
    /// The embedding dimension count.
    pub dimensions: u32,
}

/// One exported memory (upstream `ExportedMemorySchema`).
///
/// The vector is deliberately absent: an export is portable text, and the
/// importer re-embeds the content with the TARGET's embedder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportedMemory {
    /// The memory id (stable across the round trip).
    pub id: String,
    /// The memory content.
    pub content: String,
    /// The memory type.
    ///
    /// The wire key is upstream's **`type`**, not `memoryType`: upstream's
    /// `ExportedMemorySchema` reads `type` and upstream's own bundles carry
    /// it, so the port renames the field explicitly (the struct's
    /// `camelCase` rename would otherwise emit `memoryType`, and an
    /// upstream bundle's `type` key would be dropped on import — the
    /// memory type would be lost across implementations).
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub memory_type: Option<String>,
    /// The tags.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// The creation stamp (unix millis).
    pub created_at: i64,
    /// The last-update stamp (unix millis).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
    /// The pin flag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_pinned: Option<bool>,
    /// The free-form metadata object.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BTreeMap<String, Value>>,
    /// The display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// The user name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_name: Option<String>,
    /// The user email.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_email: Option<String>,
    /// The project path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_path: Option<String>,
    /// The project name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_name: Option<String>,
    /// The git remote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_repo_url: Option<String>,
}

/// One exported user profile (upstream has no tier for it; the Rust port
/// adds one as an additive sibling).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportedProfile {
    /// The profile row id.
    pub id: String,
    /// The owning user id.
    pub user_id: String,
    /// The display name.
    pub display_name: String,
    /// The user name.
    pub user_name: String,
    /// The user email.
    pub user_email: String,
    /// The profile payload.
    pub profile_data: Value,
    /// The profile version.
    pub version: i64,
    /// The creation stamp (unix millis).
    pub created_at: i64,
    /// The last-analysis stamp (unix millis).
    pub last_analyzed_at: i64,
    /// How many prompts were analysed.
    pub total_prompts_analyzed: i64,
    /// Whether the profile is the active one.
    pub is_active: bool,
}

/// One exported milestone.
///
/// The milestone reuses `mem_episodic`'s own record type (camelCase on the
/// wire, `embedding` skipped), so the port and the store can never drift
/// apart on the shape.
pub type ExportedMilestone = Milestone;

/// One exported milestone contributor.
pub type ExportedContributor = mem_episodic::milestone::Contributor;

/// The raw staging tier (upstream has no tier for it; the Rust port adds
/// one as an additive sibling, and it is OPT-IN).
///
/// The inclusion decision lives in `ExportOptions::include_staging`, not
/// in the payload — there is no `included` flag inside the tier.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StagingTier {
    /// The raw session rows.
    pub sessions: Vec<AgentSession>,
}

/// The bundle: the whole portable document.
///
/// The unknown-field map is flattened, so any top-level key this version
/// does not know about is preserved and re-emitted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryExportDocument {
    /// The schema version (the literal `1`).
    pub schema_version: SchemaVersion,
    /// The export stamp (RFC 3339, UTC).
    pub exported_at: String,
    /// The producing plugin.
    pub plugin: PluginInfo,
    /// The source identity (advisory provenance only).
    pub source: SourceInfo,
    /// The embedding the bundle was written with.
    pub embedding: EmbeddingInfo,
    /// The primary tier: the project-scope memories.
    pub memories: Vec<ExportedMemory>,
    /// The additive tier: the user-scope ("global") memories.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_memories: Option<Vec<ExportedMemory>>,
    /// The additive tier: the user profiles.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profiles: Option<Vec<ExportedProfile>>,
    /// The additive tier: the consolidated milestones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub milestones: Option<Vec<ExportedMilestone>>,
    /// The additive tier: the raw staging sessions (opt-in).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub staging: Option<StagingTier>,
    /// Every unknown top-level field, preserved and re-emitted.
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

/// The bundle alias the service speaks.
pub type Bundle = MemoryExportDocument;

impl MemoryExportDocument {
    /// The bundle with every optional tier empty and no unknown fields.
    pub fn empty() -> Self {
        Self {
            schema_version: SchemaVersion::CURRENT,
            exported_at: String::new(),
            plugin: PluginInfo {
                package: PLUGIN_PACKAGE.to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            source: SourceInfo {
                container_tag: String::new(),
                scope: BundleScope::Project,
                scope_hash: String::new(),
                project_path: None,
                project_name: None,
                git_repo_url: None,
            },
            embedding: EmbeddingInfo {
                model: String::new(),
                dimensions: 0,
            },
            memories: Vec::new(),
            global_memories: None,
            profiles: None,
            milestones: None,
            staging: None,
            extra: BTreeMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_scope_only_deserializes_project() {
        assert_eq!(
            serde_json::from_str::<BundleScope>("\"project\"").expect("project"),
            BundleScope::Project
        );
        assert!(serde_json::from_str::<BundleScope>("\"user\"").is_err());
    }

    #[test]
    fn schema_version_is_transparent() {
        assert_eq!(
            serde_json::to_string(&SchemaVersion::CURRENT).expect("serialize"),
            "1"
        );
        assert_eq!(
            serde_json::from_str::<SchemaVersion>("7").expect("deserialize"),
            SchemaVersion(7)
        );
    }
}
