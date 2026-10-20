//! Validation constants and reject-not-coerce helpers (W4.1
//! [SECURITY & EDGE CASES]).
//!
//! Every rejection is [`EpisodicError::Validation`] carrying the offending
//! field name. Inputs are never normalized into acceptance: outer whitespace,
//! NUL characters, oversize values and out-of-range numbers fail loudly.

use crate::error::EpisodicError;
use crate::payload::SessionMetadata;
use crate::record::AgentSession;
use crate::timeline::{MAX_TIMELINE_LIMIT, TimelineFilter};

/// Maximum byte length of `project_id` / `session_id` / `parent_session_id`.
pub const MAX_ID_LEN: usize = 256;
/// Maximum byte length of `agent_id` (and the derived `agent_name`).
pub const MAX_AGENT_ID_LEN: usize = 128;
/// Maximum byte length of `summary`.
pub const MAX_SUMMARY_BYTES: usize = 100_000;
/// Maximum number of `files_touched` entries.
pub const MAX_FILES: usize = 1_000;
/// Maximum byte length of one touched file path.
pub const MAX_FILE_PATH_LEN: usize = 1_024;
/// Maximum number of `completed_work` entries.
pub const MAX_WORK_ITEMS: usize = 1_000;
/// Maximum byte length of one work-item description.
pub const MAX_WORK_DESC_LEN: usize = 4_096;
/// Maximum number of `decisions` entries.
pub const MAX_DECISIONS: usize = 1_000;
/// Maximum number of `open_threads` entries.
pub const MAX_THREADS: usize = 1_000;
/// Maximum number of unknown (`extra`) keys preserved from a peer.
pub const MAX_EXTRA_KEYS: usize = 64;
/// Maximum byte length of `git_branch`.
pub const MAX_GIT_BRANCH_LEN: usize = 256;
/// Minimum length of a git commit abbreviation.
pub const MIN_GIT_COMMIT_LEN: usize = 7;
/// Maximum length of a git commit (SHA-256 repos use 64).
pub const MAX_GIT_COMMIT_LEN: usize = 64;
/// Maximum byte length of a semantic query string.
pub const MAX_QUERY_BYTES: usize = 10_000;

fn invalid(field: &str, message: impl Into<String>) -> EpisodicError {
    EpisodicError::validation(field, message)
}

/// A required identifier: non-empty, no outer whitespace, no NUL, bounded.
pub fn check_id(field: &str, value: &str, max_len: usize) -> Result<(), EpisodicError> {
    if value.is_empty() {
        return Err(invalid(field, "must not be empty"));
    }
    if value.trim() != value {
        return Err(invalid(
            field,
            "must not have leading or trailing whitespace",
        ));
    }
    if value.contains('\0') {
        return Err(invalid(field, "must not contain NUL"));
    }
    if value.len() > max_len {
        return Err(invalid(
            field,
            format!("must be at most {max_len} bytes, got {}", value.len()),
        ));
    }
    Ok(())
}

/// Validates a fully materialized record (post-defaulting).
///
/// `agent_name` is validated through the metadata payload; see
/// [`validate_metadata`].
pub fn validate(session: &AgentSession) -> Result<(), EpisodicError> {
    check_id("project_id", &session.project_id, MAX_ID_LEN)?;
    check_id("session_id", &session.session_id, MAX_ID_LEN)?;
    check_id("agent_id", &session.agent_id, MAX_AGENT_ID_LEN)?;
    if let Some(parent) = &session.parent_session_id {
        check_id("parent_session_id", parent, MAX_ID_LEN)?;
        if parent == &session.session_id {
            return Err(invalid("parent_session_id", "must differ from session_id"));
        }
    }
    if session.summary.is_empty() {
        return Err(invalid("summary", "must not be empty"));
    }
    if session.summary.contains('\0') {
        return Err(invalid("summary", "must not contain NUL"));
    }
    if session.summary.len() > MAX_SUMMARY_BYTES {
        return Err(invalid(
            "summary",
            format!(
                "must be at most {MAX_SUMMARY_BYTES} bytes, got {}",
                session.summary.len()
            ),
        ));
    }
    if session.created_at < 0 {
        return Err(invalid("created_at", "must be non-negative"));
    }
    if session.updated_at < 0 {
        return Err(invalid("updated_at", "must be non-negative"));
    }
    if session.updated_at < session.created_at {
        return Err(invalid("updated_at", "must not precede created_at"));
    }
    validate_metadata(&session.metadata)
}

