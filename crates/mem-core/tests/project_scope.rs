//! Port of `tests/project-scope.test.ts` — project identity over REAL git
//! repositories in hermetic temp dirs.
//!
//! The 3 win32-only upstream cases (`it.skipIf(process.platform !== "win32")`
//! — git-shim `bat`/`cmd` refusal, nested-repo `git.exe` rejection, trusted
//! `git.cmd` wrapper) are N-A here: upstream skips them on non-win32 and this
//! platform has no Windows runner (recorded in `docs/agents/testing.md`).
//! Their Linux-relevant containment behavior is covered by
//! `test_probe_refuses_git_executable_inside_the_untrusted_project_root`.
//!
//! Hermeticity: every git invocation strips the `GIT_DIR` family and pins
//! `GIT_CONFIG_GLOBAL=/dev/null`, `GIT_CONFIG_SYSTEM=/dev/null`,
//! `GIT_CONFIG_NOSYSTEM=1`, so a developer's `~/.gitconfig` (e.g. commit
//! signing) cannot break the run. A missing git binary panics loudly —
//! never a silent skip.

use mem_core::tags::probe::SystemGitProbe;
use mem_core::tags::{GitProbe, TagResolver, find_marker_project_root};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
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
        .expect("project identity tests require a git binary on PATH");
    assert!(
        out.status.success(),
        "git {args:?} in {cwd:?} failed: {} — {} — {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
        "project identity tests require a git binary on PATH"
    );
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("w11-project-{name}-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn create_repo(path: &Path) {
    fs::create_dir_all(path).unwrap();
    git(&["init", "-q", "-b", "main"], path);
    git(&["config", "user.email", "test@example.com"], path);
    git(&["config", "user.name", "Test"], path);
    fs::write(path.join("f.txt"), "x").unwrap();
    git(&["add", "-A"], path);
    git(
        &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "init"],
        path,
    );
}

fn scan_env() -> BTreeMap<String, String> {
    [("PATH", std::env::var("PATH").unwrap_or_default()).to_owned()]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
}

fn resolver<'a>(env: &'a BTreeMap<String, String>, git: &'a SystemGitProbe<'a>) -> TagResolver<'a> {
    TagResolver::new(PREFIX, None, None, env, git)
}

fn write_shim(bin_dir: &Path, sentinel: &Path) {
    fs::create_dir_all(bin_dir).unwrap();
    let script = format!("#!/bin/sh\ntouch {}\nexit 1\n", sentinel.display());
    let shim = bin_dir.join("git");
    fs::write(&shim, script).unwrap();
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
}

fn cleanup(paths: &[&Path]) {
    for path in paths {
        let _ = fs::remove_dir_all(path);
    }
}

#[test]
fn test_worktrees_of_one_repo_share_a_project_tag() {
    // Upstream: "uses one project tag across worktrees in the same repo".
    let repo = scratch("wt-repo");
    create_repo(&repo);
    let wt = std::env::temp_dir().join(format!("w11-project-wt-wt-{}", std::process::id()));
    git(
        &[
            "worktree",
            "add",
            "-q",
            wt.to_str().unwrap(),
            "-b",
            "wt-branch",
        ],
        &repo,
    );

    let env = scan_env();
    let probe = SystemGitProbe::new(&env);
    // Shared physical common dir:
    assert_eq!(probe.common_dir(&repo), probe.common_dir(&wt));

    let r = resolver(&env, &probe);
    let t_repo = r.project_tag_info(&repo);
    let t_wt = r.project_tag_info(&wt);
    assert_eq!(t_repo, t_wt);
    assert_eq!(
        t_wt.project_path.as_deref(),
        Some(repo.as_path()),
        "project_path must be the repo root, not the worktree dir: {:?} vs {:?}",
        t_wt.project_path,
        repo
    );
    assert_eq!(
        t_wt.project_name.as_deref(),
        Some(repo.file_name().unwrap().to_str().unwrap())
    );

    cleanup(&[&wt, &repo]);
}

#[test]
fn test_unrelated_non_git_directories_get_distinct_tags() {
    let a = scratch("nongit-a");
    let b = scratch("nongit-b");

    let env = scan_env();
    let probe = SystemGitProbe::new(&env);
    let r = resolver(&env, &probe);
    let t_a = r.project_tag_info(&a);
    let t_b = r.project_tag_info(&b);
    assert_ne!(t_a.tag, t_b.tag);

    cleanup(&[&a, &b]);
}

#[test]
fn test_nested_paths_inside_a_repo_share_the_project_tag() {
    let repo = scratch("nested-repo");
    create_repo(&repo);
    let nested = repo.join("crate").join("src");
    fs::create_dir_all(&nested).unwrap();

    let env = scan_env();
    let probe = SystemGitProbe::new(&env);
    let r = resolver(&env, &probe);
    let t_repo = r.project_tag_info(&repo);
    let t_nested = r.project_tag_info(&nested);
    assert_eq!(t_nested.tag, t_repo.tag);
    assert_eq!(t_nested.project_path, t_repo.project_path);

    cleanup(&[&repo]);
}

