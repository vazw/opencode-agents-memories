//! `GET /api/stats` — upstream `api-handlers.ts` L796–833 (`handleStats`).
//!
//! Three different counts (X3): `byScope` counts a row when its container tag
//! carries `_user_` (user) or `_project_` (project, only when it is not a user
//! row — upstream's `else if`), `total` is their sum, and `byType` tallies
//! every row with a non-empty `type` column regardless of its scope marker.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::State;
use axum::response::Response;

use super::dto::{ByScopeDto, StatsDto};
use super::envelope;
use super::port_failure;
use crate::server::AppState;

/// The user-scope marker.
const USER_MARKER: &str = "_user_";
/// The project-scope marker.
const PROJECT_MARKER: &str = "_project_";

/// `GET /api/stats`.
pub async fn stats(State(state): State<Arc<AppState>>) -> Response {
    let shards = match state.api.memory.shards(None, None).await {
        Ok(shards) => shards,
        Err(error) => return port_failure("GET /api/stats", error),
    };

    let mut user_count = 0_u64;
    let mut project_count = 0_u64;
    let mut by_type: BTreeMap<String, u64> = BTreeMap::new();

    for shard in shards {
        let memories = match state.api.memory.all_memories(&shard).await {
            Ok(memories) => memories,
            Err(error) => return port_failure("GET /api/stats", error),
        };
        for memory in memories {
            if memory.container_tag.contains(USER_MARKER) {
                user_count += 1;
            } else if memory.container_tag.contains(PROJECT_MARKER) {
                project_count += 1;
            }
            if let Some(memory_type) = memory.memory_type.filter(|kind| !kind.is_empty()) {
                *by_type.entry(memory_type).or_insert(0) += 1;
            }
        }
    }

    envelope::ok(&StatsDto {
        total: user_count + project_count,
        by_scope: ByScopeDto {
            user: user_count,
            project: project_count,
        },
        by_type,
    })
}
