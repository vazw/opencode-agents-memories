//! Issue 70 wave 1 — the `profile` wire method: one method whose absent
//! `content` is a read and whose present `content` is an explicit-preference
//! write.

use std::sync::Arc;

use mem_profile::learning_lock::LearningLock;
use mem_profile::profile_store::ProfileStore;

use crate::scope_resolver::ProjectScopeResolver;

/// The `profile` method's service: the profile store, the production learning
/// lock (fail-closed) and the one scope resolver.
pub struct ProfileService {
    profiles: Arc<ProfileStore>,
    lock: Arc<LearningLock>,
    resolver: Arc<dyn ProjectScopeResolver>,
}

impl ProfileService {
    /// Explicit constructor (the production `ToolServices::build` and the
    /// tests use the same one).
    pub fn new(
        profiles: Arc<ProfileStore>,
        lock: Arc<LearningLock>,
        resolver: Arc<dyn ProjectScopeResolver>,
    ) -> Self {
        Self {
            profiles,
            lock,
            resolver,
        }
    }
}

use mem_ai::privacy::{REDACTED, is_fully_private, strip_private_content};
use mem_profile::profile_store::{ProfileData, ProfileItem, ProfileUpdates};
use serde_json::{Value, json};

use crate::jsonrpc::Request;
use crate::project_scope::ResolvedProjectScope;

use super::{ToolError, required_string, resolve_scope};

impl ProfileService {
    /// `profile`: absent `content` reads, present `content` writes one
    /// explicit preference. The four refusals run before any store write.
    pub async fn profile(&self, request: &Request) -> Result<Value, ToolError> {
        let project_id = required_string(request, "project_id", "project_id")?;
        let scope = resolve_scope(self.resolver.as_ref(), project_id)?;
        match request.param("content") {
            None | Some(Value::Null) => self.read_profile(&scope).await,
            Some(Value::String(content)) => self.write_profile(&scope, content).await,
            Some(_) => Err(ToolError::invalid("blank")),
        }
    }

    /// The public read: `"unknown"` is the read-only fallback identity; the
    /// stored JSON is parsed and the embeddings stripped.
    async fn read_profile(&self, scope: &ResolvedProjectScope) -> Result<Value, ToolError> {
        let user_id = scope
            .user_email
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        let profile = self
            .profiles
            .get_active_profile(&user_id)
            .await
            .map_err(|_error| ToolError::internal())?;
        let Some(profile) = profile else {
            return Ok(json!({ "profile": null }));
        };
        let parsed: Value = serde_json::from_str(&profile.profile_data).map_err(|_error| {
            ToolError::internal_data(json!({ "reason": "profile_unreadable" }))
        })?;
        let public = mem_profile::profile_utils::to_public_profile_data(&parsed);
        let section = |name: &str| public.get(name).cloned().unwrap_or_else(|| json!([]));
        Ok(json!({
            "profile": {
                "preferences": section("preferences"),
                "patterns": section("patterns"),
                "workflows": section("workflows"),
                "version": profile.version,
                "last_analyzed_at": profile.last_analyzed_at,
            }
        }))
    }

    /// The write path: blank → private → no email → lock, all before any
    /// store write; the guard is released on EVERY outcome.
    async fn write_profile(
        &self,
        scope: &ResolvedProjectScope,
        content: &str,
    ) -> Result<Value, ToolError> {
        let trimmed = content.trim();
        if trimmed.is_empty() {
            return Err(ToolError::invalid("blank"));
        }
        let stripped = strip_private_content(content);
        if is_fully_private(content) || stripped.replace(REDACTED, "").trim().is_empty() {
            return Err(ToolError::invalid("private"));
        }
        let Some(user_email) = scope.user_email.clone() else {
            return Err(ToolError::invalid("no_user_email"));
        };
        let Some(guard) = self.lock.try_acquire(None).await else {
            return Err(ToolError::internal_data(
                json!({ "reason": "learning_locked", "retryable": true }),
            ));
        };
        // No `?` between the acquisition and the release: the guard covers
        // every path, including a malformed stored profile.
        let outcome = self.apply_write(scope, &user_email, trimmed).await;
        guard.release().await;
        outcome
    }

    /// The store write under the held lock: merge into the active profile, or
    /// create the first one.
    async fn apply_write(
        &self,
        scope: &ResolvedProjectScope,
        user_email: &str,
        trimmed_content: &str,
    ) -> Result<Value, ToolError> {
        let new_item = ProfileItem {
            description: trimmed_content.to_string(),
            category: Some("explicit".to_string()),
            confidence: Some(1.0),
            frequency: Some(1),
            evidence: Some(vec!["manual-write".to_string()]),
            last_seen: Some(now_millis()),
            ..ProfileItem::default()
        };
        let existing = self
            .profiles
            .get_active_profile(user_email)
            .await
            .map_err(|_error| ToolError::internal())?;
        match existing {
            Some(existing) => {
                let data: ProfileData =
                    serde_json::from_str(&existing.profile_data).map_err(|_error| {
                        ToolError::internal_data(json!({ "reason": "profile_unreadable" }))
                    })?;
                let merged = self
                    .profiles
                    .merge_profile_data(
                        &data,
                        &ProfileUpdates {
                            preferences: Some(vec![new_item]),
                            ..ProfileUpdates::default()
                        },
                        None,
                        Some(&existing.id),
                    )
                    .await
                    .map_err(|_error| ToolError::internal())?;
                let updated = self
                    .profiles
                    .update_profile(&existing.id, &merged, 0, "Manual preference update")
                    .await
                    .map_err(|_error| ToolError::internal())?;
                if !updated {
                    return Err(ToolError::internal_data(
                        json!({ "reason": "profile_conflict" }),
                    ));
                }
            }
            None => {
                self.profiles
                    .create_profile(
                        user_email,
                        &scope.display_name,
                        scope.user_name.as_deref().unwrap_or_default(),
                        user_email,
                        &ProfileData {
                            preferences: vec![new_item],
                            ..ProfileData::default()
                        },
                        0,
                    )
                    .await
                    .map_err(|_error| ToolError::internal())?;
            }
        }
        Ok(json!({ "saved": true }))
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as i64)
        .unwrap_or(0)
}
