//! `GET /api/stats` — upstream `api-handlers.ts` `handleStats` (v2.33) plus
//! the `stats-service.ts` collectors it composes.
//!
//! The scope counts keep their X3 rule: `byScope` counts a row when its
//! container tag carries `_user_` (user) or `_project_` (project, only when it
//! is not a user row — upstream's `else if`), `total` is their sum, and
//! `byType` tallies every row with a non-empty `type` column regardless of its
//! scope marker. On top of the legacy totals the handler answers the v2.33
//! payload: per-shard health rows, a profile summary, a 14-day zero-filled
//! daily series scraped from the structured log, and `generatedAt`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::response::Response;

use super::dto::{
    ByScopeDto, DailyStatsDto, ProfileCountsDto, ProfileStatsDto, ShardStatsDto, StatsDto,
    iso_string_or_null, now_iso_string,
};
use super::envelope;
use super::port_failure;
use super::ports::{PortError, ShardRef};
use crate::server::AppState;

/// The user-scope marker.
const USER_MARKER: &str = "_user_";
/// The project-scope marker.
const PROJECT_MARKER: &str = "_project_";

/// `GET /api/stats`.
pub async fn stats(
    State(state): State<Arc<AppState>>,
    Query(pairs): super::QueryPairs,
) -> Response {
    // Ownership/liveness probes must stay cheap: answer before any shard walk
    // or log scan (upstream web-server.ts:577 — `searchParams.get("probe")
    // === "1"`, body `{"success":true}` with the data key omitted).
    if pairs
        .iter()
        .any(|(key, value)| key == "probe" && value == "1")
    {
        return envelope::json(axum::http::StatusCode::OK, envelope::success_empty());
    }

    // Shard-listing failures stay hard errors, so `/api/stats` consumers (the
    // explorer header total) never treat an empty total as a successful empty
    // store (upstream's `shardsError` removal).
    let shards = match state.api.memory.shards(None, None).await {
        Ok(shards) => shards,
        Err(error) => return port_failure("GET /api/stats", error),
    };

    let (shards_dto, total, by_scope, by_type) = collect_shard_stats(&state, shards).await;

    let now = chrono::Local::now();
    let daily = collect_daily_stats(&DailyScan {
        log_dir: state.log_dir.clone(),
        days: 14,
        now,
    });

    envelope::ok(&StatsDto {
        total,
        by_scope,
        by_type,
        generated_at: now_iso_string(),
        shards: shards_dto,
        profile: collect_profile_stats(&state).await,
        daily,
        daily_error: None,
    })
}

// -------------------------------------------------------------------------
// Per-shard collection (upstream `collectShardStats`)
// -------------------------------------------------------------------------

/// The fixed per-shard error for a failed memory walk. D-9: the store detail
/// goes to `tracing` only — upstream echoes `error.message`; we must not.
const SHARD_QUERY_FAILED: &str = "shard query failed";

