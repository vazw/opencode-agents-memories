//! Port of `tests/user-profile-learning-confidence.test.ts`'s validator cases
//! plus the PRD's net-new trait-content bounds (R8), tested through
//! `UserProfileValidator::validate` itself.
//!
//! Tests:
//! - `test_trait_content_shorter_than_three_chars_is_rejected`
//! - `test_trait_content_longer_than_five_hundred_chars_is_rejected_and_the_bounds_are_inclusive`
//! - `test_trait_content_bounds_count_characters_not_bytes`
//! - `test_validator_rejects_a_description_outside_the_three_to_five_hundred_char_bounds`
//! - `test_trait_type_maps_id_prefixes_to_sections`
//! - `test_validator_reports_the_upstream_sentences_for_malformed_sections`

use serde_json::{Value, json};

use mem_profile::validation::{
    TRAIT_MAX_CHARS, TRAIT_MIN_CHARS, TraitType, UserProfileValidator, validate_trait_content,
};

#[test]
fn test_trait_content_shorter_than_three_chars_is_rejected() {
    let error = validate_trait_content(TraitType::Preference, "ab").expect_err("2 chars rejected");
    assert_eq!(
        error,
        "preferences.description must be between 3 and 500 characters"
    );
    assert!(validate_trait_content(TraitType::Preference, "abc").is_ok());
    assert!(validate_trait_content(TraitType::Preference, "").is_err());
    assert_eq!(TRAIT_MIN_CHARS, 3);
}

#[test]
fn test_trait_content_longer_than_five_hundred_chars_is_rejected_and_the_bounds_are_inclusive() {
    let longest = "a".repeat(TRAIT_MAX_CHARS);
    assert!(
        validate_trait_content(TraitType::Pattern, &longest).is_ok(),
        "500 chars is inside the inclusive bound"
    );

    let too_long = "a".repeat(TRAIT_MAX_CHARS + 1);
    let error = validate_trait_content(TraitType::Pattern, &too_long).expect_err("501 rejected");
    assert_eq!(
        error,
        "patterns.description must be between 3 and 500 characters"
    );
    assert_eq!(TRAIT_MAX_CHARS, 500);
}

#[test]
fn test_trait_content_bounds_count_characters_not_bytes() {
    // Three CJK characters are 9 bytes but exactly 3 chars.
    let three_characters = "汉字字";
    assert_eq!(three_characters.len(), 9, "multi-byte by construction");
    assert!(
        validate_trait_content(TraitType::Workflow, three_characters).is_ok(),
        "the bound is char-based, never a byte slice"
    );

    let two_characters = "汉字";
    assert!(validate_trait_content(TraitType::Workflow, two_characters).is_err());

    let five_hundred_characters = "字".repeat(TRAIT_MAX_CHARS);
    assert_eq!(five_hundred_characters.len(), 1500);
    assert!(validate_trait_content(TraitType::Workflow, &five_hundred_characters).is_ok());
    assert!(validate_trait_content(TraitType::Workflow, &"字".repeat(501)).is_err());
}

#[test]
fn test_validator_rejects_a_description_outside_the_three_to_five_hundred_char_bounds() {
    // The bound is enforced INSIDE `validate`, not only in the helper: a
    // validator that merely type-checks (the upstream body) passes this
    // payload.
    let short = json!({
        "preferences": [{
            "category": "style",
            "description": "ab",
            "confidence": 0.8,
            "evidence": ["please keep it short"]
        }],
        "patterns": [],
        "workflows": []
    });
    let result = UserProfileValidator::validate(&short);
    assert!(!result.valid, "a 2-char description is rejected");
    assert_eq!(
        result.errors,
        vec!["preferences[0].description must be between 3 and 500 characters"]
    );
    assert!(result.data.is_none());

    let long = json!({
        "patterns": [{
            "category": "style",
            "description": "a".repeat(TRAIT_MAX_CHARS + 1)
        }]
    });
    let result = UserProfileValidator::validate(&long);
    assert!(!result.valid);
    assert_eq!(
        result.errors,
        vec!["patterns[0].description must be between 3 and 500 characters"]
    );

    let inside = json!({
        "preferences": [{
            "category": "style",
            "description": "Prefers concise answers",
            "confidence": 0.8,
            "evidence": ["please keep it short"]
        }],
        "patterns": [],
        "workflows": []
    });
    assert!(
        UserProfileValidator::validate(&inside).valid,
        "an in-bounds payload still validates"
    );
}

