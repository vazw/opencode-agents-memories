//! The ported `SkillsManager` oracle (upstream
//! `tests/skills-manager.test.ts` + `tests/v2-skills-bridge.test.ts`).
//!
//! Every test here is a port of an upstream assertion:
//!
//! | Rust test | upstream test |
//! |---|---|
//! | `skill_id_validation_table` | `isValidSkillId` (accepts/rejects) |
//! | `skills_manager_crud` | "creates, lists, updates, and deletes skills" |
//! | `skills_manager_catalog` | "builds an OpenCode HTTP catalog" |
//! | `notify_reload_hits` | "notifies listeners on change" |
//! | `content_file_iff_enabled` | "lists only enabled skills for registration filtering" |
//! | `catalog_version_files` | "builds an OpenCode HTTP catalog" |
//! | `catalog_markdown_none_when_disabled` | "builds an OpenCode HTTP catalog" |
//! | `format_markdown_frontmatter` | "builds OpenCode HTTP catalog markdown …" |
//! | `v2_bridge_skill_path_absolute` | "exposes absolute skill paths for OpenCode Skill.INFO" |
//! | `rejects_duplicate_and_invalid_ids` | "rejects invalid and duplicate ids" |
//! | `reopens_after_reset` | "reopens after closeTursoAndInvalidateCaches" |

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use mem_skills::{
    SKILL_ID_MAX_LENGTH, SkillInput, SkillPatch, SkillsError, SkillsManager, is_valid_skill_id,
    on_skills_changed,
};
use mem_store::config::StoreConfig;

/// The process-wide test lock.
///
/// Upstream's `bun test` runs the tests of one file **serially**; Rust's
/// harness runs them in parallel threads. The change-listener registry is
/// process-global (upstream's shape), so a parallel file would count the
/// other tests' mutations. The lock restores upstream's serial semantics.
fn test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A scratch directory under the target dir (never `/tmp`).
fn scratch_dir(label: &str) -> PathBuf {
    let base = std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("target"));
    let dir = base.join("scratch").join("mem-skills").join(label);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

fn store_config(storage_path: &Path) -> StoreConfig {
    StoreConfig {
        storage_path: storage_path.to_path_buf(),
        embedding_dimensions: 8,
        embedding_model: "test".to_string(),
        max_vectors_per_shard: 1000,
        container_tag_prefix: "test".to_string(),
        database_encryption_enabled: false,
        database_encryption_cipher: "aes256gcm".to_string(),
        database_encryption_key: None,
    }
}

fn input(id: &str, name: &str, content: &str) -> SkillInput {
    SkillInput {
        id: id.to_string(),
        name: name.to_string(),
        description: Some("demo".to_string()),
        content: content.to_string(),
        autoinvoke: None,
        enabled: None,
        container_tag: None,
    }
}

#[test]
fn skill_id_validation_table() {
    // Accepts.
    assert!(is_valid_skill_id("git-release"));
    assert!(is_valid_skill_id("a"));
    assert!(is_valid_skill_id("pr-review-v2"));
    // Rejects.
    assert!(!is_valid_skill_id("Git-Release"));
    assert!(!is_valid_skill_id("-bad"));
    assert!(!is_valid_skill_id("bad-"));
    assert!(!is_valid_skill_id("bad--id"));
    assert!(!is_valid_skill_id("has space"));
    assert!(!is_valid_skill_id(""));
    // Length bounds (upstream: `length >= 1 && length <= 64`).
    assert!(is_valid_skill_id(&"a".repeat(SKILL_ID_MAX_LENGTH)));
    assert!(!is_valid_skill_id(&"a".repeat(SKILL_ID_MAX_LENGTH + 1)));
    // Path traversal is structurally impossible: the whitelist refuses
    // every separator and every percent sign.
    assert!(!is_valid_skill_id("../escape"));
    assert!(!is_valid_skill_id("a/b"));
    assert!(!is_valid_skill_id("a\\b"));
    assert!(!is_valid_skill_id("%2F"));
}

