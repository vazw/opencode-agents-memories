//! Port of `tests/tags.test.ts` — container-tag construction and project
//! identity (`src/services/tags.ts`; the `getProjectName` cases themselves
//! were ported in W0.3 → `tests/windows_path.rs`).
//!
//! Git is probed through the `GitProbe` seam via a test-local `FakeGitProbe`
//! — no git spawns in this file (the real-probe cases live in
//! `tests/project_scope.rs`). Env is an injected `BTreeMap` (never the
//! process environment).

use mem_core::sha256;
use mem_core::tags::GitProbe;
use mem_core::tags::TagResolver;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

fn hash16(input: &str) -> String {
    sha256::hex(&sha256::digest(input.as_bytes()))[..16].to_string()
}

/// Settable fake: every field returned verbatim, the queried directory
/// ignored.
struct FakeGitProbe {
    common_dir: Option<PathBuf>,
    top_level: Option<PathBuf>,
    remote_origin_url: Option<String>,
    user_email: Option<String>,
    user_name: Option<String>,
}

impl GitProbe for FakeGitProbe {
    fn common_dir(&self, _directory: &Path) -> Option<PathBuf> {
        self.common_dir.clone()
    }
    fn top_level(&self, _directory: &Path) -> Option<PathBuf> {
        self.top_level.clone()
    }
    fn remote_origin_url(&self, _directory: &Path) -> Option<String> {
        self.remote_origin_url.clone()
    }
    fn user_email(&self, _directory: &Path) -> Option<String> {
        self.user_email.clone()
    }
    fn user_name(&self, _directory: &Path) -> Option<String> {
        self.user_name.clone()
    }
}

fn fake_git() -> FakeGitProbe {
    FakeGitProbe {
        common_dir: None,
        top_level: None,
        remote_origin_url: None,
        user_email: None,
        user_name: None,
    }
}

fn env_map(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
    entries
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn resolver<'a>(
    prefix: &'a str,
    email_override: Option<&'a str>,
    name_override: Option<&'a str>,
    env: &'a BTreeMap<String, String>,
    git: &'a FakeGitProbe,
) -> TagResolver<'a> {
    TagResolver::new(prefix, email_override, name_override, env, git)
}

#[test]
fn test_user_tag_prefers_config_email_override() {
    // Upstream: CONFIG.userEmailOverride || getGitEmail(directory).
    let git = FakeGitProbe {
        user_email: Some("git@example.com".to_string()),
        user_name: Some("Git Name".to_string()),
        ..fake_git()
    };
    let env = env_map(&[]);
    let r = resolver("opencode", Some("override@example.com"), None, &env, &git);
    let info = r.user_tag_info(Path::new("/any/dir"));

    assert_eq!(
        info.tag,
        format!("opencode_user_{}", hash16("override@example.com"))
    );
    assert_eq!(info.user_email.as_deref(), Some("override@example.com"));
    // Display name / user_name still come from the probe (no name override).
    assert_eq!(info.display_name, "Git Name");
    assert_eq!(info.user_name.as_deref(), Some("Git Name"));
}

#[test]
fn test_user_tag_falls_back_to_git_email() {
    let git = FakeGitProbe {
        user_email: Some("dev@example.com".to_string()),
        user_name: None,
        ..fake_git()
    };
    let env = env_map(&[]);
    let r = resolver("opencode", None, None, &env, &git);
    let info = r.user_tag_info(Path::new("/any/dir"));

    assert_eq!(
        info.tag,
        format!("opencode_user_{}", hash16("dev@example.com"))
    );
    // No name → display name is the email.
    assert_eq!(info.display_name, "dev@example.com");
    assert_eq!(info.user_name, None);
    assert_eq!(info.user_email.as_deref(), Some("dev@example.com"));
}

#[test]
fn test_user_tag_fallback_chain_reaches_env_and_anonymous() {
    // Chain: name → USER → USERNAME → "anonymous" (empty strings skipped).
    // user_email is None on the whole fallback path.

    // Probe name wins over env.
    let git = FakeGitProbe {
        user_name: Some("gitname".to_string()),
        ..fake_git()
    };
    let env = env_map(&[("USER", "envuser"), ("USERNAME", "winuser")]);
    let info = resolver("opencode", None, None, &env, &git).user_tag_info(Path::new("/d"));
    assert_eq!(info.tag, format!("opencode_user_{}", hash16("gitname")));
    assert_eq!(info.user_email, None);

    // USER when no probe name.
    let git = fake_git();
    let info = resolver("opencode", None, None, &env, &git).user_tag_info(Path::new("/d"));
    assert_eq!(info.tag, format!("opencode_user_{}", hash16("envuser")));
    assert_eq!(info.user_name.as_deref(), Some("envuser"));
    assert_eq!(info.user_email, None);

    // USERNAME when USER is absent/empty.
    let env_no_user = env_map(&[("USER", ""), ("USERNAME", "winuser")]);
    let info = resolver("opencode", None, None, &env_no_user, &git).user_tag_info(Path::new("/d"));
    assert_eq!(info.tag, format!("opencode_user_{}", hash16("winuser")));

    // anonymous when everything is empty.
    let env_empty = env_map(&[]);
    let info = resolver("opencode", None, None, &env_empty, &git).user_tag_info(Path::new("/d"));
    assert_eq!(info.tag, format!("opencode_user_{}", hash16("anonymous")));
    assert_eq!(info.display_name, "anonymous");
    assert_eq!(info.user_name.as_deref(), Some("anonymous"));
    assert_eq!(info.user_email, None);
}

