//! Project identity and container tags, ported from upstream
//! `src/services/tags.ts` (subject of `tests/tags.test.ts` — this module;
//! the `getProjectName` cases were ported in W0.3 → `utils::paths`).
//!
//! Identity precedence pins desktop grouping and the memory shard layout:
//! a `path:` marker root (`.opencode-mem-project`, how a multi-repo
//! workspace collapses every nested repo onto one store) beats
//! `git-common:<common dir>` (how worktrees of one repo share one
//! identity) beats `remote:<origin url>` beats `path:<directory>`. When a
//! marker pins the root, any git remote belongs to a single nested
//! sub-repo and would mislead the grouped workspace, so `git_repo_url` is
//! unset (upstream comment at `getProjectTagInfo`).
//!
//! Git is probed exclusively through the [`GitProbe`] seam
//! (`crate::tags::probe`); a repo-local `git` executable must never be
//! trusted (see `SystemGitProbe`, W1.1 Step 6). Path→string renderings use
//! `Path::display()`; on Unix that is byte-identical to upstream
//! `path.normalize` output for the absolute paths used here (W0.3
//! divergence precedent).

pub mod migration;
pub mod probe;

pub use probe::GitProbe;

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::env::EnvSource;
use crate::sha256;

/// Marker file whose presence pins a directory as the opencode-mem project
/// root (upstream `PROJECT_MARKER`): every session started anywhere
/// underneath resolves onto that root instead of whichever physical git
/// repository the working directory lives in.
pub const PROJECT_MARKER: &str = ".opencode-mem-project";

/// Upstream `sha256` shorthand: `createHash("sha256").update(x).digest("hex").slice(0, 16)`
/// over the vendored [`sha256`] module.
fn identity_hash(input: &str) -> String {
    sha256::hex(&sha256::digest(input.as_bytes()))[..16].to_string()
}

/// One rendered tag and everything the downstream layers need to display
/// it (upstream `TagInfo`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagInfo {
    pub tag: String,
    pub display_name: String,
    pub user_name: Option<String>,
    pub user_email: Option<String>,
    pub project_path: Option<PathBuf>,
    pub project_name: Option<String>,
    pub git_repo_url: Option<String>,
}

/// Both half-tags for one directory (port of upstream `getTags`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tags {
    pub user: TagInfo,
    pub project: TagInfo,
}

/// Resolves user/project tag info from explicit parts (upstream
/// `getUserTagInfo` / `getProjectTagInfo` with `CONFIG` injected field by
/// field instead of imported).
pub struct TagResolver<'a> {
    container_tag_prefix: &'a str,
    user_email_override: Option<&'a str>,
    user_name_override: Option<&'a str>,
    env: &'a dyn EnvSource,
    git: &'a dyn GitProbe,
}

impl<'a> TagResolver<'a> {
    pub fn new(
        container_tag_prefix: &'a str,
        user_email_override: Option<&'a str>,
        user_name_override: Option<&'a str>,
        env: &'a dyn EnvSource,
        git: &'a dyn GitProbe,
    ) -> Self {
        Self {
            container_tag_prefix,
            user_email_override,
            user_name_override,
            env,
            git,
        }
    }

