//! Port of upstream `tests/user-prompt-path-normalization.test.ts` (W3.1).
//!
//! Upstream cases (traced 1:1 by name):
//! - "getCapturedPrompts matches paths regardless of separator style"
//! - "searchPrompts matches paths regardless of separator style"
//!
//! The port pins both halves of the upstream trick: the SQL-side
//! `REPLACE(project_path, '\', '/')` and the Rust-side
//! `project_path.replace(/\\/g, "/")` on the query argument.

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

    async fn save_captured_prompt(&self, project_path: &str) -> String {
        let message_id = format!(
            "msg-{}-{}",
            mem_profile::ids::now_millis(),
            project_path.len()
        );
        let id = self
            .manager
            .save_prompt("session-path-test", &message_id, project_path, "hello")
            .await
            .expect("save prompt");
        self.manager
            .mark_as_captured(&id)
            .await
            .expect("mark captured");
        id
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        common::cleanup_temp_dir(&self.dir);
    }
}

#[tokio::test]
async fn test_captured_prompts_match_paths_regardless_of_separator_style() {
    let fixture = Fixture::new("path-captured");
    fixture.save_captured_prompt("C:\\workspace\\proj").await;

    let forward = fixture
        .manager
        .get_captured_prompts(Some("C:/workspace/proj"))
        .await
        .expect("forward-slash query");
    assert_eq!(forward.len(), 1, "forward-slash query must match");

    let backslash = fixture
        .manager
        .get_captured_prompts(Some("C:\\workspace\\proj"))
        .await
        .expect("backslash query");
    assert_eq!(backslash.len(), 1, "backslash query must match");
}

#[tokio::test]
async fn test_search_prompts_match_paths_regardless_of_separator_style() {
    let fixture = Fixture::new("path-search");
    fixture.save_captured_prompt("D:\\repos\\app").await;

    let results = fixture
        .manager
        .search_prompts("hello", Some("D:/repos/app"), 20)
        .await
        .expect("search");

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].content, "hello");
}