#[tokio::test]
async fn skills_manager_crud() {
    let _serial = test_lock();
    let dir = scratch_dir("crud");
    let manager = SkillsManager::new(dir.clone(), store_config(&dir));

    let created = manager
        .create(input(
            "git-release",
            "Git Release",
            "## Steps\n\n1. Tag\n2. Publish",
        ))
        .await
        .expect("create");
    assert_eq!(created.id, "git-release");
    assert_eq!(created.name, "Git Release");
    assert!(created.autoinvoke);
    assert!(created.enabled);
    // Defaults: autoinvoke true, enabled true, description trimmed.
    assert_eq!(created.description, "demo");
    assert!(created.created_at > 0);
    assert_eq!(created.created_at, created.updated_at);

    let listed = manager.list(None, false).await.expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "git-release");

    let updated = manager
        .update(
            "git-release",
            SkillPatch {
                content: Some("## Updated".to_string()),
                enabled: Some(false),
                ..SkillPatch::default()
            },
        )
        .await
        .expect("update");
    assert_eq!(updated.content, "## Updated");
    assert!(!updated.enabled);

    let enabled_only = manager.list(None, true).await.expect("list enabled");
    assert!(enabled_only.is_empty());

    assert!(manager.delete("git-release").await.expect("delete"));
    assert!(manager.list(None, false).await.expect("list").is_empty());
    // A second delete is a no-op, not an error.
    assert!(!manager.delete("git-release").await.expect("delete again"));
}

#[tokio::test]
async fn rejects_duplicate_and_invalid_ids() {
    let _serial = test_lock();
    let dir = scratch_dir("duplicates");
    let manager = SkillsManager::new(dir.clone(), store_config(&dir));

    let error = manager
        .create(input("Bad_ID", "x", "body"))
        .await
        .expect_err("invalid id is refused");
    assert!(matches!(error, SkillsError::InvalidId));
    assert!(error.to_string().contains("Invalid skill id"));

    manager
        .create(input("ok-skill", "Ok", "body"))
        .await
        .expect("create");
    let error = manager
        .create(input("ok-skill", "Dup", "body"))
        .await
        .expect_err("duplicate is refused");
    assert!(matches!(error, SkillsError::AlreadyExists(_)));
    assert!(error.to_string().contains("already exists"));

    // The name and the content are required.
    let error = manager
        .create(input("no-name", "   ", "body"))
        .await
        .expect_err("name is required");
    assert!(matches!(error, SkillsError::NameRequired));
    let error = manager
        .create(input("no-content", "Name", "   "))
        .await
        .expect_err("content is required");
    assert!(matches!(error, SkillsError::ContentRequired));
}

#[tokio::test]
async fn skills_manager_catalog() {
    let _serial = test_lock();
    let dir = scratch_dir("catalog");
    let manager = SkillsManager::new(dir.clone(), store_config(&dir));

    manager
        .create(input("review", "Review", "Review carefully."))
        .await
        .expect("create");
    manager
        .create(SkillInput {
            enabled: Some(false),
            ..input("hidden", "Hidden", "off")
        })
        .await
        .expect("create hidden");

    let catalog = manager.catalog_index().await.expect("catalog");
    assert_eq!(catalog.len(), 1);
    assert_eq!(catalog[0].name, "review");
    assert_eq!(catalog[0].files, vec!["review.md".to_string()]);
    // The version is the `updated_at` stamp, rendered as a string.
    assert_eq!(
        catalog[0].version,
        manager
            .get("review")
            .await
            .expect("get")
            .unwrap()
            .updated_at
            .to_string()
    );

    let markdown = manager
        .catalog_markdown("review")
        .await
        .expect("catalog markdown");
    let markdown = markdown.expect("enabled skill has markdown");
    assert!(markdown.contains("name: \"Review\""));
    assert!(markdown.contains("Review carefully."));
    assert!(
        manager
            .catalog_markdown("hidden")
            .await
            .expect("catalog markdown")
            .is_none()
    );
}

#[tokio::test]
async fn catalog_version_files() {
    let _serial = test_lock();
    let dir = scratch_dir("catalog-version");
    let manager = SkillsManager::new(dir.clone(), store_config(&dir));

    let created = manager
        .create(input("git-release", "Git Release", "Tag and publish."))
        .await
        .expect("create");
    let catalog = manager.catalog_index().await.expect("catalog");
    assert_eq!(catalog.len(), 1);
    assert_eq!(catalog[0].name, "git-release");
    assert_eq!(catalog[0].version, created.updated_at.to_string());
    assert_eq!(catalog[0].files, vec!["git-release.md".to_string()]);
}

