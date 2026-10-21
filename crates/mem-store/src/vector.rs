//! Vector subset — the WRITE + aggregation half landed in W1.2 (step 7);
//! the SEARCH half (exact scans, hybrid rank, session lookup, pins) lands
//! in W1.3 (step 3). Both halves port upstream `vector-search.ts`.
//!
//! `search_across_shards` is OUT OF SCOPE this wave (no upstream test in
//! the 8 mapped files; the API/tools wave owns it) — note, no stub.
//!
//! Column/param order mirrors upstream `insertArgs` exactly (17 params).
//! Divergence: upstream `insertVector` calls `prepareShardDb` first; here
//! schema preparation lives with the shard manager (step 8 wiring), so
//! `insert_vector` only inserts — and `search_in_shard` takes the `Db`
//! seam instead of `(shard: ShardInfo)` and never prepares either
//! (recorded). `vector32(?)` text params are the same JSON strings
//! upstream feeds (`vectorToJson` + serde_json, with NaN/Infinity mapping
//! to `null` like `JSON.stringify`).

#![allow(dead_code)]

use crate::db::{Db, DbHandle, Row, Statement, Value};
use crate::error::StoreError;
use crate::vector_utils::{
    distance_to_similarity, escape_like_pattern, parse_session_id_from_metadata,
    tokenize_query_text,
};

/// Upstream `MemoryRecord` write-shape (vector-search insert path only).
#[derive(Debug, Clone)]
pub struct MemoryInsert {
    pub id: String,
    pub content: String,
    pub vector: Vec<f32>,
    pub tags_vector: Option<Vec<f32>>,
    pub container_tag: String,
    pub tags: Option<String>,
    pub memory_type: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub metadata: Option<String>,
    pub session_id: Option<String>,
    pub display_name: Option<String>,
    pub user_name: Option<String>,
    pub user_email: Option<String>,
    pub project_path: Option<String>,
    pub project_name: Option<String>,
    pub git_repo_url: Option<String>,
}

/// Upstream `vectorToJson` (serde_json maps non-finite floats to `null`,
/// matching `JSON.stringify`).
pub fn vector_to_text(vector: &[f32]) -> String {
    serde_json::to_string(vector).expect("float serialization cannot fail")
}

/// Upstream `resolveSessionId`: explicit non-empty wins, else metadata.
/// `parseSessionIdFromMetadata` moved to `vector_utils` (single home).
pub fn resolve_session_id(session_id: Option<String>, metadata: Option<&str>) -> Option<String> {
    if let Some(id) = session_id.filter(|id| !id.is_empty()) {
        return Some(id);
    }
    parse_session_id_from_metadata(metadata)
}

/// Upstream `insertArgs` — param order pinned to the frozen INSERTs.
fn insert_args(value: &MemoryInsert) -> Vec<Value> {
    let content_vector = vector_to_text(&value.vector);
    let mut params = vec![
        Value::Text(value.id.clone()),
        Value::Text(value.content.clone()),
        Value::Text(content_vector),
    ];
    if let Some(tags_vector) = &value.tags_vector {
        params.push(Value::Text(vector_to_text(tags_vector)));
    }
    params.extend([
        Value::Text(value.container_tag.clone()),
        value.tags.clone().map(Value::Text).unwrap_or(Value::Null),
        value
            .memory_type
            .clone()
            .map(Value::Text)
            .unwrap_or(Value::Null),
        Value::Integer(value.created_at),
        Value::Integer(value.updated_at),
        value
            .metadata
            .clone()
            .map(Value::Text)
            .unwrap_or(Value::Null),
        resolve_session_id(value.session_id.clone(), value.metadata.as_deref())
            .map(Value::Text)
            .unwrap_or(Value::Null),
        value
            .display_name
            .clone()
            .map(Value::Text)
            .unwrap_or(Value::Null),
        value
            .user_name
            .clone()
            .map(Value::Text)
            .unwrap_or(Value::Null),
        value
            .user_email
            .clone()
            .map(Value::Text)
            .unwrap_or(Value::Null),
        value
            .project_path
            .clone()
            .map(Value::Text)
            .unwrap_or(Value::Null),
        value
            .project_name
            .clone()
            .map(Value::Text)
            .unwrap_or(Value::Null),
        value
            .git_repo_url
            .clone()
            .map(Value::Text)
            .unwrap_or(Value::Null),
    ]);
    params
}

