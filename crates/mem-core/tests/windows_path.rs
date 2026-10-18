//! Port of upstream `example/opencode-mem/tests/windows-path.test.ts`
//! (v2.31.3 @ 906038e). The `getProjectName` cases mirror upstream
//! test-for-test; the `join`/`win32.dirname` sections map to std `Path`
//! behavior on the native platform (upstream's Node-specific path APIs are
//! covered-by std tests here plus `normalize_separators` for
//! foreign-separator strings). Documented divergence: host-dependent Node
//! root semantics for pathological foreign root paths (`C:\..`, `/..`) are
//! not emulated — inputs are display/identity strings, not filesystem ops.

use std::path::{Path, PathBuf};

use mem_core::utils::paths::{get_project_name, normalize_separators};

/// Upstream "should handle Unix-style paths correctly".
#[test]
fn test_get_project_name_unix_style_path() {
    assert_eq!(
        get_project_name("/home/user/projects/my-project"),
        "my-project"
    );
}

/// Upstream "should handle Windows-style paths correctly".
#[test]
fn test_get_project_name_windows_style_path() {
    assert_eq!(
        get_project_name("C:\\Users\\user\\projects\\my-project"),
        "my-project"
    );
}

/// Upstream "should handle Windows-style paths with forward slashes".
#[test]
fn test_get_project_name_windows_forward_slashes() {
    assert_eq!(
        get_project_name("C:/Users/user/projects/my-project"),
        "my-project"
    );
}

/// Upstream "should handle relative Windows paths".
#[test]
fn test_get_project_name_relative_windows_path() {
    assert_eq!(get_project_name("..\\projects\\my-project"), "my-project");
}

/// Upstream "should return the input if no path separators found".
#[test]
fn test_get_project_name_returns_input_without_separators() {
    assert_eq!(get_project_name("my-project"), "my-project");
}

/// Upstream asserts only `typeof result === "string"` for a trailing
/// separator; ours is the strengthened exact form.
#[test]
fn test_get_project_name_ignores_trailing_separator() {
    assert_eq!(get_project_name("/home/user/projects/my-app/"), "my-app");
}

#[test]
fn test_get_project_name_deeply_nested_path() {
    assert_eq!(get_project_name("/a/b/c/d/e/f/project"), "project");
}

#[test]
fn test_get_project_name_mixed_separators() {
    assert_eq!(
        get_project_name("C:\\Users/user/projects\\my-project"),
        "my-project"
    );
}

/// Documents the lexical `.`/`..` collapse (upstream `path.normalize`).
#[test]
fn test_get_project_name_collapses_dot_and_dot_dot_segments() {
    assert_eq!(
        get_project_name("/home/./user//projects/../my-project"),
        "my-project"
    );
    assert_eq!(get_project_name("/a/b/.."), "a");
}

/// Fallback is the RAW input (upstream `parts[parts.length-1] || directory`).
#[test]
fn test_get_project_name_returns_raw_input_when_no_segments_survive() {
    assert_eq!(get_project_name(""), "");
    assert_eq!(get_project_name("/"), "/");
    assert_eq!(get_project_name("."), ".");
}

#[test]
fn test_get_project_name_multi_byte_segment() {
    assert_eq!(get_project_name("/home/用户/我的项目"), "我的项目");
}

#[test]
fn test_normalize_separators_converts_backslashes_to_forward_slashes() {
    assert_eq!(normalize_separators("a\\b\\c"), "a/b/c");
    assert_eq!(normalize_separators("C:\\Users/user"), "C:/Users/user");
    assert_eq!(normalize_separators("/a/b"), "/a/b");
    assert_eq!(normalize_separators(""), "");
}

/// Upstream "Web UI path normalization" cluster: normalize, split on `/`,
/// filter non-empty, last part == "my-project".
#[test]
fn test_web_normalization_last_segment_matches_upstream_cases() {
    for project_path in [
        "C:\\Users\\user\\projects\\my-project",
        "/home/user/projects/my-project",
        "C:\\Users/user/projects\\my-project",
    ] {
        let normalized = normalize_separators(project_path);
        // Last non-empty segment: forward `filter(..).last()` as in upstream,
        // written rev/find to keep clippy quiet — same result.
        let last = normalized.split('/').rev().find(|p| !p.is_empty());
        assert_eq!(last, Some("my-project"), "failed for {project_path:?}");
    }
}

/// Upstream "path.join for cache directory" cluster → covered-by std
/// `PathBuf::join` tests (no doubled separators, `.cache` present).
#[test]
fn test_path_join_child_segment_has_no_doubled_separators() {
    let windows = PathBuf::from("C:\\Users\\user\\.opencode-mem").join(".cache");
    assert!(windows.to_string_lossy().contains(".cache"));
    assert!(!windows.to_string_lossy().contains("//"));

    let unix = PathBuf::from("/home/user/.opencode-mem").join(".cache");
    assert!(unix.to_string_lossy().contains(".cache"));
    assert!(!unix.to_string_lossy().contains("//"));
}

/// Upstream "dirname for database path" cluster → covered-by std
/// `Path::parent` (plus `normalize_separators` for the mixed case).
#[test]
fn test_parent_dir_extracts_shards_directory() {
    let dir = Path::new("/home/user/.opencode-mem/shards/project.db")
        .parent()
        .and_then(Path::to_str);
    assert_eq!(dir, Some("/home/user/.opencode-mem/shards"));

    let mixed = normalize_separators("C:\\Users\\user/.opencode-mem\\shards/project.db");
    let dir = Path::new(&mixed)
        .parent()
        .and_then(Path::to_str)
        .unwrap_or_default();
    assert!(
        dir.contains("shards"),
        "parent of mixed-separator path was {dir:?}"
    );
}