/// Validates a timeline/recall filter before any SQL runs (reject, never
/// coerce — W4.2 [SECURITY & EDGE CASES]).
///
/// - `since` / `until` must be non-negative and ordered (`since <= until`);
/// - `last_n_sessions` must not exceed [`MAX_TIMELINE_LIMIT`] (`Some(0)` is
///   the documented empty page, not an error);
/// - `touched_files` (when present) must be a non-empty list of bounded,
///   non-blank, NUL-free entries with no outer whitespace (trimming would be
///   coercion);
/// - `agent_id` / `session_id` go through [`check_id`].
pub fn validate_timeline_filter(filter: &TimelineFilter) -> Result<(), EpisodicError> {
    if let Some(since) = filter.since
        && since < 0
    {
        return Err(invalid("since", "must be non-negative"));
    }
    if let Some(until) = filter.until
        && until < 0
    {
        return Err(invalid("until", "must be non-negative"));
    }
    if let (Some(since), Some(until)) = (filter.since, filter.until)
        && since > until
    {
        return Err(invalid("since", "must not be after until"));
    }
    if let Some(limit) = filter.last_n_sessions
        && limit > MAX_TIMELINE_LIMIT
    {
        return Err(invalid(
            "last_n_sessions",
            format!("must be at most {MAX_TIMELINE_LIMIT}, got {limit}"),
        ));
    }
    if let Some(files) = &filter.touched_files {
        if files.is_empty() {
            return Err(invalid("touched_files", "must not be empty when present"));
        }
        if files.len() > MAX_FILES {
            return Err(invalid(
                "touched_files",
                format!("must contain at most {MAX_FILES} entries"),
            ));
        }
        for entry in files {
            if entry.is_empty() {
                return Err(invalid("touched_files", "entries must not be empty"));
            }
            if entry.trim() != entry {
                return Err(invalid(
                    "touched_files",
                    "entries must not have leading or trailing whitespace",
                ));
            }
            if entry.contains('\0') {
                return Err(invalid("touched_files", "entries must not contain NUL"));
            }
            if entry.len() > MAX_FILE_PATH_LEN {
                return Err(invalid(
                    "touched_files",
                    format!("entries must be at most {MAX_FILE_PATH_LEN} bytes"),
                ));
            }
        }
    }
    if let Some(agent_id) = &filter.agent_id {
        check_id("agent_id", agent_id, MAX_AGENT_ID_LEN)?;
    }
    if let Some(session_id) = &filter.session_id {
        check_id("session_id", session_id, MAX_ID_LEN)?;
    }
    Ok(())
}

/// Validates the structured metadata payload.
pub fn validate_metadata(metadata: &SessionMetadata) -> Result<(), EpisodicError> {
    if metadata.timestamp < 0 {
        return Err(invalid("timestamp", "must be non-negative"));
    }
    check_id("agent_name", &metadata.agent_name, MAX_AGENT_ID_LEN)?;

    if let Some(branch) = &metadata.git_branch {
        if branch.trim().is_empty() {
            return Err(invalid("git_branch", "must not be empty"));
        }
        if branch.trim() != branch {
            return Err(invalid(
                "git_branch",
                "must not have leading or trailing whitespace",
            ));
        }
        if branch.contains('\0') {
            return Err(invalid("git_branch", "must not contain NUL"));
        }
        if branch.len() > MAX_GIT_BRANCH_LEN {
            return Err(invalid(
                "git_branch",
                format!("must be at most {MAX_GIT_BRANCH_LEN} bytes"),
            ));
        }
    }

    if let Some(commit) = &metadata.git_commit {
        if !(MIN_GIT_COMMIT_LEN..=MAX_GIT_COMMIT_LEN).contains(&commit.len()) {
            return Err(invalid(
                "git_commit",
                format!(
                    "must be {MIN_GIT_COMMIT_LEN}..={MAX_GIT_COMMIT_LEN} characters, got {}",
                    commit.len()
                ),
            ));
        }
        if !commit
            .chars()
            .all(|character| character.is_ascii_hexdigit())
        {
            return Err(invalid("git_commit", "must be hexadecimal"));
        }
    }

    if metadata.files_touched.len() > MAX_FILES {
        return Err(invalid(
            "files_touched",
            format!("must contain at most {MAX_FILES} entries"),
        ));
    }
    for path in &metadata.files_touched {
        if path.is_empty() {
            return Err(invalid("files_touched", "entries must not be empty"));
        }
        if path.contains('\0') {
            return Err(invalid("files_touched", "entries must not contain NUL"));
        }
        if path.len() > MAX_FILE_PATH_LEN {
            return Err(invalid(
                "files_touched",
                format!("entries must be at most {MAX_FILE_PATH_LEN} bytes"),
            ));
        }
    }

    if metadata.completed_work.len() > MAX_WORK_ITEMS {
        return Err(invalid(
            "completed_work",
            format!("must contain at most {MAX_WORK_ITEMS} entries"),
        ));
    }
    for item in &metadata.completed_work {
        if item.description.is_empty() {
            return Err(invalid("completed_work", "descriptions must not be empty"));
        }
        if item.description.contains('\0') {
            return Err(invalid(
                "completed_work",
                "descriptions must not contain NUL",
            ));
        }
        if item.description.len() > MAX_WORK_DESC_LEN {
            return Err(invalid(
                "completed_work",
                format!("descriptions must be at most {MAX_WORK_DESC_LEN} bytes"),
            ));
        }
    }

    if metadata.decisions.len() > MAX_DECISIONS {
        return Err(invalid(
            "decisions",
            format!("must contain at most {MAX_DECISIONS} entries"),
        ));
    }

    if metadata.open_threads.len() > MAX_THREADS {
        return Err(invalid(
            "open_threads",
            format!("must contain at most {MAX_THREADS} entries"),
        ));
    }

    if metadata.extra.len() > MAX_EXTRA_KEYS {
        return Err(invalid(
            "extra",
            format!("must contain at most {MAX_EXTRA_KEYS} keys"),
        ));
    }
    for key in metadata.extra.keys() {
        if key.is_empty() || !key.is_ascii() || key.contains('\0') {
            return Err(invalid("extra", "keys must be non-empty ASCII without NUL"));
        }
    }

    Ok(())
}
