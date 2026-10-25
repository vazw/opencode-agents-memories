//! The profile endpoints (upstream `api-handlers.ts` L976–1485).
//!
//! The wave's data-protection contract lives here: every profile payload that
//! leaves the server goes through
//! `mem_profile::profile_utils::to_public_profile_data`, so no `centroid` and
//! no `anchor` is ever serialized. The display order is upstream's
//! `sortProfileItems`: preferences by descending `confidence`, patterns and
//! workflows by descending `frequency`, and frozen items
//! (`driftBelowCount >= 2`) last.

use std::cmp::Ordering;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Query, State};
use axum::response::Response;
use mem_profile::profile_utils::to_public_profile_data;
use serde::Deserialize;
use serde_json::Value;

use super::dto::{
    ChangelogDto, CleanupPreviewDto, ProfileDto, ProfileErasureDto, ProfileMissingDto,
    ProfileMutationDto, ProfileSnapshotDto, RefreshProfileDto, safe_to_iso_string,
};
use super::envelope;
use super::ports::PortError;
use super::{json_body_or_default, port_failure, query_first};
use crate::server::AppState;

/// The message of the no-profile answer (upstream's literal).
const NO_PROFILE_MESSAGE: &str = "No profile found. Keep chatting to build your profile.";
/// The refresh note (issue 48.13 — D-4).
const REFRESH_NOTE: &str =
    "Confidence decay runs immediately. Automatic profile learning is disabled in this release.";

/// `{userId?}` — the optional body of `POST /api/user-profile/refresh`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UserIdRequest {
    user_id: Option<String>,
}

/// `POST /api/user-profile/ai-cleanup` body.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AiCleanupRequest {
    user_id: Option<String>,
    include_ids: Option<Vec<String>>,
    profile_version: Option<i64>,
}

/// `PATCH /api/user-profile/item` body.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateProfileItemRequest {
    #[serde(rename = "type")]
    item_type: Option<String>,
    index: Option<i64>,
    action: Option<String>,
    category: Option<String>,
    description: Option<String>,
    steps: Option<Vec<String>>,
}

/// `GET /api/user-profile` — upstream `handleGetUserProfile`.
pub async fn get_user_profile(
    State(state): State<Arc<AppState>>,
    Query(query): Query<Vec<(String, String)>>,
) -> Response {
    let user_id = resolved_user_id(query_first(&query, "userId"), &state);
    let profile = match state.api.profile.active_profile(&user_id).await {
        Ok(profile) => profile,
        Err(error) => return profile_failure("GET /api/user-profile", error),
    };
    let Some(profile) = profile else {
        return envelope::ok(&ProfileMissingDto {
            exists: false,
            user_id,
            message: NO_PROFILE_MESSAGE,
        });
    };

    let mut data = match parse_profile_data(&profile.profile_data, "GET /api/user-profile") {
        Ok(data) => data,
        Err(response) => return response,
    };
    sort_profile_items(&mut data);

    envelope::ok(&ProfileDto {
        exists: true,
        id: profile.id,
        user_id: profile.user_id,
        display_name: profile.display_name,
        user_name: profile.user_name,
        user_email: profile.user_email,
        version: profile.version,
        created_at: safe_to_iso_string(Some(profile.created_at)),
        last_analyzed_at: safe_to_iso_string(Some(profile.last_analyzed_at)),
        total_prompts_analyzed: profile.total_prompts_analyzed,
        profile_data: to_public_profile_data(&data),
    })
}

/// `DELETE /api/user-profile` — the port-only erasure route (issue 48.14,
/// 47.1-B2′).
///
/// Why it exists: B2′ fires on a **prompt** delete, which can never happen on
/// the population the audit left live — a migrated root with a profile and
/// **zero prompts**. This is also the first user-invocable caller of
/// `delete_profile` (D-2: `mem-portability`'s import rollback already called
/// the store method, but only as an internal compensation).
///
/// Idempotent by construction: the underlying two-table delete reports 0 rows
/// for an already-absent profile, so a retry after a partial profile-DB
/// failure finishes the job rather than failing.
///
/// Residual (accepted for this wave, documented rather than fixed): the route
/// resolves only the **active** profile, so a root whose only `user_profiles`
/// row has `is_active = 0` is erasable through the prompt path (which scans
/// every row) but not through this route. A future route taking a `profileId`
/// would close it; widening this one would put the client in charge of the id.
pub async fn delete_user_profile(
    State(state): State<Arc<AppState>>,
    Query(query): Query<Vec<(String, String)>>,
) -> Response {
    let user_id = resolved_user_id(query_first(&query, "userId"), &state);
    let profile = match state.api.profile.active_profile(&user_id).await {
        Ok(profile) => profile,
        Err(error) => return profile_failure("DELETE /api/user-profile", error),
    };
    // Mirrors `PATCH …/item`'s literal; the GET answer keeps its
    // `ProfileMissingDto` shape on purpose — a DELETE has no "successfully
    // nothing" payload to describe.
    let Some(profile) = profile else {
        return envelope::business_error("No profile found");
    };

    match state.api.profile.delete_profile(&profile.id).await {
        Ok(erasure) => envelope::ok(&ProfileErasureDto {
            message: "Profile erased",
            profile_id: profile.id,
            profiles_deleted: erasure.profiles_deleted,
            changelogs_deleted: erasure.changelogs_deleted,
        }),
        Err(error) => profile_failure("DELETE /api/user-profile", error),
    }
}