#[tokio::test]
async fn catalog_markdown_none_when_disabled() {
    let _serial = test_lock();
    let dir = scratch_dir("catalog-disabled");
    let manager = SkillsManager::new(dir.clone(), store_config(&dir));

    manager
        .create(SkillInput {
            enabled: Some(false),
            ..input("off-skill", "Off", "hidden")
        })
        .await
        .expect("create");
    assert!(
        manager
            .catalog_markdown("off-skill")
            .await
            .expect("catalog markdown")
            .is_none()
    );
    // An unknown skill is `None` too.
    assert!(
        manager
            .catalog_markdown("missing-skill")
            .await
            .expect("catalog markdown")
            .is_none()
    );
    // A disabled skill is not in the catalog index.
    assert!(manager.catalog_index().await.expect("catalog").is_empty());
}

#[tokio::test]
async fn format_markdown_frontmatter() {
    let _serial = test_lock();
    let dir = scratch_dir("frontmatter");
    let manager = SkillsManager::new(dir.clone(), store_config(&dir));

    let created = manager
        .create(SkillInput {
            description: Some("Cut a release".to_string()),
            autoinvoke: Some(false),
            ..input("git-release", "Git Release", "Tag and publish.")
        })
        .await
        .expect("create");

    let markdown = manager.format_markdown(&created);
    assert_eq!(
        markdown,
        "---\nname: \"Git Release\"\ndescription: \"Cut a release\"\nmetadata:\n  opencode/autoinvoke: \"false\"\n---\n\nTag and publish."
    );
    // The indented metadata key is exactly upstream's.
    assert!(markdown.contains("\nmetadata:\n  opencode/autoinvoke: \"false\""));
}

#[tokio::test]
async fn content_file_iff_enabled() {
    let _serial = test_lock();
    let dir = scratch_dir("content-iff");
    let manager = SkillsManager::new(dir.clone(), store_config(&dir));

    manager
        .create(SkillInput {
            enabled: Some(true),
            ..input("on", "On", "a")
        })
        .await
        .expect("create");
    manager
        .create(SkillInput {
            enabled: Some(false),
            ..input("off", "Off", "b")
        })
        .await
        .expect("create");

    assert!(manager.skill_path("on").exists());
    assert!(!manager.skill_path("off").exists());

    // Disabling removes the file; re-enabling writes it back.
    manager
        .update(
            "on",
            SkillPatch {
                enabled: Some(false),
                ..SkillPatch::default()
            },
        )
        .await
        .expect("disable");
    assert!(!manager.skill_path("on").exists());
    manager
        .update(
            "on",
            SkillPatch {
                enabled: Some(true),
                ..SkillPatch::default()
            },
        )
        .await
        .expect("enable");
    assert!(manager.skill_path("on").exists());

    // Deleting removes the file too.
    manager.delete("on").await.expect("delete");
    assert!(!manager.skill_path("on").exists());
}

#[tokio::test]
async fn v2_bridge_skill_path_absolute() {
    let _serial = test_lock();
    let dir = scratch_dir("v2-bridge");
    let manager = SkillsManager::new(dir.clone(), store_config(&dir));

    manager
        .create(input("bridge-demo", "Bridge Demo", "Do the thing."))
        .await
        .expect("create");

    let path = manager.skill_path("bridge-demo");
    assert!(
        path.is_absolute(),
        "the OpenCode Skill.INFO path is absolute"
    );
    let rendered = path.to_string_lossy().replace('\\', "/");
    assert!(rendered.ends_with("bridge-demo/SKILL.md"), "{rendered}");

    let markdown = std::fs::read_to_string(&path).expect("read SKILL.md");
    assert!(markdown.contains("name: \"Bridge Demo\""));
    assert!(markdown.contains("Do the thing."));
    assert!(markdown.contains("opencode/autoinvoke: \"true\""));
}

#[tokio::test]
async fn notify_reload_hits() {
    let _serial = test_lock();
    let dir = scratch_dir("notify");
    let manager = SkillsManager::new(dir.clone(), store_config(&dir));

    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let guard = on_skills_changed(Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    }));

    manager
        .create(input("notify-me", "N", "c"))
        .await
        .expect("create");
    manager
        .update(
            "notify-me",
            SkillPatch {
                name: Some("N2".to_string()),
                ..SkillPatch::default()
            },
        )
        .await
        .expect("update");
    manager.delete("notify-me").await.expect("delete");

    // Upstream: create + update + delete == 3 hits.
    assert_eq!(hits.load(Ordering::SeqCst), 3);
    drop(guard);
}

