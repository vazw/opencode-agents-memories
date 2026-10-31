//! Issue 70 wave 1 — the `export`/`import` wire methods (T30-T35).

mod common;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use common::{FakeEmbedder, MapScopeResolver, TestDir, build_wire_fixture, unit_vector};
use mem_bridge::jsonrpc::{Response, parse_line};
use mem_bridge::tools::portability::{
    AbortKind, ExportOutcome, ExportRequest, ImportOutcome, ImportRequest, PortabilityBackend,
    PortabilityFault, RowNote,
};
use serde_json::{Value, json};

const HASH: &str = "c1c1c1c1c1c1c1c1";

/// One recorded backend call.
#[derive(Debug, Clone, PartialEq)]
enum Call {
    Export {
        project_path: String,
        output_path: PathBuf,
    },
    Import {
        project_path: String,
        input_path: PathBuf,
        dry_run: bool,
    },
}

/// The recording fake: each method takes its queued outcome and records the
/// exact request the bridge handed over.
#[derive(Default)]
struct FakeBackend {
    calls: Mutex<Vec<Call>>,
    export_outcome: Mutex<Option<Result<ExportOutcome, PortabilityFault>>>,
    import_outcome: Mutex<Option<Result<ImportOutcome, PortabilityFault>>>,
}

impl FakeBackend {
    fn with_export(outcome: Result<ExportOutcome, PortabilityFault>) -> Arc<Self> {
        let backend = FakeBackend::default();
        *backend.export_outcome.lock().expect("export outcome") = Some(outcome);
        Arc::new(backend)
    }

    fn with_import(outcome: Result<ImportOutcome, PortabilityFault>) -> Arc<Self> {
        let backend = FakeBackend::default();
        *backend.import_outcome.lock().expect("import outcome") = Some(outcome);
        Arc::new(backend)
    }

    fn calls(&self) -> Vec<Call> {
        self.calls.lock().expect("calls").clone()
    }
}

impl PortabilityBackend for FakeBackend {
    fn export<'a>(
        &'a self,
        request: &'a ExportRequest,
    ) -> mem_embed::embedder::BoxFuture<'a, Result<ExportOutcome, PortabilityFault>> {
        self.calls.lock().expect("calls").push(Call::Export {
            project_path: request.project_path.clone(),
            output_path: request.output_path.clone(),
        });
        let outcome = self
            .export_outcome
            .lock()
            .expect("export outcome")
            .take()
            .unwrap_or(Ok(ExportOutcome {
                container_tag: format!("opencode_project_{HASH}"),
                count: 0,
                scope_hash: HASH.to_string(),
            }));
        Box::pin(async move { outcome })
    }

    fn import<'a>(
        &'a self,
        request: &'a ImportRequest,
    ) -> mem_embed::embedder::BoxFuture<'a, Result<ImportOutcome, PortabilityFault>> {
        self.calls.lock().expect("calls").push(Call::Import {
            project_path: request.project_path.clone(),
            input_path: request.input_path.clone(),
            dry_run: request.dry_run,
        });
        let outcome = self
            .import_outcome
            .lock()
            .expect("import outcome")
            .take()
            .unwrap_or(Ok(ImportOutcome {
                container_tag: format!("opencode_project_{HASH}"),
                dry_run: request.dry_run,
                imported: 0,
                skipped: Vec::new(),
                rejected: Vec::new(),
            }));
        Box::pin(async move { outcome })
    }
}

async fn fixture_with(
    prefix: &str,
    portability: Option<Arc<dyn PortabilityBackend>>,
) -> common::WireFixture {
    let resolver = Arc::new(MapScopeResolver::single(
        "/probe/a",
        MapScopeResolver::project_tag(HASH),
        Some("probe@example.com"),
    ));
    build_wire_fixture(
        TestDir::new(prefix),
        resolver,
        Arc::new(FakeEmbedder::with_vector(unit_vector(0))),
        portability,
    )
    .await
}

async fn dispatch(engine: &mem_bridge::engine::Engine, method: &str, params: Value) -> Response {
    let request = parse_line(
        &json!({"id": "m", "jsonrpc": "2.0", "method": method, "params": params}).to_string(),
    )
    .expect("portability request parses");
    engine.dispatch(&request).await
}

fn error_code(response: &Response) -> i32 {
    response
        .error
        .as_ref()
        .map(|error| error.code)
        .expect("error envelope")
}

fn data(response: &Response) -> &Value {
    response
        .error
        .as_ref()
        .and_then(|error| error.data.as_ref())
        .expect("advisory data")
}

fn result(response: &Response) -> &Value {
    response.result.as_ref().expect("result envelope")
}

