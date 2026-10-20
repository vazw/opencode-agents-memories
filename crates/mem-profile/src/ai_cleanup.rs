//! `ai_cleanup` — port of upstream `src/services/user-profile/ai-cleanup.ts`.
//!
//! Two entry points share one body: [`ai_cleanup_profile`] indexes the whole
//! profile and delegates to [`ai_cleanup_profile_from_indexed`], so the
//! interactive path and the incremental (scoped) path can never drift.
//!
//! The provider path is W3.2's [`StructuredCompletion`] seam
//! ([`crate::provider_gateway`]): upstream's host `session.create`/
//! `session.delete` lifecycle has no port analogue at this layer (the host
//! transport is sealed inside `mem-ai`), so what is pinned here is the
//! *observable* contract — native-when-available, faults surfaced verbatim,
//! and **no** external fallback while a native completion exists (upstream
//! #177).
//!
//! Untrusted model output is handled defensively everywhere: the mapping is
//! coerced ([`normalize_ai_mapping`]), and every response item's `id` is read
//! from the raw object it arrived in ([`parse_cleanup_response`]) rather than
//! from its array position, so a model that reorders or drops entries cannot
//! shift a disposition onto a different trait.

use std::collections::BTreeMap;

use mem_store::db::BoxFuture;
use serde_json::Value;

use crate::decay::{CONFIDENCE_EMA_ALPHA, ema_update};
use crate::error::ProfileError;
use crate::learning_service::AnalysisError;
use crate::profile_store::{ProfileData, ProfileItem};
use crate::provider_gateway::StructuredCompletion;
use crate::validation::TraitType;

/// Upstream `callViaOpencodeWithClient`'s system prompt.
pub const CLEANUP_SYSTEM_PROMPT_OPENCODE: &str = "You are a user profile cleanup assistant. Merge duplicate entries and return only JSON without markdown wrapping.";

/// Upstream `callViaExternalAPI`'s system prompt.
pub const CLEANUP_SYSTEM_PROMPT_EXTERNAL: &str =
    "You are a user profile cleanup assistant. Merge duplicate entries and return only JSON.";

/// Upstream's sentence when no provider at all is configured.
pub const NO_PROVIDER_MESSAGE: &str = "No AI provider configured for profile cleanup";

/// The removal reason upstream attaches to a removal it can still resolve.
const REMOVED_REASON: &str = "AI determined this is a duplicate or stale entry";

/// The removal reason for an id the original profile does not contain.
const REMOVED_UNKNOWN_REASON: &str = "Entry no longer exists";

// ---------------------------------------------------------------------------
// Indexing
// ---------------------------------------------------------------------------

/// One profile entry plus the synthetic id the model refers to it by.
#[derive(Debug, Clone, PartialEq)]
pub struct IndexedItem {
    pub id: String,
    pub item: ProfileItem,
}

/// Upstream `IndexedProfile`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct IndexedProfile {
    pub preferences: Vec<IndexedItem>,
    pub patterns: Vec<IndexedItem>,
    pub workflows: Vec<IndexedItem>,
}

/// Upstream `addIdsToProfile`: `pref_{i}` / `pat_{i}` / `wf_{i}` in array order.
pub fn index_profile(data: &ProfileData) -> IndexedProfile {
    IndexedProfile {
        preferences: index_items(&data.preferences, "pref_"),
        patterns: index_items(&data.patterns, "pat_"),
        workflows: index_items(&data.workflows, "wf_"),
    }
}

fn index_items(items: &[ProfileItem], prefix: &str) -> Vec<IndexedItem> {
    items
        .iter()
        .enumerate()
        .map(|(index, item)| IndexedItem {
            id: format!("{prefix}{index}"),
            item: item.clone(),
        })
        .collect()
}

/// Upstream `buildItemIndex`.
pub fn build_item_index(indexed: &IndexedProfile) -> BTreeMap<String, ProfileItem> {
    let mut map = BTreeMap::new();
    for entry in indexed
        .preferences
        .iter()
        .chain(indexed.patterns.iter())
        .chain(indexed.workflows.iter())
    {
        map.insert(entry.id.clone(), entry.item.clone());
    }
    map
}

