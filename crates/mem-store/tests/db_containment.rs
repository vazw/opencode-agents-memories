//! Containment-helper battery (W1.2 rework, justified addition): pins
//! `db::assert_path_inside_storage` against the mid-path `..` traversal hole
//! the reviewer found — Node `path.relative` resolves `..` BEFORE comparing,
//! so a crafted `sub/../../outside.db` must be rejected just like a leading
//! `../escape.db`, while filenames merely containing `..` stay allowed.

use std::path::Path;

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
