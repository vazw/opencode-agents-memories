//! The ONE startup storage-root audit (issue 129, Step 3).
//!
//! Hard rules, all enforced here:
//!
//! - **Read-only.** The audit opens nothing, creates nothing and repairs
//!   nothing: it observes the eager actor's result and the file class it left
//!   behind, and records every lazy/disabled store as `unknown` with a fixed
//!   reason token.
//! - **Allowlisted tokens only.** A store is named by its file name
//!   ([`EPISODIC_DB`] …); `vault.db` and `shard.db` stand for the per-scope
//!   vault files and the dynamic `*_shard_*.db` class, so no raw path and no
//!   registry row ever reaches the event.
//! - **No free text.** `reason` is a fixed token, never the store's `Display`:
//!   the store error carries the storage path.
//! - **Control-free.** The one field that can carry hostile bytes is
//!   `root_name`, and it is escaped by [`escape_control`] after
//!   [`root_name`] strips the parents.
//!
//! Step 2b measured that a production startup never reaches the shard registry
//! (`metadata.db` is not created), so there is NO per-shard entry and no
//! `opaque_shard_id` here: G4 is absent by construction, not "always clean".

use std::path::Path;

use mem_episodic::error::EpisodicError;
use mem_store::db::DbHandle;
use serde::Serialize;

/// The one event name: one event per composition, i.e. per engine process.
pub(crate) const EVENT: &str = "storage.root_audit";

/// The eager store: the only row a startup actor opens.
const EPISODIC_DB: &str = "episodic.db";

/// The fixed reason token for a refused open (never the store's text).
const REFUSAL_REASON: &str = "open_failed";
/// The store existed before this boot and the open succeeded.
const REASON_OBSERVED: &str = "observed";
/// The store was absent before this boot and present after it.
const REASON_OBSERVED_CREATED: &str = "observed_created";
/// No startup actor touched this store (lazy store, or the registry).
const REASON_NOT_OBSERVED: &str = "not_observed";
/// The store is lazy and this boot did not open it.
const REASON_NOT_OPENED: &str = "not_opened";
/// The feature flag that would construct the store is off.
const REASON_DISABLED: &str = "disabled";
/// Composition failed before this store could be considered.
const REASON_NOT_ATTEMPTED: &str = "not_attempted";

/// One audited store (the event's `entries` element).
#[derive(Debug, Serialize)]
pub(crate) struct AuditEntry {
    store: &'static str,
    outcome: &'static str,
    reason: String,
}

/// The file class of one store path, observed WITHOUT opening it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Presence {
    /// Nothing at that path.
    Absent,
    /// A regular file (a symlink does NOT count: the boundary stays explicit).
    Regular,
    /// A directory, a symlink or an unreadable path: never a positive outcome.
    Other,
}

/// The file class at `path` (`symlink_metadata`: the link itself, not its
/// target). Only `NotFound` is `Absent`, so a permission error can never be
/// misread as "created".
pub(crate) fn presence(path: &Path) -> Presence {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Presence::Other,
        Ok(metadata) if metadata.is_file() => Presence::Regular,
        Ok(_) => Presence::Other,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Presence::Absent,
        Err(_) => Presence::Other,
    }
}

/// The storage root's name for the event: the BASENAME only (parents dropped)
/// with its control characters escaped.
pub(crate) fn root_name(root: &Path) -> String {
    let basename = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    escape_control(&basename)
}

/// Escapes every control character (and the escape character itself) so the
/// value is safe on a single NDJSON line and readable in evidence.
pub(crate) fn escape_control(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for character in input.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            control if control.is_control() => {
                escaped.push_str(&format!("\\u{{{:04x}}}", control as u32));
            }
            other => escaped.push(other),
        }
    }
    escaped
}

/// The fixed refusal token. The typed error is deliberately NOT read: its
/// `Display` carries the storage path.
fn refusal_reason(_error: &EpisodicError) -> &'static str {
    REFUSAL_REASON
}

/// The eager store's entry. `before`/`after` are the read-only observations
/// around the ONE `ready()` call `composition` already makes.
fn episodic_entry(
    before: Presence,
    after: Presence,
    ready: &Result<DbHandle, EpisodicError>,
) -> AuditEntry {
    let (outcome, reason) = match ready {
        // G2's single mutation site: `reason: error.to_string()` here would put
        // the store's text (with the storage path) into the event.
        Err(error) => ("refused", refusal_reason(error).to_string()),
        Ok(_) => match (before, after) {
            (Presence::Absent, Presence::Regular) => {
                ("created", REASON_OBSERVED_CREATED.to_string())
            }
            (Presence::Regular, Presence::Regular) => ("found", REASON_OBSERVED.to_string()),
            _ => ("unknown", REASON_NOT_OBSERVED.to_string()),
        },
    };
    AuditEntry {
        store: EPISODIC_DB,
        outcome,
        reason,
    }
}

/// The rows no startup actor opens (Step 2b matrix, measured).
fn unopened_entry(store: &'static str, attempted: bool, reason: &'static str) -> AuditEntry {
    AuditEntry {
        store,
        outcome: "unknown",
        reason: if attempted {
            reason.to_string()
        } else {
            REASON_NOT_ATTEMPTED.to_string()
        },
    }
}

/// Emits the ONE event for this composition. Called immediately after the
/// `ready()` result, on BOTH arms (success and failure).
pub(crate) fn emit(
    root: &Path,
    before: Presence,
    ready: &Result<DbHandle, EpisodicError>,
    promotion_pipeline: bool,
) {
    let after = presence(&root.join(EPISODIC_DB));
    let attempted = ready.is_ok();
    let mut entries = vec![episodic_entry(before, after, ready)];
    for (store, reason) in [
        // Lazy: constructed, never opened during startup.
        ("user-profiles.db", REASON_NOT_OPENED),
        ("user-prompts.db", REASON_NOT_OPENED),
        // Feature-gated: off means the store is not even constructed; on, the
        // store is constructed but startup never observes it (Step 2b matrix:
        // `not_observed`, not the lazy `not_opened`).
        (
            "facts.db",
            if promotion_pipeline {
                REASON_NOT_OBSERVED
            } else {
                REASON_DISABLED
            },
        ),
        ("skills.db", REASON_NOT_OPENED),
        // The registry is not reached during startup (Step 2b, measured).
        ("metadata.db", REASON_NOT_OBSERVED),
        ("vault.db", REASON_NOT_OPENED),
        ("shard.db", REASON_NOT_OBSERVED),
    ] {
        entries.push(unopened_entry(store, attempted, reason));
    }
    crate::engine::structured_stderr(
        EVENT,
        "info",
        "engine",
        serde_json::json!({
            "root_name": root_name(root),
            "entries": entries,
        }),
    );
}
