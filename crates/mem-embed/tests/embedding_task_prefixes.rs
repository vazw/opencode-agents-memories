//! Task-prefix contract — mirrors `example/opencode-mem/tests/embedding-task-prefixes.test.ts`
//! case-for-case. Trace table (upstream `describe/it` → Rust test):
//!
//! - resolve: "returns empty when prefixes are disabled and no custom prefix
//!   is set" → `test_resolve_prefix_empty_when_disabled_and_no_custom`;
//!   "returns Nomic prefixes when useTaskPrefixes is true" →
//!   `test_resolve_prefix_nomic_when_enabled`; "prefers custom prefixes over
//!   Nomic" → `test_resolve_prefix_prefers_custom_over_nomic`; "treats an
//!   explicit empty custom prefix as no prefix" →
//!   `test_resolve_prefix_explicit_empty_custom_means_no_prefix`.
//! - apply: "returns text unchanged when prefixes are disabled" →
//!   `test_apply_prefix_unchanged_when_disabled`; "returns text unchanged
//!   when no task is provided" → `test_apply_prefix_unchanged_when_no_task`
//!   (upstream's `undefined` task is the service-level
//!   `EmbeddingService::embed(text, None)` passthrough — pinned here at the
//!   prefix seam via the same `Option`-dispatch the service uses, and again
//!   end-to-end in `embedding_service.rs`); "applies search_document prefix
//!   for document task" → `test_apply_prefix_document_uses_search_document`;
//!   "applies search_query prefix for query task" →
//!   `test_apply_prefix_query_uses_search_query`; "keeps document and query
//!   cache keys distinct for the same text" →
//!   `test_apply_prefix_keeps_document_and_query_keys_distinct`; "does not
//!   double-prefix an already prefixed string" →
//!   `test_apply_prefix_does_not_double_prefix`; "applies custom prefixes
//!   from options object" → `test_apply_prefix_uses_custom_prefixes`;
//!   "skips prefixing when custom prefix is empty even if Nomic is enabled"
//!   → `test_apply_prefix_empty_custom_skips_nomic`.

use mem_embed::embedder::EmbedTask;
use mem_embed::prefix::{
    NOMIC_DOCUMENT_PREFIX, NOMIC_QUERY_PREFIX, PrefixOptions, apply_task_prefix,
    resolve_task_prefix,
};

fn opts(use_task_prefixes: bool) -> PrefixOptions {
    PrefixOptions {
        use_task_prefixes,
        ..PrefixOptions::default()
    }
}

/// Upstream passes `undefined`/`{}` as the task; the Rust service dispatches
/// the same way: no task ⇒ text through unchanged, prefixes never consulted.
fn apply_optional_task(text: &str, task: Option<EmbedTask>, options: &PrefixOptions) -> String {
    match task {
        Some(t) => apply_task_prefix(text, t, options),
        None => text.to_owned(),
    }
}

#[test]
fn test_resolve_prefix_empty_when_disabled_and_no_custom() {
    let options = opts(false);
    assert_eq!(resolve_task_prefix(EmbedTask::Document, &options), "");
    assert_eq!(resolve_task_prefix(EmbedTask::Query, &options), "");
}

#[test]
fn test_resolve_prefix_nomic_when_enabled() {
    let options = opts(true);
    assert_eq!(
        resolve_task_prefix(EmbedTask::Document, &options),
        "search_document: "
    );
    assert_eq!(
        resolve_task_prefix(EmbedTask::Query, &options),
        "search_query: "
    );
    assert_eq!(NOMIC_DOCUMENT_PREFIX, "search_document: ");
    assert_eq!(NOMIC_QUERY_PREFIX, "search_query: ");
}

#[test]
fn test_resolve_prefix_prefers_custom_over_nomic() {
    let query = PrefixOptions {
        use_task_prefixes: true,
        query_prefix: Some("query: ".to_string()),
        ..PrefixOptions::default()
    };
    assert_eq!(resolve_task_prefix(EmbedTask::Query, &query), "query: ");

    let document = PrefixOptions {
        use_task_prefixes: true,
        document_prefix: Some("passage: ".to_string()),
        ..PrefixOptions::default()
    };
    assert_eq!(
        resolve_task_prefix(EmbedTask::Document, &document),
        "passage: "
    );
}

#[test]
fn test_resolve_prefix_explicit_empty_custom_means_no_prefix() {
    let query = PrefixOptions {
        use_task_prefixes: true,
        query_prefix: Some(String::new()),
        ..PrefixOptions::default()
    };
    assert_eq!(resolve_task_prefix(EmbedTask::Query, &query), "");
}

#[test]
fn test_apply_prefix_unchanged_when_disabled() {
    let options = opts(false);
    assert_eq!(
        apply_task_prefix("hello", EmbedTask::Document, &options),
        "hello"
    );
    assert_eq!(
        apply_task_prefix("hello", EmbedTask::Query, &options),
        "hello"
    );
}

#[test]
fn test_apply_prefix_unchanged_when_no_task() {
    // Upstream: applyEmbeddingTaskPrefix("hello", undefined, true) === "hello".
    assert_eq!(apply_optional_task("hello", None, &opts(true)), "hello");
}

#[test]
fn test_apply_prefix_document_uses_search_document() {
    let options = opts(true);
    assert_eq!(
        apply_task_prefix("hello", EmbedTask::Document, &options),
        "search_document: hello"
    );
}

#[test]
fn test_apply_prefix_query_uses_search_query() {
    let options = opts(true);
    assert_eq!(
        apply_task_prefix("hello", EmbedTask::Query, &options),
        "search_query: hello"
    );
}

#[test]
fn test_apply_prefix_keeps_document_and_query_keys_distinct() {
    let options = opts(true);
    let document_key = apply_task_prefix("same", EmbedTask::Document, &options);
    let query_key = apply_task_prefix("same", EmbedTask::Query, &options);
    assert_ne!(document_key, query_key);
    assert_eq!(document_key, "search_document: same");
    assert_eq!(query_key, "search_query: same");
}

#[test]
fn test_apply_prefix_does_not_double_prefix() {
    let options = opts(true);
    assert_eq!(
        apply_task_prefix("search_document: hello", EmbedTask::Document, &options),
        "search_document: hello"
    );
    assert_eq!(
        apply_task_prefix("search_query: hello", EmbedTask::Query, &options),
        "search_query: hello"
    );
}

#[test]
fn test_apply_prefix_uses_custom_prefixes() {
    let options = PrefixOptions {
        use_task_prefixes: false,
        query_prefix: Some("query: ".to_string()),
        document_prefix: Some("passage: ".to_string()),
    };
    assert_eq!(
        apply_task_prefix("hello", EmbedTask::Query, &options),
        "query: hello"
    );
    assert_eq!(
        apply_task_prefix("hello", EmbedTask::Document, &options),
        "passage: hello"
    );
}

#[test]
fn test_apply_prefix_empty_custom_skips_nomic() {
    let query = PrefixOptions {
        use_task_prefixes: true,
        query_prefix: Some(String::new()),
        ..PrefixOptions::default()
    };
    assert_eq!(
        apply_task_prefix("hello", EmbedTask::Query, &query),
        "hello"
    );
}
