//! W4.1 lane B — record shape: serde wire shape, validation rejection matrix
//! (net-new, R10: no upstream oracle; these tests define the record shape).
//!
//! Pure types only — no database, no embedder.

mod common;

use std::collections::BTreeMap;

use mem_episodic::error::EpisodicError;
use mem_episodic::payload::SessionMetadata;
use mem_episodic::record::{AgentSession, Decision, NewAgentSession, WorkItem, WorkKind};
use mem_episodic::validation::{
    MAX_AGENT_ID_LEN, MAX_FILE_PATH_LEN, MAX_FILES, validate, validate_metadata,
};
use mem_store::serde_json::{self, Value};

/// Minimal valid session builder.
fn valid_session() -> AgentSession {
    let new = NewAgentSession {
        project_id: "project-a".to_string(),
        session_id: "session-1".to_string(),
        agent_id: "agent-1".to_string(),
        summary: "did the thing".to_string(),
        ..Default::default()
    };
    new.into_session(1_000)
}

fn assert_validation_field(error: EpisodicError, expected: &str) {
    match error {
        EpisodicError::Validation { field, .. } => assert_eq!(field, expected),
        other => panic!("expected Validation, got {other:?}"),
    }
}

#[test]
fn test_session_metadata_serde_round_trip() {
    let metadata = SessionMetadata {
        timestamp: 1_700_000_000_000,
        agent_name: "builder".to_string(),
        git_branch: Some("main".to_string()),
        git_commit: Some("abc1234".to_string()),
        files_touched: vec!["src/lib.rs".to_string()],
        completed_work: vec![WorkItem {
            kind: WorkKind::Feature,
            description: "added the store".to_string(),
        }],
        decisions: vec![Decision {
            description: "dedicated episodic.db".to_string(),
        }],
        open_threads: vec!["wire capture in W4.2".to_string()],
        extra: BTreeMap::new(),
    };

    let json = metadata.to_json_string().expect("serialize");
    // camelCase wire shape.
    assert!(json.contains("\"agentName\""), "json: {json}");
    assert!(json.contains("\"gitBranch\""), "json: {json}");
    assert!(json.contains("\"filesTouched\""), "json: {json}");
    assert!(json.contains("\"completedWork\""), "json: {json}");
    assert!(json.contains("\"openThreads\""), "json: {json}");

    let parsed = SessionMetadata::from_json_str(&json).expect("deserialize");
    assert_eq!(parsed, metadata);
}

#[test]
fn test_unknown_keys_land_in_extra() {
    let json = r#"{
        "timestamp": 5,
        "agentName": "builder",
        "filesTouched": [],
        "completedWork": [],
        "decisions": [],
        "openThreads": [],
        "futureField": {"nested": [1, 2]},
        "anotherOne": "kept"
    }"#;

    let metadata = SessionMetadata::from_json_str(json).expect("parse forward-compatible row");
    assert_eq!(
        metadata.extra.get("futureField"),
        Some(&serde_json::json!({"nested": [1, 2]}))
    );
    assert_eq!(
        metadata.extra.get("anotherOne"),
        Some(&Value::String("kept".to_string()))
    );

    // A round trip never drops the unknown keys.
    let round_tripped =
        SessionMetadata::from_json_str(&metadata.to_json_string().expect("serialize"))
            .expect("reparse");
    assert_eq!(round_tripped.extra, metadata.extra);
}

#[test]
fn test_work_kind_wire_names() {
    let cases = [
        (WorkKind::Feature, "\"feature\""),
        (WorkKind::BugFix, "\"bugFix\""),
        (WorkKind::Command, "\"command\""),
        (WorkKind::Other, "\"other\""),
    ];
    for (kind, wire) in cases {
        let rendered = serde_json::to_string(&kind).expect("serialize kind");
        assert_eq!(rendered, wire);
        let parsed: WorkKind = serde_json::from_str(wire).expect("parse kind");
        assert_eq!(parsed, kind);
        assert_eq!(kind.as_wire_name(), wire.trim_matches('"'));
    }
}

#[test]
fn test_validation_rejects_empty_project_id() {
    let mut session = valid_session();
    session.project_id = String::new();
    assert_validation_field(validate(&session).expect_err("rejected"), "project_id");

    session.project_id = "   ".to_string();
    assert_validation_field(validate(&session).expect_err("rejected"), "project_id");

    session.project_id = " padded ".to_string();
    assert_validation_field(validate(&session).expect_err("rejected"), "project_id");
}

#[test]
fn test_validation_rejects_empty_session_id() {
    let mut session = valid_session();
    session.session_id = String::new();
    assert_validation_field(validate(&session).expect_err("rejected"), "session_id");

    session.session_id = "trailing ".to_string();
    assert_validation_field(validate(&session).expect_err("rejected"), "session_id");
}

#[test]
fn test_validation_rejects_empty_agent_id() {
    let mut session = valid_session();
    session.agent_id = String::new();
    assert_validation_field(validate(&session).expect_err("rejected"), "agent_id");

    let mut too_long = valid_session();
    too_long.agent_id = "a".repeat(MAX_AGENT_ID_LEN + 1);
    assert_validation_field(validate(&too_long).expect_err("rejected"), "agent_id");
}

#[test]
fn test_validation_rejects_empty_summary() {
    let mut session = valid_session();
    session.summary = String::new();
    assert_validation_field(validate(&session).expect_err("rejected"), "summary");
}