/// Collects registered shard health while preserving the existing stats
/// totals (upstream `collectShardStats`, walked over the memory port).
///
/// Invariant (commit-after-full-walk): a shard's LOCAL contribution merges
/// into the global accumulators only after its entire memory walk succeeded;
/// a failed shard (or one whose db file is missing) contributes exactly zero
/// walk-derived counts and carries `error`. Local accumulators are freshly
/// zeroed per shard — no cross-shard bleed.
pub(crate) async fn collect_shard_stats(
    state: &AppState,
    shards: Vec<ShardRef>,
) -> (Vec<ShardStatsDto>, u64, ByScopeDto, BTreeMap<String, u64>) {
    let mut shard_dtos: Vec<ShardStatsDto> = Vec::with_capacity(shards.len());
    let mut total_user = 0_u64;
    let mut total_project = 0_u64;
    let mut by_type: BTreeMap<String, u64> = BTreeMap::new();

    for shard in shards {
        let file_name = shard
            .db_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let file_exists = shard.db_path.exists();

        let mut dto = ShardStatsDto {
            shard_id: shard.shard_id,
            scope: shard.scope.as_str().to_string(),
            scope_hash: shard.scope_hash.clone(),
            shard_index: shard.shard_index,
            file_name,
            file_exists,
            is_active: shard.is_active,
            registered_vector_count: shard.vector_count,
            memory_count: 0,
            oldest_created_at: None,
            newest_created_at: None,
            duplicate_tag_count: 0,
            error: None,
        };

        if !dto.file_exists {
            // No walk is attempted; the error carries `fileName` only — never
            // the db_path (D-9). Upstream: `Shard DB file missing: ${fileName}`.
            dto.error = Some(format!("Shard DB file missing: {}", dto.file_name));
            shard_dtos.push(dto);
            continue;
        }

        // LOCAL accumulators first (upstream keeps them local until every
        // query for this shard succeeds so a mid-walk failure cannot inflate
        // global totals while memoryCount stays 0).
        let mut local_user = 0_u64;
        let mut local_project = 0_u64;
        let mut local_by_type: BTreeMap<String, u64> = BTreeMap::new();
        let mut oldest: Option<i64> = None;
        let mut newest: Option<i64> = None;
        let mut tag_groups: BTreeMap<String, u64> = BTreeMap::new();

        match state.api.memory.all_memories(&shard).await {
            Ok(rows) => {
                for row in &rows {
                    if row.container_tag.contains(USER_MARKER) {
                        local_user += 1;
                    } else if row.container_tag.contains(PROJECT_MARKER) {
                        local_project += 1;
                    }
                    if let Some(kind) = row.memory_type.as_deref().filter(|kind| !kind.is_empty()) {
                        *local_by_type.entry(kind.to_string()).or_insert(0) += 1;
                    }
                    oldest = Some(oldest.map_or(row.created_at, |seen| seen.min(row.created_at)));
                    newest = Some(newest.map_or(row.created_at, |seen| seen.max(row.created_at)));
                    if let Some(tags) = row.tags.as_deref().filter(|tags| !tags.is_empty()) {
                        *tag_groups.entry(tags.to_string()).or_insert(0) += 1;
                    }
                }

                dto.memory_count = rows.len() as u64;
                dto.oldest_created_at = oldest.and_then(iso_string_or_null);
                dto.newest_created_at = newest.and_then(iso_string_or_null);
                dto.duplicate_tag_count = tag_groups
                    .values()
                    .map(|count| count.saturating_sub(1))
                    .sum();

                // The whole walk succeeded: commit the local contribution.
                total_user += local_user;
                total_project += local_project;
                for (kind, count) in local_by_type {
                    *by_type.entry(kind).or_insert(0) += count;
                }
            }
            Err(error) => {
                tracing::error!(
                    scope = shard.scope.as_str(),
                    hash = %shard.scope_hash,
                    error = %error,
                    "stats shard walk failed"
                );
                dto.error = Some(SHARD_QUERY_FAILED.to_string());
            }
        }
        shard_dtos.push(dto);
    }

    (
        shard_dtos,
        total_user + total_project,
        ByScopeDto {
            user: total_user,
            project: total_project,
        },
        by_type,
    )
}

// -------------------------------------------------------------------------
// Profile collection (upstream `collectProfileStats`)
// -------------------------------------------------------------------------

