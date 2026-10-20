//! New coverage — no upstream test file exists for the pure decay kernel.
//!
//! Behavioural origin: upstream `user-profile-manager.ts::syncConfidence`
//! (`timeFactor`) and `lazyMigrateAlpha`, plus the PRD §Phase 3 W3.3 locks
//! (confidence EMA α = 0.3; exponential half-life decay with a 30-day base).
//! The kernel is pure — no provider, no DB, no clock reads — so every value
//! here is computed from an explicit argument.
//!
//! Tests:
//! - `test_ema_update_weights_the_new_observation_by_alpha`
//! - `test_ema_update_alpha_bounds_keep_the_prior_or_adopt_the_observation`
//! - `test_half_life_scales_with_frequency`
//! - `test_decay_factor_is_one_inside_the_grace_threshold`
//! - `test_decay_factor_halves_at_one_half_life_past_the_threshold`
//! - `test_decay_factor_matches_the_closed_form_at_half_a_half_life`
//! - `test_is_stale_requires_both_a_stale_age_and_insufficient_evidence`
//! - `test_sweep_is_due_once_per_interval_with_an_injected_clock`

use mem_profile::decay::{
    CONFIDENCE_EMA_ALPHA, HALF_LIFE_DAYS, MILLIS_PER_DAY, SWEEP_INTERVAL_MILLIS, decay_factor,
    ema_update, half_life_millis, is_stale, sweep_is_due,
};

const EPS: f64 = 1e-9;

#[test]
fn test_ema_update_weights_the_new_observation_by_alpha() {
    // 0.3 * 0.9 + 0.7 * 0.4 = 0.55
    let blended = ema_update(0.4, 0.9, CONFIDENCE_EMA_ALPHA);
    assert!(
        (blended - 0.55).abs() < EPS,
        "alpha-weighted blend, got {blended}"
    );
    assert!(
        (CONFIDENCE_EMA_ALPHA - 0.3).abs() < EPS,
        "the PRD lock is alpha = 0.3, got {CONFIDENCE_EMA_ALPHA}"
    );
    // The new observation moves the value toward itself, never past it.
    assert!(blended > 0.4 && blended < 0.9);
    assert!((ema_update(0.42, 0.42, CONFIDENCE_EMA_ALPHA) - 0.42).abs() < EPS);
}

#[test]
fn test_ema_update_alpha_bounds_keep_the_prior_or_adopt_the_observation() {
    assert!(
        (ema_update(0.2, 0.9, 0.0) - 0.2).abs() < EPS,
        "alpha = 0 keeps the prior"
    );
    assert!(
        (ema_update(0.2, 0.9, 1.0) - 0.9).abs() < EPS,
        "alpha = 1 adopts the observation"
    );
}

#[test]
fn test_half_life_scales_with_frequency() {
    // upstream: halfLife = decayDays * day * (1 + log2(1 + frequency))
    let frequency_one = half_life_millis(HALF_LIFE_DAYS, 1.0);
    assert!(
        (frequency_one - 60.0 * MILLIS_PER_DAY).abs() < 1.0,
        "frequency 1 doubles the base half-life (30d -> 60d), got {frequency_one}"
    );
    let frequency_three = half_life_millis(HALF_LIFE_DAYS, 3.0);
    assert!(
        (frequency_three - 90.0 * MILLIS_PER_DAY).abs() < 1.0,
        "frequency 3 triples the base half-life (30d -> 90d), got {frequency_three}"
    );
}

#[test]
fn test_decay_factor_is_one_inside_the_grace_threshold() {
    let threshold = HALF_LIFE_DAYS * MILLIS_PER_DAY;
    for age in [0.0, 1.0 * MILLIS_PER_DAY, threshold] {
        let factor = decay_factor(age, HALF_LIFE_DAYS, 1.0);
        assert!(
            (factor - 1.0).abs() < EPS,
            "no decay before the 30-day grace threshold (age {age}), got {factor}"
        );
    }
}

#[test]
fn test_decay_factor_halves_at_one_half_life_past_the_threshold() {
    let threshold = HALF_LIFE_DAYS * MILLIS_PER_DAY;
    // frequency 1 => half-life 60 days; one half-life beyond the threshold.
    let factor = decay_factor(threshold + 60.0 * MILLIS_PER_DAY, HALF_LIFE_DAYS, 1.0);
    assert!(
        (factor - 0.5).abs() < 1e-6,
        "one half-life past the threshold halves the factor, got {factor}"
    );
}

#[test]
fn test_decay_factor_matches_the_closed_form_at_half_a_half_life() {
    let threshold = HALF_LIFE_DAYS * MILLIS_PER_DAY;
    // frequency 1 => half-life 60 days; half a half-life past the threshold.
    let factor = decay_factor(threshold + 30.0 * MILLIS_PER_DAY, HALF_LIFE_DAYS, 1.0);
    let expected = (-std::f64::consts::LN_2 * 0.5).exp();
    assert!(
        (factor - expected).abs() < 1e-9,
        "exp(-ln2 * age/halfLife), expected {expected}, got {factor}"
    );
}

#[test]
fn test_is_stale_requires_both_a_stale_age_and_insufficient_evidence() {
    let day = MILLIS_PER_DAY;
    assert!(
        is_stale(3.0 * day, 2.0, 1, 3),
        "stale age AND short evidence is stale"
    );
    assert!(
        !is_stale(3.0 * day, 2.0, 3, 3),
        "enough evidence is retained even when stale"
    );
    assert!(
        !is_stale(3.0 * day, 2.0, 4, 3),
        "more evidence is retained even when stale"
    );
    assert!(
        !is_stale(1.0 * day, 2.0, 1, 3),
        "a fresh low-evidence trait is kept"
    );
    assert!(
        !is_stale(2.0 * day, 2.0, 1, 3),
        "the stale age comparison is strict (age == stale_days is not stale)"
    );
}

#[test]
fn test_sweep_is_due_once_per_interval_with_an_injected_clock() {
    assert!(sweep_is_due(None, 0, SWEEP_INTERVAL_MILLIS), "never swept");
    assert!(
        !sweep_is_due(
            Some(1_000),
            1_000 + SWEEP_INTERVAL_MILLIS - 1,
            SWEEP_INTERVAL_MILLIS
        ),
        "still inside the interval"
    );
    assert!(
        sweep_is_due(
            Some(1_000),
            1_000 + SWEEP_INTERVAL_MILLIS,
            SWEEP_INTERVAL_MILLIS
        ),
        "exactly one interval later is due"
    );
    assert!(
        sweep_is_due(
            Some(1_000),
            1_000 + 3 * SWEEP_INTERVAL_MILLIS,
            SWEEP_INTERVAL_MILLIS
        ),
        "well past the interval is due"
    );
    assert_eq!(SWEEP_INTERVAL_MILLIS, 86_400_000, "daily interval");
}