    /// Upstream `getUserTagInfo`: config email/name overrides (empty
    /// strings skipped, mirroring JS `||`) fall back to the git probe; the
    /// final fallback chain is name → `USER` → `USERNAME` → "anonymous".
    pub fn user_tag_info(&self, directory: &Path) -> TagInfo {
        let email = self
            .user_email_override
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| self.git.user_email(directory))
            .filter(|value| !value.is_empty());
        let name = self
            .user_name_override
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| self.git.user_name(directory))
            .filter(|value| !value.is_empty());

        if let Some(email) = email {
            return TagInfo {
                tag: format!(
                    "{}_user_{}",
                    self.container_tag_prefix,
                    identity_hash(&email)
                ),
                display_name: name.clone().unwrap_or_else(|| email.clone()),
                user_name: name,
                user_email: Some(email),
                project_path: None,
                project_name: None,
                git_repo_url: None,
            };
        }

        let fallback = name
            .or_else(|| self.env.get("USER").filter(|value| !value.is_empty()))
            .or_else(|| self.env.get("USERNAME").filter(|value| !value.is_empty()))
            .unwrap_or_else(|| "anonymous".to_string());
        TagInfo {
            tag: format!(
                "{}_user_{}",
                self.container_tag_prefix,
                identity_hash(&fallback)
            ),
            display_name: fallback.clone(),
            user_name: Some(fallback),
            user_email: None,
            project_path: None,
            project_name: None,
            git_repo_url: None,
        }
    }

    /// Upstream `getProjectTagInfo`: the marker is resolved exactly once so
    /// a single call never walks the ancestry more than once; root, name,
    /// remote and identity all derive from that single resolution.
    pub fn project_tag_info(&self, directory: &Path) -> TagInfo {
        let marker_root = find_marker_project_root(directory);
        let project_root = marker_root
            .clone()
            .unwrap_or_else(|| get_git_project_root(directory, self.git));
        let project_name = crate::utils::paths::get_project_name(&project_root.to_string_lossy());
        let git_repo_url = if marker_root.is_some() {
            None
        } else {
            self.git.remote_origin_url(directory)
        };
        let identity = match &marker_root {
            Some(root) => format!("path:{}", root.display()),
            None => get_git_project_identity(&project_root, self.git),
        };

        TagInfo {
            tag: format!(
                "{}_project_{}",
                self.container_tag_prefix,
                identity_hash(&identity)
            ),
            display_name: project_root.display().to_string(),
            user_name: None,
            user_email: None,
            project_path: Some(project_root),
            project_name: Some(project_name),
            git_repo_url,
        }
    }

    /// Upstream `getTags`.
    pub fn tags(&self, directory: &Path) -> Tags {
        Tags {
            user: self.user_tag_info(directory),
            project: self.project_tag_info(directory),
        }
    }
}

/// Walks up from `directory` (inclusive) to the filesystem root looking
/// for [`PROJECT_MARKER`]; returns the first directory containing it, or
/// `None` so the caller can fall back to git detection (upstream
/// `resolve`).
///
/// Documented divergence: `std::path::absolute` does not collapse `..`
/// components — production call sites pass absolute project directories.
pub fn find_marker_project_root(directory: &Path) -> Option<PathBuf> {
    let mut dir = std::path::absolute(directory).unwrap_or_else(|_| directory.to_path_buf());
    loop {
        if dir.join(PROJECT_MARKER).exists() {
            return Some(dir);
        }
        match dir.parent() {
            Some(parent) if parent != dir => dir = parent.to_path_buf(),
            _ => return None,
        }
    }
}

/// Git-only project-root fallback (upstream `getGitProjectRoot`): a common
/// dir whose basename is `.git` means the directory is a worktree of a
/// repo rooted at the common dir's parent; else `--show-toplevel`; else
/// the raw directory.
pub fn get_git_project_root(directory: &Path, git: &dyn GitProbe) -> PathBuf {
    if let Some(common_dir) = git.common_dir(directory)
        && common_dir.file_name() == Some(OsStr::new(".git"))
    {
        return common_dir
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or(common_dir);
    }
    git.top_level(directory)
        .unwrap_or_else(|| directory.to_path_buf())
}

/// Git-only identity precedence (upstream `getGitProjectIdentity`):
/// common dir → origin url → raw path.
pub fn get_git_project_identity(directory: &Path, git: &dyn GitProbe) -> String {
    if let Some(common_dir) = git.common_dir(directory) {
        return format!("git-common:{}", common_dir.display());
    }
    if let Some(url) = git.remote_origin_url(directory) {
        return format!("remote:{url}");
    }
    format!("path:{}", directory.display())
}

/// Marker-aware project root (upstream `getProjectRoot`).
pub fn get_project_root(directory: &Path, git: &dyn GitProbe) -> PathBuf {
    find_marker_project_root(directory).unwrap_or_else(|| get_git_project_root(directory, git))
}

/// Marker-aware identity (upstream standalone `getProjectIdentity` — it
/// passes the raw directory into the git identity, while
/// `getProjectTagInfo` passes the project root; both are ported as-is).
pub fn get_project_identity(directory: &Path, git: &dyn GitProbe) -> String {
    match find_marker_project_root(directory) {
        Some(root) => format!("path:{}", root.display()),
        None => get_git_project_identity(directory, git),
    }
}

/// The tags-embedding input line (upstream `vector-utils.ts`; homed here
/// because the migration core computes the tags-embedding text and later
/// consumers reuse this instead of duplicating it).
pub fn format_tags_for_embedding(tags: &[String]) -> String {
    format!("Topics: {}", tags.join(", "))
}
