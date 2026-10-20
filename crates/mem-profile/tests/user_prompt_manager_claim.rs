//! Port of upstream `tests/user-prompt-manager-claim.test.ts` (W3.1).
//!
//! Upstream cases (traced 1:1 by name):
//! - "claimPrompt transitions captured 0 → 2 and returns true"
//! - "claimPrompt returns false when the row is already claimed"
//! - "releaseClaim transitions captured 2 → 0 and exposes the row to retry"
//! - "releaseClaim is a no-op when the row is already captured=1"
//! - "releaseClaim is a no-op when the row was never claimed"
//! - "supports a full claim → release → re-claim retry cycle"
//! - "ignores prompts that have exceeded max retries"
//! - "returns all uncaptured prompts for a session oldest first"
//!
//! Upstream's `TestableManager.ready()` escape hatch is a public API here
//! (`UserPromptManager::ready`), so the raw column assertions the upstream
//! test makes through `db.get` assert on-disk state through the same seam.

mod common;

use mem_profile::prompt::UserPromptManager;

const MAX_RETRIES: u32 = 3;

struct Fixture {
    dir: std::path::PathBuf,
    manager: UserPromptManager,
}

impl Fixture {
    fn new(prefix: &str) -> Self {
        let dir = common::temp_dir(prefix);
        let config = common::store_config(dir.clone());
        Self {
            manager: UserPromptManager::new(dir.clone(), config, MAX_RETRIES),
            dir,
        }
    }

    async fn raw_captured(&self, id: &str) -> i64 {
        let db = self.manager.ready().await.expect("ready");
        let row = mem_store::db::query_one(
            db.as_ref(),
            "SELECT captured FROM user_prompts WHERE id = ?",
            vec![mem_store::db::Value::Text(id.to_string())],
        )
        .await
        .expect("query captured");
        row.map(|row| row.i64("captured").unwrap_or(-1))
            .unwrap_or(-1)
    }

    async fn set_created_at(&self, id: &str, created_at: i64) {
        let db = self.manager.ready().await.expect("ready");
        db.execute(
            "UPDATE user_prompts SET created_at = ? WHERE id = ?",
            vec![
                mem_store::db::Value::Integer(created_at),
                mem_store::db::Value::Text(id.to_string()),
            ],
        )
        .await
        .expect("set created_at");
    }

    async fn new_prompt(&self, session_id: &str, content: &str) -> String {
        let message_id = format!("msg-{}-{}", mem_profile::ids::now_millis(), content.len());
        self.manager
            .save_prompt(session_id, &message_id, "/tmp/proj", content)
            .await
            .expect("save prompt")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        common::cleanup_temp_dir(&self.dir);
    }
}

#[tokio::test]
async fn test_claim_transitions_captured_zero_to_two_and_returns_true() {
    let fixture = Fixture::new("claim-basic");
    let id = fixture.new_prompt("session-test", "hello").await;

    assert!(fixture.manager.claim_prompt(&id).await.expect("claim"));
    assert_eq!(fixture.raw_captured(&id).await, 2);
}

#[tokio::test]
async fn test_claim_fails_when_row_already_claimed() {
    let fixture = Fixture::new("claim-twice");
    let id = fixture.new_prompt("session-test", "hello").await;

    assert!(
        fixture
            .manager
            .claim_prompt(&id)
            .await
            .expect("first claim")
    );
    assert!(
        !fixture
            .manager
            .claim_prompt(&id)
            .await
            .expect("second claim")
    );
}

#[tokio::test]
async fn test_release_claim_transitions_two_to_zero_and_exposes_row_to_retry() {
    let fixture = Fixture::new("claim-release");
    let id = fixture.new_prompt("session-release", "hello").await;

    assert!(fixture.manager.claim_prompt(&id).await.expect("claim"));
    assert!(fixture.manager.release_claim(&id).await.expect("release"));
    assert_eq!(fixture.raw_captured(&id).await, 0);

    let next = fixture
        .manager
        .get_last_uncaptured_prompt("session-release")
        .await
        .expect("query")
        .expect("released row is visible again");
    assert_eq!(next.id, id);
}

#[tokio::test]
async fn test_release_claim_no_op_when_already_captured_one() {
    let fixture = Fixture::new("claim-released-captured");
    let id = fixture.new_prompt("session-test", "hello").await;

    assert!(fixture.manager.claim_prompt(&id).await.expect("claim"));
    fixture
        .manager
        .mark_as_captured(&id)
        .await
        .expect("mark captured");

    assert!(!fixture.manager.release_claim(&id).await.expect("release"));
    assert_eq!(fixture.raw_captured(&id).await, 1);
}

#[tokio::test]
async fn test_release_claim_no_op_when_never_claimed() {
    let fixture = Fixture::new("claim-release-unclaimed");
    let id = fixture.new_prompt("session-test", "hello").await;

    assert!(!fixture.manager.release_claim(&id).await.expect("release"));
    assert_eq!(fixture.raw_captured(&id).await, 0);
}

#[tokio::test]
async fn test_claim_release_reclaim_retry_cycle() {
    let fixture = Fixture::new("claim-cycle");
    let id = fixture.new_prompt("session-test", "hello").await;

    assert!(fixture.manager.claim_prompt(&id).await.expect("claim"));
    assert!(fixture.manager.release_claim(&id).await.expect("release"));
    assert!(fixture.manager.claim_prompt(&id).await.expect("re-claim"));
    assert_eq!(fixture.raw_captured(&id).await, 2);
}

#[tokio::test]
async fn test_uncaptured_query_ignores_prompts_exceeding_max_retries() {
    let fixture = Fixture::new("claim-retries");
    let id = fixture.new_prompt("session-retries", "hello retry").await;

    for _ in 0..MAX_RETRIES + 1 {
        fixture
            .manager
            .record_failed_attempt(&id)
            .await
            .expect("record failed attempt");
    }

    assert!(
        fixture
            .manager
            .get_last_uncaptured_prompt("session-retries")
            .await
            .expect("query")
            .is_none(),
        "a row at/over max retries must be invisible to the uncaptured query"
    );
}

#[tokio::test]
async fn test_uncaptured_prompts_for_session_returns_oldest_first() {
    let fixture = Fixture::new("claim-batch");

    let later = fixture.new_prompt("session-batch", "later").await;
    let older = fixture.new_prompt("session-batch", "older").await;
    let captured = fixture.new_prompt("session-batch", "captured").await;
    let other_session = fixture.new_prompt("session-other", "other").await;
    let retried_out = fixture.new_prompt("session-batch", "retried out").await;

    fixture.set_created_at(&later, 20).await;
    fixture.set_created_at(&older, 10).await;
    fixture.set_created_at(&captured, 15).await;
    fixture.set_created_at(&other_session, 5).await;
    fixture.set_created_at(&retried_out, 25).await;
    fixture
        .manager
        .mark_as_captured(&captured)
        .await
        .expect("mark captured");
    for _ in 0..MAX_RETRIES + 1 {
        fixture
            .manager
            .record_failed_attempt(&retried_out)
            .await
            .expect("record failed attempt");
    }

    let prompts = fixture
        .manager
        .get_uncaptured_prompts_for_session("session-batch")
        .await
        .expect("query");

    let ids: Vec<String> = prompts.into_iter().map(|prompt| prompt.id).collect();
    assert_eq!(ids, vec![older, later]);
}
