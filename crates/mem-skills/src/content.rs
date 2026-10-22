//! The per-skill `SKILL.md` content file (upstream
//! `skills-manager.ts`'s `skillPath` / `formatMarkdown` / `writeSkillFile` /
//! `removeSkillFile`).
//!
//! The layout is upstream's, byte for byte:
//!
//! ```text
//! {storage_path}/skills.db
//! {storage_path}/skills-content/{id}/SKILL.md
//! ```
//!
//! The markdown is upstream's frontmatter block, character for character:
//!
//! ```text
//! ---
//! name: "Git Release"
//! description: "Prepare releases"
//! metadata:
//!   opencode/autoinvoke: "true"
//! ---
//!
//! <content>
//! ```
//!
//! The name/description are `JSON.stringify`-quoted, so a name carrying a
//! quote or a newline cannot break the frontmatter — the same reason
//! upstream quotes them.

use std::path::{Path, PathBuf};

use crate::schema::{SKILLS_CONTENT_DIR, Skill, SkillsError};

/// The absolute `SKILL.md` path of one skill (upstream `skillPath`).
///
/// The path is derived from the **validated** skill id at every call site;
/// `skill_content_path` itself refuses an id that is not a legal skill id,
/// so a traversal attempt fails here rather than at the filesystem.
pub fn skill_content_path(storage_path: &Path, id: &str) -> Result<PathBuf, SkillsError> {
    if !crate::schema::is_valid_skill_id(id) {
        return Err(SkillsError::InvalidId);
    }
    Ok(storage_path
        .join(SKILLS_CONTENT_DIR)
        .join(id)
        .join("SKILL.md"))
}

/// Upstream `formatMarkdown` — the OpenCode-compatible `SKILL.md` body.
///
/// The frontmatter is exactly upstream's: two `---` fences, the quoted
/// `name`, the quoted `description`, the `metadata:` block with the
/// indented `opencode/autoinvoke` flag, one blank line, then the content.
pub fn format_markdown(skill: &Skill) -> String {
    let name = serde_json::to_string(&skill.name).unwrap_or_else(|_| "\"\"".to_string());
    let description =
        serde_json::to_string(&skill.description).unwrap_or_else(|_| "\"\"".to_string());
    let autoinvoke = if skill.autoinvoke {
        "\"true\""
    } else {
        "\"false\""
    };
    [
        "---".to_string(),
        format!("name: {name}"),
        format!("description: {description}"),
        "metadata:".to_string(),
        format!("  opencode/autoinvoke: {autoinvoke}"),
        "---".to_string(),
        String::new(),
        skill.content.clone(),
    ]
    .join("\n")
}

/// The failure surface of the content helpers.
#[cfg(test)]
mod tests {
    use super::format_markdown;
    use crate::schema::Skill;

    fn skill() -> Skill {
        Skill {
            id: "git-release".to_string(),
            name: "Git Release".to_string(),
            description: "Prepare releases".to_string(),
            content: "## Steps\n\n1. Tag".to_string(),
            autoinvoke: true,
            enabled: true,
            container_tag: None,
            created_at: 1,
            updated_at: 2,
        }
    }

    #[test]
    fn renders_the_upstream_frontmatter() {
        let markdown = format_markdown(&skill());
        assert_eq!(
            markdown,
            "---\nname: \"Git Release\"\ndescription: \"Prepare releases\"\nmetadata:\n  opencode/autoinvoke: \"true\"\n---\n\n## Steps\n\n1. Tag"
        );
    }

    #[test]
    fn renders_the_false_autoinvoke_flag() {
        let mut skill = skill();
        skill.autoinvoke = false;
        assert!(format_markdown(&skill).contains("  opencode/autoinvoke: \"false\""));
    }
}