/// Upstream `insertVector` statement form — the frozen INSERT chosen by
/// `tags_vector.is_some()` plus the frozen `insert_args` params. Consumed by
/// `insert_vector` and the migration/re-embed batch write paths.
pub fn insert_vector_statement(value: &MemoryInsert) -> Statement {
    let params = insert_args(value);
    if value.tags_vector.is_some() {
        Statement::new(crate::sql::INSERT_WITH_TAGS, params)
    } else {
        Statement::new(crate::sql::INSERT_WITHOUT_TAGS, params)
    }
}

/// Upstream `insertVector` (no `prepareShardDb` — manager wiring, step 8).
pub async fn insert_vector(db: &dyn Db, value: &MemoryInsert) -> Result<(), StoreError> {
    let statement = insert_vector_statement(value);
    db.execute(&statement.sql, statement.params).await?;
    Ok(())
}

/// Upstream `countAllVectors`.
pub async fn count_all_vectors(db: &dyn Db) -> u64 {
    crate::db::query_one(db, crate::sql::COUNT_ALL_VECTORS, Vec::new())
        .await
        .ok()
        .flatten()
        .and_then(|row| row.i64("count"))
        .unwrap_or(0) as u64
}

/// Upstream `countVectors` (per container tag).
pub async fn count_vectors_by_container_tag(db: &dyn Db, container_tag: &str) -> u64 {
    crate::db::query_one(
        db,
        crate::sql::COUNT_VECTORS_BY_CONTAINER_TAG,
        vec![Value::Text(container_tag.to_string())],
    )
    .await
    .ok()
    .flatten()
    .and_then(|row| row.i64("count"))
    .unwrap_or(0) as u64
}

/// Upstream `getAllMemories` (`SELECT *`, `created_at DESC`).
pub async fn get_all_memories(db: &dyn Db) -> Vec<Row> {
    db.query(crate::sql::GET_ALL_MEMORIES, Vec::new())
        .await
        .unwrap_or_default()
}

/// Upstream `getDistinctTags`.
pub async fn get_distinct_tags(db: &dyn Db) -> Vec<Row> {
    db.query(crate::sql::GET_DISTINCT_TAGS, Vec::new())
        .await
        .unwrap_or_default()
}

/// Upstream `getProjectPathCounts` row shape.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectPathCount {
    pub project_path: String,
    pub count: u64,
    pub container_tag: Option<String>,
}

/// Upstream `getProjectPathCounts` (`cnt DESC, project_path ASC`, NULL/'' cut).
pub async fn get_project_path_counts(db: &dyn Db) -> Vec<ProjectPathCount> {
    db.query(crate::sql::GET_PROJECT_PATH_COUNTS, Vec::new())
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|row| ProjectPathCount {
            project_path: row.text("project_path").unwrap_or_default(),
            count: row.i64("cnt").unwrap_or(0) as u64,
            container_tag: row.text("container_tag"),
        })
        .collect()
}

/// Upstream `updateProjectAssociation` update shape.
#[derive(Debug, Clone)]
pub struct ProjectAssociation {
    pub container_tag: String,
    pub project_path: Option<String>,
    pub project_name: Option<String>,
    pub display_name: Option<String>,
    pub git_repo_url: Option<String>,
}

/// Upstream `updateProjectAssociation` — returns rows changed.
pub async fn update_project_association(
    db: &dyn Db,
    old_container_tag: &str,
    update: &ProjectAssociation,
) -> Result<u64, StoreError> {
    let result = db
        .execute(
            crate::sql::UPDATE_PROJECT_ASSOCIATION,
            vec![
                Value::Text(update.container_tag.clone()),
                update
                    .project_path
                    .clone()
                    .map(Value::Text)
                    .unwrap_or(Value::Null),
                update
                    .project_name
                    .clone()
                    .map(Value::Text)
                    .unwrap_or(Value::Null),
                update
                    .display_name
                    .clone()
                    .map(Value::Text)
                    .unwrap_or(Value::Null),
                update
                    .git_repo_url
                    .clone()
                    .map(Value::Text)
                    .unwrap_or(Value::Null),
                Value::Text(old_container_tag.to_string()),
            ],
        )
        .await?;
    Ok(result.changes)
}

