use std::collections::BTreeSet;

use mem_profile::error::{ProfileError, failure_reason};

/// One constructible instance per `ProfileError` variant, with its expected token.
fn one_of_each() -> Vec<(&'static str, ProfileError)> {
    vec![
        (
            "store",
            ProfileError::Store(mem_store::error::StoreError::Json {
                message: "x".into(),
            }),
        ),
        (
            "io",
            ProfileError::Io {
                message: "x".into(),
                os_code: None,
                kind: None,
            },
        ),
        (
            "json",
            ProfileError::Json {
                message: "x".into(),
                line: 1,
                column: 1,
            },
        ),
        ("cold-buffer", ProfileError::ColdBuffer("x".into())),
        ("lock-untrusted", ProfileError::LockUntrusted),
        ("private-content", ProfileError::PrivateContentBlocked),
        ("save", ProfileError::Save("x".into())),
        ("analysis", ProfileError::Analysis("x".into())),
        (
            "erasure-failed",
            ProfileError::ErasureFailed {
                prompt_id: "p".into(),
                message: "x".into(),
            },
        ),
        (
            "prompt-delete-after-erasure",
            ProfileError::PromptDeleteAfterErasure {
                prompt_id: "p".into(),
                profiles_erased: 1,
                message: "x".into(),
            },
        ),
        (
            "lock-poisoned",
            ProfileError::LockPoisoned {
                lock: "l",
                operation: "o",
            },
        ),
    ]
}

#[test]
fn test_failure_reason_table_is_exact() {
    for (token, error) in one_of_each() {
        assert_eq!(error.failure_reason(), token, "{error}");
    }
}

#[test]
fn test_failure_reason_covers_the_closed_set_with_no_duplicates() {
    let mut exercised = BTreeSet::new();
    for (_, error) in one_of_each() {
        assert!(
            exercised.insert(error.failure_reason()),
            "duplicate token {}",
            error.failure_reason()
        );
    }
    let declared: BTreeSet<&'static str> = failure_reason::ALL.iter().copied().collect();
    assert_eq!(
        exercised, declared,
        "every token in failure_reason::ALL needs a fixture here"
    );
}