#[tokio::test]
async fn notify_stops_after_the_guard_drops() {
    let _serial = test_lock();
    let dir = scratch_dir("notify-guard");
    let manager = SkillsManager::new(dir.clone(), store_config(&dir));

    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let guard = on_skills_changed(Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    }));
    manager
        .create(input("one", "N", "c"))
        .await
        .expect("create");
    drop(guard);
    manager
        .create(input("two", "N", "c"))
        .await
        .expect("create");
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}

/// `stop()` DETACHES the listener: a mutation after the stop notifies
/// nothing (the guard's whole job is to remove the listener).
#[tokio::test]
async fn notify_stops_after_the_guard_stops() {
    let _serial = test_lock();
    let dir = scratch_dir("notify-guard-stop");
    let manager = SkillsManager::new(dir.clone(), store_config(&dir));

    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    let guard = on_skills_changed(Arc::new(move || {
        counter.fetch_add(1, Ordering::SeqCst);
    }));
    manager
        .create(input("one", "N", "c"))
        .await
        .expect("create");
    assert_eq!(hits.load(Ordering::SeqCst), 1);

    guard.stop();

    manager
        .create(input("two", "N", "c"))
        .await
        .expect("create");
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "stop() must detach the listener"
    );
}

/// An ILLEGAL skill id has no content path, and the fallback never
/// escapes the storage root (a traversal id cannot reach the path).
#[tokio::test]
async fn the_invalid_skill_path_stays_inside_the_storage_root() {
    let _serial = test_lock();
    let dir = scratch_dir("invalid-path");
    let manager = SkillsManager::new(dir.clone(), store_config(&dir));

    for id in ["../../etc/passwd", "..", "a/b", "%2F"] {
        let path = manager.skill_path(id);
        let rendered = path.to_string_lossy().replace('\\', "/");
        assert!(
            path.starts_with(manager.storage_path()),
            "the fallback escaped the storage root: {rendered}"
        );
        assert!(!rendered.contains(".."), "{rendered}");
        assert!(!path.exists(), "{rendered}");
    }
}

#[tokio::test]
async fn reopens_after_reset() {
    let _serial = test_lock();
    let dir = scratch_dir("reset");
    let mut manager = SkillsManager::new(dir.clone(), store_config(&dir));
    manager
        .create(input("keep-me", "Keep", "body"))
        .await
        .expect("create");
    assert_eq!(manager.list(None, true).await.expect("list").len(), 1);

    manager.reset();

    let listed = manager.list(None, true).await.expect("list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "keep-me");
}

#[tokio::test]
async fn list_filters_by_container_tag() {
    let _serial = test_lock();
    let dir = scratch_dir("tag-filter");
    let manager = SkillsManager::new(dir.clone(), store_config(&dir));

    manager
        .create(SkillInput {
            container_tag: Some("tag-a".to_string()),
            ..input("tagged", "Tagged", "a")
        })
        .await
        .expect("create");
    manager
        .create(input("global", "Global", "b"))
        .await
        .expect("create");

    // Upstream's filter keeps the global (NULL tag) skills visible.
    let filtered = manager.list(Some("tag-a"), false).await.expect("list");
    assert_eq!(filtered.len(), 2);
    // No filter means "all".
    assert_eq!(manager.list(None, false).await.expect("list").len(), 2);
}

#[tokio::test]
async fn concurrent_listeners_all_fire() {
    let _serial = test_lock();
    let dir = scratch_dir("listeners");
    let manager = SkillsManager::new(dir.clone(), store_config(&dir));

    let hits = Arc::new(Mutex::new(Vec::<usize>::new()));
    let mut guards = Vec::new();
    for index in 0..3usize {
        let hits = hits.clone();
        guards.push(on_skills_changed(Arc::new(move || {
            hits.lock().expect("hits lock").push(index);
        })));
    }
    manager
        .create(input("multi", "M", "c"))
        .await
        .expect("create");
    let recorded = hits.lock().expect("hits lock").clone();
    assert_eq!(recorded.len(), 3);
    assert_eq!(recorded, vec![0, 1, 2]);
    drop(guards);
}
