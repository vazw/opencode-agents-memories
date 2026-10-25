//! 48.10 — the profile-erasure canary. OUR MEASUREMENT, NOT A VENDOR GUARANTEE (47.4b).
//! turso_core 0.8.3 has no `secure_delete`: a DELETE removes logical rows and leaves
//! their bytes in the file and the -wal. So this test ASSERTS logical erasure and
//! MEASURES byte residue, recording it as ADR-0019's residual. VACUUM is NOT used:
//! turso promises only that unused pages are removed, and VACUUM writes a full copy to
//! {temp_dir}/etilqs_{db}, which is outside what this canary walks (47.4, named residual).
//! Limits: it walks user-profiles.db and user-profiles.db-wal ONLY. -tshm, cold-buffer.json,
//! .bak families, the vault and logs/exports/media are 47.4's other residual classes.
//!
//! Why the checkpoint and why the control stays on `user-profiles.db`:
//! every open in this tree is multiprocess-WAL, so on a fresh root both the
//! migration DDL and `create_profile`'s INSERT live in the `-wal` and the main
//! file can be a single 4096-byte header page. Without the checkpoint the
//! positive control would read a main file that provably cannot hold the
//! marker and fail for a reason unrelated to erasure. Widening the control to
//! `db + wal` is forbidden: after the erase the deleted payload is still in
//! the `-wal`, so `db_hits` would be 0 both before and after — a measurement
//! that cannot discriminate. The checkpoint is only trustworthy because the
//! `-wal`-is-0 byte assertion follows it: a refused checkpoint returns `Ok`
//! with `busy = 1` (`turso_core-0.8.3/vdbe/execute.rs:799-806`) and truncates
//! nothing, so the PRAGMA's return value proves nothing by itself.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mem_profile::erasure::{ProfileEraser, PromptDeletion};
use mem_profile::profile_store::{ProfileConfig, ProfileData, ProfileItem, ProfileStore};
use mem_profile::prompt::UserPromptManager;

/// The marker copied into `user_profile_changelogs.profile_data_snapshot` by
/// `create_profile`.
const CANARY: &str = "CANARY-48-PROFILE-6f3a1c9d";
/// The canary's user identity.
const USER_ID: &str = "canary@example.com";

/// A local two-liner: `crates/mem-profile/tests/common/mod.rs` has no such
/// helper (the one in `turso_capabilities.rs` is private to `mem-store`).
fn file_len(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|m| m.len())
}

/// Non-overlapping occurrence count of [`CANARY`] in `bytes`.
fn count_hits(bytes: &[u8]) -> usize {
    bytes
        .windows(CANARY.len())
        .filter(|window| *window == CANARY.as_bytes())
        .count()
}

fn profile_data() -> ProfileData {
    ProfileData {
        preferences: vec![ProfileItem {
            description: format!("the canary preference {CANARY}"),
            ..ProfileItem::default()
        }],
        ..ProfileData::default()
    }
}

#[tokio::test]
async fn test_profile_erasure_canary_asserts_logical_erasure_and_measures_byte_residue() {
    let root = common::temp_dir("erasure-canary");
    let store = Arc::new(ProfileStore::new(
        root.clone(),
        common::store_config(root.clone()),
        ProfileConfig::default(),
    ));
    let manager = UserPromptManager::new(root.clone(), common::store_config(root.clone()), 3);

    // 1. Seed a contributing prompt, then the profile (creation copies the
    //    marker into the `create` changelog snapshot). Prompt first so the
    //    watermark (= now) is >= the prompt's created_at.
    let prompt_id = manager
        .save_prompt(
            "canary-session",
            "canary-message",
            "/canary",
            "canary prompt",
        )
        .await
        .expect("seed the prompt");
    let profile_id = store
        .create_profile(USER_ID, "Canary", "canary", USER_ID, &profile_data(), 1)
        .await
        .expect("seed the profile");

    // 2. Truncate the WAL through the store's own public handle (the same
    //    handle that committed the writes), then PROVE the truncation with
    //    the byte observable — the PRAGMA's `Ok` is not proof.
    let db = store.ready().await.expect("profile store ready");
    db.query("PRAGMA wal_checkpoint(TRUNCATE)", Vec::new())
        .await
        .expect("PRAGMA wal_checkpoint(TRUNCATE)");
    let mut os = store.db_path().as_os_str().to_os_string();
    os.push("-wal");
    let wal = PathBuf::from(os);
    assert_eq!(
        file_len(&wal),
        Some(0),
        "the checkpoint must have truncated the -wal before the positive control"
    );

    // 3. Positive control, on `user-profiles.db` only: the marker must be
    //    reachable in the main file BEFORE the erase, otherwise the
    //    after-measurement proves nothing.
    let before = std::fs::read(store.db_path()).expect("read the main profile db");
    let before_hits = count_hits(&before);
    assert!(
        before_hits >= 1,
        "the canary marker must be present in the main db before the erase (got {before_hits})"
    );

    // 4. Run the real mechanism: the same `Arc<ProfileStore>` is the eraser.
    let eraser: Arc<dyn ProfileEraser> = store.clone();
    let outcome = manager
        .with_eraser(eraser)
        .delete_prompt(&prompt_id)
        .await
        .expect("delete through the seam");
    assert!(
        matches!(
            outcome,
            PromptDeletion::Deleted {
                erasure: Some(report)
            } if report.profiles_deleted == 1 && report.changelogs_deleted == 1
        ),
        "the erase is real: {outcome:?}"
    );

    // 5. Assert LOGICAL erasure — and that a fresh peer sees it too.
    assert!(
        store
            .get_active_profile(USER_ID)
            .await
            .expect("read the profile")
            .is_none(),
        "the profile row is logically gone"
    );
    assert!(
        store
            .get_profile_changelogs(&profile_id, 10)
            .await
            .expect("read the changelogs")
            .is_empty(),
        "the changelog snapshots are logically gone"
    );
    let peer = ProfileStore::new(
        root.clone(),
        common::store_config(root.clone()),
        ProfileConfig::default(),
    );
    assert!(
        peer.get_active_profile(USER_ID)
            .await
            .expect("peer reads the profile")
            .is_none(),
        "a fresh ProfileStore over the same root also sees no profile"
    );
    assert!(
        peer.get_profile_changelogs(&profile_id, 10)
            .await
            .expect("peer reads the changelogs")
            .is_empty(),
        "a fresh ProfileStore over the same root also sees no changelogs"
    );

    // 6. MEASURE the residue (no assertion on the count): the marker's bytes
    //    are expected to survive a DELETE without `secure_delete`. Assert only
    //    that the measurement actually read real bytes.
    let after = std::fs::read(store.db_path()).expect("read the main profile db after the erase");
    let db_len = after.len();
    let db_hits = count_hits(&after);
    let (wal_hits, wal_len, wal_state) = match std::fs::read(&wal) {
        Ok(bytes) => (count_hits(&bytes), bytes.len(), "present"),
        Err(_) => (0, 0, "absent"),
    };
    eprintln!(
        "RESIDUAL-48: db={db_hits} wal={wal_hits} db_bytes={db_len} wal_bytes={wal_len} wal_state={wal_state}"
    );
    assert!(db_len > 0, "the main db was actually read");
    assert!(
        db_len + wal_len > 0,
        "the residue measurement read real bytes"
    );
}
