//! Port of upstream `tests/secret-resolver-permissions.test.ts` (AC4) with the
//! r1 mode-table re-partition (D2).
//!
//! Upstream's `checkFilePermissions` *warns*; this port **rejects** (issue 11:
//! "Secret resolver rejects files with permissive permissions (strict
//! checks)"). The bit-mask property (`group_or_other_bits`), the zero-padded
//! octal mode in the message and the platform skip all carry over — `Ok`
//! becomes `Err`.
//!
//! Anti-root: against a *real* fixture an owner-unreadable mode (`0o000`,
//! `0o200`, `0o060`, `0o006`, `0o007`, `0o077`) can never reach the permission
//! check, because the resolver's own uid is denied at `open(O_RDONLY)` first.
//! Those modes are therefore proved twice: by the pure predicate, and by a real
//! read-denied `Err`.

#[path = "common/mod.rs"]
mod common;

use std::collections::BTreeMap;
use std::sync::Arc;

use mem_ai::privacy::secret::{FileSecretResolver, PermissionPolicy, group_or_other_bits};
use mem_ai::provider::ProviderErrorKind;
use mem_core::env::EnvSource;

const SECRET: &str = "sk-super-secret-value";

/// Upstream `SAFE_MODES`: owner-only, no group/other bit.
const SAFE_MODES: [u32; 5] = [0o600, 0o400, 0o200, 0o700, 0o000];

/// Upstream `EXPOSED_MODES`: every one grants read, write or execute to group
/// or other.
const EXPOSED_MODES: [u32; 10] = [
    0o644, 0o640, 0o604, 0o606, 0o404, 0o060, 0o006, 0o007, 0o077, 0o777,
];

/// Owner-readable group/other-exposed modes: these reach the permission check
/// and produce the permission sentence.
const OWNER_READABLE_EXPOSED: [u32; 6] = [0o644, 0o640, 0o604, 0o606, 0o404, 0o777];

/// Owner-*unreadable* exposed modes: read-denied before the check can run.
const OWNER_UNREADABLE_EXPOSED: [u32; 4] = [0o060, 0o006, 0o007, 0o077];

fn arc_env(pairs: &[(&str, &str)]) -> Arc<dyn EnvSource + Send + Sync> {
    let mut map = BTreeMap::new();
    for (key, value) in pairs {
        map.insert((*key).to_owned(), (*value).to_owned());
    }
    Arc::new(map)
}

fn resolver(policy: PermissionPolicy) -> FileSecretResolver {
    FileSecretResolver::new()
        .with_env(arc_env(&[]))
        .with_permission_policy(policy)
}

fn file_ref(path: &std::path::Path) -> String {
    format!("file://{}", path.display())
}

fn secret_file(dir: &std::path::Path, name: &str, mode: u32) -> std::path::PathBuf {
    let path = common::write_file(dir, name, &format!("{SECRET}\n"));
    common::chmod(&path, mode);
    path
}

// ---------------------------------------------------------------------------
// Pure predicate — runs on every platform, root or not.
// ---------------------------------------------------------------------------

#[test]
fn test_permission_bits_exposed_ignores_file_type_bits() {
    // The file-type bits above 0o777 must be masked off; this is where
    // 0o000/0o200 (and every owner-only mode) are proved.
    for mode in SAFE_MODES {
        assert_eq!(
            group_or_other_bits(0o100000 | mode),
            0,
            "mode {mode:03o} must report no exposure"
        );
    }
    for mode in EXPOSED_MODES {
        assert_ne!(
            group_or_other_bits(0o100000 | mode),
            0,
            "mode {mode:03o} must report exposure"
        );
    }
}

#[test]
fn test_owner_only_mode_bits_report_no_exposure() {
    for mode in [0o600, 0o400, 0o200, 0o700, 0o000] {
        assert_eq!(group_or_other_bits(mode), 0);
    }
}

// ---------------------------------------------------------------------------
// Real-file acceptance: readable owner-only modes only.
// ---------------------------------------------------------------------------

