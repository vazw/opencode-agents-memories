//! Vector helpers — port of upstream `src/services/turso/vector-utils.ts`
//! (blueprint W1.3 step 2), plus the moved `parseSessionIdFromMetadata`
//! (single home; `vector.rs` re-uses it via `resolve_session_id`).
//!
//! Fixture note: legacy test files store vectors as raw F32 little-endian
//! BLOBs (no header) — [`blob_to_float32_array`] decodes exactly that shape;
//! Turso's own `vector_extract()` output is JSON text, parsed by the same
//! function's `Text` branch. The Float32Array/ArrayBuffer/view branches of
//! the upstream JS are N-A: the `Value` seam has no such variants (recorded).

#![allow(dead_code)]

use crate::db::Value as DbValue;

/// Upstream `vectorToJson` — kept in `vector.rs` (serde_json maps non-finite
/// floats to `null`, matching `JSON.stringify`).

/// Upstream `blobToFloat32Array`: `null`/non-blob non-text → `None`;
/// `Value::Blob`: empty or `len % 4 != 0` → `None`, else raw F32 LE chunks;
/// `Value::Text`: JSON array of f64 → lossy `as f32`, parse failure → `None`.
pub fn blob_to_float32_array(value: &DbValue) -> Option<Vec<f32>> {
    match value {
        DbValue::Blob(bytes) => {
            if bytes.is_empty() || bytes.len() % 4 != 0 {
                return None;
            }
            Some(
                bytes
                    .chunks_exact(4)
                    .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                    .collect(),
            )
        }
        DbValue::Text(text) => {
            let parsed: serde_json::Value = serde_json::from_str(text).ok()?;
            let array = parsed.as_array()?;
            Some(
                array
                    .iter()
                    .map(|item| item.as_f64().unwrap_or_default() as f32)
                    .collect(),
            )
        }
        _ => None,
    }
}

/// Upstream `parseExtractedVector`: `Text` → JSON array (must be non-empty)
/// else `None`; otherwise [`blob_to_float32_array`].
pub fn parse_extracted_vector(value: &DbValue) -> Option<Vec<f32>> {
    match value {
        DbValue::Text(text) => {
            let parsed: serde_json::Value = serde_json::from_str(text).ok()?;
            let array = parsed.as_array()?;
            if array.is_empty() {
                return None;
            }
            Some(
                array
                    .iter()
                    .map(|item| item.as_f64().unwrap_or_default() as f32)
                    .collect(),
            )
        }
        other => blob_to_float32_array(other),
    }
}

/// Upstream `distanceToSimilarity`: `1 - distance`; non-finite → `0`;
/// clamped into `[0, 1]` (cosine float artifacts — pinned by tests).
pub fn distance_to_similarity(distance: f64) -> f64 {
    let similarity = 1.0 - distance;
    if !similarity.is_finite() {
        return 0.0;
    }
    similarity.clamp(0.0, 1.0)
}

/// Upstream `formatTagsForEmbedding` — byte-contract across write paths.
pub fn format_tags_for_embedding(tags: &[String]) -> String {
    format!("Topics: {}", tags.join(", "))
}

/// Upstream `tokenizeQueryText`: `None`/empty → `vec![]`; else lowercase,
/// split on any of whitespace `, . _ / -` (runs collapse like the JS
/// `/[\s,._/-]+/`), trim, drop single-char tokens, cap at 16.
pub fn tokenize_query_text(text: Option<&str>) -> Vec<String> {
    let Some(text) = text.filter(|text| !text.is_empty()) else {
        return Vec::new();
    };
    text.to_lowercase()
        .split(|ch: char| ch.is_whitespace() || matches!(ch, ',' | '.' | '_' | '/' | '-'))
        .map(str::trim)
        .filter(|token| token.chars().count() > 1)
        .take(16)
        .map(str::to_string)
        .collect()
}

/// Upstream `escapeLikePattern`: backslash FIRST, then `%`, then `_`
/// (order is the contract; pair with `ESCAPE '\'` in SQL).
pub fn escape_like_pattern(token: &str) -> String {
    token
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Upstream `parseSessionIdFromMetadata` (moved here from `vector.rs` —
/// single home): JSON object's non-empty string `sessionID`.
pub fn parse_session_id_from_metadata(metadata: Option<&str>) -> Option<String> {
    let metadata = metadata.filter(|text| !text.is_empty())?;
    let parsed: serde_json::Value = serde_json::from_str(metadata).ok()?;
    let session_id = parsed.get("sessionID")?.as_str()?;
    if session_id.is_empty() {
        return None;
    }
    Some(session_id.to_string())
}