/// Convenience adapter used by callers holding a handle.
pub async fn insert_vector_handle(db: &DbHandle, value: &MemoryInsert) -> Result<(), StoreError> {
    insert_vector(db.as_ref(), value).await
}

// ---------------------------------------------------------------------------
// W1.3 search half (blueprint step 3) — port of upstream `vector-search.ts`
// `searchInShard` (L99–208), `keywordScores` (L213–246), `exactScanKind`
// (L248–278), `getMemoriesBySessionID` (L407–424), `pinMemory`/`unpinMemory`
// (L504–510). `searchAcrossShards` is out of scope this wave (see module doc).
// ---------------------------------------------------------------------------

/// Which vector column an exact scan ranks — upstream's `columnName`
/// argument (`"vector"` / `"tags_vector"`) made type-safe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VectorColumn {
    Content,
    Tags,
}

/// Upstream `SearchResult` — field-for-field the hydrated object
/// (`session_id` is selected but NOT mapped upstream; do not add it).
#[derive(Debug, Clone, PartialEq)]
pub struct SearchResult {
    pub id: String,
    pub memory: String,
    pub similarity: f64,
    pub created_at: i64,
    pub tags: Vec<String>,
    pub metadata: Option<serde_json::Value>,
    pub container_tag: String,
    pub display_name: Option<String>,
    pub user_name: Option<String>,
    pub user_email: Option<String>,
    pub project_path: Option<String>,
    pub project_name: Option<String>,
    pub git_repo_url: Option<String>,
    pub is_pinned: Option<i64>,
}

/// Upstream `parseMetadata` (L14–21). Divergence (recorded): only JSON
/// objects survive — arrays are rejected (upstream's cast merely lied).
fn parse_metadata(value: Option<&Value>) -> Option<serde_json::Value> {
    let text = match value {
        Some(Value::Text(text)) if !text.is_empty() => text,
        _ => return None,
    };
    let parsed: serde_json::Value = serde_json::from_str(text).ok()?;
    parsed.is_object().then_some(parsed)
}

/// Upstream `exactScanKind` (L248–278) over the frozen `SEARCH_EXACT_*`
/// consts — never `vector_top_k`, plain `ORDER BY dist ASC LIMIT ?`.
/// Returns `(id, similarity)` rows in scan order.
pub async fn exact_scan_kind(
    db: &dyn Db,
    query_json: &str,
    k: usize,
    container_tag: &str,
    column: VectorColumn,
) -> Result<Vec<(String, f64)>, StoreError> {
    let filtered = !container_tag.is_empty();
    let sql = match (column, filtered) {
        (VectorColumn::Content, false) => crate::sql::SEARCH_EXACT_VECTOR_ALL,
        (VectorColumn::Content, true) => crate::sql::SEARCH_EXACT_VECTOR_TAGGED,
        (VectorColumn::Tags, false) => crate::sql::SEARCH_EXACT_TAGS_VECTOR_ALL,
        (VectorColumn::Tags, true) => crate::sql::SEARCH_EXACT_TAGS_VECTOR_TAGGED,
    };
    let mut params = vec![Value::Text(query_json.to_string())];
    if filtered {
        params.push(Value::Text(container_tag.to_string()));
    }
    params.push(Value::Integer(k as i64));
    let rows = db.query(sql, params).await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            (
                row.text("id").unwrap_or_default(),
                // WHERE <column> IS NOT NULL pins dist non-NULL; the
                // unwrap_or mirrors upstream's Number() coercion.
                distance_to_similarity(row.f64("dist").unwrap_or(0.0)),
            )
        })
        .collect())
}

