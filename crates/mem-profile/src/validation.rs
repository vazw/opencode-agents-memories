//! `validation` — port of upstream
//! `src/services/ai/validators/user-profile-validator.ts` plus the PRD's
//! net-new trait-content bounds (R8).
//!
//! Every upstream error sentence is reproduced byte-for-byte. R8 adds a trait
//! *type* enum ([`TraitType`]) and a 3–500 **character** content bound, both
//! enforced inside [`UserProfileValidator::validate`] — so a provider payload
//! that violates them is rejected before any merge — and at
//! [`validate_analysis_value`], which turns a payload fault into a
//! parse-class [`ProfileError::Save`] (never fallback-eligible, never carrying
//! a provider message).
//!
//! The validator is **total**: untrusted model output can never panic it, and
//! a rejected payload yields no `data`, so nothing downstream can be rewritten
//! from a partially parsed response.

use serde_json::Value;

use crate::error::ProfileError;
use crate::profile_store::{ProfileData, ProfileUpdates};

/// Upstream `USER_PROFILE_LLM_CONFIDENCE_MAX` — the maximum the analysis tool
/// schema advertises and the validator enforces.
pub const USER_PROFILE_LLM_CONFIDENCE_MAX: f64 = 1.0;

/// PRD §Phase 3 W3.3: "Validation: trait type enum + content length bounds
/// (3–500 chars)" — inclusive lower bound.
pub const TRAIT_MIN_CHARS: usize = 3;

/// PRD §Phase 3 W3.3 — inclusive upper bound.
pub const TRAIT_MAX_CHARS: usize = 500;

/// The three profile sections a trait id can belong to (R8's type enum).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraitType {
    Preference,
    Pattern,
    Workflow,
}

impl TraitType {
    /// Upstream `itemTypeFromId`: the synthetic id's prefix selects the section.
    pub fn from_id_prefix(id: &str) -> Option<TraitType> {
        if id.starts_with("pref_") {
            Some(TraitType::Preference)
        } else if id.starts_with("pat_") {
            Some(TraitType::Pattern)
        } else if id.starts_with("wf_") {
            Some(TraitType::Workflow)
        } else {
            None
        }
    }

    /// The upstream section name for this type.
    pub fn section(self) -> &'static str {
        match self {
            TraitType::Preference => "preferences",
            TraitType::Pattern => "patterns",
            TraitType::Workflow => "workflows",
        }
    }
}

/// The one place the bounds sentence is templated, so the helper's
/// `preferences.description …` shape and the validator's index-prefixed
/// `preferences[0].description …` shape can never drift apart.
fn content_bounds_sentence(prefix: &str) -> String {
    format!(
        "{prefix}.description must be between {TRAIT_MIN_CHARS} and {TRAIT_MAX_CHARS} characters"
    )
}

/// PRD's content bound for one trait kind (R8).
///
/// Counts **characters** (`chars().count()`), so a 3-character CJK description
/// is valid and no slice can split a UTF-8 sequence. Both bounds are inclusive.
pub fn validate_trait_content(kind: TraitType, description: &str) -> Result<(), String> {
    let length = description.chars().count();
    if !(TRAIT_MIN_CHARS..=TRAIT_MAX_CHARS).contains(&length) {
        return Err(content_bounds_sentence(kind.section()));
    }
    Ok(())
}

/// Upstream `ValidationResult`.
#[derive(Debug, Clone, PartialEq)]
pub struct ValidationResult {
    pub valid: bool,
    pub errors: Vec<String>,
    /// `Some` only when `valid`.
    pub data: Option<ProfileData>,
}

/// Upstream `UserProfileValidator`.
pub struct UserProfileValidator;

impl UserProfileValidator {
    /// Upstream `UserProfileValidator.validate`, plus the R8 description bound.
    ///
    /// Never panics and never mutates its input.
    pub fn validate(value: &Value) -> ValidationResult {
        let Some(object) = value.as_object() else {
            let sentence = if value.is_array() {
                "Response cannot be an array"
            } else {
                "Response is not an object"
            };
            return invalid(vec![sentence.to_string()]);
        };
        if object.is_empty() {
            return invalid(vec!["Response object is empty".to_string()]);
        }

        let mut errors: Vec<String> = Vec::new();
        for (key, member) in object {
            if member.is_null() {
                errors.push(format!("Field '{key}' is null or undefined"));
            }
        }
        if !errors.is_empty() {
            return invalid(errors);
        }

        if let Some(preferences) = object.get("preferences") {
            errors.extend(validate_preferences(preferences));
        }
        if let Some(patterns) = object.get("patterns") {
            errors.extend(validate_patterns(patterns));
        }
        if let Some(workflows) = object.get("workflows") {
            errors.extend(validate_workflows(workflows));
        }
        if !errors.is_empty() {
            return invalid(errors);
        }

        // DIVERGENCE (disclosed): upstream hands the raw object back as
        // `UserProfileData` (TypeScript is structural, and it trusts the checks
        // above). Rust needs a real `ProfileData`, so a payload that passes
        // every check but carries an unmodelled field of the wrong type (e.g.
        // `"frequency": "two"`) is *rejected* rather than partially parsed —
        // refusing is the data-loss-safe answer (R7/R11).
        match serde_json::from_value::<ProfileData>(value.clone()) {
            Ok(data) => ValidationResult {
                valid: true,
                errors: Vec::new(),
                data: Some(data),
            },
            Err(error) => invalid(vec![format!(
                "Response is not a parseable profile payload: {error}"
            )]),
        }
    }
}

