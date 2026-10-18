//! Port of `tests/tag-migration-skip-tagged.test.ts` — the migration core
//! extracted from upstream `handleRunTagMigrationBatch`
//! (`src/services/api-handlers.ts`): a sync state machine with injected
//! seams (provider → [`TagGeneration`], DB/embedding → [`MigrationSink`]).
//! HTTP handler wiring is covered-by W6.2; the store adapters land in
//! W1.2/W6.2 like upstream's mock boundaries.

use mem_core::tags::migration::{MemoryRecord, MigrationSink, TagGeneration, TagMigrator};
use std::cell::RefCell;

struct StubGenerator {
    result: Result<Vec<String>, String>,
    calls: RefCell<Vec<(String, String)>>,
}

impl TagGeneration for StubGenerator {
    fn generate_tags(&self, memory_id: &str, content: &str) -> Result<Vec<String>, String> {
        self.calls
            .borrow_mut()
            .push((memory_id.to_string(), content.to_string()));
        match &self.result {
            Ok(tags) => Ok(tags.clone()),
            Err(message) => Err(message.clone()),
        }
    }
}

#[derive(Default)]
struct RecordingSink {
    tag_writes: RefCell<Vec<(String, String)>>,
    embed_texts: RefCell<Vec<String>>,
    vector_updates: RefCell<Vec<(String, Option<usize>)>>,
}

impl MigrationSink for RecordingSink {
    fn write_tags_csv(&mut self, memory_id: &str, tags_csv: &str) -> Result<(), String> {
        self.tag_writes
            .borrow_mut()
            .push((memory_id.to_string(), tags_csv.to_string()));
        Ok(())
    }

    fn embed_document(&mut self, text: &str) -> Result<Vec<f32>, String> {
        self.embed_texts.borrow_mut().push(text.to_string());
        Ok(vec![1.0, 2.0, 3.0])
    }

    fn update_vector(
        &mut self,
        memory_id: &str,
        _content_vector: &[f32],
        tags_vector: Option<&[f32]>,
    ) -> Result<(), String> {
        self.vector_updates
            .borrow_mut()
            .push((memory_id.to_string(), tags_vector.map(<[f32]>::len)));
        Ok(())
    }
}

fn record(id: &str, content: &str, tags: Option<&str>, has_tags_vector: bool) -> MemoryRecord {
    MemoryRecord {
        id: id.to_string(),
        content: content.to_string(),
        tags: tags.map(str::to_string),
        has_tags_vector,
    }
}

#[test]
fn test_batch_skips_fully_migrated_memory_and_rewrites_untagged_one() {
    // Upstream: "does not re-vectorize a memory that already has tags and a
    // tags vector". `"done1"` is fully migrated; `"todo1"` needs tagging.
    let memories = vec![
        record("done1", "already done content", Some("alpha,beta"), true),
        record("todo1", "needs tagging", None, false),
    ];
    let generator = StubGenerator {
        result: Ok(vec!["generated".to_string(), "tags".to_string()]),
        calls: RefCell::new(Vec::new()),
    };
    let mut sink = RecordingSink::default();
    let mut migrator = TagMigrator::new();

    let outcome = migrator.run_batch(5, &memories, &generator, &mut sink);

    // Both memories are accounted for, but only the untagged one is rewritten.
    assert_eq!(outcome.processed, 2);
    assert_eq!(outcome.total, 2);
    assert!(!outcome.has_more);
    assert_eq!(outcome.errors, 0);

    // Only the untagged memory is re-vectorized.
    assert_eq!(
        sink.vector_updates.borrow().clone(),
        vec![("todo1".to_string(), Some(3))]
    );

    // Its content is embedded once (content) plus once (tags) — never the
    // done memory.
    assert_eq!(
        sink.embed_texts.borrow().clone(),
        vec![
            "needs tagging".to_string(),
            "Topics: generated, tags".to_string(),
        ]
    );

    // Only the untagged memory's tags are written (CSV join, no spaces).
    assert_eq!(
        sink.tag_writes.borrow().clone(),
        vec![("todo1".to_string(), "generated,tags".to_string())]
    );

    // Extras: the progress cursor snapped to the end and the pass completed.
    let progress = migrator.progress();
    assert_eq!(progress.cursor, 2);
    assert!(progress.is_complete);
}