// ---------------------------------------------------------------------------
// T30 — absolute `.json` paths only, checked before the backend runs
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_export_and_import_require_an_absolute_json_path() {
    let backend = FakeBackend::with_export(Ok(ExportOutcome {
        container_tag: format!("opencode_project_{HASH}"),
        count: 0,
        scope_hash: HASH.to_string(),
    }));
    let fixture = fixture_with("wire-port-path", Some(backend.clone())).await;

    let export_cases = ["out.json", "/tmp/out.txt"];
    for path in export_cases {
        let response = dispatch(
            &fixture.engine,
            "export",
            json!({
                "session_id": "s",
                "project_id": "/probe/a",
                "output_path": path,
            }),
        )
        .await;
        assert_eq!(error_code(&response), -32602, "{path} is refused");
        assert_eq!(
            data(&response).get("reason").and_then(Value::as_str),
            Some("path")
        );
    }
    let import_cases = ["in.json", "/tmp/in.txt"];
    for path in import_cases {
        let response = dispatch(
            &fixture.engine,
            "import",
            json!({
                "session_id": "s",
                "project_id": "/probe/a",
                "input_path": path,
            }),
        )
        .await;
        assert_eq!(error_code(&response), -32602, "{path} is refused");
        assert_eq!(
            data(&response).get("reason").and_then(Value::as_str),
            Some("path")
        );
    }
    assert!(
        backend.calls().is_empty(),
        "the path rules are enforced before the backend runs"
    );
}

// ---------------------------------------------------------------------------
// T31 — the client's absolute path is handed over verbatim
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_export_hands_the_backend_the_absolute_output_path() {
    let backend = FakeBackend::with_export(Ok(ExportOutcome {
        container_tag: format!("opencode_project_{HASH}"),
        count: 3,
        scope_hash: HASH.to_string(),
    }));
    let fixture = fixture_with("wire-port-export", Some(backend.clone())).await;

    let response = dispatch(
        &fixture.engine,
        "export",
        json!({
            "session_id": "s",
            "project_id": "/probe/a",
            "output_path": "/var/tmp/x.json",
        }),
    )
    .await;
    let result = result(&response);
    assert_eq!(result["output_path"], "/var/tmp/x.json");
    assert_eq!(result["count"], 3);
    assert_eq!(result["scope_hash"], HASH);
    assert_eq!(
        backend.calls(),
        vec![Call::Export {
            project_path: "/probe/a".to_string(),
            output_path: PathBuf::from("/var/tmp/x.json"),
        }],
        "the path is never re-joined under the storage root"
    );
}

// ---------------------------------------------------------------------------
// T32 — `dry_run` and the input path as the backend sees them
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_import_passes_the_dry_run_flag_and_the_absolute_input_path() {
    let backend = FakeBackend::with_import(Ok(ImportOutcome {
        container_tag: format!("opencode_project_{HASH}"),
        dry_run: true,
        imported: 2,
        skipped: vec![RowNote {
            id: "skip-1".to_string(),
            reason: "empty".to_string(),
        }],
        rejected: Vec::new(),
    }));
    let fixture = fixture_with("wire-port-import", Some(backend.clone())).await;

    let response = dispatch(
        &fixture.engine,
        "import",
        json!({
            "session_id": "s",
            "project_id": "/probe/a",
            "input_path": "/var/tmp/in.json",
            "dry_run": true,
        }),
    )
    .await;
    let result = result(&response);
    assert_eq!(result["dry_run"], true);
    assert_eq!(result["imported"], 2);
    assert_eq!(result["skipped"][0]["id"], "skip-1");
    assert_eq!(
        backend.calls(),
        vec![Call::Import {
            project_path: "/probe/a".to_string(),
            input_path: PathBuf::from("/var/tmp/in.json"),
            dry_run: true,
        }]
    );

    // Omitted `dry_run` defaults to false.
    let backend = FakeBackend::with_import(Ok(ImportOutcome {
        container_tag: format!("opencode_project_{HASH}"),
        dry_run: false,
        imported: 0,
        skipped: Vec::new(),
        rejected: Vec::new(),
    }));
    let fixture = fixture_with("wire-port-import-default", Some(backend.clone())).await;
    let response = dispatch(
        &fixture.engine,
        "import",
        json!({
            "session_id": "s",
            "project_id": "/probe/a",
            "input_path": "/var/tmp/in.json",
        }),
    )
    .await;
    assert!(response.result.is_some());
    assert_eq!(
        backend.calls(),
        vec![Call::Import {
            project_path: "/probe/a".to_string(),
            input_path: PathBuf::from("/var/tmp/in.json"),
            dry_run: false,
        }]
    );
}

// ---------------------------------------------------------------------------
// T33 — no backend is a degrade, never an empty success
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_portability_degrades_without_a_backend() {
    let fixture = fixture_with("wire-port-none", None).await;
    let response = dispatch(
        &fixture.engine,
        "export",
        json!({
            "session_id": "s",
            "project_id": "/probe/a",
            "output_path": "/var/tmp/x.json",
        }),
    )
    .await;
    assert_eq!(error_code(&response), -32001);
    assert!(response.result.is_none(), "no fabricated empty export");

    let response = dispatch(
        &fixture.engine,
        "import",
        json!({
            "session_id": "s",
            "project_id": "/probe/a",
            "input_path": "/var/tmp/in.json",
        }),
    )
    .await;
    assert_eq!(error_code(&response), -32001);
}