#[test]
fn test_validation_rejects_self_parent() {
    let mut session = valid_session();
    session.parent_session_id = Some(session.session_id.clone());
    assert_validation_field(
        validate(&session).expect_err("rejected"),
        "parent_session_id",
    );

    // A distinct parent is fine.
    session.parent_session_id = Some("session-parent".to_string());
    validate(&session).expect("distinct parent accepted");
}

#[test]
fn test_validation_rejects_oversized_files_list() {
    let mut session = valid_session();
    session.metadata.files_touched = vec!["src/lib.rs".to_string(); MAX_FILES + 1];
    assert_validation_field(
        validate_metadata(&session.metadata).expect_err("rejected"),
        "files_touched",
    );

    let mut path_too_long = valid_session();
    path_too_long.metadata.files_touched = vec!["x".repeat(MAX_FILE_PATH_LEN + 1)];
    assert_validation_field(
        validate_metadata(&path_too_long.metadata).expect_err("rejected"),
        "files_touched",
    );
}

#[test]
fn test_validation_rejects_bad_git_commit() {
    let mut session = valid_session();
    session.metadata.git_commit = Some("not-a-hex".to_string());
    assert_validation_field(
        validate_metadata(&session.metadata).expect_err("rejected"),
        "git_commit",
    );

    session.metadata.git_commit = Some("abc".to_string());
    assert_validation_field(
        validate_metadata(&session.metadata).expect_err("rejected"),
        "git_commit",
    );

    session.metadata.git_commit = Some("abc1234".to_string());
    validate_metadata(&session.metadata).expect("7-hex commit accepted");
}

#[test]
fn test_new_session_agent_name_default_three_cases() {
    // 1. empty → defaults to agent_id.
    let empty = NewAgentSession {
        project_id: "project-a".to_string(),
        session_id: "session-1".to_string(),
        agent_id: "agent-1".to_string(),
        summary: "s".to_string(),
        agent_name: String::new(),
        ..Default::default()
    };
    let defaulted = empty.into_session(10);
    assert_eq!(defaulted.metadata.agent_name, "agent-1");
    validate(&defaulted).expect("defaulted record is valid");

    // 2. non-empty → preserved.
    let named = NewAgentSession {
        project_id: "project-a".to_string(),
        session_id: "session-1".to_string(),
        agent_id: "agent-1".to_string(),
        summary: "s".to_string(),
        agent_name: "reviewer".to_string(),
        ..Default::default()
    };
    let preserved = named.into_session(10);
    assert_eq!(preserved.metadata.agent_name, "reviewer");
    validate(&preserved).expect("named record is valid");

    // 3. whitespace-only → rejected (never defaulted).
    let whitespace = NewAgentSession {
        project_id: "project-a".to_string(),
        session_id: "session-1".to_string(),
        agent_id: "agent-1".to_string(),
        summary: "s".to_string(),
        agent_name: "   ".to_string(),
        ..Default::default()
    };
    let rejected = whitespace.into_session(10);
    assert_eq!(rejected.metadata.agent_name, "   ");
    assert_validation_field(validate(&rejected).expect_err("rejected"), "agent_name");
}

#[test]
fn test_new_session_sets_created_equals_updated() {
    let new = NewAgentSession {
        project_id: "project-a".to_string(),
        session_id: "session-1".to_string(),
        agent_id: "agent-1".to_string(),
        summary: "s".to_string(),
        ..Default::default()
    };
    let session = new.into_session(4_242);
    assert_eq!(session.created_at, 4_242);
    assert_eq!(session.updated_at, 4_242);
    assert_eq!(session.metadata.timestamp, 4_242);
    assert!(session.id.is_empty(), "id is minted at insert time");
    assert!(
        session.embedding.is_empty(),
        "embedding is produced at insert time"
    );
}

#[test]
fn test_metadata_timestamp_must_be_non_negative() {
    let mut metadata = valid_session().metadata;
    metadata.timestamp = -1;
    assert_validation_field(
        validate_metadata(&metadata).expect_err("rejected"),
        "timestamp",
    );

    metadata.timestamp = 0;
    validate_metadata(&metadata).expect("zero timestamp accepted");
}

// ---------------------------------------------------------------------------
// W4.2 Step 1 — v2 record surface (soft delete + orphan flag)
// ---------------------------------------------------------------------------

#[test]
fn test_into_session_defaults_soft_delete_fields() {
    let session = valid_session();
    assert!(session.deleted_at.is_none(), "a fresh record is live");
    assert!(
        !session.orphaned,
        "a fresh record that never had a parent is not orphaned"
    );
}

#[test]
fn test_agent_session_serializes_without_embedding() {
    let mut session = valid_session();
    session.embedding = vec![0.5, 0.25, 0.125];
    session.orphaned = true;
    session.deleted_at = Some(42);

    let value = serde_json::to_value(&session).expect("serialize agent session");
    assert!(
        value.get("embedding").is_none(),
        "the embedding must never travel the wire: {value}"
    );
    assert_eq!(value["orphaned"], Value::Bool(true));
    assert_eq!(value["deletedAt"], serde_json::json!(42));
    assert!(
        value.get("projectId").is_some(),
        "camelCase wire shape: {value}"
    );

    let parsed: AgentSession = serde_json::from_value(value).expect("deserialize agent session");
    assert!(
        parsed.embedding.is_empty(),
        "the skipped embedding defaults"
    );
    assert!(parsed.orphaned);
    assert_eq!(parsed.deleted_at, Some(42));
    assert_eq!(parsed.project_id, session.project_id);
    assert_eq!(parsed.metadata, session.metadata);
}