#[test]
fn test_tags_without_vector_are_re_vectorized() {
    // Upstream's documented branch: non-empty tags but no tags vector →
    // processed WITHOUT tag generation (and without rewriting the tags).
    let memories = vec![record("t1", "some content", Some("Alpha, Beta"), false)];
    let generator = StubGenerator {
        result: Ok(vec!["should-not-be-used".to_string()]),
        calls: RefCell::new(Vec::new()),
    };
    let mut sink = RecordingSink::default();
    let mut migrator = TagMigrator::new();

    let outcome = migrator.run_batch(5, &memories, &generator, &mut sink);

    assert_eq!(outcome.processed, 1);
    assert_eq!(outcome.errors, 0);
    // No generation, no tag rewrite.
    assert!(generator.calls.borrow().is_empty());
    assert!(sink.tag_writes.borrow().is_empty());
    // Content + the lowercased tags line are embedded; vector updated.
    assert_eq!(
        sink.embed_texts.borrow().clone(),
        vec![
            "some content".to_string(),
            "Topics: alpha, beta".to_string(),
        ]
    );
    assert_eq!(
        sink.vector_updates.borrow().clone(),
        vec![("t1".to_string(), Some(3))]
    );
}

#[test]
fn test_second_batch_resumes_from_cursor() {
    let memories = vec![
        record("done1", "already done content", Some("alpha,beta"), true),
        record("todo1", "needs tagging", None, false),
    ];
    let generator = StubGenerator {
        result: Ok(vec!["generated".to_string(), "tags".to_string()]),
        calls: RefCell::new(Vec::new()),
    };
    let mut sink = RecordingSink::default();
    let mut migrator = TagMigrator::new();

    let first = migrator.run_batch(1, &memories, &generator, &mut sink);
    assert_eq!(first.processed, 1);
    assert!(first.has_more);
    assert_eq!(migrator.progress().cursor, 1);

    // Mid-pass: no reset, the window moves forward — `done1` must not be
    // reprocessed and `todo1` must not be skipped over.
    let second = migrator.run_batch(1, &memories, &generator, &mut sink);
    assert_eq!(second.processed, 2);
    assert!(!second.has_more);
    assert_eq!(
        sink.vector_updates.borrow().clone(),
        vec![("todo1".to_string(), Some(3))]
    );
}

#[test]
fn test_empty_generated_tags_update_vector_without_tags_embedding() {
    // Upstream truthy empty array: a successful generation returning no
    // tags updates the vector WITHOUT a tags embedding.
    let memories = vec![record("todo1", "needs tagging", None, false)];
    let generator = StubGenerator {
        result: Ok(vec![]),
        calls: RefCell::new(Vec::new()),
    };
    let mut sink = RecordingSink::default();
    let mut migrator = TagMigrator::new();

    let outcome = migrator.run_batch(5, &memories, &generator, &mut sink);

    assert_eq!(outcome.processed, 1);
    assert_eq!(outcome.errors, 0);
    // Tags must still be written (the empty CSV), exactly one embed call
    // (content only).
    assert_eq!(
        sink.tag_writes.borrow().clone(),
        vec![("todo1".to_string(), String::new())]
    );
    assert_eq!(
        sink.embed_texts.borrow().clone(),
        vec!["needs tagging".to_string()]
    );
    assert_eq!(
        sink.vector_updates.borrow().clone(),
        vec![("todo1".to_string(), None)]
    );
}

#[test]
fn test_zero_batch_size_is_clamped_to_one() {
    // Documented divergence: upstream `batchSize 0` would produce
    // `Math.ceil(len/0)` and a zero-width window with no progress; the
    // port clamps to 1.
    let memories = vec![
        record("m1", "one", None, false),
        record("m2", "two", None, false),
    ];
    let generator = StubGenerator {
        result: Ok(vec!["x".to_string()]),
        calls: RefCell::new(Vec::new()),
    };
    let mut sink = RecordingSink::default();
    let mut migrator = TagMigrator::new();

    let first = migrator.run_batch(0, &memories, &generator, &mut sink);
    assert_eq!(first.processed, 1);
    assert!(first.has_more);
    assert_eq!(migrator.progress().total_batches, memories.len() as u32);

    let second = migrator.run_batch(0, &memories, &generator, &mut sink);
    assert_eq!(second.processed, 2);
    assert!(!second.has_more);
    assert_eq!(sink.vector_updates.borrow().len(), 2);
}
