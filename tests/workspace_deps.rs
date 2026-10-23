//! Enforcement of the frozen crate dependency graph (ADR-0001).
//!
//! Each workspace crate may only depend on the `mem-` crates listed for it in
//! `ALLOWED` (a strict ceiling). External dependencies are outside this rule;
//! dev-dependency edges are included (`-e normal,dev`) so dev-dependency
//! cycles stay banned too.

use std::collections::BTreeSet;
use std::process::Command;

/// Executable copy of ADR-0001's allowed-edge table.
const ALLOWED: &[(&str, &[&str])] = &[
    ("mem-core", &[]),
    ("mem-store", &["mem-core"]),
    ("mem-embed", &["mem-core"]),
    ("mem-ai", &["mem-core", "mem-embed"]),
    ("mem-profile", &["mem-core", "mem-store", "mem-ai"]),
    (
        "mem-episodic",
        &["mem-core", "mem-store", "mem-embed", "mem-ai"],
    ),
    ("mem-facts", &["mem-core", "mem-store"]),
    ("mem-vault", &["mem-core", "mem-store"]),
    ("mem-skills", &["mem-core", "mem-store"]),
    (
        "mem-portability",
        &[
            "mem-core",
            "mem-store",
            "mem-embed",
            "mem-profile",
            "mem-episodic",
        ],
    ),
    (
        "mem-web",
        &[
            "mem-core",
            "mem-store",
            "mem-embed",
            "mem-ai",
            "mem-profile",
            "mem-episodic",
            "mem-skills",
            "mem-portability",
        ],
    ),
    (
        "mem-bridge",
        &[
            "mem-core",
            "mem-store",
            "mem-embed",
            "mem-ai",
            "mem-profile",
            "mem-episodic",
            "mem-facts",
            "mem-vault",
        ],
    ),
];

/// Direct mem-* dependencies of `pkg`, via `cargo tree` (normal + dev edges).
fn direct_mem_deps(pkg: &str) -> BTreeSet<String> {
    let output = Command::new(env!("CARGO"))
        .args([
            "tree",
            "-p",
            pkg,
            "--prefix",
            "none",
            "--depth",
            "1",
            "-e",
            "normal,dev",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to spawn cargo tree");
    if !output.status.success() {
        panic!(
            "cargo tree -p {pkg} failed (exit {:?}):\n{}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| *name != pkg)
        .filter(|name| name.starts_with("mem-"))
        .map(str::to_owned)
        .collect()
}

fn allowed_keys() -> BTreeSet<String> {
    ALLOWED.iter().map(|(name, _)| (*name).to_owned()).collect()
}

#[test]
fn test_workspace_crate_dependencies_stay_within_allowed_edges() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");

    // (a) crates/ contains exactly the 12 allowed packages: no missing
    // crate, and no unlisted thirteenth crate.
    let mut found: BTreeSet<String> = std::fs::read_dir(format!("{manifest_dir}/crates"))
        .unwrap_or_else(|e| panic!("cannot list crates/ under {manifest_dir}: {e}"))
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_dir())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    found.remove("CONTEXT.md");
    let expected = allowed_keys();
    assert_eq!(
        found, expected,
        "crates/ directory set must equal the ALLOWED table exactly"
    );

    // (b) actual direct mem-* deps ⊆ allowed set for every crate.
    for (name, allowed) in ALLOWED {
        let allowed_set: BTreeSet<String> = allowed.iter().map(|s| (*s).to_owned()).collect();
        let actual = direct_mem_deps(name);
        let extra: Vec<&String> = actual.difference(&allowed_set).collect();
        assert!(
            extra.is_empty(),
            "{name} contains mem-* dependencies outside its allowed set {allowed:?}: {extra:?}"
        );
    }
}
