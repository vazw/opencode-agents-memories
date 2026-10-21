//! W5.3 Step 2 — the ONE project-scope seam.
//!
//! Every consumer (tier-1 write, inject read, episodic request key and the
//! `PromotionScope`) must receive the SAME resolved
//! `{container_tag, scope_hash}` derived from the wrapper's host
//! `project_id` through exactly one `TagResolver::project_tag_info` call
//! plus exactly one `extract_scope_from_container_tag` validation. The test
//! drives real git repositories in hermetic temp dirs (same hermeticity as
//! `mem-core`'s own `project_scope.rs`).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const PREFIX: &str = "opencode";

fn git(args: &[&str], cwd: &Path) {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("project scope tests require a git binary on PATH");
    assert!(
        out.status.success(),
        "git {args:?} in {cwd:?} failed: {} — {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "w53-project-scope-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.subsec_nanos())
            .unwrap_or(0)
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn create_repo(path: &Path) {
    fs::create_dir_all(path).unwrap();
    git(&["init", "-q", "-b", "main"], path);
}

fn scan_env() -> BTreeMap<String, String> {
    [("PATH", std::env::var("PATH").unwrap_or_default()).to_owned()]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
}

fn resolver<'a>(
    env: &'a BTreeMap<String, String>,
    git: &'a mem_core::tags::probe::SystemGitProbe<'a>,
) -> mem_core::tags::TagResolver<'a> {
    mem_core::tags::TagResolver::new(PREFIX, None, None, env, git)
}

/// AC-Step2: given the wrapper's host directory `project_id`, the seam
/// derives the project tag and the validated scope hash, and both agree
/// with an independent `TagResolver::project_tag_info` call (ONE derivation,
/// one wire shape for every consumer).
#[test]
fn test_project_scope_from_directory_derives_one_project_tag_and_scope() {
    let repo = scratch("repo");
    create_repo(&repo);

    let env = scan_env();
    let git_probe = mem_core::tags::probe::SystemGitProbe::new(&env);
    let tag_resolver = resolver(&env, &git_probe);

    let resolved =
        mem_bridge::project_scope::resolve_project_scope(&repo.to_string_lossy(), &tag_resolver)
            .expect("a real repo directory resolves");

    // Exactly the prefix_project_<16hex> shape.
    let prefix = format!("{PREFIX}_project_");
    let hash = resolved
        .container_tag
        .strip_prefix(prefix.as_str())
        .expect("container tag carries the project scope");
    assert_eq!(
        hash.len(),
        16,
        "the scope hash segment is 16 lowercase hex characters"
    );

    // The resolved hash agrees with the tag's own hash segment.
    assert_eq!(resolved.scope_hash, hash);

    // The validated scope re-parse agrees: Scope::Project + Some(hash).
    let scope_ref = mem_core::scope::extract_scope_from_container_tag(&resolved.container_tag)
        .expect("the produced tag re-validates through the scope parser");
    assert_eq!(scope_ref.scope, mem_core::scope::Scope::Project);
    assert_eq!(
        scope_ref.hash.map(|h| h.as_str().to_string()),
        Some(hash.to_string())
    );

    // One derivation means the seam's tag IS TagResolver's tag for the same
    // directory — not a separately hashed rendering.
    let direct = tag_resolver.project_tag_info(Path::new(&repo));
    assert_eq!(resolved.container_tag, direct.tag);

    // Reading a second time for the same project yields the same triple, so
    // write/read/promotion consumers never diverge.
    let resolved_again =
        mem_bridge::project_scope::resolve_project_scope(&repo.to_string_lossy(), &tag_resolver)
            .expect("resolution is stable");
    assert_eq!(resolved.container_tag, resolved_again.container_tag);
    assert_eq!(resolved.scope_hash, resolved_again.scope_hash);

    let _ = fs::remove_dir_all(&repo);
}

/// A blank project scope is a hard rejection — never the empty tag (the
/// empty tag means "all memories" to the public list function).
#[test]
fn test_project_scope_blank_project_id_is_rejected() {
    let env = scan_env();
    let git_probe = mem_core::tags::probe::SystemGitProbe::new(&env);
    let tag_resolver = resolver(&env, &git_probe);
    for blank in ["", "   "] {
        let outcome = mem_bridge::project_scope::resolve_project_scope(blank, &tag_resolver);
        assert!(
            outcome.is_err(),
            "blank project id {blank:?} must be rejected"
        );
        let _ = outcome;
    }
}