// ---------------------------------------------------------------------------
// T34 — every typed fault reason, both abort kinds
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_import_surfaces_every_typed_fault_reason() {
    struct Case {
        fault: PortabilityFault,
        reason: &'static str,
    }

    let cases = vec![
        Case {
            fault: PortabilityFault::FileNotFound("/var/tmp/in.json".to_string()),
            reason: "file_not_found",
        },
        Case {
            fault: PortabilityFault::InvalidJson("bad document".to_string()),
            reason: "invalid_json",
        },
        Case {
            fault: PortabilityFault::UnsupportedVersion(9),
            reason: "unsupported_version",
        },
        Case {
            fault: PortabilityFault::InvalidBundle("missing source".to_string()),
            reason: "invalid_bundle",
        },
        Case {
            fault: PortabilityFault::TooLarge,
            reason: "too_large",
        },
        Case {
            fault: PortabilityFault::UnsafePath("/var/tmp/link.json".to_string()),
            reason: "unsafe_path",
        },
        Case {
            fault: PortabilityFault::InvalidScope("bad tag".to_string()),
            reason: "invalid_scope",
        },
        Case {
            fault: PortabilityFault::ImportAborted {
                kind: AbortKind::Memory,
                count: 1,
                rejected: vec![RowNote {
                    id: "dup-id".to_string(),
                    reason: "memory id already exists in target project".to_string(),
                }],
            },
            reason: "import_aborted",
        },
    ];

    for case in cases {
        let fixture = fixture_with(
            "wire-port-fault",
            Some(FakeBackend::with_import(Err(case.fault))),
        )
        .await;
        let response = dispatch(
            &fixture.engine,
            "import",
            json!({
                "session_id": "s",
                "project_id": "/probe/a",
                "input_path": "/var/tmp/in.json",
            }),
        )
        .await;
        assert_eq!(
            error_code(&response),
            -32602,
            "fault {} must ride -32602",
            case.reason
        );
        assert_eq!(
            data(&response).get("reason").and_then(Value::as_str),
            Some(case.reason)
        );
    }

    // The memory abort carries its kind, count and rejected rows.
    let fixture = fixture_with(
        "wire-port-abort-memory",
        Some(FakeBackend::with_import(Err(
            PortabilityFault::ImportAborted {
                kind: AbortKind::Memory,
                count: 1,
                rejected: vec![RowNote {
                    id: "dup-id".to_string(),
                    reason: "memory id already exists in target project".to_string(),
                }],
            },
        ))),
    )
    .await;
    let response = dispatch(
        &fixture.engine,
        "import",
        json!({
            "session_id": "s",
            "project_id": "/probe/a",
            "input_path": "/var/tmp/in.json",
        }),
    )
    .await;
    let abort = data(&response);
    assert_eq!(abort.get("kind").and_then(Value::as_str), Some("memory"));
    assert_eq!(abort.get("count"), Some(&json!(1)));
    assert_eq!(abort["rejected"][0]["id"], "dup-id");

    // The milestone abort shares the arm and changes only `kind`.
    let fixture = fixture_with(
        "wire-port-abort-milestone",
        Some(FakeBackend::with_import(Err(
            PortabilityFault::ImportAborted {
                kind: AbortKind::Milestone,
                count: 1,
                rejected: Vec::new(),
            },
        ))),
    )
    .await;
    let response = dispatch(
        &fixture.engine,
        "import",
        json!({
            "session_id": "s",
            "project_id": "/probe/a",
            "input_path": "/var/tmp/in.json",
        }),
    )
    .await;
    let abort = data(&response);
    assert_eq!(abort.get("kind").and_then(Value::as_str), Some("milestone"));
    assert_eq!(abort.get("count"), Some(&json!(1)));
}

// ---------------------------------------------------------------------------
// T35 — the internal class never echoes detail
// ---------------------------------------------------------------------------

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_store_faults_become_internal_without_echoing_detail() {
    let fixture = fixture_with(
        "wire-port-store",
        Some(FakeBackend::with_import(Err(PortabilityFault::Store))),
    )
    .await;
    let response = dispatch(
        &fixture.engine,
        "import",
        json!({
            "session_id": "s",
            "project_id": "/probe/a",
            "input_path": "/var/tmp/in.json",
        }),
    )
    .await;
    let error = response.error.as_ref().expect("error envelope");
    assert_eq!(error.code, -32603);
    // The generic reason token is the ONLY advisory member: a fixed string, so
    // the store's own message (which may name a path or a table) never leaks.
    // A reason-less reject would be reported by the wrapper as
    // `engine_unavailable`, i.e. "the engine is gone".
    assert_eq!(
        error.data,
        Some(json!({ "reason": "internal" })),
        "the store fault carries the generic token and nothing else"
    );
    assert_eq!(error.message, "Internal error");
}
