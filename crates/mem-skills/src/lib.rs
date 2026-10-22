//! mem-skills — the skills manager (W6.4).
//!
//! Upstream `src/services/skills/skills-manager.ts` + `types.ts` are the
//! behavioural contract: a `skills.db` table plus one `SKILL.md` content
//! file per **enabled** skill, written under
//! `{storage_path}/skills-content/{id}/SKILL.md`.
//!
//! Two load-bearing invariants survive the port:
//!
//! - the **content file exists iff the skill is enabled** — a disabled skill
//!   has no `SKILL.md`, so OpenCode's skill discovery never sees it;
//! - every mutation **notifies the change listeners** exactly once
//!   (`on_skills_changed`), which is what drives the host-side reload.
//!
//! DIVERGENCE (recorded): upstream's `SkillsManager` is a process-wide
//! singleton over the global `CONFIG.storagePath`; the port takes the
//! storage path as an explicit constructor argument (the established
//! W1.x/W2.x/W3.x pattern) and exposes `reset`/`rebind` for the
//! close-and-reopen path.

/// Stable crate identifier for composition checks.
pub const CRATE_NAME: &str = "mem-skills";

pub mod content;
pub mod manager;
pub mod schema;

pub use content::{format_markdown, skill_content_path};
pub use manager::{
    CatalogEntry, SkillsChangedGuard, SkillsChangedListener, SkillsManager, notify_skills_changed,
    on_skills_changed,
};
pub use schema::{
    SKILL_ID_MAX_LENGTH, SKILL_ID_MIN_LENGTH, SKILL_ID_PATTERN, SKILLS_CONTENT_DIR, SKILLS_DB_NAME,
    Skill, SkillInput, SkillPatch, SkillsError, is_valid_skill_id, skills_table_ddl,
};