/// Upstream `filterProfileForCleanup`: the same indexing, restricted to the
/// ids the caller scoped (used by the incremental cleanup path).
pub fn filter_profile_for_cleanup(data: &ProfileData, include_ids: &[String]) -> IndexedProfile {
    let keep = |entry: &IndexedItem| include_ids.iter().any(|id| id == &entry.id);
    let indexed = index_profile(data);
    IndexedProfile {
        preferences: indexed.preferences.into_iter().filter(keep).collect(),
        patterns: indexed.patterns.into_iter().filter(keep).collect(),
        workflows: indexed.workflows.into_iter().filter(keep).collect(),
    }
}

// ---------------------------------------------------------------------------
// Prompt + schema
// ---------------------------------------------------------------------------

const CLEANUP_PROMPT_PREFIX: &str = r#"You are a user profile analyst. The profile below contains duplicate entries within each category (preferences, patterns, workflows). Output a cleaned profile.

Rules:
1. Merge semantically identical entries ONLY within the same section (pref_ with pref_, pat_ with pat_, wf_ with wf_)
2. Do NOT merge across sections — preferences and patterns are different things
3. When merging, keep the most specific description. Do not artificially shorten or inflate — the natural length of the original is fine.
4. Do not add new entries; only merge and remove
5. Prefer merging over removing. If entries share the same topic or describe similar behavior, merge them. Only remove truly irrelevant/generic items that add no value (e.g. "uses tools", "checks things").
6. You MUST return a mapping showing the disposition of each id

Current profile:
"#;

const CLEANUP_PROMPT_SUFFIX: &str = r#"

Return JSON only (no markdown):
{
  "preferences": [
    { "id": "pref_0", "category": "...", "description": "..." }
  ],
  "patterns": [...],
  "workflows": [...],
  "mapping": {
    "kept": ["pref_0", "pat_1"],
    "merged": [["pref_2", "pref_5"], ["pat_3", "pat_8"]],
    "removed": ["pref_4"]
  }
}

The first id in each merged group is the kept entry; the rest are merged into it."#;

/// Upstream `buildAICleanupPrompt`: the rules, the pretty-printed indexed
/// profile, then the pinned response shape.
pub fn build_cleanup_prompt(indexed: &IndexedProfile) -> String {
    let profile_json = serde_json::to_string_pretty(&serde_json::json!({
        "preferences": indexed.preferences.iter().map(format_for_ai).collect::<Vec<_>>(),
        "patterns": indexed.patterns.iter().map(format_for_ai).collect::<Vec<_>>(),
        "workflows": indexed.workflows.iter().map(format_for_ai).collect::<Vec<_>>(),
    }))
    .unwrap_or_default();

    format!("{CLEANUP_PROMPT_PREFIX}{profile_json}{CLEANUP_PROMPT_SUFFIX}")
}

/// Upstream `formatForAI`: only the four modelled keys, and `undefined` fields
/// are dropped (mirroring `JSON.stringify`).
fn format_for_ai(entry: &IndexedItem) -> Value {
    let mut object = serde_json::Map::new();
    object.insert("id".to_string(), Value::String(entry.id.clone()));
    if let Some(category) = entry.item.category.as_ref() {
        object.insert("category".to_string(), Value::String(category.clone()));
    }
    object.insert(
        "description".to_string(),
        Value::String(entry.item.description.clone()),
    );
    if let Some(frequency) = entry.item.frequency {
        object.insert("frequency".to_string(), Value::from(frequency));
    }
    Value::Object(object)
}