#[test]
fn test_sibling_repos_without_marker_get_separate_tags() {
    // Upstream marker-case G: sibling repos, no workspace marker.
    let a = scratch("sibling-a");
    let b = scratch("sibling-b");
    create_repo(&a);
    create_repo(&b);

    let env = scan_env();
    let probe = SystemGitProbe::new(&env);
    let r = resolver(&env, &probe);
    assert_ne!(r.project_tag_info(&a).tag, r.project_tag_info(&b).tag);

    cleanup(&[&a, &b]);
}

#[test]
fn test_marker_collapses_nested_repos_onto_the_marker_root() {
    let ws = scratch("marker-ws");
    let a = ws.join("repo-a");
    let b = ws.join("repo-b");
    create_repo(&a);
    create_repo(&b);
    fs::write(ws.join(".opencode-mem-project"), "").unwrap();

    let env = scan_env();
    let probe = SystemGitProbe::new(&env);
    let r = resolver(&env, &probe);
    let t_root = r.project_tag_info(&ws);
    assert_eq!(r.project_tag_info(&a), t_root);
    assert_eq!(r.project_tag_info(&b), t_root);
    assert_eq!(t_root.project_path.as_deref(), Some(ws.as_path()));
    assert_eq!(
        t_root.project_name.as_deref(),
        Some(ws.file_name().unwrap().to_str().unwrap())
    );

    cleanup(&[&ws]);
}

#[test]
fn test_marker_resolves_deep_nested_paths_to_the_root() {
    // Upstream case I.
    let ws = scratch("marker-deep");
    let deep = ws.join("gateway").join("sdk").join("examples").join("api");
    fs::create_dir_all(&deep).unwrap();
    fs::write(ws.join(".opencode-mem-project"), "").unwrap();

    let env = scan_env();
    let probe = SystemGitProbe::new(&env);
    let r = resolver(&env, &probe);
    let info = r.project_tag_info(&deep);
    assert_eq!(info.project_path.as_deref(), Some(ws.as_path()));
    assert_eq!(
        info.tag,
        r.project_tag_info(&ws).tag,
        "deep nested path must resolve to the marker root's tag"
    );

    cleanup(&[&ws]);
}

#[test]
fn test_marker_wins_over_inner_repo_and_drops_remote_url() {
    let ws = scratch("marker-remote");
    let inner = ws.join("service");
    create_repo(&inner);
    git(
        &[
            "remote",
            "add",
            "origin",
            "https://example.com/org/service.git",
        ],
        &inner,
    );
    fs::write(ws.join(".opencode-mem-project"), "").unwrap();

    let env = scan_env();
    let probe = SystemGitProbe::new(&env);
    let r = resolver(&env, &probe);
    let info = r.project_tag_info(&inner);
    assert_eq!(info.project_path.as_deref(), Some(ws.as_path()));
    assert_eq!(
        info.git_repo_url, None,
        "marker pins the root: the nested repo's remote must not leak in"
    );

    cleanup(&[&ws]);
}

#[test]
fn test_find_marker_project_root_returns_none_without_marker_and_ancestor_with_marker() {
    // Upstream case K, incl. "session started at the marker root resolves
    // to itself".
    let none_dir = scratch("marker-none");
    assert_eq!(find_marker_project_root(&none_dir), None);

    let ws = scratch("marker-walk");
    let nested = ws.join("x").join("y");
    fs::create_dir_all(&nested).unwrap();
    assert_eq!(find_marker_project_root(&nested), None);
    fs::write(ws.join(".opencode-mem-project"), "").unwrap();
    assert_eq!(
        find_marker_project_root(&nested).as_deref(),
        Some(ws.as_path())
    );
    assert_eq!(find_marker_project_root(&ws).as_deref(), Some(ws.as_path()));

    cleanup(&[&ws, &none_dir]);
}

#[test]
fn test_probe_refuses_git_executable_inside_the_untrusted_project_root() {
    // Linux adaptation of the win32 shim cases: a repo-local `git` shim is
    // never trusted — the probe must not execute it — while a shim sitting
    // OUTSIDE the untrusted root is executed (the scan really runs trusted
    // candidates).
    let repo = scratch("shim-repo");
    create_repo(&repo);
    let refused_sentinel = repo.join("sentinel-refused");
    let hostile_bin = repo.join("hostile-bin");
    write_shim(&hostile_bin, &refused_sentinel);

    let hostile_env = [("PATH", hostile_bin.display().to_string())]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect::<BTreeMap<String, String>>();
    let probe = SystemGitProbe::new(&hostile_env);
    assert_eq!(probe.common_dir(&repo), None);
    assert!(
        !refused_sentinel.exists(),
        "repo-local git shim must never be executed"
    );

    // The same shim OUTSIDE the untrusted root IS a trusted candidate.
    let trusted_bin = scratch("shim-bin");
    let executed_sentinel = trusted_bin.join("sentinel-executed");
    write_shim(&trusted_bin, &executed_sentinel);
    let trusted_env = [("PATH", trusted_bin.display().to_string())]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect::<BTreeMap<String, String>>();
    let probe = SystemGitProbe::new(&trusted_env);
    let _ = probe.common_dir(&repo); // shim exits 1 → None, but it ran
    assert!(
        executed_sentinel.exists(),
        "a git shim outside the untrusted root must be executed by the scan"
    );

    cleanup(&[&repo, &trusted_bin]);
}