#[test]
fn test_trait_type_maps_id_prefixes_to_sections() {
    assert_eq!(
        TraitType::from_id_prefix("pref_0"),
        Some(TraitType::Preference)
    );
    assert_eq!(TraitType::from_id_prefix("pat_3"), Some(TraitType::Pattern));
    assert_eq!(TraitType::from_id_prefix("wf_1"), Some(TraitType::Workflow));
    assert_eq!(TraitType::from_id_prefix("pref"), None);
    assert_eq!(TraitType::from_id_prefix("pref0"), None);
    assert_eq!(TraitType::from_id_prefix(""), None);
    assert_eq!(TraitType::from_id_prefix("other_0"), None);

    assert_eq!(TraitType::Preference.section(), "preferences");
    assert_eq!(TraitType::Pattern.section(), "patterns");
    assert_eq!(TraitType::Workflow.section(), "workflows");
}

#[test]
fn test_validator_reports_the_upstream_sentences_for_malformed_sections() {
    let cases: Vec<(Value, Vec<&str>)> = vec![
        (
            Value::String("nope".to_string()),
            vec!["Response is not an object"],
        ),
        (json!([1, 2]), vec!["Response cannot be an array"]),
        (json!({}), vec!["Response object is empty"]),
        (
            json!({"preferences": null}),
            vec!["Field 'preferences' is null or undefined"],
        ),
        (
            json!({"preferences": "nope"}),
            vec!["preferences must be an array"],
        ),
        (
            json!({"preferences": [null]}),
            vec!["preferences[0] is not an object"],
        ),
        (
            json!({"preferences": [{
                "description": "a long enough description",
                "confidence": 0.5,
                "evidence": ["e"]
            }]}),
            vec!["preferences[0].category is missing or invalid"],
        ),
        (
            json!({"preferences": [{
                "category": "style",
                "description": "a long enough description",
                "confidence": 1.1,
                "evidence": ["e"]
            }]}),
            vec!["preferences[0].confidence must be a finite number between 0 and 1"],
        ),
        (
            json!({"preferences": [{
                "category": "style",
                "description": "a long enough description",
                "confidence": 0.5
            }]}),
            vec!["preferences[0].evidence must be an array"],
        ),
        (
            json!({"preferences": [{
                "category": "style",
                "description": "a long enough description",
                "confidence": 0.5,
                "evidence": []
            }]}),
            vec!["preferences[0].evidence cannot be empty"],
        ),
        (
            json!({"workflows": [{"description": "a long enough description"}]}),
            vec!["workflows[0].steps must be an array"],
        ),
        (
            json!({"workflows": [{"description": "a long enough description", "steps": []}]}),
            vec!["workflows[0].steps cannot be empty"],
        ),
        (
            json!({"patterns": [{"description": 7}]}),
            vec![
                "patterns[0].category is missing or invalid",
                "patterns[0].description is missing or invalid",
            ],
        ),
    ];

    for (payload, expected) in cases {
        let result = UserProfileValidator::validate(&payload);
        assert!(!result.valid, "payload {payload} must be rejected");
        assert_eq!(
            result.errors, expected,
            "upstream sentences are byte-for-byte: {payload}"
        );
        assert!(result.data.is_none(), "no data on a rejected payload");
    }

    let valid = json!({
        "preferences": [{
            "category": "style",
            "description": "Prefers concise answers",
            "confidence": 0.8,
            "evidence": ["please keep it short"]
        }],
        "patterns": [],
        "workflows": []
    });
    let result = UserProfileValidator::validate(&valid);
    assert!(result.valid, "errors: {:?}", result.errors);
    assert!(result.errors.is_empty());
    let data = result.data.expect("valid payload yields data");
    assert_eq!(data.preferences.len(), 1);
    assert_eq!(data.preferences[0].description, "Prefers concise answers");
    assert_eq!(data.preferences[0].confidence, Some(0.8));
    assert!(data.patterns.is_empty() && data.workflows.is_empty());
}
