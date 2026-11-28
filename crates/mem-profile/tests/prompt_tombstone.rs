//! Issue 143 — `delete_prompt` records a tombstone in the same transaction as
//! the prompt's hard DELETE.
//!
//! Every case drives the public seam (`UserPromptManager::delete_prompt`) over
//! one temp root, with no eraser (the erase arms are pinned by
//! `profile_erasure_watermark.rs`).

mod common;

use std::path::PathBuf;

use mem_profile::erasure::PromptDeletion;
use mem_profile::ids::now_millis;
use mem_profile::prompt::UserPromptManager;
use mem_store::db::Value;

fn fixture() -> (PathBuf, UserPromptManager) {
    let root = common::temp_dir("prompt-tombstone");
    let manager = UserPromptManager::new(root.clone(), common::store_config(root.clone()), 3);
    (root, manager)
}

async fn seed_prompt(manager: &UserPromptManager) -> String {
    manager
        .save_prompt("session-143", "message-143", "/probe", "probe prompt", None)
        .await
        .expect("seed the prompt")
}

async fn tombstone_count(manager: &UserPromptManager) -> i64 {
    let db = manager.ready().await.expect("prompts ready");
    db.query("SELECT COUNT(*) AS n FROM prompt_tombstones", Vec::new())
        .await
        .expect("count tombstones")
        .first()
        .and_then(|row| row.i64("n"))
        .expect("a count row")
}

async fn tombstone_row(manager: &UserPromptManager, id: &str) -> Option<i64> {
    let db = manager.ready().await.expect("prompts ready");
    db.query(
        "SELECT deleted_at_unix_ms FROM prompt_tombstones WHERE id = ?",
        vec![Value::Text(id.to_string())],
    )
    .await
    .expect("read the tombstone")
    .first()
    .and_then(|row| row.i64("deleted_at_unix_ms"))
}

#[tokio::test]
async fn test_delete_writes_one_tombstone_with_the_deletion_time() {
    let (_root, manager) = fixture();
    let prompt_id = seed_prompt(&manager).await;

    let before = now_millis();
    let outcome = manager
        .delete_prompt(&prompt_id)
        .await
        .expect("delete the prompt");
    let after = now_millis();

    assert!(matches!(outcome, PromptDeletion::Deleted { .. }));
    assert_eq!(
        tombstone_count(&manager).await,
        1,
        "one tombstone per delete"
    );
    let at = tombstone_row(&manager, &prompt_id)
        .await
        .expect("the tombstone names the deleted id");
    assert!(
        before <= at && at <= after,
        "the tombstone time {at} lies within the delete ({before}..={after})"
    );
}

#[tokio::test]
async fn test_a_second_delete_is_absent_and_keeps_the_tombstone() {
    let (_root, manager) = fixture();
    let prompt_id = seed_prompt(&manager).await;

    manager
        .delete_prompt(&prompt_id)
        .await
        .expect("first delete");
    let first_at = tombstone_row(&manager, &prompt_id)
        .await
        .expect("the first delete wrote the tombstone");

    let second = manager
        .delete_prompt(&prompt_id)
        .await
        .expect("a second delete is not an error");
    assert!(
        matches!(second, PromptDeletion::Absent),
        "the second delete finds no row and answers Absent"
    );
    assert_eq!(tombstone_count(&manager).await, 1, "no second tombstone");
    assert_eq!(
        tombstone_row(&manager, &prompt_id).await,
        Some(first_at),
        "the first deletion time is unchanged"
    );
}

#[tokio::test]
async fn test_a_delete_over_an_existing_tombstone_keeps_the_first_deletion_time() {
    let (_root, manager) = fixture();
    let prompt_id = seed_prompt(&manager).await;

    // Seed the tombstone at a FIXED past time, not a measured one: a real first
    // delete could share a millisecond with the second write, and then a
    // `INSERT OR REPLACE` would be indistinguishable from `INSERT OR IGNORE`.
    const FIRST_AT: i64 = 1_000;
    {
        let db = manager.ready().await.expect("prompts ready");
        db.execute(
            "INSERT INTO prompt_tombstones (id, deleted_at_unix_ms) VALUES (?, ?)",
            vec![Value::Text(prompt_id.clone()), Value::Integer(FIRST_AT)],
        )
        .await
        .expect("seed the first tombstone");
    }
    // The prompt row is still present (seeded above, never deleted), so this
    // delete reaches the batch with a tombstone already on file: the legal
    // "id reused / peer raced" shape a plain INSERT would abort on.

    let outcome = manager
        .delete_prompt(&prompt_id)
        .await
        .expect("the batch must not abort on the existing tombstone");
    assert!(
        matches!(outcome, PromptDeletion::Deleted { .. }),
        "the row existed, so the delete is Deleted"
    );
    assert_eq!(tombstone_count(&manager).await, 1, "still one tombstone");
    assert_eq!(
        tombstone_row(&manager, &prompt_id).await,
        Some(FIRST_AT),
        "INSERT OR IGNORE keeps the first deletion time"
    );
}
