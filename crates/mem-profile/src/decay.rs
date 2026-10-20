//! Pure confidence/decay kernel (W3.3).
//!
//! The PRD §Phase 3 W3.3 locks live here as named constants so no call site
//! carries a magic literal:
//!
//! - **Confidence** — "exponential moving average (α = 0.3)"
//!   ([`CONFIDENCE_EMA_ALPHA`]);
//! - **Decay** — "exponential half-life (30 days)" ([`HALF_LIFE_DAYS`]).
//!
//! Behavioural origin: upstream `user-profile-manager.ts::syncConfidence`
//! (`timeFactor`, including the 30-day grace threshold and the frequency
//! scaling `halfLife = decayThreshold * (1 + log2(1 + freq))`) and
//! `lazyMigrateAlpha`.
//!
//! Everything here is pure: no provider, no DB, and **no clock reads** — the
//! caller passes `now`/`age` explicitly (injected-clock discipline).

/// PRD §Phase 3 W3.3: "Confidence: exponential moving average (α=0.3)".
pub const CONFIDENCE_EMA_ALPHA: f64 = 0.3;

/// PRD §Phase 3 W3.3: "Decay: exponential half-life (30 days)".
///
/// This is the *base* half-life: a frequency-1 trait gets
/// `HALF_LIFE_DAYS * (1 + log2(2))` = 60 days, i.e. upstream's frequency
/// scaling is retained on top of the PRD's base (R2).
pub const HALF_LIFE_DAYS: f64 = 30.0;

/// Milliseconds in one day (upstream's `24 * 60 * 60 * 1000`).
pub const MILLIS_PER_DAY: f64 = 86_400_000.0;

/// The daily sweep interval (R5): the store-held gate between two persisted
/// decay sweeps.
pub const SWEEP_INTERVAL_MILLIS: i64 = 86_400_000;

/// The PRD's exponential moving average: `α·observation + (1 − α)·previous`.
///
/// No clamping — callers own the range, and only the two boundary values
/// (`0.0`, `1.0`) are meaningful here.
pub fn ema_update(previous: f64, observation: f64, alpha: f64) -> f64 {
    alpha * observation + (1.0 - alpha) * previous
}

/// Upstream `decayThreshold * (1 + Math.log2(1 + freq))` in milliseconds: the
/// effective half-life grows with how often the trait has been observed.
pub fn half_life_millis(decay_days: f64, frequency: f64) -> f64 {
    decay_days * MILLIS_PER_DAY * (1.0 + (1.0 + frequency).log2())
}

/// Upstream `timeFactor`: `exp(-ln2 · max(0, age − decayThreshold) / halfLife)`.
///
/// Returns exactly `1.0` for any age inside the `decay_days` grace threshold —
/// no decay happens before it.
pub fn decay_factor(age_millis: f64, decay_days: f64, frequency: f64) -> f64 {
    let past_threshold = (age_millis - decay_days * MILLIS_PER_DAY).max(0.0);
    (-std::f64::consts::LN_2 * past_threshold / half_life_millis(decay_days, frequency)).exp()
}

/// Upstream's staleness rule (`ageDays > staleDays && evidenceCount <
/// minEvidence`): a one-off trait fades, an evidenced one never does.
pub fn is_stale(
    age_millis: f64,
    stale_days: f64,
    evidence_count: usize,
    min_evidence: usize,
) -> bool {
    age_millis / MILLIS_PER_DAY > stale_days && evidence_count < min_evidence
}

/// Whether the persisted daily sweep is due.
///
/// The clock is injected and monotonic by construction: no wall-clock access
/// anywhere in this module, so a sweep can be driven deterministically.
pub fn sweep_is_due(last_sweep_millis: Option<i64>, now_millis: i64, interval_millis: i64) -> bool {
    last_sweep_millis.is_none_or(|last| now_millis - last >= interval_millis)
}