/// Upstream `keywordScores` (L213–246): per token, `%{escaped}%` LIKE over
/// content/tags, each hit adds `1 / tokens.len()` on top of the id's
/// previous score. Insertion-ordered like the JS `Map` (a repeat hit
/// updates its slot in place); `Vec` + linear update mirrors it.
pub async fn keyword_scores(
    db: &dyn Db,
    query_text: Option<&str>,
    container_tag: &str,
    limit: usize,
) -> Result<Vec<(String, f64)>, StoreError> {
    let tokens = tokenize_query_text(query_text);
    if tokens.is_empty() {
        return Ok(Vec::new());
    }
    let filtered = !container_tag.is_empty();
    let mut scores: Vec<(String, f64)> = Vec::new();
    let bump = 1.0 / tokens.len() as f64;
    for token in &tokens {
        let pattern = format!("%{}%", escape_like_pattern(token));
        let sql = if filtered {
            crate::sql::KEYWORD_SCORES_TAGGED
        } else {
            crate::sql::KEYWORD_SCORES_ALL
        };
        let mut params = Vec::with_capacity(4);
        if filtered {
            params.push(Value::Text(container_tag.to_string()));
        }
        params.push(Value::Text(pattern.clone()));
        params.push(Value::Text(pattern));
        params.push(Value::Integer(limit as i64));
        let rows = db.query(sql, params).await?;
        for row in rows {
            let id = row.text("id").unwrap_or_default();
            match scores.iter_mut().find(|(seen, _)| *seen == id) {
                Some((_, score)) => *score += bump,
                None => scores.push((id, bump)),
            }
        }
    }
    Ok(scores)
}

