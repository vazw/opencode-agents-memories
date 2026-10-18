//! Tag-migration core, extracted from upstream
//! `handleRunTagMigrationBatch` / `handleGetTagMigrationProgress`
//! (`src/services/api-handlers.ts`, subject of
//! `tests/tag-migration-skip-tagged.test.ts` and
//! `tests/tag-migration-soft-failure.test.ts`).
//!
//! Translation shape (documented divergence): upstream's async HTTP handler
//! with module-global progress state is a sync state machine with injected
//! seams here — [`TagGeneration`] is the provider seam, [`MigrationSink`]
//! the DB/embedding seam. The real provider/embedding/turso adapters and
//! the HTTP wiring land in W6.2/W1.2 like upstream's mock boundaries.
//!
//! The invariants that matter:
//! - a memory that already has tags AND a tags vector is skipped (a no-op
//!   re-embed), but still accounted as processed;
//! - a soft failure (generation error or any sink error) records exactly
//!   one error naming the memory, writes/embeds/updates NOTHING for that
//!   memory, and yet still advances the cursor;
//! - a completed pass (`total == 0 || is_complete`) resets counters on the
//!   next batch, so soft failures can be retried; mid-pass batches
//!   continue from the cursor;
//! - `batch_size` 0 is clamped to 1 (upstream's zero-width window is a
//!   no-progress hazard — documented divergence);
//! - a shrunk memory list yields an empty window and the cursor snaps to
//!   `total` (never panics).

use crate::tags::format_tags_for_embedding;

/// Upstream's progress payload (`handleGetTagMigrationProgress`).
///
/// `Default` mirrors upstream's initializer: `is_complete` starts `true`
/// (no pass has begun), everything else zero and empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationProgress {
    pub processed: usize,
    pub cursor: usize,
    pub total: usize,
    pub current_batch: u32,
    pub total_batches: u32,
    pub is_complete: bool,
    pub errors: Vec<String>,
}

impl Default for MigrationProgress {
    fn default() -> Self {
        Self {
            processed: 0,
            cursor: 0,
            total: 0,
            current_batch: 0,
            total_batches: 0,
            is_complete: true,
            errors: Vec::new(),
        }
    }
}

/// One candidate memory (upstream's SELECT row: `id`, `content`, `tags`,
/// `tags_vector != null`). `tags: None`/empty string means untagged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryRecord {
    pub id: String,
    pub content: String,
    pub tags: Option<String>,
    pub has_tags_vector: bool,
}

/// Upstream's `data` payload of one batch run. `processed` is cumulative
/// across the whole pass (the progress counter), matching both upstream
/// tests' assertions over batch and progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchOutcome {
    pub processed: usize,
    pub total: usize,
    pub has_more: bool,
    pub errors: usize,
}

/// The provider seam (upstream `AIProviderFactory.createProvider()` →
/// `executeToolCall`).
///
/// An `Ok(vec![])` is a *successful* generation returning no tags (the
/// upstream truthy empty array — the vector is updated without a tags
/// embedding). The adapter maps a missing/failed `tags` field to
/// `Err(result.error or "no tags returned")`; W6.2 owns prompt,
/// tool-schema and parsing.
pub trait TagGeneration {
    fn generate_tags(&self, memory_id: &str, content: &str) -> Result<Vec<String>, String>;
}

/// The DB/embedding seam (upstream `UPDATE memories SET tags = ? WHERE
/// id = ?`, `embeddingService.embedWithTimeout`, `tursoVectorSearch.updateVector`).
pub trait MigrationSink {
    /// The exact upstream bind: CSV-joined tags (`tags.join(",")`, no
    /// spaces).
    fn write_tags_csv(&mut self, memory_id: &str, tags_csv: &str) -> Result<(), String>;

    /// Embedding for `task: document` (content and tags text alike).
    fn embed_document(&mut self, text: &str) -> Result<Vec<f32>, String>;

    fn update_vector(
        &mut self,
        memory_id: &str,
        content_vector: &[f32],
        tags_vector: Option<&[f32]>,
    ) -> Result<(), String>;
}

/// The extracted migration state machine.
#[derive(Debug, Default)]
pub struct TagMigrator {
    progress: MigrationProgress,
}

impl TagMigrator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn progress(&self) -> &MigrationProgress {
        &self.progress
    }

    /// Runs one batch window (upstream `handleRunTagMigrationBatch`).
    /// Branch order is the contract — see the module doc.
    pub fn run_batch(
        &mut self,
        batch_size: usize,
        memories: &[MemoryRecord],
        generation: &dyn TagGeneration,
        sink: &mut dyn MigrationSink,
    ) -> BatchOutcome {
        let batch_size = batch_size.max(1);

        // Reset rule (fresh run, or retry after a completed pass).
        if self.progress.total == 0 || self.progress.is_complete {
            self.progress.processed = 0;
            self.progress.cursor = 0;
            self.progress.total = memories.len();
            self.progress.total_batches = self.progress.total.div_ceil(batch_size) as u32;
            self.progress.current_batch = 0;
            self.progress.is_complete = false;
            self.progress.errors.clear();
        }

        let total = self.progress.total;
        let start = self.progress.cursor;
        let end = (start + batch_size).min(total);

        if start < end {
            for memory in &memories[start..end] {
                let mut current_tags = existing_tags(memory);

                if !current_tags.is_empty() && memory.has_tags_vector {
                    // The skip rule: re-embedding is a no-op.
                    self.progress.processed += 1;
                    continue;
                }

                if current_tags.is_empty() {
                    match generation.generate_tags(&memory.id, &memory.content) {
                        Ok(tags) => {
                            current_tags = tags;
                        }
                        Err(message) => {
                            self.progress.errors.push(format!(
                                "Tag generation failed for memory {}: {}",
                                memory.id, message
                            ));
                            // Soft failure: not processed, window moves on.
                            continue;
                        }
                    }
                    if let Err(error) = sink.write_tags_csv(&memory.id, &current_tags.join(",")) {
                        self.progress.errors.push(error);
                        continue;
                    }
                }

                let content_vector = match sink.embed_document(&memory.content) {
                    Ok(vector) => vector,
                    Err(error) => {
                        self.progress.errors.push(error);
                        continue;
                    }
                };
                let tags_vector = if current_tags.is_empty() {
                    None
                } else {
                    match sink.embed_document(&format_tags_for_embedding(&current_tags)) {
                        Ok(vector) => Some(vector),
                        Err(error) => {
                            self.progress.errors.push(error);
                            continue;
                        }
                    }
                };
                if let Err(error) =
                    sink.update_vector(&memory.id, &content_vector, tags_vector.as_deref())
                {
                    self.progress.errors.push(error);
                    continue;
                }

                self.progress.processed += 1;
            }
        }

        self.progress.cursor = end;
        self.progress.current_batch += 1;
        let has_more = self.progress.cursor < total;
        if !has_more {
            self.progress.is_complete = true;
        }

        BatchOutcome {
            processed: self.progress.processed,
            total,
            has_more,
            errors: self.progress.errors.len(),
        }
    }
}

/// Upstream `m.tags.split(",").map(t => t.trim().toLowerCase()).filter(Boolean)`.
/// `tags: None` (or empty) means untagged → empty list.
fn existing_tags(memory: &MemoryRecord) -> Vec<String> {
    match memory.tags.as_deref() {
        Some(raw) => raw
            .split(',')
            .map(|tag| tag.trim().to_lowercase())
            .filter(|tag| !tag.is_empty())
            .collect(),
        None => Vec::new(),
    }
}
