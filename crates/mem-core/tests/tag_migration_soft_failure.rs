//! Port of `tests/tag-migration-soft-failure.test.ts` — soft failures
//! ("does not count untagged memories as processed when tag generation
//! soft-fails"): each failed memory records exactly one error naming it,
//! is never written/embedded/vectorized, yet still advances the cursor; a
//! completed pass resets counters so soft failures can be retried.

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

fn record(id: &str, content: &str, tags: Option<&str>) -> MemoryRecord {
    MemoryRecord {
        id: id.to_string(),
        content: content.to_string(),
        tags: tags.map(str::to_string),
        has_tags_vector: false,
    }
}

fn failing_memories() -> Vec<MemoryRecord> {
    vec![
        record("m1", "untagged one", None),
        record("m2", "untagged two", Some("")),
    ]
}

fn failing_generator() -> StubGenerator {
    StubGenerator {
        result: Err("empty tool-call arguments".to_string()),
        calls: RefCell::new(Vec::new()),
    }
}

#[test]
fn test_soft_failure_counts_errors_and_leaves_memories_unprocessed() {
    // Upstream: "does not count untagged memories as processed when tag
    // generation soft-fails".
    let memories = failing_memories();
    let generator = failing_generator();
    let mut sink = RecordingSink::default();
    let mut migrator = TagMigrator::new();

    let outcome = migrator.run_batch(5, &memories, &generator, &mut sink);

    assert_eq!(outcome.processed, 0);
    assert!(!outcome.has_more);
    assert_eq!(outcome.errors, 2);

    let progress = migrator.progress();
    assert_eq!(progress.processed, 0);
    assert!(progress.is_complete);
    assert_eq!(progress.errors.len(), 2);
    assert!(progress.errors[0].contains("m1"));

    // No vector updates, no tag writes.
    assert!(sink.vector_updates.borrow().is_empty());
    assert!(sink.tag_writes.borrow().is_empty());
}

#[test]
fn test_soft_failed_batch_advances_cursor_and_marks_complete() {
    let memories = failing_memories();
    let generator = failing_generator();
    let mut sink = RecordingSink::default();
    let mut migrator = TagMigrator::new();

    let _ = migrator.run_batch(5, &memories, &generator, &mut sink);

    let progress = migrator.progress();
    assert_eq!(
        progress.cursor, 2,
        "cursor advances so the window moves forward"
    );
    assert!(progress.is_complete);
}

#[test]
fn test_completed_pass_reset_retries_soft_failures() {
    // Reset rule (fresh run or retry after a completed pass): the second
    // pass with a healthy generator retries everything.
    let memories = failing_memories();
    let failing = failing_generator();
    let succeeding = StubGenerator {
        result: Ok(vec!["x".to_string()]),
        calls: RefCell::new(Vec::new()),
    };
    let mut sink = RecordingSink::default();
    let mut migrator = TagMigrator::new();

    let _ = migrator.run_batch(5, &memories, &failing, &mut sink);
    assert_eq!(migrator.progress().errors.len(), 2);

    let retried = migrator.run_batch(5, &memories, &succeeding, &mut sink);
    assert_eq!(retried.processed, 2);
    assert_eq!(retried.errors, 0, "errors were cleared by the reset");
    assert_eq!(
        sink.vector_updates.borrow().clone(),
        vec![("m1".to_string(), Some(3)), ("m2".to_string(), Some(3))]
    );
    assert_eq!(
        sink.tag_writes.borrow().clone(),
        vec![
            ("m1".to_string(), "x".to_string()),
            ("m2".to_string(), "x".to_string()),
        ]
    );
}

#[test]
fn test_soft_failure_error_message_names_the_memory() {
    // The errors list is the observable (upstream asserts its contents);
    // byte-pinned here as the one deliberate string payload.
    let memories = failing_memories();
    let generator = failing_generator();
    let mut sink = RecordingSink::default();
    let mut migrator = TagMigrator::new();

    let _ = migrator.run_batch(5, &memories, &generator, &mut sink);

    let progress = migrator.progress();
    assert_eq!(
        progress.errors[0],
        "Tag generation failed for memory m1: empty tool-call arguments"
    );
    assert_eq!(
        progress.errors[1],
        "Tag generation failed for memory m2: empty tool-call arguments"
    );
}