/// Upstream `searchInShard` (L99–208), byte-order-faithful. Divergence
/// (recorded): takes the `Db` seam and does NOT prepare the schema —
/// preparation is manager-owned since W1.2.
pub async fn search_in_shard(
    db: &dyn Db,
    query_vector: &[f32],
    container_tag: &str,
    limit: usize,
    query_text: Option<&str>,
) -> Result<Vec<SearchResult>, StoreError> {
    let query_json = vector_to_text(query_vector);
    // Exact cosine already returns the best-k rows; over-fetch a little so
    // content+tags+keyword candidates can merge before the hybrid re-rank.
    let k = (limit.saturating_mul(2)).max(32);

    let content_hits =
        exact_scan_kind(db, &query_json, k, container_tag, VectorColumn::Content).await?;
    let tags_hits = exact_scan_kind(db, &query_json, k, container_tag, VectorColumn::Tags).await?;
    let keyword = keyword_scores(db, query_text, container_tag, k).await?;

    // Insertion-ordered candidate ids (content hits, then tags hits, then
    // keyword ids) — mirrors the JS `Set` merge.
    let mut seen = std::collections::HashSet::new();
    let mut ids: Vec<String> = Vec::new();
    for id in content_hits
        .into_iter()
        .map(|(id, _)| id)
        .chain(tags_hits.into_iter().map(|(id, _)| id))
        .chain(keyword.iter().map(|(id, _)| id.clone()))
    {
        if seen.insert(id.clone()) {
            ids.push(id);
        }
    }
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    // Placeholder-count clamp (the frozen hydrate builder clamps the same
    // way; ids stay BOUND PARAMS — never interpolated).
    ids.truncate(1000);

    let filtered = !container_tag.is_empty();
    let sql = crate::sql::hydrate_candidates_sql(ids.len(), filtered);
    let mut params = vec![
        Value::Text(query_json.clone()),
        Value::Text(query_json.clone()),
    ];
    params.extend(ids.into_iter().map(Value::Text));
    if filtered {
        params.push(Value::Text(container_tag.to_string()));
    }
    let rows = db.query(&sql, params).await?;

    let query_words = tokenize_query_text(query_text);
    let has_keyword = !query_words.is_empty();

    let mut results: Vec<SearchResult> = rows
        .into_iter()
        .map(|row| {
            let id = row.text("id").unwrap_or_default();
            // NULL dist (keyword-only candidate) coerces like upstream's
            // `Number(null)` → 0 before the similarity transform.
            let content_sim = distance_to_similarity(row.f64("content_dist").unwrap_or(0.0));
            let tags_sim = match row.f64("tags_dist") {
                Some(dist) => distance_to_similarity(dist),
                None => 0.0,
            };
            let memory_tags_str = row.text("tags").unwrap_or_default();
            // filter(Boolean): "".split(",") → [""], and "query".contains("")
            // is always true — trim/lowercase then drop empties first.
            let memory_tags: Vec<String> = memory_tags_str
                .split(',')
                .map(|tag| tag.trim().to_lowercase())
                .filter(|tag| !tag.is_empty())
                .collect();

            let mut exact_match_boost = 0.0;
            if !query_words.is_empty() && !memory_tags.is_empty() {
                let matches = query_words
                    .iter()
                    .filter(|word| {
                        memory_tags
                            .iter()
                            .any(|tag| tag.contains(word.as_str()) || word.contains(tag.as_str()))
                    })
                    .count();
                exact_match_boost = matches as f64 / query_words.len().max(1) as f64;
            }

            let keyword_sim = keyword
                .iter()
                .find(|(scored_id, _)| *scored_id == id)
                .map(|(_, score)| *score)
                .unwrap_or(0.0);
            let final_tags_sim = tags_sim.max(exact_match_boost);
            // With query text: blend vector + keyword. Without: classic 0.6/0.4.
            let similarity = if has_keyword {
                content_sim * 0.5 + final_tags_sim * 0.3 + keyword_sim * 0.2
            } else {
                content_sim * 0.6 + final_tags_sim * 0.4
            };

            SearchResult {
                id,
                memory: row.text("content").unwrap_or_default(),
                similarity,
                created_at: row.i64("created_at").unwrap_or(0),
                tags: if memory_tags_str.is_empty() {
                    Vec::new()
                } else {
                    memory_tags_str.split(',').map(str::to_string).collect()
                },
                metadata: parse_metadata(row.get("metadata")),
                container_tag: row.text("container_tag").unwrap_or_default(),
                // Upstream truthiness: empty string collapses to undefined.
                display_name: row.text("display_name").filter(|text| !text.is_empty()),
                user_name: row.text("user_name").filter(|text| !text.is_empty()),
                user_email: row.text("user_email").filter(|text| !text.is_empty()),
                project_path: row.text("project_path").filter(|text| !text.is_empty()),
                project_name: row.text("project_name").filter(|text| !text.is_empty()),
                git_repo_url: row.text("git_repo_url").filter(|text| !text.is_empty()),
                is_pinned: row.i64("is_pinned"),
            }
        })
        .collect();

    // Stable sort, similarity descending (always finite by construction:
    // every component is finite; partial_cmp guards defensively).
    results.sort_by(|a, b| {
        b.similarity
            .partial_cmp(&a.similarity)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(limit);
    Ok(results)
}

/// Upstream `getMemoriesBySessionID` (L407–424): direct `session_id` match
/// OR a metadata LIKE fallback for pre-v2 rows (`prepareShardDb` skipped —
/// manager-owned, recorded). Raw rows are returned as-is (the port's shape).
pub async fn get_memories_by_session_id(
    db: &dyn Db,
    session_id: &str,
) -> Result<Vec<Row>, StoreError> {
    let rows = db
        .query(
            crate::sql::GET_MEMORIES_BY_SESSION_ID,
            vec![
                Value::Text(session_id.to_string()),
                Value::Text(format!(r#"%"sessionID":"{session_id}"%"#)),
            ],
        )
        .await?;
    Ok(rows)
}

/// Upstream `pinMemory` (L504–506).
pub async fn pin_memory(db: &dyn Db, memory_id: &str) -> Result<(), StoreError> {
    db.execute(
        crate::sql::PIN_MEMORY,
        vec![Value::Text(memory_id.to_string())],
    )
    .await?;
    Ok(())
}

/// Upstream `unpinMemory` (L508–510).
pub async fn unpin_memory(db: &dyn Db, memory_id: &str) -> Result<(), StoreError> {
    db.execute(
        crate::sql::UNPIN_MEMORY,
        vec![Value::Text(memory_id.to_string())],
    )
    .await?;
    Ok(())
}
/// Upstream `listMemories` (L359–381): an empty tag selects every row, a
/// non-empty one the rows of exactly that container tag. Both branches order
/// `is_pinned DESC, created_at DESC` and take the limit as a bound integer.
pub async fn list_memories_by_container_tag(
    db: &dyn Db,
    container_tag: &str,
    limit: usize,
) -> Result<Vec<Row>, StoreError> {
    if container_tag.is_empty() {
        db.query(
            crate::sql::LIST_MEMORIES_ALL,
            vec![Value::Integer(limit as i64)],
        )
        .await
    } else {
        db.query(
            crate::sql::LIST_MEMORIES_BY_CONTAINER_TAG,
            vec![
                Value::Text(container_tag.to_string()),
                Value::Integer(limit as i64),
            ],
        )
        .await
    }
}