/// Reads the active user's compact profile summary without returning profile
/// contents (upstream `collectProfileStats`).
///
/// D-9 deviation (declared): a store fault answers bare `{exists:false}` with
/// the detail in `tracing` only — upstream echoes `error.message`. Business
/// rejections (`PortError::Invalid`) keep their verbatim message, and broken
/// `profileData` echoes its parse error (a data problem, not an internal one).
pub(crate) async fn collect_profile_stats(state: &AppState) -> ProfileStatsDto {
    match state.api.profile.active_profile(&state.user_id).await {
        Err(PortError::Invalid(message)) => ProfileStatsDto {
            exists: false,
            error: Some(message),
            ..ProfileStatsDto::missing()
        },
        Err(error) => {
            tracing::error!(error = %error, "stats profile read failed");
            ProfileStatsDto::missing()
        }
        Ok(None) => ProfileStatsDto::missing(),
        Ok(Some(record)) => match serde_json::from_str::<serde_json::Value>(&record.profile_data) {
            Err(error) => ProfileStatsDto {
                exists: true,
                version: Some(record.version),
                error: Some(error.to_string()),
                ..ProfileStatsDto::missing()
            },
            Ok(data) => {
                let array_count = |key: &str| {
                    data.get(key)
                        .and_then(|value| value.as_array())
                        .map(|items| items.len() as u64)
                        .unwrap_or(0)
                };
                ProfileStatsDto {
                    exists: true,
                    version: Some(record.version),
                    counts: Some(ProfileCountsDto {
                        preferences: array_count("preferences"),
                        patterns: array_count("patterns"),
                        workflows: array_count("workflows"),
                    }),
                    last_analyzed_at: iso_string_or_null(record.last_analyzed_at),
                    total_prompts_analyzed: Some(record.total_prompts_analyzed),
                    error: None,
                }
            }
        },
    }
}

// -------------------------------------------------------------------------
// Parsers + daily series (this wave; upstream `stats-service.ts`)
// -------------------------------------------------------------------------

/// The upstream `defaultLogDir()` seam for the composition root: the directory
/// of the env-overridden log file, else `<home>/.opencode-mem` (the same env
/// contract as [`mem_core::logging::LogSink::from_env`], with a panic-free
/// `"."` fallback when no home is resolvable). The handler itself only reads
/// `AppState::log_dir` — this helper is called once, when the state is built.
pub fn default_log_dir() -> PathBuf {
    let env = mem_core::env::ProcessEnv;
    let home = mem_core::env::home_dir(&env).unwrap_or_else(|| PathBuf::from("."));
    mem_core::logging::LogSink::from_env(&env, &home)
        .path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// One daily counter scraped from the structured log (upstream `DailyCounter`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DailyCounter {
    Captures,
    CaptureSkipped,
    ProfileInjections,
    CompactionInjections,
}

/// One parsed structured-logger line (upstream `ParsedLogLine`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedLogLine {
    /// The event's local date, `YYYY-MM-DD` (the stamp as written, never
    /// re-based to the reader's zone).
    pub date: String,
    pub message: String,
    /// The compact-JSON payload after the first `: {` delimiter.
    pub payload: Option<String>,
}

/// The daily classification of one parsed message (upstream
/// `LogClassification`; at most one field is set).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LogClassification {
    pub counter: Option<DailyCounter>,
    pub error_bucket: Option<String>,
}

/// The five named error buckets, checked as message prefixes in upstream order.
const NAMED_ERROR_BUCKETS: [&str; 5] = [
    "Turso ready gate failed",
    "Failed to load skills for OpenCode registration",
    "Auto-capture final error",
    "Compaction handler error",
    "Idle processing error",
];

/// The fixed `[YYYY-MM-DDTHH:MM:SS.mmm±HH:MM]` stamp grammar (29 bytes): the
/// scanner validates the shape so chrono only has to validate the values —
/// together they mirror upstream's anchored regex exactly (2-digit fields,
/// 3-digit fraction, `±HH:MM` offset).
fn is_timestamp_shape(stamp: &str) -> bool {
    const SEPARATORS: [usize; 8] = [4, 7, 10, 13, 16, 19, 23, 26];
    let bytes = stamp.as_bytes();
    bytes.len() == 29
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes[10] == b'T'
        && bytes[13] == b':'
        && bytes[16] == b':'
        && bytes[19] == b'.'
        && (bytes[23] == b'+' || bytes[23] == b'-')
        && bytes[26] == b':'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| SEPARATORS.contains(&index) || byte.is_ascii_digit())
}

