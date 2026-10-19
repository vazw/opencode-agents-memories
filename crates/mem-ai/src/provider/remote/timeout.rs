//! Per-request timeout middleware.
//!
//! Each outbound call carries *its own* budget (the trait passes
//! `timeout: Option<Duration>`); there is no shared/global timeout. The budget
//! is applied by composing the injected [`HttpTransport`] behind tower's
//! [`TimeoutLayer`], so a hung transport is abandoned deterministically and
//! surfaces as [`ProviderErrorKind::Timeout`].

use std::sync::Arc;
use std::time::Duration;

use crate::provider::error::{ProviderError, ProviderErrorKind};
use crate::provider::remote::plumbing::{HttpRequest, HttpResponse, HttpTransport};

/// Run `request` through `transport` under a tower timeout stack.
///
/// The inner transport error is preserved (downcast back out of the tower
/// `BoxError`); an elapsed timeout becomes a [`ProviderErrorKind::Timeout`].
pub async fn send_with_timeout(
    transport: Arc<dyn HttpTransport>,
    request: HttpRequest,
    budget: Duration,
    provider: &str,
) -> Result<HttpResponse, ProviderError> {
    use tower::timeout::TimeoutLayer;
    use tower::{ServiceBuilder, ServiceExt, service_fn};

    let service = ServiceBuilder::new()
        .layer(TimeoutLayer::new(budget))
        .service(service_fn(move |request: HttpRequest| {
            let transport = transport.clone();
            async move { transport.send(request).await }
        }));

    match service.oneshot(request).await {
        Ok(response) => Ok(response),
        Err(error) => Err(classify_tower_error(error, provider, budget)),
    }
}

/// Turn a tower `BoxError` back into a [`ProviderError`].
fn classify_tower_error(
    error: Box<dyn std::error::Error + Send + Sync>,
    provider: &str,
    budget: Duration,
) -> ProviderError {
    if error.is::<tower::timeout::error::Elapsed>() {
        return ProviderError::timeout(provider, budget);
    }
    match error.downcast::<ProviderError>() {
        Ok(provider_error) => *provider_error,
        Err(other) => ProviderError::network(other.to_string()).with_provider(provider),
    }
}

/// Compile-time guard that the timeout classification stays a distinct kind.
pub const TIMEOUT_KIND: ProviderErrorKind = ProviderErrorKind::Timeout;