#[test]
fn test_project_tag_uses_git_common_dir_identity() {
    // Worktrees of one repo share the common dir → one identity.
    let git = FakeGitProbe {
        common_dir: Some(PathBuf::from("/repo/.git")),
        ..fake_git()
    };
    let env = env_map(&[]);
    let info =
        resolver("opencode", None, None, &env, &git).project_tag_info(Path::new("/repo/worktree"));

    assert_eq!(
        info.tag,
        format!("opencode_project_{}", hash16("git-common:/repo/.git"))
    );
    // The common dir's basename is `.git`, so the project root is its parent.
    assert_eq!(info.project_path.as_deref(), Some(Path::new("/repo")));
    assert_eq!(info.project_name.as_deref(), Some("repo"));
}

#[test]
fn test_project_tag_uses_remote_url_identity_without_common_dir() {
    let git = FakeGitProbe {
        top_level: Some(PathBuf::from("/repo2")),
        remote_origin_url: Some("https://example.com/org/repo2.git".to_string()),
        ..fake_git()
    };
    let env = env_map(&[]);
    let info = resolver("opencode", None, None, &env, &git).project_tag_info(Path::new("/repo2"));

    assert_eq!(
        info.tag,
        format!(
            "opencode_project_{}",
            hash16("remote:https://example.com/org/repo2.git")
        )
    );
    assert_eq!(
        info.git_repo_url.as_deref(),
        Some("https://example.com/org/repo2.git")
    );
    assert_eq!(info.project_path.as_deref(), Some(Path::new("/repo2")));
}

#[test]
fn test_project_tag_falls_back_to_path_identity_for_non_git_directory() {
    let git = fake_git();
    let env = env_map(&[]);
    let info =
        resolver("opencode", None, None, &env, &git).project_tag_info(Path::new("/plain/dir"));

    assert_eq!(
        info.tag,
        format!("opencode_project_{}", hash16("path:/plain/dir"))
    );
    assert_eq!(info.git_repo_url, None);
    assert_eq!(info.project_path.as_deref(), Some(Path::new("/plain/dir")));
    assert_eq!(info.project_name.as_deref(), Some("dir"));
}

#[test]
fn test_tags_returns_user_and_project_info() {
    let git = FakeGitProbe {
        user_email: Some("dev@example.com".to_string()),
        ..fake_git()
    };
    let env = env_map(&[]);
    let tags = resolver("opencode", None, None, &env, &git).tags(Path::new("/plain/dir"));

    assert!(tags.user.tag.contains("_user_"));
    assert_eq!(
        tags.user.tag,
        format!("opencode_user_{}", hash16("dev@example.com"))
    );
    assert!(tags.project.tag.contains("_project_"));
    assert_eq!(
        tags.project.tag,
        format!("opencode_project_{}", hash16("path:/plain/dir"))
    );
}

#[test]
fn test_container_tag_prefix_comes_from_config() {
    let git = FakeGitProbe {
        user_email: Some("dev@example.com".to_string()),
        ..fake_git()
    };
    let env = env_map(&[]);
    let tags = resolver("acme", None, None, &env, &git).tags(Path::new("/plain/dir"));

    assert!(tags.user.tag.starts_with("acme_user_"));
    assert!(tags.project.tag.starts_with("acme_project_"));
}

#[test]
fn test_marker_root_wins_over_git_probe_and_drops_remote_url() {
    let root = std::env::temp_dir().join(format!("w11-tags-marker-{}", std::process::id()));
    let nested = root.join("sub").join("deeper");
    fs::create_dir_all(&nested).unwrap();
    fs::write(root.join(".opencode-mem-project"), "").unwrap();

    let git = FakeGitProbe {
        common_dir: Some(root.join(".git")),
        top_level: Some(root.clone()),
        remote_origin_url: Some("https://example.com/org/nested-repo.git".to_string()),
        ..fake_git()
    };
    let env = env_map(&[]);
    let info = resolver("opencode", None, None, &env, &git).project_tag_info(&nested);

    assert_eq!(
        info.tag,
        format!(
            "opencode_project_{}",
            hash16(&format!("path:{}", root.display()))
        )
    );
    assert_eq!(info.project_path.as_deref(), Some(root.as_path()));
    assert_eq!(info.git_repo_url, None);
    assert_eq!(
        info.project_name.as_deref(),
        Some(root.file_name().unwrap().to_str().unwrap())
    );

    let _ = fs::remove_dir_all(&root);
}
