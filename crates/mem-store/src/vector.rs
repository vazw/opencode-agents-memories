//! Vector subset — the WRITE + aggregation half of upstream
//! `vector-search.ts` (blueprint W1.2 step 7; search/filters in W1.3).
//!
//! Column/param order mirrors upstream `insertArgs` exactly (17 params).
//! Divergence: upstream `insertVector` calls `prepareShardDb` first; here
//! schema preparation lives with the shard manager (step 8 wiring), so
//! `insert_vector` only inserts. `vector32(?)` text params are the same
//! JSON strings upstream feeds (`vectorToJson` + serde_json, with
//! NaN/Infinity mapping to `null` like `JSON.stringify`).

#![allow(dead_code)]

use crate::db::{Db, DbHandle, Row, Value};
use crate::error::StoreError;

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

/// Upstream `parseSessionIdFromMetadata`: JSON object's `sessionID` string.
fn parse_session_id_from_metadata(metadata: Option<&str>) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(metadata?).ok()?;
    value.get("sessionID")?.as_str().map(|id| id.to_string())
}

/// Upstream `resolveSessionId`: explicit non-empty wins, else metadata.
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

/// Upstream `insertVector` (no `prepareShardDb` — manager wiring, step 8).
pub async fn insert_vector(db: &dyn Db, value: &MemoryInsert) -> Result<(), StoreError> {
    if value.tags_vector.is_some() {
        db.execute(crate::sql::INSERT_WITH_TAGS, insert_args(value))
            .await?;
        return Ok(());
    }
    db.execute(crate::sql::INSERT_WITHOUT_TAGS, insert_args(value))
        .await?;
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
