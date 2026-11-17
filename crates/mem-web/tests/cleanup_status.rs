//! `GET /api/cleanup/status` (issue 41): the four facts, their wire names, and
//! the degraded composition.
//!
//! Every case drives the real router, handler, auth gate and DTO. Only the port
//! is a fake, so a field the handler drops — or a `skip_serializing_if` that
//! makes an absent stamp vanish instead of answering `null` — reds here.
//!
//! Six cases over five fixtures: the interval and the running state share one
//! fixture, because they are two facts of the same answer.

mod common;

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use common::{RawResponse, api_request, app_state_with_api};
use mem_web::api::ports::{
    ApiServices, CleanupStatus, DedupReport, MaintenancePort, MigrationReport, MigrationRunReport,
    PortError, TagMigrationProgressView, TagMigrationReport,
};
use mem_web::server::build_router;
use mem_web::web_auth::WebAuth;

const TOKEN: &str = "test-token";

/// A maintenance port that answers exactly one `CleanupStatus`, or
/// `Unavailable` — nothing else is reached by this endpoint.
struct StatusPort(Option<CleanupStatus>);

#[async_trait]
impl MaintenancePort for StatusPort {
    async fn detect_migration(&self) -> Result<MigrationReport, PortError> {
        Err(PortError::Unavailable)
    }

    async fn run_migration(&self, _strategy: &str) -> Result<MigrationRunReport, PortError> {
        Err(PortError::Unavailable)
    }

    async fn detect_tag_migration(&self) -> Result<TagMigrationReport, PortError> {
        Err(PortError::Unavailable)
    }

    async fn tag_migration_progress(&self) -> Result<TagMigrationProgressView, PortError> {
        Err(PortError::Unavailable)
    }

    async fn tag_migration_records(
        &self,
    ) -> Result<Vec<mem_core::tags::migration::MemoryRecord>, PortError> {
        Err(PortError::Unavailable)
    }

    async fn generate_tags(&self, _memory_id: &str, _content: &str) -> Result<Vec<String>, String> {
        Err("unavailable".to_string())
    }

    async fn run_tag_migration_batch(
        &self,
        _batch_size: usize,
        _records: Vec<mem_core::tags::migration::MemoryRecord>,
        _tags: std::collections::HashMap<String, Result<Vec<String>, String>>,
    ) -> Result<mem_core::tags::migration::BatchOutcome, PortError> {
        Err(PortError::Unavailable)
    }

    async fn deduplicate(&self) -> Result<DedupReport, PortError> {
        Err(PortError::Unavailable)
    }

    async fn cleanup_status(&self) -> Result<CleanupStatus, PortError> {
        self.0.ok_or(PortError::Unavailable)
    }
}

fn router_with(services: ApiServices) -> axum::Router {
    let state = app_state_with_api(
        WebAuth::new(None, None, "user"),
        Some(TOKEN),
        None,
        PathBuf::from("/tmp/unused-dist"),
        common::throwaway_log_dir(),
        services,
        "unknown",
        "127.0.0.1",
    );
    build_router(Arc::new(state))
}

fn router(status: CleanupStatus) -> axum::Router {
    router_with(ApiServices::unavailable().with_maintenance(Arc::new(StatusPort(Some(status)))))
}

async fn get_status(router: axum::Router) -> RawResponse {
    api_request(router, "GET", "/api/cleanup/status", Some(TOKEN), None).await
}

/// The whole `data` object, in declaration order: a renamed field, a field the
/// handler forgets to copy, or `skip_serializing_if` on `last_run_ms` all red
/// this single assertion — which is why it is exact rather than per-key.
#[tokio::test]
async fn test_status_returns_the_four_fields() {
    let response = get_status(router(CleanupStatus {
        enabled: false,
        interval_seconds: 7_200,
        last_run_ms: None,
        running: true,
    }))
    .await;
    assert_eq!(response.status, 200, "{}", response.body_string());
    let body = response.body_string();
    assert!(
        body.contains(
            r#""data":{"enabled":false,"intervalSeconds":7200,"lastRunMs":null,"running":true}"#
        ),
        "the four facts, under their wire names, with a null lastRunMs: {body}"
    );
}

/// A hardcoded `enabled:false` in the DTO reds here.
#[tokio::test]
async fn test_status_reports_an_enabled_cleanup() {
    let response = get_status(router(CleanupStatus {
        enabled: true,
        interval_seconds: 86_400,
        last_run_ms: None,
        running: false,
    }))
    .await;
    assert_eq!(response.status, 200, "{}", response.body_string());
    assert!(
        response.body_string().contains(r#""enabled":true"#),
        "an enabled cleanup must report true: {}",
        response.body_string()
    );
}

/// A hardcoded `lastRunMs: null` reds here.
#[tokio::test]
async fn test_status_reports_the_last_run() {
    let response = get_status(router(CleanupStatus {
        enabled: true,
        interval_seconds: 86_400,
        last_run_ms: Some(1_700_000_000_000),
        running: false,
    }))
    .await;
    assert_eq!(response.status, 200, "{}", response.body_string());
    assert!(
        response
            .body_string()
            .contains(r#""lastRunMs":1700000000000"#),
        "the persisted stamp must reach the wire: {}",
        response.body_string()
    );
}

/// The configured interval, not `DEFAULTS`' 86 400: a handler that reaches for
/// the default constant reds on 7 200.
#[tokio::test]
async fn test_status_reports_the_configured_interval() {
    let response = get_status(router(CleanupStatus {
        enabled: true,
        interval_seconds: 7_200,
        last_run_ms: None,
        running: true,
    }))
    .await;
    assert_eq!(response.status, 200, "{}", response.body_string());
    assert!(
        response.body_string().contains(r#""intervalSeconds":7200"#),
        "the configured interval must reach the wire: {}",
        response.body_string()
    );
}

/// A held guard, as the same fixture as the interval case: a hardcoded
/// `running:false` reds here.
#[tokio::test]
async fn test_status_reports_running_state() {
    let response = get_status(router(CleanupStatus {
        enabled: true,
        interval_seconds: 7_200,
        last_run_ms: None,
        running: true,
    }))
    .await;
    assert_eq!(response.status, 200, "{}", response.body_string());
    assert!(
        response.body_string().contains(r#""running":true"#),
        "a sweep in flight must report running: {}",
        response.body_string()
    );
}

/// The documented degrade: a build with no maintenance port answers 501 and
/// never invents defaults for the four facts.
#[tokio::test]
async fn test_status_is_501_when_the_port_is_unavailable() {
    let response = get_status(router_with(ApiServices::unavailable())).await;
    assert_eq!(
        response.status,
        501,
        "the unavailable composition answers 501: {}",
        response.body_string()
    );
    assert!(
        response
            .body_string()
            .contains("the endpoint is not available in this build"),
        "the 501 carries the documented message: {}",
        response.body_string()
    );
}
