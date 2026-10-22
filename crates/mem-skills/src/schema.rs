//! The `skills` table, its rows and the id rule (upstream
//! `src/services/skills/types.ts`).
//!
//! The DDL is upstream's `SKILLS_MIGRATIONS` baseline, byte-for-byte in
//! substance: the column set, the defaults and the three indexes are the
//! ones upstream creates in `skills.db`.
//!
//! DIVERGENCE (recorded): upstream applies the baseline through the shared
//! `applySchemaMigrations` helper (which stamps `PRAGMA user_version`); the
//! port applies the same idempotent `CREATE TABLE IF NOT EXISTS` statements
//! directly, because `mem_store::schema_migrations` is a different crate's
//! module and this wave may not widen that crate's file set. The observable
//! schema is identical, and the statements are idempotent, so a second
//! `SkillsManager` over the same `skills.db` is a no-op.

use serde::{Deserialize, Serialize};

/// Upstream `SKILLS_DB_NAME` — the database file inside the storage path.
pub const SKILLS_DB_NAME: &str = "skills.db";

/// The directory (beside `skills.db`) that holds the per-skill content.
pub const SKILLS_CONTENT_DIR: &str = "skills-content";

/// Upstream `SKILL_ID_PATTERN` — lowercase kebab-case.
pub const SKILL_ID_PATTERN: &str = "^[a-z0-9]+(-[a-z0-9]+)*$";

/// Upstream's inclusive length bounds (`id.length >= 1 && id.length <= 64`).
pub const SKILL_ID_MIN_LENGTH: usize = 1;
pub const SKILL_ID_MAX_LENGTH: usize = 64;

/// The upstream `skills` table DDL (baseline migration v1).
pub const SKILLS_TABLE_DDL: &str = "CREATE TABLE IF NOT EXISTS skills (\n  \
    id TEXT PRIMARY KEY,\n  \
    name TEXT NOT NULL,\n  \
    description TEXT NOT NULL DEFAULT '',\n  \
    content TEXT NOT NULL,\n  \
    autoinvoke INTEGER NOT NULL DEFAULT 1,\n  \
    enabled INTEGER NOT NULL DEFAULT 1,\n  \
    container_tag TEXT,\n  \
    created_at INTEGER NOT NULL,\n  \
    updated_at INTEGER NOT NULL\n  \
)";

/// The three upstream indexes, in creation order.
pub const SKILLS_INDEXES: [&str; 3] = [
    "CREATE INDEX IF NOT EXISTS idx_skills_enabled ON skills(enabled)",
    "CREATE INDEX IF NOT EXISTS idx_skills_container_tag ON skills(container_tag)",
    "CREATE INDEX IF NOT EXISTS idx_skills_updated_at ON skills(updated_at DESC)",
];

/// The full baseline statement list (DDL + indexes), in creation order.
pub fn skills_table_ddl() -> Vec<String> {
    let mut statements = vec![SKILLS_TABLE_DDL.to_string()];
    statements.extend(SKILLS_INDEXES.iter().map(|sql| (*sql).to_string()));
    statements
}

/// Upstream `SkillRecord` — one `skills` row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub description: String,
    pub content: String,
    pub autoinvoke: bool,
    pub enabled: bool,
    pub container_tag: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Upstream `SkillInput` — the create payload.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillInput {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub content: String,
    pub autoinvoke: Option<bool>,
    pub enabled: Option<bool>,
    pub container_tag: Option<String>,
}

/// Upstream `SkillUpdate` — the patch payload (`None` = "not sent").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillPatch {
    pub name: Option<String>,
    pub description: Option<String>,
    pub content: Option<String>,
    pub autoinvoke: Option<bool>,
    pub enabled: Option<bool>,
    pub container_tag: Option<String>,
}

/// Upstream `isValidSkillId` — the id rule, verbatim.
///
/// The rule is a **whitelist**: lowercase ASCII alphanumerics separated by
/// single hyphens. Every other byte is refused, which is what makes the
/// path-traversal guard structural: a `/`, a `\` and a percent-encoded
/// `%2F` all fail the pattern, so a skill id can never escape
/// `{storage}/skills-content/`.
pub fn is_valid_skill_id(id: &str) -> bool {
    if id.len() < SKILL_ID_MIN_LENGTH || id.len() > SKILL_ID_MAX_LENGTH {
        return false;
    }
    // `^[a-z0-9]+(-[a-z0-9]+)*$` — every hyphen-separated segment is
    // non-empty and made only of lowercase ASCII alphanumerics.
    id.split('-').all(|segment| {
        !segment.is_empty()
            && segment
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
    })
}

/// The crate's failure surface.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum SkillsError {
    /// The id is not a legal skill id (upstream's
    /// "Invalid skill id: use lowercase kebab-case …").
    #[error(
        "Invalid skill id: use lowercase kebab-case (a-z, 0-9, single hyphens), 1-64 characters"
    )]
    InvalidId,
    /// The name is missing.
    #[error("Skill name is required")]
    NameRequired,
    /// The content is missing.
    #[error("Skill content is required")]
    ContentRequired,
    /// The id already exists.
    #[error("Skill already exists: {0}")]
    AlreadyExists(String),
    /// The skill does not exist.
    #[error("Skill not found: {0}")]
    NotFound(String),
    /// The write did not produce a readable row.
    #[error("Failed to write skill: {0}")]
    MissingAfterWrite(String),
    /// The content file could not be written or removed.
    #[error("skills content io error: {0}")]
    Io(String),
    /// The store refused the operation.
    #[error("skills store error: {0}")]
    Store(String),
}

impl From<mem_store::error::StoreError> for SkillsError {
    fn from(error: mem_store::error::StoreError) -> Self {
        SkillsError::Store(error.message().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{SKILL_ID_MAX_LENGTH, is_valid_skill_id};

    #[test]
    fn accepts_kebab_case_ids() {
        assert!(is_valid_skill_id("git-release"));
        assert!(is_valid_skill_id("a"));
        assert!(is_valid_skill_id("pr-review-v2"));
        assert!(is_valid_skill_id("0"));
        assert!(is_valid_skill_id("a1-b2-c3"));
    }

    #[test]
    fn rejects_invalid_ids() {
        assert!(!is_valid_skill_id("Git-Release"));
        assert!(!is_valid_skill_id("-bad"));
        assert!(!is_valid_skill_id("bad-"));
        assert!(!is_valid_skill_id("bad--id"));
        assert!(!is_valid_skill_id("has space"));
        assert!(!is_valid_skill_id(""));
        assert!(!is_valid_skill_id("under_score"));
    }

    #[test]
    fn rejects_oversized_ids() {
        let long = "a".repeat(SKILL_ID_MAX_LENGTH);
        assert!(is_valid_skill_id(&long));
        let too_long = "a".repeat(SKILL_ID_MAX_LENGTH + 1);
        assert!(!is_valid_skill_id(&too_long));
    }

    #[test]
    fn rejects_path_traversal_shapes() {
        assert!(!is_valid_skill_id("../escape"));
        assert!(!is_valid_skill_id("a/b"));
        assert!(!is_valid_skill_id("a\\b"));
        assert!(!is_valid_skill_id("%2F"));
        assert!(!is_valid_skill_id("%2f"));
        assert!(!is_valid_skill_id(".."));
    }
}