/// The structured-output schema for a cleanup response.
pub fn cleanup_schema() -> Value {
    let section = serde_json::json!({
        "type": "array",
        "items": {
            "type": "object",
            "required": ["id", "category", "description"],
            "properties": {
                "id": { "type": "string" },
                "category": { "type": "string" },
                "description": { "type": "string" }
            }
        }
    });
    serde_json::json!({
        "type": "object",
        "required": ["preferences", "patterns", "workflows", "mapping"],
        "properties": {
            "preferences": section.clone(),
            "patterns": section.clone(),
            "workflows": section,
            "mapping": {
                "type": "object",
                "required": ["kept", "merged", "removed"],
                "properties": {
                    "kept": { "type": "array", "items": { "type": "string" } },
                    "merged": {
                        "type": "array",
                        "items": { "type": "array", "items": { "type": "string" } }
                    },
                    "removed": { "type": "array", "items": { "type": "string" } }
                }
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Response parsing
// ---------------------------------------------------------------------------

/// The model's disposition map (upstream `AIMapping`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CleanupMapping {
    pub kept: Vec<String>,
    pub merged: Vec<Vec<String>>,
    pub removed: Vec<String>,
}

/// One accepted merge (upstream's `{ ids, result }`).
#[derive(Debug, Clone, PartialEq)]
pub struct MergedGroup {
    pub ids: Vec<String>,
    pub result: String,
}

/// One removal (upstream's `{ id, reason }`).
#[derive(Debug, Clone, PartialEq)]
pub struct RemovedItem {
    pub id: String,
    pub reason: String,
}

/// Upstream `CleanupDiff`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CleanupDiff {
    pub kept: Vec<String>,
    pub merged: Vec<MergedGroup>,
    pub removed: Vec<RemovedItem>,
}

/// Upstream `AICleanupResult`.
#[derive(Debug, Clone, PartialEq)]
pub struct AiCleanupResult {
    pub cleaned: ProfileData,
    pub diff: CleanupDiff,
}

/// Upstream `normalizeAIMapping`, total over untrusted model output: a
/// non-object degrades to the empty mapping (a no-op cleanup that preserves
/// every original as "unmentioned"), non-string entries are dropped, and merge
/// groups that are not arrays or become empty are dropped.
pub fn normalize_ai_mapping(raw: &Value) -> CleanupMapping {
    let Some(object) = raw.as_object() else {
        return CleanupMapping::default();
    };

    let string_array = |value: Option<&Value>| -> Vec<String> {
        value
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    };

    let merged = object
        .get("merged")
        .and_then(Value::as_array)
        .map(|groups| {
            groups
                .iter()
                .map(|group| string_array(Some(group)))
                .filter(|group| !group.is_empty())
                .collect()
        })
        .unwrap_or_default();

    CleanupMapping {
        kept: string_array(object.get("kept")),
        merged,
        removed: string_array(object.get("removed")),
    }
}

/// Splits a cleanup response into the indexed profile it returned and the
/// normalized mapping.
///
/// **Id provenance:** each item's `id` is read from the raw JSON object it
/// arrived in (`id` is an unmodelled field that `ProfileItem`'s
/// `#[serde(flatten)] extra` map preserves), never from its array position. An
/// element without a usable `id` — or one that does not deserialize into a
/// profile item at all — is skipped, which preserves it as "unmentioned"
/// rather than mis-attributing another element's disposition to it.
pub fn parse_cleanup_response(value: &Value) -> (IndexedProfile, CleanupMapping) {
    let section = |name: &str| -> Vec<IndexedItem> {
        value
            .get(name)
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| {
                        let id = entry.get("id").and_then(Value::as_str)?;
                        let item: ProfileItem = serde_json::from_value(entry.clone()).ok()?;
                        Some(IndexedItem {
                            id: id.to_string(),
                            item,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    };

    let indexed = IndexedProfile {
        preferences: section("preferences"),
        patterns: section("patterns"),
        workflows: section("workflows"),
    };
    let mapping = normalize_ai_mapping(value.get("mapping").unwrap_or(&Value::Null));
    (indexed, mapping)
}

/// Upstream `extractTextFromPromptResult`: the assistant's `info` and the text
/// parts joined by newlines.
///
/// `info.text` is **never** consulted (upstream #177 — an assistant message has
/// no `text` field). Total over malformed inputs.
pub fn extract_text_from_prompt_result(value: &Value) -> (Value, String) {
    let info = value
        .get("data")
        .and_then(|data| data.get("info"))
        .filter(|info| !info.is_null())
        .or_else(|| value.get("info").filter(|info| !info.is_null()))
        .cloned()
        .unwrap_or(Value::Null);

    let parts = value
        .get("data")
        .and_then(|data| data.get("parts"))
        .and_then(Value::as_array)
        .or_else(|| value.get("parts").and_then(Value::as_array))
        .cloned()
        .unwrap_or_default();

    let raw_text = parts
        .iter()
        .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();

    (info, raw_text)
}

// ---------------------------------------------------------------------------
// Provider seam
// ---------------------------------------------------------------------------

/// The cleanup gateway: a native completion (the in-host provider) and an
/// optional external one — W3.2's [`StructuredCompletion`] seam.
pub struct ProfileCleanupGateway {
    native: Option<std::sync::Arc<dyn StructuredCompletion>>,
    external: Option<std::sync::Arc<dyn StructuredCompletion>>,
}

impl ProfileCleanupGateway {
    pub fn new(
        native: Option<std::sync::Arc<dyn StructuredCompletion>>,
        external: Option<std::sync::Arc<dyn StructuredCompletion>>,
    ) -> Self {
        Self { native, external }
    }

    /// Runs one cleanup completion (R10).
    ///
    /// Native present ⇒ the native completion, and **every** [`AnalysisError`]
    /// propagates verbatim — no external fallback and no message rewriting.
    /// Native absent + external present ⇒ the external completion. Both absent
    /// ⇒ [`NO_PROVIDER_MESSAGE`].
    pub fn complete_cleanup<'a>(
        &'a self,
        user_prompt: &'a str,
        schema: Value,
    ) -> BoxFuture<'a, Result<Value, AnalysisError>> {
        Box::pin(async move {
            if let Some(native) = self.native.as_ref() {
                return native
                    .complete(CLEANUP_SYSTEM_PROMPT_OPENCODE, user_prompt, schema)
                    .await;
            }
            if let Some(external) = self.external.as_ref() {
                return external
                    .complete(CLEANUP_SYSTEM_PROMPT_EXTERNAL, user_prompt, schema)
                    .await;
            }
            Err(AnalysisError::Provider(NO_PROVIDER_MESSAGE.to_string()))
        })
    }
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// Upstream `aiCleanupProfileFromIndexed`.
pub async fn ai_cleanup_profile_from_indexed(
    indexed: &IndexedProfile,
    gateway: &ProfileCleanupGateway,
) -> Result<AiCleanupResult, ProfileError> {
    let prompt = build_cleanup_prompt(indexed);
    // A provider fault maps through `From<AnalysisError> for ProfileError`, so
    // it surfaces as `ProfileError::Analysis(message)` verbatim.
    let response = gateway.complete_cleanup(&prompt, cleanup_schema()).await?;

    let (cleaned_indexed, mapping) = parse_cleanup_response(&response);
    let cleaned_by_id = build_item_index(&cleaned_indexed);
    let original_by_id = build_item_index(indexed);

    let cleaned = rebuild_profile_using(&mapping, &cleaned_by_id, &original_by_id);
    let diff = generate_diff(indexed, &mapping, &cleaned_by_id);

    Ok(AiCleanupResult { cleaned, diff })
}

/// Upstream `aiCleanupProfile`: index the whole profile, then delegate — the
/// two paths share one body so they cannot drift.
pub async fn ai_cleanup_profile(
    data: &ProfileData,
    gateway: &ProfileCleanupGateway,
) -> Result<AiCleanupResult, ProfileError> {
    ai_cleanup_profile_from_indexed(&index_profile(data), gateway).await
}

// ---------------------------------------------------------------------------
// Rebuild
// ---------------------------------------------------------------------------

/// The metadata carried over from the original when the cleaned entry omits it
/// (upstream's `preserveKeys`).
fn copy_missing_metadata(target: &mut ProfileItem, source: &ProfileItem) {
    if target.centroid.is_none() {
        target.centroid = source.centroid.clone();
    }
    if target.anchor.is_none() {
        target.anchor = source.anchor.clone();
    }
    if target.weak_hit_count.is_none() {
        target.weak_hit_count = source.weak_hit_count;
    }
    if target.last_weak_hit_at.is_none() {
        target.last_weak_hit_at = source.last_weak_hit_at;
    }
    if target.drift_below_count.is_none() {
        target.drift_below_count = source.drift_below_count;
    }
    if target.frequency.is_none() {
        target.frequency = source.frequency;
    }
    if target.evidence.is_none() {
        target.evidence = source.evidence.clone();
    }
    if target.steps.is_none() {
        target.steps = source.steps.clone();
    }
    if target.confidence.is_none() {
        target.confidence = source.confidence;
    }
    if target.alpha.is_none() {
        target.alpha = source.alpha;
    }
    if target.beta.is_none() {
        target.beta = source.beta;
    }
    if target.weak_alpha.is_none() {
        target.weak_alpha = source.weak_alpha;
    }
    if target.weak_beta.is_none() {
        target.weak_beta = source.weak_beta;
    }
    if target.last_match_time.is_none() {
        target.last_match_time = source.last_match_time;
    }
    if target.first_seen.is_none() {
        target.first_seen = source.first_seen;
    }
    if target.pending_validation.is_none() {
        target.pending_validation = source.pending_validation;
    }
}

/// Splits a merged group's ids into (leader, sources), skipping blank ids.
fn merge_group_ids(group: &[String]) -> (Option<&str>, Vec<&str>) {
    let mut ids = group.iter().map(String::as_str).filter(|id| !id.is_empty());
    let leader = ids.next();
    (leader, ids.collect())
}

/// Upstream `rebuildProfileUsing` L413-590, with the PRD's α = 0.3 merge fold
/// (R1) applied on top of the carried metadata.
///
/// Items that are merged sources only are **not** re-emitted as unmentioned;
/// every other original the model did not mention is carried over verbatim.
pub fn rebuild_profile_using(
    mapping: &CleanupMapping,
    cleaned_by_id: &BTreeMap<String, ProfileItem>,
    original_by_id: &BTreeMap<String, ProfileItem>,
) -> ProfileData {
    // Kept ids: `mapping.kept` then each merge leader, order-preserving and
    // deduplicated.
    let mut kept_ids: Vec<String> = Vec::new();
    let push_unique = |ids: &mut Vec<String>, id: &str| {
        if !id.is_empty() && !ids.iter().any(|existing| existing == id) {
            ids.push(id.to_string());
        }
    };
    for id in &mapping.kept {
        push_unique(&mut kept_ids, id);
    }
    for group in &mapping.merged {
        if let (Some(leader), _) = merge_group_ids(group) {
            push_unique(&mut kept_ids, leader);
        }
    }

    let merged_groups: Vec<&Vec<String>> = mapping
        .merged
        .iter()
        .filter(|group| group.len() > 1)
        .collect();

    // A merged group's leader is emitted once; its sources are folded into it.
    let mut result = ProfileData::default();

    for id in &kept_ids {
        let cleaned_item = cleaned_by_id.get(id);
        let original_item = original_by_id.get(id);
        let Some(item) = cleaned_item.or(original_item) else {
            continue;
        };

        let mut result_item = item.clone();
        if cleaned_item.is_some() {
            // Strip the synthetic `id`: it is an unmodelled field the raw
            // response carried, preserved by `ProfileItem::extra`. The port
            // keeps synthetic ids outside `ProfileItem`, so this only ever
            // fires for model-supplied items — the original's own fields are
            // never touched.
            result_item.extra.remove("id");
        }

        if let Some(original) = original_item {
            copy_missing_metadata(&mut result_item, original);
            if cleaned_item.is_some_and(|cleaned| cleaned.description != original.description) {
                // The embeddings describe the old wording.
                result_item.centroid = None;
                result_item.anchor = None;
            }
        }

        let group = merged_groups
            .iter()
            .find(|group| merge_group_ids(group).0 == Some(id.as_str()))
            .copied();

        if let Some(group) = group {
            // A hallucinated keeper id (present only in the model's output) is
            // skipped instead of aborting the whole run.
            let Some(original) = original_item else {
                continue;
            };
            let (_, sources) = merge_group_ids(group);

            let mut best_frequency = original.frequency.unwrap_or(0);
            let mut best_centroid = original.centroid.clone();
            let mut best_anchor = original.anchor.clone();
            for source in &sources {
                let Some(source_item) = original_by_id.get(*source) else {
                    continue;
                };
                let source_frequency = source_item.frequency.unwrap_or(0);
                if source_frequency > best_frequency {
                    best_frequency = source_frequency;
                    best_centroid = source_item.centroid.clone();
                    best_anchor = source_item.anchor.clone();
                }
                if let Some(evidence) = source_item.evidence.as_ref() {
                    let mut merged: Vec<String> = Vec::new();
                    for entry in evidence
                        .iter()
                        .chain(result_item.evidence.as_deref().unwrap_or_default())
                    {
                        if !merged.contains(entry) {
                            merged.push(entry.clone());
                        }
                    }
                    merged.truncate(10);
                    result_item.evidence = Some(merged);
                }
            }

            let mut total_frequency = original.frequency.unwrap_or(0);
            let mut merged_alpha = original.alpha.unwrap_or(1.0);
            let mut merged_beta = original.beta.unwrap_or(1.0);
            let mut merged_weak_alpha = original.weak_alpha.unwrap_or(1.0);
            let mut merged_weak_beta = original.weak_beta.unwrap_or(1.0);
            let mut last_match_time = original.last_match_time.unwrap_or(0);
            let mut last_seen = original.last_seen.unwrap_or(0);
            let mut pending_validation = original.pending_validation.unwrap_or(false);

            for source in &sources {
                let Some(source_item) = original_by_id.get(*source) else {
                    continue;
                };
                total_frequency += source_item.frequency.unwrap_or(0);
                merged_alpha += source_item.alpha.unwrap_or(1.0);
                merged_beta += source_item.beta.unwrap_or(1.0);
                merged_weak_alpha += source_item.weak_alpha.unwrap_or(1.0) - 1.0;
                merged_weak_beta += source_item.weak_beta.unwrap_or(1.0) - 1.0;
                last_match_time = last_match_time.max(source_item.last_match_time.unwrap_or(0));
                last_seen = last_seen.max(source_item.last_seen.unwrap_or(0));
                pending_validation =
                    pending_validation && source_item.pending_validation.unwrap_or(false);
            }

            result_item.frequency = Some(total_frequency);
            result_item.alpha = Some(merged_alpha);
            result_item.beta = Some(merged_beta);
            result_item.weak_alpha = Some(merged_weak_alpha);
            result_item.weak_beta = Some(merged_weak_beta);
            result_item.last_match_time = Some(last_match_time);
            result_item.last_seen = Some(last_seen);
            result_item.pending_validation = Some(pending_validation);
            if best_centroid.is_some() {
                result_item.centroid = best_centroid;
            }
            if best_anchor.is_some() {
                result_item.anchor = best_anchor;
            }

            // R1: the incoming trait's confidence lands here as the α = 0.3 EMA
            // folded left-to-right over the group's *original* confidences. A
            // member without a confidence leaves the accumulator unchanged, and
            // the fold only applies when at least one confidence existed.
            let mut accumulator: Option<f64> = None;
            for member in group.iter() {
                let Some(confidence) = original_by_id.get(member).and_then(|item| item.confidence)
                else {
                    continue;
                };
                accumulator = Some(match accumulator {
                    Some(previous) => ema_update(previous, confidence, CONFIDENCE_EMA_ALPHA),
                    None => confidence,
                });
            }
            if let Some(confidence) = accumulator {
                result_item.confidence = Some(confidence);
            }
        }

        push_by_prefix(&mut result, id, result_item);
    }

    // Everything the model did not mention survives verbatim — except a merged
    // group's sources, which were folded into their leader.
    let unmentioned: Vec<String> = original_by_id
        .keys()
        .filter(|id| {
            !kept_ids.contains(id)
                && !mapping
                    .merged
                    .iter()
                    .any(|group| merge_group_ids(group).1.contains(&id.as_str()))
                && !mapping.removed.iter().any(|removed| removed == *id)
        })
        .cloned()
        .collect();

    for id in unmentioned {
        let Some(original) = original_by_id.get(&id) else {
            continue;
        };
        // A straight clone: these items are originals, so there is no synthetic
        // id to strip and nothing to preserve.
        push_by_prefix(&mut result, &id, original.clone());
    }

    result
}

/// Places a rebuilt item in the section its id prefix selects (upstream's
/// `startsWith("pref_")` / `"pat_"` / `"wf_"`, falling back to preferences).
fn push_by_prefix(result: &mut ProfileData, id: &str, item: ProfileItem) {
    match TraitType::from_id_prefix(id) {
        Some(TraitType::Preference) => result.preferences.push(item),
        Some(TraitType::Pattern) => result.patterns.push(item),
        Some(TraitType::Workflow) => result.workflows.push(item),
        None => result.preferences.push(item),
    }
}

/// Upstream `generateDiff`.
pub fn generate_diff(
    original: &IndexedProfile,
    mapping: &CleanupMapping,
    cleaned_by_id: &BTreeMap<String, ProfileItem>,
) -> CleanupDiff {
    let index = build_item_index(original);

    let kept = mapping
        .kept
        .iter()
        .map(|id| {
            index
                .get(id)
                .map(|item| item.description.clone())
                .unwrap_or_else(|| id.clone())
        })
        .collect();

    let merged = mapping
        .merged
        .iter()
        .map(|group| {
            let leader = group.first().cloned().unwrap_or_default();
            let result = cleaned_by_id
                .get(&leader)
                .map(|item| item.description.clone())
                .or_else(|| index.get(&leader).map(|item| item.description.clone()))
                .unwrap_or_else(|| leader.clone());
            MergedGroup {
                ids: group.clone(),
                result,
            }
        })
        .collect();

    let removed = mapping
        .removed
        .iter()
        .map(|id| RemovedItem {
            id: id.clone(),
            reason: if index.contains_key(id) {
                REMOVED_REASON.to_string()
            } else {
                REMOVED_UNKNOWN_REASON.to_string()
            },
        })
        .collect();

    CleanupDiff {
        kept,
        merged,
        removed,
    }
}
