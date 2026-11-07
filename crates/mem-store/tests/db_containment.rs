//! Containment-helper battery (W1.2 rework, justified addition): pins
//! `db::assert_path_inside_storage` against the mid-path `..` traversal hole
//! the reviewer found — Node `path.relative` resolves `..` BEFORE comparing,
//! so a crafted `sub/../../outside.db` must be rejected just like a leading
//! `../escape.db`, while filenames merely containing `..` stay allowed.
//!
//! engine-silent-500 B1 re-derivation: containment compares NORMALIZED
//! forms. The mixed-form pair that fed it before the fix (relative root from
//! the raw argv, absolute `metadata.db` from the ShardManager's
//! canonicalize) is the same directory expressed twice and must be ACCEPTED
//! when both sides absolutize into that one directory — while a genuinely
//! different directory stays refused in every form combination.

use std::path::{Path, PathBuf};

use mem_store::db::assert_path_inside_storage;
use mem_store::error::StoreError;

#[test]
fn test_assert_path_inside_storage_resolves_dotdot_before_comparing() {
    let root = Path::new("/tmp/mem-storage-containment-root");

    // Mid-path `..` chains resolve outside the root -> rejected.
    for relative in [
        "sub/../../outside.db",
        "a/../../../etc",
        "a/b/../../../../escape.db",
    ] {
        let error = assert_path_inside_storage(root, &root.join(relative))
            .expect_err("mid-path traversal must be rejected");
        assert!(
            matches!(error, StoreError::PathOutsideStorage { .. }),
            "typed PathOutsideStorage for {relative}: {error}"
        );
    }
    // Leading escape and cross-anchor (absolute vs relative) stay rejected.
    assert!(assert_path_inside_storage(root, &root.join("../escape.db")).is_err());
    assert!(assert_path_inside_storage(root, Path::new("relative.db")).is_err());
    assert!(assert_path_inside_storage(Path::new("relative-root"), root).is_err());

    // Allowed: a filename merely containing "..", a `..` that stays inside,
    // and plain nested paths.
    assert!(assert_path_inside_storage(root, &root.join("..x.db")).is_ok());
    assert!(assert_path_inside_storage(root, &root.join("sub/../ok.db")).is_ok());
    assert!(assert_path_inside_storage(root, &root.join("nested/deep.db")).is_ok());
}

/// B1 (engine-silent-500): containment compares NORMALIZED forms — both
/// sides are lexically resolved and absolutized first, so the same directory
/// expressed in different path forms is one directory. This is the exact
/// pair today's bug produced: the root stayed the raw relative argv value
/// while the ShardManager's `metadata.db` path came out absolute (its own
/// `canonicalize`), and the absolute-vs-relative form check refused the
/// engine's own registry.
#[test]
fn test_normalized_forms_accept_mixed_relative_root_with_absolute_target_same_dir() {
    let base = std::env::temp_dir().join(format!(
        "mem-store-containment-b1-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let absolute_root = base.join("root");
    std::fs::create_dir_all(&absolute_root).expect("create the containment root");

    // The relative form of the SAME directory, pathdiff-style against the
    // test process's cwd (what a manually-launched engine would be fed).
    let relative_root = relative_to(&std::env::current_dir().expect("cwd"), &absolute_root);

    // The defect pair: relative root vs ABSOLUTE target in the same
    // directory — refused before B1 (today's 500), accepted after.
    let metadata = absolute_root.join("metadata.db");
    assert!(
        assert_path_inside_storage(&relative_root, &metadata).is_ok(),
        "absolute target inside the same directory as the relative root must be accepted post-B1"
    );

    // The acceptance must never relax into "mixed forms are always fine":
    // a DIFFERENT directory stays refused in every form combination, a bare
    // relative target vs an absolute root stays refused (upstream
    // `path.resolve` anchors it at the cwd — outside the root), and leading
    // traversal is refused against both root forms.
    let escape = base.join("other").join("outside.db");
    assert!(
        assert_path_inside_storage(&relative_root, &escape).is_err(),
        "absolute target in a different directory vs relative root stays refused"
    );
    assert!(
        assert_path_inside_storage(&absolute_root, Path::new("metadata.db")).is_err(),
        "a bare relative target vs an absolute root stays refused"
    );
    assert!(
        assert_path_inside_storage(&absolute_root, &PathBuf::from("../escape.db")).is_err(),
        "leading traversal stays refused against the absolute root"
    );
    assert!(
        assert_path_inside_storage(&relative_root, &PathBuf::from("../escape.db")).is_err(),
        "leading traversal stays refused against the relative root"
    );

    let _ = std::fs::remove_dir_all(&base);
}

/// A relative path from `from` to `to` when both share a prefix (the test
/// cwd and the temp dir always do): drop the common components, prepend one
/// `..` per remaining `from` component. String-level only — this mirrors how
/// a launcher derives the relative argv form, and B1's absolutization is
/// lexical, so the pair above resolves back to `to` exactly.
fn relative_to(from: &Path, to: &Path) -> PathBuf {
    use std::path::Component;
    let from: Vec<Component> = from.components().collect();
    let to: Vec<Component> = to.components().collect();
    let shared = from
        .iter()
        .zip(to.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let mut output = PathBuf::new();
    for _ in shared..from.len() {
        output.push("..");
    }
    for component in &to[shared..] {
        output.push(component.as_os_str());
    }
    output
}
