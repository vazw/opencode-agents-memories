//! W5.5 — pure byte-exact renderer oracles (`CompactionMode` + `render`).
//!
//! No store, no shard, no I/O: every row is built through
//! `mem_store::db::Row::new` with an explicit column vector, so the only thing
//! under test is the byte contract. The restore bytes are upstream's
//! `formatMemoriesForCompaction` (`example/opencode-mem/src/index.ts:1425-1441`
//! and the footer stripper at `:1392-1423`); the ingest heading is port-only.

use std::sync::Arc;

use mem_bridge::compaction::{CompactionMode, CompactionPolicy, render};
use mem_store::db::{Row, Value};

const RESTORE_2: &str = "## Restored Session Memory\n\n### Memory 1\nRetries are capped at three\n\nTags: project, opencode-mem\n\n### Memory 2\nDeploys go through the staging gate first\n\nTags: project, opencode-mem\n\n";
const INGEST_2: &str = "## Session Memory Before Compaction\n\n### Memory 1\nRetries are capped at three\n\nTags: project, opencode-mem\n\n### Memory 2\nDeploys go through the staging gate first\n\nTags: project, opencode-mem\n\n";

fn row(content: &str, tags: Option<&str>, created_at: i64) -> Row {
    let columns = Arc::new(vec![
        "id".to_string(),
        "content".to_string(),
        "tags".to_string(),
        "created_at".to_string(),
    ]);
    let values = vec![
        Value::Text(format!("row-{created_at}")),
        Value::Text(content.to_string()),
        tags.map(|tags| Value::Text(tags.to_string()))
            .unwrap_or(Value::Null),
        Value::Integer(created_at),
    ];
    Row::new(columns, values)
}

fn fixture_s_ascending() -> Vec<Row> {
    vec![
        row(
            "Deploys go through the staging gate first",
            Some("project,opencode-mem"),
            1000,
        ),
        row(
            "Retries are capped at three",
            Some("project,opencode-mem"),
            2000,
        ),
    ]
}

fn default_policy() -> CompactionPolicy {
    CompactionPolicy::default()
}

#[test]
fn test_restore_block_is_byte_exact_upstream_format() {
    let block = render(
        CompactionMode::Restore,
        fixture_s_ascending(),
        &default_policy(),
    )
    .expect("two rows render a block");
    assert_eq!(
        block, RESTORE_2,
        "restore bytes must equal upstream's formatMemoriesForCompaction output"
    );
}

#[test]
fn test_ingest_block_is_byte_exact_and_never_carries_the_injected_block() {
    let block = render(
        CompactionMode::Ingest,
        fixture_s_ascending(),
        &default_policy(),
    )
    .expect("two rows render a block");
    assert_eq!(
        block, INGEST_2,
        "the ingest heading is the only mode difference"
    );
    assert!(
        !block.contains("<project_agent_session_history>"),
        "the ingest render is session-scoped, never the project history wrapper"
    );
    assert!(
        !block.contains("The deploy key lives in op://infra/deploy"),
        "project-scoped facts are not part of the session render"
    );
}

#[test]
fn test_restore_strips_a_matching_embedded_tags_footer() {
    let matching = row(
        "Ports ship behind a feature flag\n\nTags: opencode-mem, project",
        Some("project,opencode-mem"),
        3000,
    );
    let block = render(CompactionMode::Restore, vec![matching], &default_policy())
        .expect("one row renders a block");
    assert_eq!(
        block,
        "## Restored Session Memory\n\n### Memory 1\nPorts ship behind a feature flag\n\nTags: project, opencode-mem\n\n",
        "a footer whose tag SET matches is stripped before the row body is rendered"
    );

    let mismatch = row(
        "Ports ship behind a feature flag\n\nTags: opencode-mem, project",
        Some("other"),
        3000,
    );
    let block = render(CompactionMode::Restore, vec![mismatch], &default_policy())
        .expect("one row renders a block");
    assert_eq!(
        block,
        "## Restored Session Memory\n\n### Memory 1\nPorts ship behind a feature flag\n\nTags: opencode-mem, project\n\nTags: other\n\n",
        "a footer whose tag set does NOT match renders unmodified"
    );
}

#[test]
fn test_restore_returns_none_when_the_rows_are_empty() {
    assert_eq!(
        render(CompactionMode::Restore, Vec::new(), &default_policy()),
        None,
        "zero rows is None, never a bare heading"
    );
    assert_eq!(
        render(CompactionMode::Ingest, Vec::new(), &default_policy()),
        None,
        "the ingest half returns None on zero rows too"
    );
}

#[test]
fn test_render_takes_newest_first_and_respects_memory_limit() {
    assert_eq!(
        CompactionPolicy::default(),
        CompactionPolicy {
            enabled: true,
            memory_limit: 10,
        },
        "Default is pinned to the core defaults {{enabled: true, memory_limit: 10}}"
    );
    let policy = CompactionPolicy {
        enabled: true,
        memory_limit: 1,
    };
    let block = render(CompactionMode::Restore, fixture_s_ascending(), &policy)
        .expect("one row survives the limit");
    assert_eq!(
        block,
        "## Restored Session Memory\n\n### Memory 1\nRetries are capped at three\n\nTags: project, opencode-mem\n\n",
        "the ascending input must be sorted created_at DESC and truncated AFTER sorting"
    );
}

#[test]
fn test_render_returns_none_when_the_policy_is_disabled() {
    let policy = CompactionPolicy {
        enabled: false,
        memory_limit: 10,
    };
    assert_eq!(
        render(CompactionMode::Restore, fixture_s_ascending(), &policy),
        None,
        "a disabled policy renders nothing on the restore leg"
    );
    assert_eq!(
        render(CompactionMode::Ingest, fixture_s_ascending(), &policy),
        None,
        "a disabled policy renders nothing on the ingest leg"
    );
}
