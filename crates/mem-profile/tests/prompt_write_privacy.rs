//! Prompt-write privacy gate (W3.1 D-V6) — the issue directive that the
//! prompt write path is where fully-private rejection lands.
//!
//! Upstream applies `stripPrivateContent` / `isFullyPrivate` at the
//! memory-add and explicit-profile-write tool boundaries (`index.ts:970-971`,
//! `1040-1046`); the issue directs the same policy onto stored prompts. The
//! gate uses ONLY the locked W2.3 public surface
//! (`mem_ai::privacy::{strip_private_content, is_fully_private, REDACTED}`,
//! hash-3fc66017d7f3) and keeps upstream's sentence
//! `Private content blocked`.
//!
//! Cases (one per pinned behavior):
//! - `test_save_prompt_rejects_fully_private_content`
//! - `test_save_prompt_rejects_content_left_only_with_redaction_marker`
//! - `test_save_prompt_strips_private_span_before_persisting`
//! - `test_save_prompt_persists_plain_content_unchanged`

mod common;

use mem_ai::privacy::REDACTED;
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

    async fn row_count(&self) -> i64 {
        let db = self.manager.ready().await.expect("ready");
        let row = mem_store::db::query_one(
            db.as_ref(),
            "SELECT COUNT(*) as count FROM user_prompts",
            Vec::new(),
        )
        .await
        .expect("count rows");
        row.and_then(|row| row.i64("count")).unwrap_or(0)
    }

    async fn stored_content(&self, id: &str) -> String {
        self.manager
            .get_prompt_by_id(id)
            .await
            .expect("query")
            .expect("row exists")
            .content
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        common::cleanup_temp_dir(&self.dir);
    }
}

#[tokio::test]
async fn test_save_prompt_rejects_fully_private_content() {
    let fixture = Fixture::new("privacy-fully-private");

    let error = fixture
        .manager
        .save_prompt("s", "m", "/p", "before <private>secret token</private>")
        .await;

    assert!(error.is_ok(), "a partially private prompt stays storable");
    assert_eq!(fixture.row_count().await, 1);

    let blocked = fixture
        .manager
        .save_prompt("s", "m2", "/p", "<private>whole thing is secret</private>")
        .await;
    let error = blocked.expect_err("fully private content must be rejected");
    assert_eq!(error.to_string(), "Private content blocked");
    assert_eq!(
        fixture.row_count().await,
        1,
        "a rejected save must not write a row"
    );
}

#[tokio::test]
async fn test_save_prompt_rejects_content_left_only_with_redaction_marker() {
    let fixture = Fixture::new("privacy-marker-only");

    for (index, content) in [
        "<private>a</private><private>b</private>".to_string(),
        "  <private>a</private>  <private>b</private>  ".to_string(),
        format!("<private>a</private>{REDACTED}"),
        "   ".to_string(),
        String::new(),
    ]
    .into_iter()
    .enumerate()
    {
        let message_id = format!("m{index}");
        let result = fixture
            .manager
            .save_prompt("s", &message_id, "/p", &content)
            .await;
        let error = result.expect_err("content left only with redaction markers is blocked");
        assert_eq!(error.to_string(), "Private content blocked");
    }
    assert_eq!(fixture.row_count().await, 0);
}

#[tokio::test]
async fn test_save_prompt_strips_private_span_before_persisting() {
    let fixture = Fixture::new("privacy-strip");

    let id = fixture
        .manager
        .save_prompt(
            "s",
            "m",
            "/p",
            "keep this <private>hide that</private> and this",
        )
        .await
        .expect("partial private span persists its stripped form");

    let stored = fixture.stored_content(&id).await;
    assert_eq!(stored, format!("keep this {REDACTED} and this"));
    assert!(
        !stored.contains("hide that"),
        "the private span must never reach storage: {stored}"
    );
}

#[tokio::test]
async fn test_save_prompt_persists_plain_content_unchanged() {
    let fixture = Fixture::new("privacy-plain");
    let content = "plain prompt with no private spans";

    let id = fixture
        .manager
        .save_prompt("s", "m", "/p", content)
        .await
        .expect("plain content persists");

    assert_eq!(fixture.stored_content(&id).await, content);
}
