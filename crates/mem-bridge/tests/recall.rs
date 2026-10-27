//! W5.4 — the `recall` wire method's engine-level oracles.
//!
//! Steps 1-3 of the W5.4 blueprint: two accepted parameter shapes, the
//! project-scoped read, the refusals, full sessions, degraded mode and the
//! semantic ranking. Every oracle dispatches through [`Engine::dispatch`] —
//! the fixture oracles' own seam — except the classifier test, which calls
//! [`RecallRequest::from_params`] directly.

mod common;

use std::sync::Arc;

use mem_bridge::engine::{Engine, EngineConfig, EngineRuntime};
use mem_bridge::jsonrpc::{Request, Response, parse_line};
use mem_bridge::recall::{RecallError, RecallRequest};
use mem_episodic::record::NewAgentSession;
use mem_store::serde_json::{Value, json};

use common::{TestDir, build_fixture_runtime};

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}

/// One fixture engine over three seeded rows — two in `project-a` and one in
/// `project-b` — so both the project filter and the page width are observable.
struct Seeded {
    engine: Engine,
    _dir: TestDir,
}

async fn seeded_engine() -> Seeded {
    let dir = TestDir::new("w54-recall");
    let fixture = build_fixture_runtime(dir.path()).await;
    for (project_id, session_id, agent_id, summary) in [
        (
            "project-a",
            "sess-1",
            "agent-1",
            "Fixed the login retry bug",
        ),
        (
            "project-a",
            "sess-2",
            "agent-2",
            "Added the vector store tests",
        ),
        ("project-b", "sess-3", "agent-3", "Wrote the project-b docs"),
    ] {
        fixture
            .store
            .insert_session(
                NewAgentSession {
                    project_id: project_id.to_string(),
                    session_id: session_id.to_string(),
                    agent_id: agent_id.to_string(),
                    summary: summary.to_string(),
                    ..Default::default()
                },
                fixture.embedder.as_ref(),
            )
            .await
            .unwrap();
    }
    let runtime = EngineRuntime {
        capture: fixture.capture,
        inject: fixture.inject,
        recall: fixture.recall,
    };
    Seeded {
        engine: Engine::with_runtime(EngineConfig::default(), Arc::new(runtime)),
        _dir: dir,
    }
}

async fn dispatch_line(engine: &Engine, line: &str) -> Response {
    let request: Request = parse_line(line).unwrap();
    engine.dispatch(&request).await
}

fn result_of(response: &Response) -> &Value {
    response
        .result
        .as_ref()
        .unwrap_or_else(|| panic!("expected a result, got {:?}", response.error))
}

#[test]
fn test_recall_returns_rows_for_the_scoped_project() {
    block_on(async {
        let seeded = seeded_engine().await;
        let response = dispatch_line(
            &seeded.engine,
            r#"{"id":"recall-1","jsonrpc":"2.0","method":"recall","params":{"session_id":"sess-1","projectId":"project-a"}}"#,
        )
        .await;
        assert!(
            response.error.is_none(),
            "canonical recall must succeed: {:?}",
            response.error
        );
        let result = result_of(&response);
        assert_eq!(result["count"].as_u64(), Some(2));
        let items = result["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        for item in items {
            assert_eq!(item["projectId"], "project-a");
        }
        assert!(
            items[0]["summary"]
                .as_str()
                .is_some_and(|summary| !summary.is_empty()),
            "the first item carries its summary"
        );
        assert!(
            result.get("summaries").is_none(),
            "the wire page is {{count, items}}, never the tool vocabulary"
        );
    });
}

#[test]
fn test_recall_excludes_other_projects() {
    block_on(async {
        let seeded = seeded_engine().await;
        let response = dispatch_line(
            &seeded.engine,
            r#"{"id":"recall-2","jsonrpc":"2.0","method":"recall","params":{"session_id":"sess-1","projectId":"project-b"}}"#,
        )
        .await;
        assert!(
            response.error.is_none(),
            "canonical recall must succeed: {:?}",
            response.error
        );
        let result = result_of(&response);
        assert_eq!(result["count"].as_u64(), Some(1));
        let items = result["items"].as_array().unwrap();
        assert_eq!(items[0]["sessionId"], "sess-3");
    });
}

