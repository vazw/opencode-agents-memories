//! The one web-side identity resolver: upstream `getTags(directory).user
//! .userEmail || "unknown"`, through the inject path's own resolver.
//!
//! The seam is the resolver itself, so the contract is total and hermetic: no
//! `git` subprocess, no process env, no filesystem. The production wrapper
//! (`resolve_user_id`) composes `mem_bridge::composition::EnvResolver` — the
//! SAME resolver `composition.rs` hands to capture and inject — and is the
//! only part the composition root calls.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use mem_bridge::project_scope::{ProjectScopeError, ResolvedProjectScope};
use mem_bridge::scope_resolver::ProjectScopeResolver;
use mem_core::config::Config;
use mem_core::config::ConfigPaths;
use opencode_agents_memory::web_identity::{
    UNKNOWN_USER_ID, resolve_user_id, resolve_user_id_with,
};

/// The directory the resolving fixtures answer about.
const PROJECT_DIR: &str = "/tmp/proj";

/// A hermetic resolved config: a throwaway home with no config file at all, so
/// every value is the documented default.
fn test_config() -> (PathBuf, Config) {
    let home = unique_temp_dir("web-identity-home");
    let env: BTreeMap<String, String> = BTreeMap::new();
    let config =
        Config::load(&ConfigPaths::standard(&home), &env).expect("the default config resolves");
    (home, config)
}

/// A unique temp directory under `std::env::temp_dir()` (house law: no
/// dev-dependencies, no `tempfile`).
fn unique_temp_dir(prefix: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos())
        .unwrap_or(0);
    let dir =
        std::env::temp_dir().join(format!("{prefix}-{}-{counter}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create the throwaway home");
    dir
}

/// What the fake resolver answers.
#[derive(Debug, Clone)]
enum Answer {
    /// Resolve to this email (the scope's `user_email`).
    Email(Option<String>),
    /// Fail like the real resolver does for a blank scope.
    Fail,
}

/// A resolver that answers about exactly one directory and records every
/// directory it was asked about.
struct RecordingResolver {
    directory: String,
    answer: Answer,
    asked: Mutex<Vec<String>>,
}

impl RecordingResolver {
    fn new(answer: Answer) -> Self {
        Self {
            directory: PROJECT_DIR.to_string(),
            answer,
            asked: Mutex::new(Vec::new()),
        }
    }

    fn asked(&self) -> Vec<String> {
        self.asked.lock().expect("asked lock").clone()
    }
}

impl ProjectScopeResolver for RecordingResolver {
    fn resolve(&self, project_id: &str) -> Result<ResolvedProjectScope, ProjectScopeError> {
        self.asked
            .lock()
            .expect("asked lock")
            .push(project_id.to_string());
        if project_id != self.directory {
            return Err(ProjectScopeError::BlankProjectId);
        }
        match &self.answer {
            Answer::Fail => Err(ProjectScopeError::BlankProjectId),
            Answer::Email(email) => Ok(ResolvedProjectScope {
                project_id: project_id.to_string(),
                container_tag: "mem_project_0123456789abcdef".to_string(),
                scope_hash: "0123456789abcdef".to_string(),
                display_name: "proj".to_string(),
                user_name: None,
                user_email: email.clone(),
                project_path: Some(PathBuf::from(project_id)),
                project_name: None,
                git_repo_url: None,
            }),
        }
    }
}

/// The config override is authoritative: a configured email wins even when the
/// resolver would answer something else (or nothing). Mutant: consulting the
/// resolver first and treating the override as a fallback.
#[test]
fn test_user_identity_prefers_the_config_override() {
    let (home, mut config) = test_config();
    config.user_email_override = Some("vaz@siamstr.com".to_string());
    let resolver = RecordingResolver::new(Answer::Email(Some("git@elsewhere".to_string())));

    assert_eq!(
        resolve_user_id_with(&config, &resolver, Path::new(PROJECT_DIR)),
        "vaz@siamstr.com"
    );
    assert!(
        resolver.asked().is_empty(),
        "the override short-circuits the resolver, got {:?}",
        resolver.asked()
    );
    let _ = std::fs::remove_dir_all(home);
}