/// `GET /api/user-profile/changelog` — upstream `handleGetProfileChangelog`.
///
/// The route layer requires `profileId` (its message is the literal
/// `"profileId parameter required"`, `web-server.ts:665`).
pub async fn get_profile_changelog(
    State(state): State<Arc<AppState>>,
    Query(query): Query<Vec<(String, String)>>,
) -> Response {
    let Some(profile_id) = query_first(&query, "profileId").filter(|id| !id.is_empty()) else {
        return envelope::business_error("profileId parameter required");
    };
    let limit = query_first(&query, "limit")
        .and_then(|raw| raw.parse::<usize>().ok())
        .unwrap_or(5);

    let changelogs = match state.api.profile.changelogs(profile_id, limit).await {
        Ok(changelogs) => changelogs,
        Err(error) => return profile_failure("GET /api/user-profile/changelog", error),
    };
    let items: Vec<ChangelogDto> = changelogs
        .into_iter()
        .map(|changelog| ChangelogDto {
            id: changelog.id,
            profile_id: changelog.profile_id,
            version: changelog.version,
            change_type: changelog.change_type,
            change_summary: changelog.change_summary,
            created_at: safe_to_iso_string(Some(changelog.created_at)),
        })
        .collect();

    envelope::ok(&items)
}

/// `GET /api/user-profile/snapshot` — upstream `handleGetProfileSnapshot`.
///
/// The parameter is upstream's typo `chlogId` (the UI sends exactly that); the
/// route layer answers `"changelogId parameter required"` when it is absent.
pub async fn get_profile_snapshot(
    State(state): State<Arc<AppState>>,
    Query(query): Query<Vec<(String, String)>>,
) -> Response {
    let Some(changelog_id) = query_first(&query, "chlogId").filter(|id| !id.is_empty()) else {
        return envelope::business_error("changelogId parameter required");
    };

    let changelog = match state.api.profile.changelog_by_id(changelog_id).await {
        Ok(Some(changelog)) => changelog,
        Ok(None) => return envelope::business_error("Changelog not found"),
        Err(error) => return profile_failure("GET /api/user-profile/snapshot", error),
    };

    let data = match parse_profile_data(
        &changelog.profile_data_snapshot,
        "GET /api/user-profile/snapshot",
    ) {
        Ok(data) => data,
        Err(response) => return response,
    };

    envelope::ok(&ProfileSnapshotDto {
        version: changelog.version,
        created_at: safe_to_iso_string(Some(changelog.created_at)),
        profile_data: to_public_profile_data(&data),
    })
}

/// `POST /api/user-profile/refresh` — upstream `handleRefreshProfile`.
pub async fn refresh_profile(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let request: UserIdRequest = json_body_or_default(&body);
    let user_id = resolved_user_id(request.user_id.as_deref(), &state);

    let profile = match state.api.profile.active_profile(&user_id).await {
        Ok(profile) => profile,
        Err(error) => return profile_failure("POST /api/user-profile/refresh", error),
    };

    let mut decay_applied = false;
    if let Some(profile) = &profile {
        let data = match parse_profile_data(&profile.profile_data, "POST /api/user-profile/refresh")
        {
            Ok(data) => data,
            Err(response) => return response,
        };
        let (decayed, has_changes) = match state.api.profile.decay(&data).await {
            Ok(outcome) => outcome,
            Err(error) => return profile_failure("POST /api/user-profile/refresh", error),
        };
        if has_changes {
            match state
                .api
                .profile
                .update_profile(&profile.id, &decayed, "Applied confidence decay")
                .await
            {
                Ok(true) => decay_applied = true,
                Ok(false) => {}
                Err(error) => return profile_failure("POST /api/user-profile/refresh", error),
            }
        }
    }

    envelope::ok(&RefreshProfileDto {
        message: if decay_applied {
            "Profile confidence decay applied"
        } else {
            "Profile refresh queued"
        },
        profile_exists: profile.is_some(),
        decay_applied,
        note: REFRESH_NOTE,
    })
}