/// Parses one structured logger line, splitting the message only at the first
/// `: {` (a 3-character delimiter: colon, space, open brace).
///
/// Returns `None` for separators, malformed lines and invalid timestamps —
/// the caller skips them silently, like upstream's `if (!parsed) continue`.
pub(crate) fn parse_log_line(line: &str) -> Option<ParsedLogLine> {
    let normalized = line.strip_suffix('\r').unwrap_or(line);
    let rest = normalized.strip_prefix('[')?;
    let close = rest.find(']')?;
    let stamp = &rest[..close];
    // Upstream `\]\s(.*)`: exactly one whitespace, then the content.
    let content = rest[close + 1..].strip_prefix(char::is_whitespace)?;
    if !is_timestamp_shape(stamp) {
        return None;
    }
    chrono::DateTime::parse_from_str(stamp, "%Y-%m-%dT%H:%M:%S%.3f%:z").ok()?;

    let date = stamp[..10].to_string();
    let (message, payload) = match content.find(": {") {
        None => (content, None),
        // The delimiter is `": {"`: the message stops before the colon, the
        // payload starts after the separator's colon+space (`idx + 2`).
        Some(index) => (&content[..index], Some(content[index + 2..].to_string())),
    };
    Some(ParsedLogLine {
        date,
        message: message.to_string(),
        payload,
    })
}

/// Classifies a parsed logger message as one daily counter or a named error
/// bucket (upstream `classifyLogMessage`, same precedence).
pub(crate) fn classify_log_message(message: &str) -> LogClassification {
    if message.starts_with("Auto-capture memory persisted") {
        return LogClassification {
            counter: Some(DailyCounter::Captures),
            error_bucket: None,
        };
    }
    if message.starts_with("Auto-capture skipped") {
        return LogClassification {
            counter: Some(DailyCounter::CaptureSkipped),
            error_bucket: None,
        };
    }
    if message.starts_with("profile inject") {
        return LogClassification {
            counter: Some(DailyCounter::ProfileInjections),
            error_bucket: None,
        };
    }
    if message.starts_with("Compaction memory injected") {
        return LogClassification {
            counter: Some(DailyCounter::CompactionInjections),
            error_bucket: None,
        };
    }
    for bucket in NAMED_ERROR_BUCKETS {
        if message.starts_with(bucket) {
            return LogClassification {
                counter: None,
                error_bucket: Some(bucket.to_string()),
            };
        }
    }

    // The generic `^.*:\s*error$` suffix rule (case-insensitive): the bucket
    // is the trimmed prefix before the final `: error` suffix. `get(..)` keeps
    // a multibyte tail from panicking the slice.
    if let Some(tail) = message.get(message.len().saturating_sub(5)..)
        && tail.eq_ignore_ascii_case("error")
    {
        let head = message[..message.len() - 5].trim_end_matches(char::is_whitespace);
        if let Some(prefix) = head.strip_suffix(':') {
            return LogClassification {
                counter: None,
                error_bucket: Some(prefix.trim().to_string()),
            };
        }
    }
    LogClassification::default()
}

/// `^opencode-mem-\d{4}-\d{2}-\d{2}\.log$` — the dated archive names.
pub(crate) fn is_dated_log_file(name: &str) -> bool {
    let Some(stem) = name
        .strip_prefix("opencode-mem-")
        .and_then(|rest| rest.strip_suffix(".log"))
    else {
        return false;
    };
    let bytes = stem.as_bytes();
    bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| (index == 4 || index == 7) || byte.is_ascii_digit())
}