/// The directory handed to the resolver is the caller's, not `.`, the storage
/// root or the process cwd. Mutant: passing `Path::new(".")` / the storage
/// root; the fixture only answers about `/tmp/proj`.
#[test]
fn test_user_identity_asks_the_resolver_about_the_given_directory() {
    let (home, config) = test_config();
    let resolver = RecordingResolver::new(Answer::Email(Some("git@x".to_string())));

    let identity = resolve_user_id_with(&config, &resolver, Path::new(PROJECT_DIR));

    assert_eq!(identity, "git@x");
    assert_eq!(resolver.asked(), vec![PROJECT_DIR.to_string()]);
    let _ = std::fs::remove_dir_all(home);
}

/// A whitespace-only email is not an identity. Mutant: `.map(str::to_string)`
/// without trim, which yields `"   "`.
#[test]
fn test_user_identity_trims_and_rejects_a_blank_email() {
    let (home, config) = test_config();
    let blank = RecordingResolver::new(Answer::Email(Some("   ".to_string())));
    assert_eq!(
        resolve_user_id_with(&config, &blank, Path::new(PROJECT_DIR)),
        UNKNOWN_USER_ID
    );

    let trimmed = RecordingResolver::new(Answer::Email(Some("  git@x  ".to_string())));
    assert_eq!(
        resolve_user_id_with(&config, &trimmed, Path::new(PROJECT_DIR)),
        "git@x"
    );

    // A blank override is not an identity either: it falls through to the
    // resolver rather than shadowing it.
    let (home_blank, mut config_blank) = test_config();
    config_blank.user_email_override = Some("   ".to_string());
    let resolved = RecordingResolver::new(Answer::Email(Some("git@x".to_string())));
    assert_eq!(
        resolve_user_id_with(&config_blank, &resolved, Path::new(PROJECT_DIR)),
        "git@x"
    );
    let _ = std::fs::remove_dir_all(home);
    let _ = std::fs::remove_dir_all(home_blank);
}

/// A resolver error is a missing identity, never a launch failure. Mutant:
/// `expect(...)`/`unwrap()` (panic) or `""` (an empty `userId`).
#[test]
fn test_user_identity_is_unknown_when_the_resolver_fails() {
    let (home, config) = test_config();
    let failing = RecordingResolver::new(Answer::Fail);

    assert_eq!(
        resolve_user_id_with(&config, &failing, Path::new(PROJECT_DIR)),
        UNKNOWN_USER_ID
    );
    assert_eq!(failing.asked(), vec![PROJECT_DIR.to_string()]);
    let _ = std::fs::remove_dir_all(home);
}

/// Totality: no directory value can panic the resolution. The resolver only
/// answers about `/tmp/proj`, so a blank directory, a non-UTF-8 path and a
/// nonexistent one all fall to the unknown identity.
#[test]
fn test_user_identity_never_panics_on_odd_directories() {
    let (home, config) = test_config();
    let resolver = RecordingResolver::new(Answer::Email(Some("git@x".to_string())));

    assert_eq!(
        resolve_user_id_with(&config, &resolver, Path::new("")),
        UNKNOWN_USER_ID
    );
    assert_eq!(
        resolve_user_id_with(&config, &resolver, Path::new("/does/not/exist")),
        UNKNOWN_USER_ID
    );
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let non_utf8 = Path::new(std::ffi::OsStr::from_bytes(b"/tmp/\xff"));
        assert_eq!(
            resolve_user_id_with(&config, &resolver, non_utf8),
            UNKNOWN_USER_ID
        );
    }
    let _ = std::fs::remove_dir_all(home);
}

/// The production wrapper exists and is total: over a real config and a real
/// directory it answers either the configured override or `"unknown"` — never
/// a panic. (The resolved identity of a live host is O1's business, not this
/// test's.)
#[test]
fn test_production_wrapper_is_total_over_a_real_directory() {
    let (home, mut config) = test_config();
    config.user_email_override = Some("vaz@siamstr.com".to_string());
    assert_eq!(
        resolve_user_id(&config, Path::new(PROJECT_DIR)),
        "vaz@siamstr.com"
    );

    let (fallback_home, fallback) = test_config();
    // No override: the real resolver probes `git` for this directory. The
    // assertion is the totality, not a specific host value.
    let identity = resolve_user_id(&fallback, Path::new(PROJECT_DIR));
    assert!(
        identity == UNKNOWN_USER_ID || !identity.trim().is_empty(),
        "a resolved identity is never blank, got {identity:?}"
    );
    let _ = std::fs::remove_dir_all(home);
    let _ = std::fs::remove_dir_all(fallback_home);
}
