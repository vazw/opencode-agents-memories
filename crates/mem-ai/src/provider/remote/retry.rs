//! Retry policy: exponential backoff with full jitter.
//!
//! Only transient failures are retried — the classification lives on
//! [`ProviderError::is_retryable`] (429, 5xx, network, timeout); auth and
//! client errors (401/403/404) never are. The delay is `base * 2^(attempt-1)`
//! capped at `max_delay`, then full-jittered into `[0, cap]` so a fleet of
//! callers does not stampede the provider in lockstep.

use std::time::Duration;

use crate::provider::error::ProviderError;

/// Retry configuration shared by every remote provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Total attempts including the first (upstream parity: 3).
    pub max_attempts: u32,
    /// First backoff step.
    pub base_delay: Duration,
    /// Backoff ceiling.
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(2),
        }
    }
}

impl RetryPolicy {
    /// A policy with the given attempt ceiling and backoff bounds.
    pub fn new(max_attempts: u32, base_delay: Duration, max_delay: Duration) -> Self {
        Self {
            max_attempts: max_attempts.max(1),
            base_delay,
            max_delay,
        }
    }

    /// Whether `error` may be retried at all.
    pub fn should_retry(&self, error: &ProviderError) -> bool {
        error.is_retryable()
    }

    /// The jittered delay before retry number `attempt` (1-based).
    ///
    /// `entropy` is any caller-supplied value (a counter, a clock) — the delay
    /// is deterministic given `(attempt, entropy)`, which keeps it testable.
    pub fn delay(&self, attempt: u32, entropy: u64) -> Duration {
        if self.base_delay.is_zero() {
            return Duration::ZERO;
        }
        let shift = attempt.saturating_sub(1).min(32);
        let scaled = self
            .base_delay
            .checked_mul(2u32.saturating_pow(shift))
            .unwrap_or(self.max_delay);
        let cap = scaled.min(self.max_delay);
        let cap_ms = cap.as_millis() as u64;
        if cap_ms == 0 {
            return Duration::ZERO;
        }
        // xorshift64* — cheap, dependency-free full jitter.
        let mut x = entropy | 1;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        let jittered = (x.wrapping_mul(0x2545_F491_4F6C_DD1D)) % (cap_ms + 1);
        Duration::from_millis(jittered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::error::ProviderError;

    #[test]
    fn retries_only_transient_failures() {
        let policy = RetryPolicy::default();
        assert!(policy.should_retry(&ProviderError::from_status("p", 429, "slow")));
        assert!(policy.should_retry(&ProviderError::from_status("p", 503, "down")));
        assert!(policy.should_retry(&ProviderError::timeout("p", Duration::from_secs(1))));
        // Non-retryable: auth + client errors.
        assert!(!policy.should_retry(&ProviderError::from_status("p", 401, "no")));
        assert!(!policy.should_retry(&ProviderError::from_status("p", 403, "no")));
        assert!(!policy.should_retry(&ProviderError::from_status("p", 404, "gone")));
    }

    #[test]
    fn backoff_is_exponential_and_capped() {
        let policy = RetryPolicy::new(5, Duration::from_millis(100), Duration::from_millis(400));
        // cap for attempt 3 is min(400, 100*4)=400ms; jittered value in [0, 400].
        let delay = policy.delay(3, 12345);
        assert!(delay <= Duration::from_millis(400));
        // attempt 1 cap is 100ms.
        assert!(policy.delay(1, 999) <= Duration::from_millis(100));
        // zero base delay disables waiting (used by fast tests).
        assert_eq!(
            RetryPolicy::new(3, Duration::ZERO, Duration::ZERO).delay(1, 7),
            Duration::ZERO
        );
    }

    #[test]
    fn default_attempt_ceiling_is_three() {
        assert_eq!(RetryPolicy::default().max_attempts, 3);
    }
}