/// `POST /api/user-profile/ai-cleanup` — upstream `handleAICleanup`.
pub async fn ai_cleanup(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let request: AiCleanupRequest = json_body_or_default(&body);
    let user_id = resolved_user_id(request.user_id.as_deref(), &state);

    let profile = match state.api.profile.active_profile(&user_id).await {
        Ok(profile) => profile,
        Err(error) => return profile_failure("POST /api/user-profile/ai-cleanup", error),
    };
    let Some(profile) = profile else {
        return envelope::business_error("No profile found to clean up");
    };
    if let Some(version) = request.profile_version
        && version != profile.version
    {
        return envelope::business_error("Profile changed. Reload it before running AI cleanup.");
    }

    let mut data =
        match parse_profile_data(&profile.profile_data, "POST /api/user-profile/ai-cleanup") {
            Ok(data) => data,
            Err(response) => return response,
        };
    sort_profile_items(&mut data);
    let include_ids = request.include_ids.filter(|ids| !ids.is_empty());

    let preview = match state
        .api
        .profile
        .ai_cleanup(&user_id, profile.version, &data, include_ids)
        .await
    {
        Ok(preview) => preview,
        Err(error) => return profile_failure("POST /api/user-profile/ai-cleanup", error),
    };

    // Strip embeddings only on the HTTP response; the port keeps the full
    // vectors for the later apply/merge.
    envelope::ok(&CleanupPreviewDto {
        old: to_public_profile_data(&data),
        new: to_public_profile_data(&preview.cleaned),
        changes: preview.changes,
    })
}

/// `POST /api/user-profile/ai-cleanup/apply` — upstream `handleApplyCleanup`.
pub async fn apply_cleanup(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let value: Value = json_body_or_default(&body);
    let body_user_id = value.get("userId").and_then(Value::as_str);
    let user_id = resolved_user_id(body_user_id, &state);

    match state.api.profile.apply_cleanup(&user_id, &value).await {
        Ok(applied) => envelope::ok(&ProfileMutationDto {
            message: applied.message,
            version: applied.version,
        }),
        Err(error) => profile_failure("POST /api/user-profile/ai-cleanup/apply", error),
    }
}

/// `PATCH /api/user-profile/item` — upstream `handleUpdateProfileItem`.
pub async fn update_profile_item(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let request: UpdateProfileItemRequest = json_body_or_default(&body);
    let user_id = state.user_id.clone();

    let profile = match state.api.profile.active_profile(&user_id).await {
        Ok(profile) => profile,
        Err(error) => return profile_failure("PATCH /api/user-profile/item", error),
    };
    let Some(profile) = profile else {
        return envelope::business_error("No profile found");
    };

    let UpdateProfileItemRequest {
        item_type,
        index,
        action,
        category,
        description,
        steps,
    } = request;
    let (Some(item_type), Some(index), Some(action)) = (item_type, index, action) else {
        return envelope::business_error("type, index, and action are required");
    };
    if !matches!(item_type.as_str(), "preferences" | "patterns" | "workflows") {
        return envelope::business_error("type must be preferences, patterns, or workflows");
    }
    if !matches!(action.as_str(), "edit" | "delete") {
        return envelope::business_error("action must be edit or delete");
    }

    let mut data = match parse_profile_data(&profile.profile_data, "PATCH /api/user-profile/item") {
        Ok(data) => data,
        Err(response) => return response,
    };
    if !data.is_object() {
        return port_failure(
            "PATCH /api/user-profile/item",
            PortError::Store("profile data is not a JSON object".to_string()),
        );
    }

    let mut items: Vec<Value> = data
        .get(&item_type)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    // Re-sort to match `GET /api/user-profile`'s display order (the index the
    // UI sends is an index into the sorted list).
    sort_items(
        &mut items,
        if item_type == "preferences" {
            "confidence"
        } else {
            "frequency"
        },
    );

    if index < 0 || index as usize >= items.len() {
        return envelope::business_error("index out of range");
    }
    let index = index as usize;

    if action == "delete" {
        items.remove(index);
    } else {
        let Some(item) = items.get_mut(index) else {
            return envelope::business_error("Item not found");
        };
        if let Some(object) = item.as_object_mut() {
            if item_type != "workflows"
                && let Some(category) = category
            {
                object.insert("category".to_string(), Value::String(category));
            }
            if let Some(description) = description
                && object.get("description").and_then(Value::as_str) != Some(description.as_str())
            {
                object.insert("description".to_string(), Value::String(description));
                // The old vectors no longer describe the item.
                object.remove("centroid");
                object.remove("anchor");
            }
            if item_type == "workflows"
                && let Some(steps) = steps
            {
                object.insert(
                    "steps".to_string(),
                    Value::Array(steps.into_iter().map(Value::String).collect()),
                );
            }
        }
    }

    if let Some(container) = data.as_object_mut() {
        container.insert(item_type.clone(), Value::Array(items));
    }

    let singular = item_type.strip_suffix('s').unwrap_or(&item_type);
    let change_summary = if action == "delete" {
        format!("Deleted {singular} at index {index}")
    } else {
        format!("Edited {singular} at index {index}")
    };

    match state
        .api
        .profile
        .update_profile(&profile.id, &data, &change_summary)
        .await
    {
        Ok(true) => envelope::ok(&ProfileMutationDto {
            message: format!("{action} successful"),
            version: profile.version + 1,
        }),
        Ok(false) => {
            envelope::business_error("Profile was modified by another session. Please retry.")
        }
        Err(error) => profile_failure("PATCH /api/user-profile/item", error),
    }
}