fn invalid(errors: Vec<String>) -> ValidationResult {
    ValidationResult {
        valid: false,
        errors,
        data: None,
    }
}

/// Upstream `validatePreferences` + the R8 description bound.
fn validate_preferences(value: &Value) -> Vec<String> {
    let Some(items) = value.as_array() else {
        return vec!["preferences must be an array".to_string()];
    };
    let mut errors = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let Some(fields) = item.as_object() else {
            errors.push(format!("preferences[{index}] is not an object"));
            continue;
        };
        if !is_non_empty_string(fields.get("category")) {
            errors.push(format!(
                "preferences[{index}].category is missing or invalid"
            ));
        }
        check_description(fields, "preferences", index, &mut errors);
        let confidence_valid = fields
            .get("confidence")
            .and_then(Value::as_f64)
            .is_some_and(|confidence| {
                (0.0..=USER_PROFILE_LLM_CONFIDENCE_MAX).contains(&confidence)
            });
        if !confidence_valid {
            errors.push(format!(
                "preferences[{index}].confidence must be a finite number between 0 and 1"
            ));
        }
        match fields.get("evidence") {
            Some(Value::Array(entries)) if entries.is_empty() => {
                errors.push(format!("preferences[{index}].evidence cannot be empty"));
            }
            Some(Value::Array(_)) => {}
            _ => errors.push(format!("preferences[{index}].evidence must be an array")),
        }
    }
    errors
}

/// Upstream `validatePatterns` + the R8 description bound.
fn validate_patterns(value: &Value) -> Vec<String> {
    let Some(items) = value.as_array() else {
        return vec!["patterns must be an array".to_string()];
    };
    let mut errors = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let Some(fields) = item.as_object() else {
            errors.push(format!("patterns[{index}] is not an object"));
            continue;
        };
        if !is_non_empty_string(fields.get("category")) {
            errors.push(format!("patterns[{index}].category is missing or invalid"));
        }
        check_description(fields, "patterns", index, &mut errors);
    }
    errors
}

/// Upstream `validateWorkflows` + the R8 description bound.
fn validate_workflows(value: &Value) -> Vec<String> {
    let Some(items) = value.as_array() else {
        return vec!["workflows must be an array".to_string()];
    };
    let mut errors = Vec::new();
    for (index, item) in items.iter().enumerate() {
        let Some(fields) = item.as_object() else {
            errors.push(format!("workflows[{index}] is not an object"));
            continue;
        };
        check_description(fields, "workflows", index, &mut errors);
        match fields.get("steps") {
            Some(Value::Array(entries)) if entries.is_empty() => {
                errors.push(format!("workflows[{index}].steps cannot be empty"));
            }
            Some(Value::Array(_)) => {}
            _ => errors.push(format!("workflows[{index}].steps must be an array")),
        }
    }
    errors
}

/// One item's description: presence/type (upstream) then the R8 bounds.
fn check_description(
    fields: &serde_json::Map<String, Value>,
    section: &str,
    index: usize,
    errors: &mut Vec<String>,
) {
    match fields.get("description").and_then(Value::as_str) {
        Some(description) if !description.is_empty() => {
            if validate_trait_content(trait_type_for(section), description).is_err() {
                errors.push(content_bounds_sentence(&format!("{section}[{index}]")));
            }
        }
        _ => errors.push(format!(
            "{section}[{index}].description is missing or invalid"
        )),
    }
}

/// The section's [`TraitType`] (the validator reaches here per section).
fn trait_type_for(section: &str) -> TraitType {
    match section {
        "patterns" => TraitType::Pattern,
        "workflows" => TraitType::Workflow,
        _ => TraitType::Preference,
    }
}

/// Upstream's `!value || typeof value !== "string"` (an empty string is falsy).
fn is_non_empty_string(value: Option<&Value>) -> bool {
    value
        .and_then(Value::as_str)
        .is_some_and(|text| !text.is_empty())
}

/// Validates one provider payload and hands back the three category updates.
///
/// A payload fault is a **parse-class** fault (`ProfileError::Save`), exactly
/// like upstream's `AnalysisError::Parse → Save`: it is never fallback-eligible
/// and never carries a provider message.
pub fn validate_analysis_value(value: &Value) -> Result<ProfileUpdates, ProfileError> {
    let result = UserProfileValidator::validate(value);
    if !result.valid {
        return Err(ProfileError::Save(result.errors.join("; ")));
    }
    let data = result.data.unwrap_or_default();
    // Upstream `if (data.preferences)`: an absent section is an empty one.
    Ok(ProfileUpdates {
        preferences: Some(data.preferences),
        patterns: Some(data.patterns),
        workflows: Some(data.workflows),
    })
}

/// Upstream `shouldRunAutomaticProfileCleanup` L15-27.
///
/// `interval` is an `f64` on purpose: upstream's `Number.isInteger(interval)`
/// guard rejects a fractional configured interval, and
/// `ProfileConfig::auto_cleanup_interval` is a float.
pub fn should_run_automatic_profile_cleanup(
    previous_prompt_count: i64,
    added_prompt_count: i64,
    interval: f64,
    enabled: bool,
) -> bool {
    if !enabled || !interval.is_finite() || interval.fract() != 0.0 || interval <= 0.0 {
        return false;
    }
    ((previous_prompt_count as f64) / interval).floor()
        < (((previous_prompt_count + added_prompt_count) as f64) / interval).floor()
}
