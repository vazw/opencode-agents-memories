//! W4.2 Step 6 — the typed `recall_project_sessions` tool schema (net-new,
//! R10: upstream `src/services/ai/tools/tool-schema.ts` pins only the *shape*
//! of a tool's `parameters`; there is no upstream timeline/recall service or
//! test).
//!
//! Registration, runtime and handler are W5.4 — this file pins the arguments,
//! the JSON schema and the result shape only.
//!
//! Comparison discipline (D-18): every schema assertion compares parsed
//! `Value`s, never raw JSON strings — the workspace pins `serde_json = "1"`
//! without `preserve_order`, so `Value::Object` is a `BTreeMap` and
//! serialization emits keys alphabetically.

use std::collections::BTreeSet;

use mem_episodic::error::EpisodicError;
use mem_episodic::record::NewAgentSession;
use mem_episodic::timeline::{MAX_TIMELINE_LIMIT, SessionSummary, TimelineFilter};
use mem_episodic::tool_schema::{
    RECALL_PROJECT_SESSIONS_DESCRIPTION, RECALL_PROJECT_SESSIONS_TOOL_NAME, RecallMode,
    RecallProjectSessionsArgs, RecallProjectSessionsResult,
};
use mem_store::serde_json::{self, Value, json};

fn assert_validation_field(error: EpisodicError, expected: &str) {
    match error {
        EpisodicError::Validation { field, .. } => assert_eq!(field, expected),
        other => panic!("expected Validation, got {other:?}"),
    }
}