/// The resolved user identity: the request's non-empty `userId`, else the
/// identity the composition root supplied (upstream `getTags(...).userEmail ||
/// "unknown"`).
fn resolved_user_id(requested: Option<&str>, state: &Arc<AppState>) -> String {
    requested
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| state.user_id.clone())
}

/// Maps a profile port failure: an upstream business message is answered at
/// HTTP 200 with `success:false`, everything else is the fixed `500` (D-9).
fn profile_failure(endpoint: &'static str, error: PortError) -> Response {
    match error {
        PortError::Invalid(message) => envelope::business_error(&message),
        other => port_failure(endpoint, other),
    }
}

/// Parses the stored `profile_data` JSON text. A corrupt column is a store
/// fault (D-9: the detail goes to the log, never to the client).
fn parse_profile_data(raw: &str, endpoint: &'static str) -> Result<Value, Response> {
    serde_json::from_str(raw).map_err(|_| {
        port_failure(
            endpoint,
            PortError::Store("profile data is not valid JSON".to_string()),
        )
    })
}

/// Upstream `sortProfileItems` over the three sections of a profile payload.
fn sort_profile_items(data: &mut Value) {
    let Some(container) = data.as_object_mut() else {
        return;
    };
    for (section, metric) in [
        ("preferences", "confidence"),
        ("patterns", "frequency"),
        ("workflows", "frequency"),
    ] {
        let Some(items) = container.get_mut(section).and_then(Value::as_array_mut) else {
            continue;
        };
        sort_items(items, metric);
    }
}

/// Upstream `sortProfileItems`: frozen items last, then the metric descending
/// (preferences tie-break on `frequency`, where `0`/missing counts as `1`).
fn sort_items(items: &mut [Value], metric: &str) {
    items.sort_by(|left, right| compare_items(left, right, metric));
}

fn compare_items(left: &Value, right: &Value, metric: &str) -> Ordering {
    let left_frozen = is_frozen(left);
    let right_frozen = is_frozen(right);
    if left_frozen != right_frozen {
        return left_frozen.cmp(&right_frozen);
    }

    let primary = metric_value(right, metric).total_cmp(&metric_value(left, metric));
    if primary != Ordering::Equal {
        return primary;
    }
    if metric == "confidence" {
        return frequency_or_one(right).total_cmp(&frequency_or_one(left));
    }
    Ordering::Equal
}

/// Upstream `isFrozen`: two or more consecutive observations below the drift
/// threshold.
fn is_frozen(item: &Value) -> bool {
    item.get("driftBelowCount")
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
        >= 2.0
}

/// Upstream `item[metric] || 0`.
fn metric_value(item: &Value, metric: &str) -> f64 {
    item.get(metric)
        .and_then(Value::as_f64)
        .filter(|value| *value != 0.0)
        .unwrap_or(0.0)
}

/// Upstream `item.frequency || 1` (a missing or zero frequency counts as 1).
fn frequency_or_one(item: &Value) -> f64 {
    item.get("frequency")
        .and_then(Value::as_f64)
        .filter(|value| *value != 0.0)
        .unwrap_or(1.0)
}
