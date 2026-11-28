//! Issue 174 — the prompt ledger's identity column, through the manager's
//! public API only.
//!
//! Tests:
//! - `test_save_prompt_stamps_the_identity`
//! - `test_save_prompt_retry_keeps_the_first_identity`
//! - `test_distinct_user_ids_lists_only_unanalyzed_identified_rows`

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use mem_profile::prompt::UserPromptManager;

const MAX_RETRIES: u32 = 3;

struct TempDir(PathBuf);

impl TempDir {
    fn new(prefix: &str) -> Self {
        Self(common::temp_dir(prefix))
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        common::cleanup_temp_dir(&self.0);
    }
}

fn manager(dir: &TempDir) -> Arc<UserPromptManager> {
    Arc::new(UserPromptManager::new(
        dir.path().to_path_buf(),
        common::store_config(dir.path().to_path_buf()),
        MAX_RETRIES,
    ))
}

#[tokio::test]
async fn test_save_prompt_stamps_the_identity() {
    let dir = TempDir::new("prompt-identity-stamp");
    let manager = manager(&dir);

    manager
        .save_prompt("session-1", "message-1", "/workspace", "hello", Some("a@x"))
        .await
        .expect("save");

    let rows = manager
        .get_prompts_for_user_learning(10, "a@x")
        .await
        .expect("select by identity");
    assert_eq!(rows.len(), 1, "the row is selected under its own identity");
    assert_eq!(rows[0].content, "hello");

    assert!(
        manager
            .get_prompts_for_user_learning(10, "b@y")
            .await
            .expect("select under another identity")
            .is_empty(),
        "an identity's select never sees another identity's row"
    );
}

#[tokio::test]
async fn test_save_prompt_retry_keeps_the_first_identity() {
    let dir = TempDir::new("prompt-identity-retry");
    let manager = manager(&dir);

    let first = manager
        .save_prompt("session-1", "message-1", "/workspace", "hello", Some("a@x"))
        .await
        .expect("first save");
    let second = manager
        .save_prompt("session-1", "message-1", "/workspace", "hello", Some("b@y"))
        .await
        .expect("retry save");

    assert_eq!(first, second, "one (session_id, message_id) is one row");
    assert_eq!(
        manager
            .get_prompts_for_user_learning(10, "a@x")
            .await
            .expect("select under the first identity")
            .len(),
        1,
        "the first identity is kept"
    );
    assert!(
        manager
            .get_prompts_for_user_learning(10, "b@y")
            .await
            .expect("select under the retry identity")
            .is_empty(),
        "a retry never rewrites an existing row's identity"
    );
}

#[tokio::test]
async fn test_distinct_user_ids_lists_only_unanalyzed_identified_rows() {
    let dir = TempDir::new("prompt-identity-distinct");
    let manager = manager(&dir);

    manager
        .save_prompt("session-a", "message-a", "/workspace", "A", Some("a@x"))
        .await
        .expect("save A");
    manager
        .save_prompt("session-b", "message-b", "/workspace", "B", Some("b@y"))
        .await
        .expect("save B");
    let captured = manager
        .save_prompt("session-c", "message-c", "/workspace", "C", Some("c@z"))
        .await
        .expect("save C");
    manager
        .mark_multiple_as_user_learning_captured(&[captured])
        .await
        .expect("mark C analyzed");
    manager
        .save_prompt("session-n", "message-n", "/workspace", "N", None)
        .await
        .expect("save the NULL-identity row");

    assert_eq!(
        manager
            .distinct_user_ids_for_learning()
            .await
            .expect("distinct identities"),
        vec!["a@x".to_string(), "b@y".to_string()],
        "an analyzed row's identity and a NULL identity are both excluded"
    );
}
