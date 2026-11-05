//! Discriminating measurement harness for Issue 62: unreachable endpoint latency
//! and disconnected provider terminal dead-lettering.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::{FakeEmbedder, FixedScopeResolver, RecorderTier1Writer, TestDir, unit_vector};
use mem_ai::provider::{
    BoxFuture, HostRequest, HostResponse, HostTransport, OpenCodeHostConfig, OpenCodeHostProvider,
    ProviderError, ReqwestHostTransport,
};
use mem_bridge::capture::{CaptureParams, CaptureService};
use mem_bridge::capture_prompt::{CaptureSummarizer, ProviderCaptureSummarizer};
use mem_bridge::capture_provider::Structured;
use mem_episodic::inbox::CaptureInbox;
use mem_episodic::store::EpisodicStore;

struct Mock502Transport {
    body: String,
}

impl HostTransport for Mock502Transport {
    fn send(
        &self,
        _request: HostRequest,
    ) -> BoxFuture<'static, Result<HostResponse, ProviderError>> {
        let response = HostResponse {
            status: 502,
            body: self.body.clone(),
        };
        Box::pin(std::future::ready(Ok(response)))
    }
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_measurement_unreachable_endpoint_latency_and_typing() {
    let dir = TestDir::new("measurement-unreachable");
    let store = Arc::new(EpisodicStore::new(
        dir.path().to_path_buf(),
        dir.store_config(),
    ));
    let config = OpenCodeHostConfig {
        base_url: "http://127.0.0.1:40999".to_string(),
        timeout: Duration::from_millis(100),
        ..OpenCodeHostConfig::default()
    };
    let transport = Arc::new(ReqwestHostTransport::new());
    let provider = Arc::new(OpenCodeHostProvider::new(config, transport));
    let structured = Arc::new(Structured::new(provider));
    let summarizer = Arc::new(ProviderCaptureSummarizer::new(
        structured,
        "beam-free",
        131_072,
    ));

    let capture = CaptureService::new(
        store.clone(),
        CaptureInbox::new(store),
        Arc::new(RecorderTier1Writer::new(0)),
        Arc::new(FakeEmbedder::with_vector(unit_vector(0))),
        Some(summarizer.clone()),
        Arc::new(FixedScopeResolver::default()),
        131_072,
    );

    let capture_id = "019a0000-0000-7f00-8000-00000000e001";
    let params = CaptureParams {
        session_id: "sess-measurement-1".to_string(),
        capture_id: capture_id.to_string(),
        project_id: "/srv/measurement".to_string(),
        agent_id: "agent-1".to_string(),
        transcript: Some("user: test unreachable host".to_string()),
        ..Default::default()
    };
    capture.enqueue(&params).await.expect("enqueue");

    let start = Instant::now();
    let report = capture.drain_once().await.expect("drain");
    let elapsed = start.elapsed();

    let failure = summarizer
        .last_failure()
        .expect("summarizer recorded failure");
    let attempts = capture.inbox().attempts(capture_id).await;

    println!(
        "[MEASUREMENT unreachable_endpoint] latency={:?}, reason={}, error_kind={}, retryable={}, attempts={}",
        elapsed, failure.reason, failure.error_kind, failure.retryable, attempts
    );

    assert!(
        elapsed < Duration::from_millis(100),
        "unreachable endpoint latency must be < 100ms, was {elapsed:?}"
    );
    assert_eq!(failure.reason, "summarizer_failed");
    assert_eq!(failure.error_kind, "network");
    assert!(failure.retryable);
    assert_eq!(
        attempts, 1,
        "transient network failure increments attempt by 1"
    );
    assert_eq!(report.done, 0);
    assert_eq!(report.retryable, 1);
    assert_eq!(report.dead_lettered, 0);
}

#[mem_store::tokio::test(crate = "mem_store::tokio")]
async fn test_measurement_disconnected_provider_terminal_deadletter() {
    let dir = TestDir::new("measurement-disconnected");
    let store = Arc::new(EpisodicStore::new(
        dir.path().to_path_buf(),
        dir.store_config(),
    ));
    let config = OpenCodeHostConfig {
        base_url: "http://127.0.0.1:40998".to_string(),
        ..OpenCodeHostConfig::default()
    };
    let transport = Arc::new(Mock502Transport {
        body:
            r#"{"_tag":"ProviderNotFoundError","providerID":"p","message":"Provider p not found"}"#
                .to_string(),
    });
    let provider = Arc::new(OpenCodeHostProvider::new(config, transport));
    let structured = Arc::new(Structured::new(provider));
    let summarizer = Arc::new(ProviderCaptureSummarizer::new(
        structured,
        "beam-free",
        131_072,
    ));

    let capture = CaptureService::new(
        store.clone(),
        CaptureInbox::new(store),
        Arc::new(RecorderTier1Writer::new(0)),
        Arc::new(FakeEmbedder::with_vector(unit_vector(0))),
        Some(summarizer.clone()),
        Arc::new(FixedScopeResolver::default()),
        131_072,
    );

    let capture_id = "019a0000-0000-7f00-8000-00000000e002";
    let params = CaptureParams {
        session_id: "sess-measurement-2".to_string(),
        capture_id: capture_id.to_string(),
        project_id: "/srv/measurement".to_string(),
        agent_id: "agent-1".to_string(),
        transcript: Some("user: test disconnected provider".to_string()),
        ..Default::default()
    };
    capture.enqueue(&params).await.expect("enqueue");

    let start = Instant::now();
    let report = capture.drain_once().await.expect("drain");
    let elapsed = start.elapsed();

    let failure = summarizer
        .last_failure()
        .expect("summarizer recorded failure");
    let attempts = capture.inbox().attempts(capture_id).await;

    println!(
        "[MEASUREMENT disconnected_provider] latency={:?}, reason={}, error_kind={}, retryable={}, attempts={}",
        elapsed, failure.reason, failure.error_kind, failure.retryable, attempts
    );

    assert!(
        elapsed < Duration::from_millis(10),
        "disconnected provider terminal latency must be < 10ms, was {elapsed:?}"
    );
    assert_eq!(failure.reason, "host_provider_not_found");
    assert!(!failure.retryable);
    assert_eq!(
        attempts, 0,
        "terminal provider error deadletters on attempt 0 without retries"
    );
    assert_eq!(report.done, 0);
    assert_eq!(report.retryable, 0);
    assert_eq!(report.dead_lettered, 1);
}