fn args_with_project() -> RecallProjectSessionsArgs {
    RecallProjectSessionsArgs {
        project_id: "project-a".to_string(),
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Identity + argument resolution
// ---------------------------------------------------------------------------

#[test]
fn test_tool_name_is_recall_project_sessions() {
    assert_eq!(RECALL_PROJECT_SESSIONS_TOOL_NAME, "recall_project_sessions");
    assert!(
        RECALL_PROJECT_SESSIONS_DESCRIPTION.contains("timeline"),
        "description: {RECALL_PROJECT_SESSIONS_DESCRIPTION}"
    );
}

#[test]
fn test_args_require_only_project_id() {
    let args = args_with_project();
    args.validate().expect("project_id alone is a valid call");
    assert!(!args.include_full_sessions, "off by default");

    // The defaulted struct has no project id at all and is rejected.
    let empty = RecallProjectSessionsArgs::default();
    assert_validation_field(
        empty.validate().expect_err("project_id is required"),
        "project_id",
    );
}

#[test]
fn test_args_reject_empty_project_id() {
    for project_id in ["", "   ", " padded "] {
        let args = RecallProjectSessionsArgs {
            project_id: project_id.to_string(),
            ..Default::default()
        };
        assert_validation_field(
            args.validate().expect_err("blank project id rejected"),
            "project_id",
        );
    }

    let too_long = RecallProjectSessionsArgs {
        project_id: "p".repeat(257),
        ..Default::default()
    };
    assert_validation_field(
        too_long
            .validate()
            .expect_err("oversized project id rejected"),
        "project_id",
    );
}

#[test]
fn test_args_default_mode_is_chronological_without_query() {
    let args = args_with_project();
    assert_eq!(args.mode, None);
    assert_eq!(
        args.resolved_mode().expect("resolvable"),
        RecallMode::Chronological
    );
    args.validate().expect("chronological default validates");
}

#[test]
fn test_args_default_mode_is_semantic_with_query() {
    let args = RecallProjectSessionsArgs {
        query: Some("where did we wire the store".to_string()),
        ..args_with_project()
    };
    assert_eq!(args.mode, None);
    assert_eq!(
        args.resolved_mode().expect("resolvable"),
        RecallMode::Semantic
    );
    args.validate().expect("semantic default validates");
}

#[test]
fn test_args_reject_query_outside_semantic_mode() {
    for mode in [RecallMode::Chronological, RecallMode::Filtered] {
        let args = RecallProjectSessionsArgs {
            mode: Some(mode),
            query: Some("a query".to_string()),
            ..args_with_project()
        };
        let error = args
            .resolved_mode()
            .expect_err("query outside semantic mode rejected");
        assert_validation_field(error, "query");
        assert_validation_field(
            args.validate()
                .expect_err("validate surfaces the same error"),
            "query",
        );
    }
}

#[test]
fn test_args_reject_semantic_mode_without_query() {
    let args = RecallProjectSessionsArgs {
        mode: Some(RecallMode::Semantic),
        ..args_with_project()
    };
    let error = args
        .resolved_mode()
        .expect_err("semantic mode needs a query");
    assert_validation_field(error, "query");
    assert_validation_field(
        args.validate()
            .expect_err("validate surfaces the same error"),
        "query",
    );
}

#[test]
fn test_args_reject_unknown_field() {
    // Args are an INPUT contract: a model must not invent parameters.
    let error = serde_json::from_str::<RecallProjectSessionsArgs>(
        r#"{"projectId":"project-a","bogusParameter":true}"#,
    )
    .expect_err("unknown field rejected");
    assert!(
        error.to_string().contains("bogusParameter"),
        "error names the field: {error}"
    );
}

#[test]
fn test_args_reject_invalid_time_range() {
    let inverted = RecallProjectSessionsArgs {
        since: Some(2_000),
        until: Some(1_000),
        ..args_with_project()
    };
    assert_validation_field(
        inverted.validate().expect_err("inverted range rejected"),
        "since",
    );

    let above_max = RecallProjectSessionsArgs {
        last_n_sessions: Some(MAX_TIMELINE_LIMIT as u32 + 1),
        ..args_with_project()
    };
    assert_validation_field(
        above_max.validate().expect_err("above-max last_n rejected"),
        "last_n_sessions",
    );

    let negative = RecallProjectSessionsArgs {
        since: Some(-1),
        ..args_with_project()
    };
    assert_validation_field(
        negative.validate().expect_err("negative since rejected"),
        "since",
    );
}

#[test]
fn test_args_round_trip_camel_case_json() {
    let args = RecallProjectSessionsArgs {
        project_id: "project-a".to_string(),
        mode: Some(RecallMode::Filtered),
        query: None,
        since: Some(10),
        until: Some(20),
        last_n_sessions: Some(7),
        touched_files: Some(vec!["src/lib.rs".to_string()]),
        agent_id: Some("agent-1".to_string()),
        session_id: Some("session-1".to_string()),
        include_full_sessions: true,
    };

    let json = serde_json::to_string(&args).expect("serialize args");
    for key in [
        "\"projectId\"",
        "\"lastNSessions\"",
        "\"touchedFiles\"",
        "\"agentId\"",
        "\"sessionId\"",
        "\"includeFullSessions\"",
    ] {
        assert!(json.contains(key), "missing {key} in {json}");
    }

    // D-17: every `skip_serializing_if` is paired with `default`, so the
    // struct's own output always deserializes back.
    let parsed: RecallProjectSessionsArgs = serde_json::from_str(&json).expect("deserialize args");
    assert_eq!(parsed, args);

    // The minimal call round-trips too (every optional key omitted).
    let minimal = args_with_project();
    let json = serde_json::to_string(&minimal).expect("serialize minimal");
    let parsed: RecallProjectSessionsArgs =
        serde_json::from_str(&json).expect("deserialize minimal");
    assert_eq!(parsed, minimal);
}

#[test]
fn test_args_map_to_timeline_filter() {
    let args = RecallProjectSessionsArgs {
        project_id: "project-a".to_string(),
        mode: Some(RecallMode::Filtered),
        query: None,
        since: Some(100),
        until: Some(200),
        last_n_sessions: Some(9),
        touched_files: Some(vec!["a.rs".to_string(), "b.rs".to_string()]),
        agent_id: Some("agent-1".to_string()),
        session_id: Some("session-1".to_string()),
        include_full_sessions: false,
    };

    let filter = args.to_timeline_filter();
    assert_eq!(
        filter,
        TimelineFilter {
            since: Some(100),
            until: Some(200),
            last_n_sessions: Some(9),
            touched_files: Some(vec!["a.rs".to_string(), "b.rs".to_string()]),
            agent_id: Some("agent-1".to_string()),
            session_id: Some("session-1".to_string()),
        }
    );
    // The mapped filter is what `validate_timeline_filter` accepts.
    mem_episodic::validation::validate_timeline_filter(&filter).expect("filter validates");

    // An unset last_n maps to "use the default page size".
    let bare = args_with_project().to_timeline_filter();
    assert_eq!(bare, TimelineFilter::default());
    assert_eq!(
        mem_episodic::timeline::timeline_limit(&bare),
        mem_episodic::timeline::DEFAULT_TIMELINE_LIMIT
    );
}

// ---------------------------------------------------------------------------
// JSON schema (compared as Value, never as a string — D-18)
// ---------------------------------------------------------------------------

#[test]
fn test_json_schema_requires_only_project_id() {
    let schema = RecallProjectSessionsArgs::to_json_schema();
    assert_eq!(schema["type"], json!("object"));
    assert_eq!(schema["required"], json!(["projectId"]));
}

#[test]
fn test_json_schema_declares_every_filter_property() {
    let schema = RecallProjectSessionsArgs::to_json_schema();

    // The full content contract, compared as a parsed `Value`.
    assert_eq!(
        schema,
        json!({
            "type": "object",
            "properties": {
                "projectId": {"type": "string"},
                "mode": {"type": "string", "enum": ["chronological", "semantic", "filtered"]},
                "query": {"type": "string"},
                "since": {"type": "integer"},
                "until": {"type": "integer"},
                "lastNSessions": {"type": "integer", "minimum": 0, "maximum": MAX_TIMELINE_LIMIT},
                "touchedFiles": {"type": "array", "items": {"type": "string"}},
                "agentId": {"type": "string"},
                "sessionId": {"type": "string"},
                "includeFullSessions": {"type": "boolean"}
            },
            "required": ["projectId"],
            "additionalProperties": false
        })
    );

    // Key-set equality (independent of the literal above).
    let properties = schema["properties"].as_object().expect("properties object");
    let actual: BTreeSet<&str> = properties.keys().map(String::as_str).collect();
    let expected: BTreeSet<&str> = [
        "projectId",
        "mode",
        "query",
        "since",
        "until",
        "lastNSessions",
        "touchedFiles",
        "agentId",
        "sessionId",
        "includeFullSessions",
    ]
    .into_iter()
    .collect();
    assert_eq!(actual, expected);

    // `maximum` is the single source of truth, not a copied literal.
    assert_eq!(
        schema["properties"]["lastNSessions"]["maximum"],
        json!(MAX_TIMELINE_LIMIT)
    );
    assert_eq!(schema["properties"]["lastNSessions"]["minimum"], json!(0));
}

#[test]
fn test_json_schema_disallows_additional_properties() {
    let schema = RecallProjectSessionsArgs::to_json_schema();
    assert_eq!(schema["additionalProperties"], Value::Bool(false));
}

#[test]
fn test_json_schema_mode_enum_matches_recall_mode_wire_names() {
    let schema = RecallProjectSessionsArgs::to_json_schema();
    let variants: Vec<String> = schema["properties"]["mode"]["enum"]
        .as_array()
        .expect("enum array")
        .iter()
        .map(|value| value.as_str().expect("enum entry is a string").to_string())
        .collect();
    assert_eq!(variants, vec!["chronological", "semantic", "filtered"]);

    // Each variant's own serde wire name is in that list.
    for mode in [
        RecallMode::Chronological,
        RecallMode::Semantic,
        RecallMode::Filtered,
    ] {
        let wire = serde_json::to_value(mode).expect("serialize mode");
        let wire = wire.as_str().expect("mode serializes as a string");
        assert!(variants.contains(&wire.to_string()), "missing {wire}");
    }
}

// ---------------------------------------------------------------------------
// Result shape
// ---------------------------------------------------------------------------

#[test]
fn test_result_serializes_summaries_and_full_sessions() {
    let summary = SessionSummary {
        id: "row-1".to_string(),
        project_id: "project-a".to_string(),
        session_id: "session-1".to_string(),
        agent_id: "agent-1".to_string(),
        agent_name: "agent-1".to_string(),
        summary: "did the thing".to_string(),
        parent_session_id: None,
        orphaned: false,
        files_touched: vec!["src/lib.rs".to_string()],
        created_at: 10,
        updated_at: 10,
        similarity: None,
    };
    let result = RecallProjectSessionsResult::Summaries(vec![summary.clone()]);
    let json = serde_json::to_value(&result).expect("serialize summaries");
    let variant = json.get("summaries").expect("externally tagged variant");
    assert!(variant.is_array(), "{json}");
    // D-17 guard: the omitted `similarity` key must still deserialize.
    let parsed: RecallProjectSessionsResult =
        serde_json::from_value(json).expect("deserialize summaries");
    assert_eq!(parsed, result);

    // A similarity-bearing summary round-trips too.
    let with_similarity = SessionSummary {
        similarity: Some(0.75),
        ..summary.clone()
    };
    let result = RecallProjectSessionsResult::Summaries(vec![with_similarity.clone()]);
    let json = serde_json::to_value(&result).expect("serialize scored summaries");
    assert_eq!(json["summaries"][0]["similarity"], json!(0.75));
    let parsed: RecallProjectSessionsResult =
        serde_json::from_value(json).expect("deserialize scored summaries");
    assert_eq!(parsed, result);

    // Full sessions on demand: the embedding never travels the wire.
    let session = NewAgentSession {
        project_id: "project-a".to_string(),
        session_id: "session-1".to_string(),
        agent_id: "agent-1".to_string(),
        summary: "did the thing".to_string(),
        ..Default::default()
    }
    .into_session(11);
    let result = RecallProjectSessionsResult::FullSessions(vec![session.clone()]);
    let json = serde_json::to_value(&result).expect("serialize full sessions");
    let variant = json.get("fullSessions").expect("externally tagged variant");
    assert!(variant.is_array(), "{json}");
    assert!(
        variant[0].get("embedding").is_none(),
        "the embedding never leaves the store: {json}"
    );
    let parsed: RecallProjectSessionsResult =
        serde_json::from_value(json).expect("deserialize full sessions");
    assert_eq!(parsed, result);
}