/// The dated archives plus the two current log names (archive names sorted,
/// then `opencode-mem.log`, then `opencode-mem.log.old`). A directory read
/// failure yields an empty list — upstream's catch.
pub(crate) fn list_log_files(log_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(log_dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let mut files: Vec<PathBuf> = names
        .iter()
        .filter(|name| is_dated_log_file(name))
        .map(|name| log_dir.join(name))
        .collect();
    for current in ["opencode-mem.log", "opencode-mem.log.old"] {
        if names.iter().any(|name| name == current) {
            files.push(log_dir.join(current));
        }
    }
    files
}

/// The inputs of one daily-series scan (upstream `CollectDailyStatsOptions`).
/// `now` is a parameter — no clock read inside the pure collector.
pub(crate) struct DailyScan {
    pub log_dir: PathBuf,
    pub days: u32,
    pub now: chrono::DateTime<chrono::Local>,
}

/// One zeroed day cell (upstream `zeroDay`).
fn zero_day(date: String) -> DailyStatsDto {
    DailyStatsDto {
        date,
        captures: 0,
        capture_skipped: 0,
        profile_injections: 0,
        compaction_injections: 0,
        errors: BTreeMap::new(),
    }
}

/// Reads and aggregates the structured log counters into a zero-filled
/// local-date series, oldest first.
///
/// Infallible by design: an unreadable directory yields the bare window and an
/// unreadable file is skipped (upstream's try/catch never fires either). Only
/// parsed + classified events whose date key is inside the window are counted.
pub(crate) fn collect_daily_stats(scan: &DailyScan) -> Vec<DailyStatsDto> {
    let day_count = u64::from(scan.days.max(1));
    let end = scan.now.date_naive();
    let mut series: Vec<DailyStatsDto> = Vec::new();
    let mut positions: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for offset in (0..day_count).rev() {
        // checked_sub_days: an extreme `days` value shortens the window instead
        // of panicking (ADR-0013; unreachable at the handler's `days: 14`).
        let Some(date) = end.checked_sub_days(chrono::Days::new(offset)) else {
            continue;
        };
        let key = date.format("%Y-%m-%d").to_string();
        positions.insert(key.clone(), series.len());
        series.push(zero_day(key));
    }

    for path in list_log_files(&scan.log_dir) {
        let Ok(contents) = std::fs::read_to_string(&path) else {
            continue;
        };
        for line in contents.lines() {
            let Some(parsed) = parse_log_line(line) else {
                continue;
            };
            let classification = classify_log_message(&parsed.message);
            let Some(&position) = positions.get(&parsed.date) else {
                continue;
            };
            let day = &mut series[position];
            match (classification.counter, classification.error_bucket) {
                (Some(DailyCounter::Captures), _) => day.captures += 1,
                (Some(DailyCounter::CaptureSkipped), _) => day.capture_skipped += 1,
                (Some(DailyCounter::ProfileInjections), _) => day.profile_injections += 1,
                (Some(DailyCounter::CompactionInjections), _) => day.compaction_injections += 1,
                // An empty bucket is upstream-falsy (`errorBucket: ""` is never
                // counted), so the guard mirrors the collector exactly.
                (None, Some(bucket)) if !bucket.is_empty() => {
                    *day.errors.entry(bucket).or_insert(0) += 1;
                }
                _ => {}
            }
        }
    }
    series
}

// -------------------------------------------------------------------------
// Handler tests + parser oracles
// -------------------------------------------------------------------------

#[cfg(test)]
mod parse_tests {
    use super::*;

    /// A unique empty log dir under `std::env::temp_dir()` (TMPDIR-redirected
    /// into `.tmp/scratch` at the cargo invocation).
    fn temp_log_dir(label: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        let dir = std::env::temp_dir().join(format!(
            "mem-web-stats-{label}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("the temp log dir is creatable");
        dir
    }

    /// One structured-logger line, upstream `timestamp(date, hour)` shape.
    fn line(date: &str, hour: &str, content: &str) -> String {
        format!("[{date}T{hour}+00:00] {content}\n")
    }

    /// The fixed scan instant: 2026-10-18 12:00 local (upstream test #1's
    /// `new Date(2026, 9, 18, 12, 0, 0)`), so the 14-day window and its keys
    /// are literal.
    fn fixed_now() -> chrono::DateTime<chrono::Local> {
        use chrono::TimeZone as _;
        chrono::Local
            .with_ymd_and_hms(2026, 10, 18, 12, 0, 0)
            .unwrap()
    }

    #[test]
    fn test_parse_log_line_parses_counter_signatures() {
        let fixtures = [
            (
                "Auto-capture memory persisted: {\"id\":\"memory-1\"}",
                DailyCounter::Captures,
            ),
            (
                "Auto-capture skipped: {\"reason\":\"short\"}",
                DailyCounter::CaptureSkipped,
            ),
            (
                "profile inject: {\"prefs\":[]}",
                DailyCounter::ProfileInjections,
            ),
            (
                "Compaction memory injected: {\"count\":1}",
                DailyCounter::CompactionInjections,
            ),
        ];

        for (content, counter) in fixtures {
            let parsed = parse_log_line(&format!("[2026-10-16T19:18:33.096+11:00] {content}"))
                .unwrap_or_else(|| panic!("the counter line parses: {content}"));
            assert_eq!(parsed.date, "2026-10-16");
            let classification = classify_log_message(&parsed.message);
            assert_eq!(classification.counter, Some(counter), "{content}");
            assert_eq!(classification.error_bucket, None, "{content}");
        }
    }

    #[test]
    fn test_classify_log_message_known_structured_error_signatures() {
        let fixtures = [
            (
                "[2026-10-16T19:18:33.096+11:00] Turso ready gate failed: {\"error\":\"Error: failed to open database /home/x/.opencode-mem/data/user-profiles.db: I/O error: short read on WAL frame at offset 32: expected 4096 bytes, got 0\"}",
                "Turso ready gate failed",
            ),
            (
                "[2026-10-13T00:32:31.331+11:00] Failed to load skills for OpenCode registration: {\"error\":\"TypeError: The database connection is not open\"}",
                "Failed to load skills for OpenCode registration",
            ),
            (
                "[2026-10-16T19:18:46.118+11:00] listMemories: error: {\"error\":\"failed to open database ... short read on WAL frame\"}",
                "listMemories",
            ),
            (
                "[2026-10-16T19:20:00.000+11:00] Auto-capture final error after 3 attempts: {\"error\":\"timeout\"}",
                "Auto-capture final error",
            ),
            (
                "[2026-10-16T19:21:00.000+11:00] Compaction handler error: {\"error\":\"boom\"}",
                "Compaction handler error",
            ),
            (
                "[2026-10-16T19:22:00.000+11:00] Idle processing error: {\"error\":\"boom\"}",
                "Idle processing error",
            ),
            (
                "[2026-10-16T19:23:00.000+11:00] chat.message: ERROR: {\"error\":\"boom\"}",
                "chat.message",
            ),
        ];

        for (raw, bucket) in fixtures {
            let parsed =
                parse_log_line(raw).unwrap_or_else(|| panic!("the fixture line parses: {raw}"));
            let classification = classify_log_message(&parsed.message);
            assert_eq!(
                classification.error_bucket.as_deref(),
                Some(bucket),
                "{raw}"
            );
            assert_eq!(classification.counter, None, "{raw}");
        }
    }

    #[test]
    fn test_parse_log_line_ignores_separators_and_malformed_lines_and_splits_payload() {
        assert!(
            parse_log_line("\n--- Session started: 2026-10-18T12:00:00.000+00:00 ---").is_none()
        );
        assert!(parse_log_line("not a timestamp: Auto-capture memory persisted").is_none());
        assert!(parse_log_line("[2026-99-99T19:18:33.096+11:00] Auto-capture skipped").is_none());

        let parsed = parse_log_line(
            "[2026-10-16T19:18:33.096+11:00] request: error: {\"detail\":\"url: https://example.test/a\"}",
        )
        .expect("the structured line parses");
        assert_eq!(parsed.date, "2026-10-16");
        assert_eq!(parsed.message, "request: error");
        assert_eq!(
            parsed.payload.as_deref(),
            Some("{\"detail\":\"url: https://example.test/a\"}")
        );
        assert_eq!(
            classify_log_message(&parsed.message)
                .error_bucket
                .as_deref(),
            Some("request")
        );

        // A CR-terminated line (windows writer) parses to the same message.
        let cr = parse_log_line(
            "[2026-10-16T19:18:33.096+11:00] Auto-capture skipped: {\"reason\":\"short\"}\r",
        )
        .expect("the CR-terminated line parses");
        assert_eq!(cr.message, "Auto-capture skipped");
    }

    #[test]
    fn test_collect_daily_stats_zero_fills_a_fixed_now_window_and_counts_buckets() {
        let log_dir = temp_log_dir("fixed-window");
        let day_1 = "2026-10-17";
        let day_3 = "2026-10-15";
        let day_4 = "2026-10-14";

        std::fs::write(
            log_dir.join("opencode-mem-2026-10-01.log"),
            format!(
                "{}{}{}",
                line(
                    day_1,
                    "10:00:00.000",
                    "Auto-capture memory persisted: {\"id\":\"1\"}"
                ),
                line(
                    day_1,
                    "10:01:00.000",
                    "Auto-capture skipped: {\"reason\":\"short\"}"
                ),
                line(
                    day_1,
                    "10:02:00.000",
                    "listMemories: error: {\"error\":\"fixture\"}"
                ),
            ),
        )
        .expect("the dated archive is writable");
        std::fs::write(
            log_dir.join("opencode-mem.log.old"),
            line(
                day_3,
                "11:00:00.000",
                "Compaction memory injected: {\"count\":1}",
            ),
        )
        .expect("the .old log is writable");
        std::fs::write(
            log_dir.join("opencode-mem.log"),
            format!(
                "{}{}",
                line(day_4, "09:00:00.000", "profile inject: {\"prefs\":[]}"),
                // Noise the collector must ignore: a malformed line and an
                // event outside the 14-day window.
                "not a timestamp: Auto-capture memory persisted\n",
            ),
        )
        .expect("the current log is writable");
        std::fs::write(log_dir.join("notes.txt"), "ignored\n")
            .expect("the unrelated file is writable");

        let series = collect_daily_stats(&DailyScan {
            log_dir: log_dir.clone(),
            days: 14,
            now: fixed_now(),
        });

        let expected_dates: Vec<String> = (0..14_u32)
            .rev()
            .map(|offset| {
                (fixed_now() - chrono::Duration::days(offset.into()))
                    .format("%Y-%m-%d")
                    .to_string()
            })
            .collect();
        let dates: Vec<String> = series.iter().map(|day| day.date.clone()).collect();
        assert_eq!(dates, expected_dates, "the window is oldest-first");

        let first = &series[0];
        assert_eq!(
            (
                first.captures,
                first.capture_skipped,
                first.profile_injections,
                first.compaction_injections
            ),
            (0, 0, 0, 0)
        );
        assert!(first.errors.is_empty(), "a zero day carries no buckets");

        let day = series
            .iter()
            .find(|day| day.date == day_1)
            .expect("day(1) in window");
        assert_eq!(day.captures, 1);
        assert_eq!(day.capture_skipped, 1);
        assert_eq!(day.errors.get("listMemories"), Some(&1));

        let compaction = series
            .iter()
            .find(|day| day.date == day_3)
            .expect("day(3) in window");
        assert_eq!(compaction.compaction_injections, 1);

        let profile = series
            .iter()
            .find(|day| day.date == day_4)
            .expect("day(4) in window");
        assert_eq!(profile.profile_injections, 1);

        let _ = std::fs::remove_dir_all(&log_dir);
    }

    #[test]
    fn test_collect_daily_stats_missing_log_dir_yields_zero_filled_window() {
        let missing = std::env::temp_dir().join(format!(
            "mem-web-stats-missing-{}-does-not-exist",
            std::process::id()
        ));
        let series = collect_daily_stats(&DailyScan {
            log_dir: missing,
            days: 14,
            now: fixed_now(),
        });
        assert_eq!(series.len(), 14);
        assert!(series.iter().all(|day| day.captures == 0
            && day.capture_skipped == 0
            && day.profile_injections == 0
            && day.compaction_injections == 0
            && day.errors.is_empty()));
    }
}