#[test]
fn test_owner_only_readable_modes_are_accepted() {
    let dir = common::unique_temp_dir("secret-owner-only");
    for mode in [0o600, 0o400, 0o700] {
        let path = secret_file(&dir, &format!("key-{mode:03o}"), mode);
        let resolved = resolver(PermissionPolicy::Enforce)
            .resolve_reference(&file_ref(&path))
            .unwrap_or_else(|error| panic!("mode {mode:03o} should resolve: {error}"));
        assert_eq!(
            resolved
                .expect("owner-only file should yield a secret")
                .expose_secret(),
            SECRET
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Owner-unreadable modes: proved by a real read-denied `Err` (guarded against
// root/CAP_DAC_OVERRIDE, where no fixture mode can express denial).
// ---------------------------------------------------------------------------

#[test]
fn test_owner_only_nonreadable_modes_are_read_denied() {
    if common::process_bypasses_file_modes() {
        println!("SKIP: process bypasses file modes");
        return;
    }

    let dir = common::unique_temp_dir("secret-owner-nonreadable");
    for mode in [0o000, 0o200] {
        let path = secret_file(&dir, &format!("key-{mode:03o}"), mode);
        let error = resolver(PermissionPolicy::Enforce)
            .resolve_reference(&file_ref(&path))
            .expect_err("an unreadable file must not resolve");

        assert_eq!(error.kind, ProviderErrorKind::Config);
        assert!(
            error.message.contains(&path.display().to_string()),
            "message must name the path: {}",
            error.message
        );
        assert_ne!(
            error.message,
            format!("Secret file not found: {}", path.display())
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_owner_unwritable_but_exposed_modes_are_read_denied() {
    if common::process_bypasses_file_modes() {
        println!("SKIP: process bypasses file modes");
        return;
    }

    let dir = common::unique_temp_dir("secret-owner-unreadable-exposed");
    for mode in OWNER_UNREADABLE_EXPOSED {
        let path = secret_file(&dir, &format!("key-{mode:03o}"), mode);
        let error = resolver(PermissionPolicy::Enforce)
            .resolve_reference(&file_ref(&path))
            .expect_err("an unreadable file must not resolve");

        assert_eq!(error.kind, ProviderErrorKind::Config);
        assert!(error.message.contains(&path.display().to_string()));
        assert_ne!(
            error.message,
            format!("Secret file not found: {}", path.display())
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Real-file rejection with the permission sentence.
// ---------------------------------------------------------------------------

#[test]
fn test_group_or_other_modes_are_rejected() {
    let dir = common::unique_temp_dir("secret-exposed");
    for mode in OWNER_READABLE_EXPOSED {
        let path = secret_file(&dir, &format!("key-{mode:03o}"), mode);
        let error = resolver(PermissionPolicy::Enforce)
            .resolve_reference(&file_ref(&path))
            .expect_err("a group/other-readable file must be rejected");

        assert_eq!(error.kind, ProviderErrorKind::Config);
        assert!(
            error.message.contains("permissive permissions"),
            "message must carry upstream's sentence: {}",
            error.message
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_permission_error_names_mode_in_octal() {
    // `0o404` is world-readable yet sorts numerically *below* `0o600`; the
    // zero-padded octal in the message is the only signal the user gets.
    let dir = common::unique_temp_dir("secret-octal");
    let world_readable = secret_file(&dir, "world", 0o604);
    let below_threshold = secret_file(&dir, "below", 0o404);

    let exposed = resolver(PermissionPolicy::Enforce)
        .resolve_reference(&file_ref(&world_readable))
        .unwrap_err();
    let less_exposed = resolver(PermissionPolicy::Enforce)
        .resolve_reference(&file_ref(&below_threshold))
        .unwrap_err();

    assert!(exposed.message.contains("(604)"), "{}", exposed.message);
    assert!(
        less_exposed.message.contains("(404)"),
        "{}",
        less_exposed.message
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_permission_error_never_contains_secret_value() {
    let dir = common::unique_temp_dir("secret-no-echo");
    let path = secret_file(&dir, "exposed", 0o644);

    let error = resolver(PermissionPolicy::Enforce)
        .resolve_reference(&file_ref(&path))
        .unwrap_err();

    assert!(!error.message.contains(SECRET));
    assert!(!error.to_string().contains(SECRET));

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Policy, taxonomy and bounds.
// ---------------------------------------------------------------------------

#[test]
fn test_permissive_file_is_accepted_when_policy_is_skip() {
    // Upstream's win32 early return, kept testable on Linux.
    let dir = common::unique_temp_dir("secret-skip-policy");
    let path = secret_file(&dir, "exposed", 0o777);

    let resolved = resolver(PermissionPolicy::Skip)
        .resolve_reference(&file_ref(&path))
        .expect("Skip must not reject a permissive file");
    assert_eq!(resolved.unwrap().expose_secret(), SECRET);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_missing_file_is_a_config_error() {
    let dir = common::unique_temp_dir("secret-missing");
    let path = dir.join("absent-key");

    let error = resolver(PermissionPolicy::Enforce)
        .resolve_reference(&file_ref(&path))
        .unwrap_err();

    assert_eq!(error.kind, ProviderErrorKind::Config);
    assert_eq!(
        error.message,
        format!("Secret file not found: {}", path.display())
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_unreadable_file_error_names_path_only() {
    if common::process_bypasses_file_modes() {
        println!("SKIP: process bypasses file modes");
        return;
    }

    let dir = common::unique_temp_dir("secret-unreadable");
    let path = secret_file(&dir, "denied", 0o000);

    let error = resolver(PermissionPolicy::Enforce)
        .resolve_reference(&file_ref(&path))
        .unwrap_err();

    // The io error rides `with_source`; the message names the path and nothing
    // else — no errno text, no permission sentence it could not have observed.
    assert_eq!(
        error.message,
        format!("Secret file could not be opened: {}", path.display())
    );
    assert!(!error.message.contains("permissive permissions"));
    assert!(std::error::Error::source(&error).is_some());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_directory_path_is_rejected() {
    let dir = common::unique_temp_dir("secret-directory");
    let nested = dir.join("a-directory");
    std::fs::create_dir_all(&nested).unwrap();

    let error = resolver(PermissionPolicy::Enforce)
        .resolve_reference(&file_ref(&nested))
        .unwrap_err();

    assert_eq!(
        error.message,
        format!("Secret file is not a regular file: {}", nested.display())
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_oversized_file_is_rejected() {
    let dir = common::unique_temp_dir("secret-oversized");
    let path = dir.join("huge");
    std::fs::write(
        &path,
        vec![b'a'; (FileSecretResolver::MAX_SECRET_BYTES + 1) as usize],
    )
    .unwrap();
    common::chmod(&path, 0o600);

    let error = resolver(PermissionPolicy::Enforce)
        .resolve_reference(&file_ref(&path))
        .unwrap_err();

    assert!(
        error
            .message
            .contains(&FileSecretResolver::MAX_SECRET_BYTES.to_string()),
        "message must name the limit: {}",
        error.message
    );
    assert!(error.message.contains(&path.display().to_string()));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_file_at_size_limit_is_accepted() {
    // Proves `take(MAX + 1)` is inclusive: exactly MAX bytes is not "too big".
    let dir = common::unique_temp_dir("secret-at-limit");
    let path = dir.join("exact");
    std::fs::write(
        &path,
        vec![b'a'; FileSecretResolver::MAX_SECRET_BYTES as usize],
    )
    .unwrap();
    common::chmod(&path, 0o600);

    let resolved = resolver(PermissionPolicy::Enforce)
        .resolve_reference(&file_ref(&path))
        .expect("a file at the limit must resolve");

    assert_eq!(
        resolved.unwrap().expose_secret().len() as u64,
        FileSecretResolver::MAX_SECRET_BYTES
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_file_content_is_trimmed_of_surrounding_whitespace() {
    let dir = common::unique_temp_dir("secret-trim");
    let path = common::write_file(&dir, "key", &format!("\n  {SECRET}\t\n"));
    common::chmod(&path, 0o600);

    let resolved = resolver(PermissionPolicy::Enforce)
        .resolve_reference(&file_ref(&path))
        .unwrap();
    assert_eq!(resolved.unwrap().expose_secret(), SECRET);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_non_utf8_file_is_decoded_lossily() {
    // Mirrors Node's replacement decoding rather than failing the read.
    let dir = common::unique_temp_dir("secret-lossy");
    let path = dir.join("key");
    let mut bytes = b"\xff\xfe".to_vec();
    bytes.extend_from_slice(SECRET.as_bytes());
    std::fs::write(&path, bytes).unwrap();
    common::chmod(&path, 0o600);

    let resolved = resolver(PermissionPolicy::Enforce)
        .resolve_reference(&file_ref(&path))
        .unwrap()
        .expect("lossy decoding must still yield a secret");
    assert!(resolved.expose_secret().contains(SECRET));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_symlink_target_mode_is_enforced() {
    // Upstream `statSync` follows symlinks, so the *target's* mode decides.
    let dir = common::unique_temp_dir("secret-symlink");
    let permissive = secret_file(&dir, "permissive", 0o644);
    let owner_only = secret_file(&dir, "owner-only", 0o600);
    let link_permissive = dir.join("link-permissive");
    let link_owner_only = dir.join("link-owner-only");
    std::os::unix::fs::symlink(&permissive, &link_permissive).unwrap();
    std::os::unix::fs::symlink(&owner_only, &link_owner_only).unwrap();

    let rejected = resolver(PermissionPolicy::Enforce)
        .resolve_reference(&file_ref(&link_permissive))
        .expect_err("the symlink's permissive target must be rejected");
    assert!(rejected.message.contains("permissive permissions"));

    let accepted = resolver(PermissionPolicy::Enforce)
        .resolve_reference(&file_ref(&link_owner_only))
        .expect("the symlink's owner-only target must resolve");
    assert_eq!(accepted.unwrap().expose_secret(), SECRET);

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Cross-crate consistency: the two resolvers must not drift silently.
// ---------------------------------------------------------------------------

#[test]
fn test_resolver_agrees_with_config_resolution_for_literal_and_env() {
    let env = arc_env(&[("OPENAI_API_KEY", SECRET)]);
    let resolver = FileSecretResolver::new().with_env(Arc::clone(&env));

    for reference in [SECRET, "env://OPENAI_API_KEY"] {
        let ours = resolver.resolve_reference(reference).unwrap().unwrap();
        let config = mem_core::config::resolve_secret_value(Some(reference), env.as_ref()).unwrap();
        assert_eq!(Some(ours.expose_secret().to_owned()), config);
    }
}

#[test]
fn test_resolver_agrees_with_config_resolution_for_owner_only_file() {
    let dir = common::unique_temp_dir("secret-agree-file");
    let path = secret_file(&dir, "key", 0o600);
    let env = arc_env(&[]);
    let reference = file_ref(&path);

    let ours = resolver(PermissionPolicy::Enforce)
        .resolve_reference(&reference)
        .unwrap()
        .unwrap();
    let config = mem_core::config::resolve_secret_value(Some(&reference), env.as_ref()).unwrap();

    assert_eq!(Some(ours.expose_secret().to_owned()), config);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_strict_resolver_rejects_what_config_resolution_accepts() {
    // Documents D2: mem-core's `resolve_secret_value` does no permission check,
    // the strict resolver rejects.
    let dir = common::unique_temp_dir("secret-strict-vs-config");
    let path = secret_file(&dir, "exposed", 0o644);
    let env = arc_env(&[]);
    let reference = file_ref(&path);

    let config = mem_core::config::resolve_secret_value(Some(&reference), env.as_ref())
        .expect("mem-core accepts the file");
    assert_eq!(config, Some(SECRET.to_owned()));

    let error = resolver(PermissionPolicy::Enforce)
        .resolve_reference(&reference)
        .expect_err("the strict resolver rejects the same file");
    assert!(error.message.contains("permissive permissions"));

    let _ = std::fs::remove_dir_all(&dir);
}
