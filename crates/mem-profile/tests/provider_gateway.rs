//! The real `AiProvider` adapter seam (W3.2).
//!
//! `mem-ai`'s `AiProviderStructured` is sealed to that crate, so the adapter is
//! split at the local [`StructuredCompletion`] seam: these tests drive
//! [`ProviderAnalysisGateway`] over a recording fake (proving prompt selection
//! and error mapping), and a compile-time assertion proves the production
//! `AiProviderCompletion` satisfies the same seam.
//!
//! Tests:
//! - `test_provider_gateway_uses_json_prompt_for_native`
//! - `test_provider_gateway_uses_tool_prompt_for_external_when_native_fails`
//! - `test_provider_gateway_preserves_provider_error_message`
//! - `test_provider_gateway_reports_external_unconfigured_and_never_calls_external`
//! - `test_ai_provider_completion_implements_the_structured_completion_seam`

use std::sync::{Arc, Mutex};

use mem_profile::learning_service::{AnalysisContext, AnalysisError, AnalysisGateway};
use mem_profile::provider_gateway::{
    AiProviderCompletion, ProviderAnalysisGateway, StructuredCompletion,
};
use mem_store::db::BoxFuture;
use serde_json::{Value, json};

const NATIVE_SENTENCE: &str = "opencode-host internal: provider call failed";
const EXTERNAL_UNCONFIGURED: &str = "External API not configured for user memory learning";

#[derive(Clone, Debug, PartialEq, Eq)]
struct Recorded {
    system_prompt: String,
    user_prompt: String,
}

struct FakeCompletion {
    result: Result<Value, AnalysisError>,
    recorded: Arc<Mutex<Vec<Recorded>>>,
}

impl FakeCompletion {
    fn ok(value: Value, recorded: Arc<Mutex<Vec<Recorded>>>) -> Self {
        Self {
            result: Ok(value),
            recorded,
        }
    }

    fn failing(error: AnalysisError, recorded: Arc<Mutex<Vec<Recorded>>>) -> Self {
        Self {
            result: Err(error),
            recorded,
        }
    }
}

impl StructuredCompletion for FakeCompletion {
    fn complete<'a>(
        &'a self,
        system_prompt: &'a str,
        user_prompt: &'a str,
        _schema: Value,
    ) -> BoxFuture<'a, Result<Value, AnalysisError>> {
        self.recorded.lock().expect("record lock").push(Recorded {
            system_prompt: system_prompt.to_string(),
            user_prompt: user_prompt.to_string(),
        });
        let result = self.result.clone();
        Box::pin(async move { result })
    }
}

fn context() -> AnalysisContext {
    AnalysisContext {
        prompts: Vec::new(),
        existing_profile: None,
        lang_name: "English".to_string(),
        user_prompt: "the assembled user prompt".to_string(),
        system_prompt_json: "JSON-MODE SYSTEM PROMPT".to_string(),
        system_prompt_tool: "TOOL-MODE SYSTEM PROMPT".to_string(),
    }
}

fn analysis_value() -> Value {
    json!({"preferences": [], "patterns": [], "workflows": []})
}

#[tokio::test]
async fn test_provider_gateway_uses_json_prompt_for_native() {
    let native_recorded = Arc::new(Mutex::new(Vec::new()));
    let gateway = ProviderAnalysisGateway::new(
        Arc::new(FakeCompletion::ok(
            analysis_value(),
            native_recorded.clone(),
        )),
        None,
    );

    let raw = gateway
        .native_complete(&context())
        .await
        .expect("native completion succeeds");

    let recorded = native_recorded.lock().expect("record lock");
    assert_eq!(
        recorded.as_slice(),
        &[Recorded {
            system_prompt: "JSON-MODE SYSTEM PROMPT".to_string(),
            user_prompt: "the assembled user prompt".to_string(),
        }]
    );
    // The structured value reaches the round as JSON text.
    assert_eq!(raw, analysis_value().to_string());
}

#[tokio::test]
async fn test_provider_gateway_uses_tool_prompt_for_external_when_native_fails() {
    let native_recorded = Arc::new(Mutex::new(Vec::new()));
    let external_recorded = Arc::new(Mutex::new(Vec::new()));
    let gateway = ProviderAnalysisGateway::new(
        Arc::new(FakeCompletion::failing(
            AnalysisError::Provider(NATIVE_SENTENCE.to_string()),
            native_recorded.clone(),
        )),
        Some(Arc::new(FakeCompletion::ok(
            analysis_value(),
            external_recorded.clone(),
        ))),
    );

    let raw = gateway
        .external_complete(&context())
        .await
        .expect("external completion succeeds");

    let recorded = external_recorded.lock().expect("record lock");
    assert_eq!(
        recorded.as_slice(),
        &[Recorded {
            system_prompt: "TOOL-MODE SYSTEM PROMPT".to_string(),
            user_prompt: "the assembled user prompt".to_string(),
        }]
    );
    assert_eq!(raw, analysis_value().to_string());
}

#[tokio::test]
async fn test_provider_gateway_preserves_provider_error_message() {
    let native_recorded = Arc::new(Mutex::new(Vec::new()));
    let external_recorded = Arc::new(Mutex::new(Vec::new()));
    let gateway = ProviderAnalysisGateway::new(
        Arc::new(FakeCompletion::failing(
            AnalysisError::Provider(NATIVE_SENTENCE.to_string()),
            native_recorded.clone(),
        )),
        Some(Arc::new(FakeCompletion::failing(
            AnalysisError::Provider("external provider exploded".to_string()),
            external_recorded.clone(),
        ))),
    );

    let native_error = gateway
        .native_complete(&context())
        .await
        .expect_err("native failure propagates");
    assert_eq!(
        native_error,
        AnalysisError::Provider(NATIVE_SENTENCE.to_string()),
        "the provider sentence is never wrapped or translated"
    );

    let external_error = gateway
        .external_complete(&context())
        .await
        .expect_err("external failure propagates");
    assert_eq!(
        external_error,
        AnalysisError::Provider("external provider exploded".to_string())
    );
}

#[tokio::test]
async fn test_provider_gateway_reports_external_unconfigured_and_never_calls_external() {
    let native_recorded = Arc::new(Mutex::new(Vec::new()));
    let gateway = ProviderAnalysisGateway::new(
        Arc::new(FakeCompletion::ok(
            analysis_value(),
            native_recorded.clone(),
        )),
        None,
    );

    assert!(!gateway.external_configured());
    let error = gateway
        .external_complete(&context())
        .await
        .expect_err("no external provider to call");
    assert_eq!(
        error,
        AnalysisError::Provider(EXTERNAL_UNCONFIGURED.to_string())
    );
    // The native recorder proves nothing was routed through the (absent)
    // external provider.
    assert!(
        native_recorded.lock().expect("record lock").is_empty(),
        "an unconfigured external provider must not reach the native seam"
    );

    let configured = ProviderAnalysisGateway::new(
        Arc::new(FakeCompletion::ok(analysis_value(), native_recorded)),
        Some(Arc::new(FakeCompletion::ok(
            analysis_value(),
            Arc::new(Mutex::new(Vec::new())),
        ))),
    );
    assert!(configured.external_configured());
}

#[test]
fn test_ai_provider_completion_implements_the_structured_completion_seam() {
    // Compile-time proof that the production adapter is usable in the seam the
    // gateway holds (the provider itself is sealed to `mem-ai`).
    fn assert_seam<T: StructuredCompletion + Send + Sync + 'static>() {}
    assert_seam::<AiProviderCompletion>();
}