#[test]
fn test_recall_classifies_the_legacy_shape_and_not_every_scopeless_request() {
    // 1. The frozen fixture's params classify as the legacy shape.
    let frozen: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/bridge/recall-request.json"
    )))
    .unwrap();
    let frozen_params = frozen["params"].clone();
    assert!(matches!(
        RecallRequest::from_params(&frozen_params),
        Ok(RecallRequest::Legacy)
    ));

    // 2. A `projectId` selects the canonical shape.
    let params = json!({"session_id": "sess-1", "projectId": "project-a"});
    match RecallRequest::from_params(&params) {
        Ok(RecallRequest::Canonical(args)) => {
            assert_eq!(args.project_id, "project-a");
        }
        other => panic!("expected the canonical shape, got {other:?}"),
    }

    // 3. A `query` never demotes a scoped request: it selects Semantic mode.
    let params = json!({"session_id": "sess-1", "projectId": "project-a", "query": "vector store"});
    assert!(matches!(
        RecallRequest::from_params(&params),
        Ok(RecallRequest::Canonical(_))
    ));

    // 4. A request with neither shape is invalid params, never a silent page.
    assert!(matches!(
        RecallRequest::from_params(&json!({"session_id": "sess-1"})),
        Err(RecallError::InvalidParams)
    ));

    // 5. The legacy clause is a KEY SET, not "has no projectId".
    assert!(matches!(
        RecallRequest::from_params(
            &json!({"session_id": "sess-1", "query": "vector store", "bogusParameter": true})
        ),
        Err(RecallError::InvalidParams)
    ));

    // 6. A legacy marker without a query is not a legacy shape.
    assert!(matches!(
        RecallRequest::from_params(&json!({"session_id": "sess-1", "limit": 5})),
        Err(RecallError::InvalidParams)
    ));

    // 7. A non-string query is not a legacy shape.
    assert!(matches!(
        RecallRequest::from_params(&json!({"session_id": "sess-1", "query": 5})),
        Err(RecallError::InvalidParams)
    ));

    // 8. The envelope strip removes `session_id` only: the W4.2 filter
    // `sessionId` is a different key and must survive into the args.
    let params = json!({"session_id": "sess-1", "projectId": "project-a", "sessionId": "filter-1"});
    match RecallRequest::from_params(&params) {
        Ok(RecallRequest::Canonical(args)) => {
            assert_eq!(args.session_id.as_deref(), Some("filter-1"))
        }
        other => panic!("expected the canonical shape, got {other:?}"),
    }
}

#[test]
fn test_recall_rejects_blank_or_missing_project() {
    block_on(async {
        let seeded = seeded_engine().await;
        // `session_id` is present and non-empty in both: the refusal is the
        // classifier's / the validator's, never the envelope gate's.
        for (id, params) in [
            ("recall-3a", json!({"session_id": "sess-1"})),
            (
                "recall-3b",
                json!({"session_id": "sess-1", "projectId": "   "}),
            ),
        ] {
            let request = json!({"id": id, "jsonrpc": "2.0", "method": "recall", "params": params});
            let response =
                dispatch_line(&seeded.engine, &serde_json::to_string(&request).unwrap()).await;
            assert_eq!(
                response.error.as_ref().map(|error| error.code),
                Some(-32602),
                "{id} must be invalid params, got {:?}",
                response.result
            );
            assert!(response.result.is_none(), "{id} answers no result");
        }
    });
}

#[test]
fn test_recall_rejects_unknown_argument() {
    block_on(async {
        let seeded = seeded_engine().await;
        let response = dispatch_line(
            &seeded.engine,
            r#"{"id":"recall-4","jsonrpc":"2.0","method":"recall","params":{"session_id":"sess-1","projectId":"project-a","bogusParameter":true}}"#,
        )
        .await;
        assert_eq!(
            response.error.as_ref().map(|error| error.code),
            Some(-32602),
            "an unknown field is refused, never served"
        );
        assert!(response.result.is_none());
    });
}

#[test]
fn test_recall_without_a_runtime_is_degraded() {
    block_on(async {
        let engine = Engine::new(EngineConfig::default());
        let response = dispatch_line(
            &engine,
            r#"{"id":"recall-7","jsonrpc":"2.0","method":"recall","params":{"session_id":"sess-1","projectId":"project-a"}}"#,
        )
        .await;
        assert_eq!(
            response.error.as_ref().map(|error| error.code),
            Some(-32001),
            "a missing runtime is degraded, never an empty page"
        );
    });
}

#[test]
fn test_recall_returns_full_sessions_when_requested() {
    block_on(async {
        let seeded = seeded_engine().await;
        let response = dispatch_line(
            &seeded.engine,
            r#"{"id":"recall-8","jsonrpc":"2.0","method":"recall","params":{"session_id":"sess-1","projectId":"project-a","includeFullSessions":true}}"#,
        )
        .await;
        assert!(
            response.error.is_none(),
            "full-session recall must succeed: {:?}",
            response.error
        );
        let result = result_of(&response);
        assert_eq!(result["count"].as_u64(), Some(2));
        let items = result["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        for item in items {
            assert!(
                item.get("metadata").is_some(),
                "an includeFullSessions item is a full record"
            );
            // Green by construction (`AgentSession.embedding` is
            // `#[serde(skip)]`); kept as a drift guard, not as proof.
            assert!(
                item.get("embedding").is_none(),
                "the embedding never travels"
            );
        }
    });
}

#[test]
fn test_recall_reports_similarity_in_semantic_mode() {
    block_on(async {
        let seeded = seeded_engine().await;
        let response = dispatch_line(
            &seeded.engine,
            r#"{"id":"recall-5","jsonrpc":"2.0","method":"recall","params":{"session_id":"sess-1","projectId":"project-a","query":"vector store"}}"#,
        )
        .await;
        assert!(
            response.error.is_none(),
            "semantic recall must succeed: {:?}",
            response.error
        );
        let result = result_of(&response);
        assert_eq!(result["count"].as_u64(), Some(2));
        let items = result["items"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        for item in items {
            assert!(
                item.get("similarity").is_some_and(Value::is_number),
                "every semantically ranked item carries a numeric similarity"
            );
        }
    });
}
