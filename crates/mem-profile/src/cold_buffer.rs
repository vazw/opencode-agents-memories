//! `cold-buffer.json` snapshot store (W3.1 step 6) — port of upstream
//! `user-profile-manager.ts` L227-330 (`loadColdBuffers`, `beginColdBufferRound`,
//! `getColdBuffer`, `saveColdBuffers`).
//!
//! **Fail-closed loads.** A missing file legitimately means "empty snapshot";
//! every other failure (permissions, unparseable JSON, wrong shape) means the
//! on-disk state is *unknown* and refuses the round. Treating it as empty
//! would persist an empty snapshot over data nobody read (D-V5).
//!
//! **Atomic, short-write-hardened saves.** A unique tmp file is opened
//! `create_new` with mode 0600, the payload is written as UTF-8 *bytes*
//! (byte-accurate — a string-offset loop corrupts multibyte content), fsynced,
//! closed, and only then renamed over the target. The rename is the ONLY
//! publish: on any failure the tmp file is unlinked and the previous snapshot
//! bytes are untouched.
//!
//! Invariant (write-all loop): at the top of every iteration, bytes
//! `payload[0..written)` have been forwarded to the tmp fd and
//! `written <= payload.len()`. Termination: each iteration advances `written`
//! by at least one byte or terminates with a `write stalled after {N} bytes`
//! error — a non-positive forward progress can never loop forever.

use std::collections::BTreeMap;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::ProfileError;
use crate::ids::next_token;
use crate::profile_store::ProfileItem;

/// The snapshot: per-profile buckets (upstream `Map<string, bucket>`).
pub type Snapshot = BTreeMap<String, Bucket>;

/// One profile's buffered observations (upstream's
/// `{ preferences, patterns, workflows }` triple; a missing category is an
/// empty array).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Bucket {
    #[serde(default)]
    pub preferences: Vec<ProfileItem>,
    #[serde(default)]
    pub patterns: Vec<ProfileItem>,
    #[serde(default)]
    pub workflows: Vec<ProfileItem>,
}

impl Bucket {
    /// An empty bucket.
    pub fn new() -> Self {
        Self::default()
    }

    /// Upstream save predicate: a bucket with no items in any category is
    /// omitted from the persisted snapshot.
    pub fn is_empty(&self) -> bool {
        self.preferences.is_empty() && self.patterns.is_empty() && self.workflows.is_empty()
    }
}

/// Upstream `COLD_BUFFER_FILE_NAME`.
pub const COLD_BUFFER_FILE_NAME: &str = "cold-buffer.json";

/// The three legacy flat-format keys (upstream `legacyKeys`).
const LEGACY_KEYS: [&str; 3] = ["preferences", "patterns", "workflows"];

/// Fault-injection seam for the write-all loop.
///
/// Fixture-only (doc-commented as such): production `save_snapshot` always
/// passes `WriteFaults::default()`. It mirrors upstream's
/// `profile-cold-buffer-worker-preload.ts` monkey-patch — "limit how many
/// bytes a single low-level write forwards" — without touching the real
/// tmp+rename flow or the real fd bookkeeping.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WriteFaults {
    /// Maximum bytes forwarded per single `write` call (`None` = unlimited).
    pub chunk_limit: Option<usize>,
    /// At this offset, the next `write` returns 0 bytes (no forward progress).
    pub zero_at: Option<usize>,
    /// At/after this offset, the next `write` fails with a simulated EIO.
    pub fail_at: Option<usize>,
}

/// The path a snapshot for a storage dir lives at.
pub fn cold_buffer_path(storage_path: &Path) -> PathBuf {
    storage_path.join(COLD_BUFFER_FILE_NAME)
}

/// Upstream `loadColdBuffers` — strict read.
pub fn load_snapshot(path: &Path) -> Result<Snapshot, ProfileError> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Snapshot::new()),
        Err(error) => {
            return Err(ProfileError::ColdBuffer(format!(
                "profile cold buffer: unreadable ({}): {error}",
                path.display()
            )));
        }
    };
    parse_snapshot(&raw, path)
}

/// Parses a snapshot payload strictly, reporting the exact upstream sentences.
pub fn parse_snapshot(raw: &str, path: &Path) -> Result<Snapshot, ProfileError> {
    let data: serde_json::Value = serde_json::from_str(raw).map_err(|error| {
        ProfileError::ColdBuffer(format!(
            "profile cold buffer: corrupt JSON, refusing to overwrite ({}): {error}",
            path.display()
        ))
    })?;

    let Some(object) = data.as_object() else {
        return Err(ProfileError::ColdBuffer(format!(
            "profile cold buffer: unexpected schema, refusing to overwrite ({})",
            path.display()
        )));
    };

    // Explicit legacy detection: the flat format is exactly the three category
    // keys (each an array, at least one present) and nothing else. Anything
    // else containing those keys is an unknown shape, not "legacy".
    let present_legacy: Vec<&str> = LEGACY_KEYS
        .iter()
        .copied()
        .filter(|key| object.contains_key(*key))
        .collect();
    if !present_legacy.is_empty() {
        let only_legacy_keys = object.keys().all(|key| LEGACY_KEYS.contains(&key.as_str()));
        let all_arrays = LEGACY_KEYS.iter().all(|key| {
            object
                .get(*key)
                .map(|value| value.is_array())
                .unwrap_or(true)
        });
        if only_legacy_keys && all_arrays {
            // Cross-user contaminated and not attributable to a profile: the
            // documented migration path drops it rather than replaying it.
            tracing::info!(
                keys = ?present_legacy,
                "profile cold buffer: dropping legacy unattributed buffer"
            );
            return Ok(Snapshot::new());
        }
        return Err(ProfileError::ColdBuffer(format!(
            "profile cold buffer: legacy keys with unknown shape, refusing to overwrite ({})",
            path.display()
        )));
    }

    let mut snapshot = Snapshot::new();
    for (profile_id, value) in object {
        let Some(bucket_value) = value.as_object() else {
            return Err(ProfileError::ColdBuffer(format!(
                "profile cold buffer: bucket \"{profile_id}\" is not an object, refusing to overwrite ({})",
                path.display()
            )));
        };
        let mut bucket = Bucket::new();
        for category in LEGACY_KEYS {
            let Some(items) = bucket_value.get(category) else {
                continue;
            };
            let Some(items) = items.as_array() else {
                return Err(ProfileError::ColdBuffer(format!(
                    "profile cold buffer: bucket \"{profile_id}.{category}\" is not an array, refusing to overwrite ({})",
                    path.display()
                )));
            };
            let mut parsed_items = Vec::with_capacity(items.len());
            for item in items {
                let Some(item_object) = item.as_object() else {
                    return Err(ProfileError::ColdBuffer(format!(
                        "profile cold buffer: bucket \"{profile_id}.{category}\" holds a non-object item, refusing to overwrite ({})",
                        path.display()
                    )));
                };
                if !item_object
                    .get("description")
                    .is_some_and(|v| v.is_string())
                {
                    return Err(ProfileError::ColdBuffer(format!(
                        "profile cold buffer: bucket \"{profile_id}.{category}\" holds an item without a string description, refusing to overwrite ({})",
                        path.display()
                    )));
                }
                let parsed: ProfileItem = serde_json::from_value(item.clone()).map_err(|error| {
                    ProfileError::ColdBuffer(format!(
                        "profile cold buffer: bucket \"{profile_id}.{category}\" holds a non-object item, refusing to overwrite ({}): {error}",
                        path.display()
                    ))
                })?;
                parsed_items.push(parsed);
            }
            match category {
                "preferences" => bucket.preferences = parsed_items,
                "patterns" => bucket.patterns = parsed_items,
                "workflows" => bucket.workflows = parsed_items,
                _ => {}
            }
        }
        snapshot.insert(profile_id.clone(), bucket);
    }

    if !snapshot.is_empty() {
        tracing::info!(
            profiles = snapshot.len(),
            "profile cold buffer: loaded from disk"
        );
    }
    Ok(snapshot)
}

/// Serializes the snapshot exactly as upstream does: empty buckets omitted,
/// compact JSON, UTF-8 bytes.
pub fn serialize_snapshot(snapshot: &Snapshot) -> Result<Vec<u8>, ProfileError> {
    let mut object = serde_json::Map::new();
    for (profile_id, bucket) in snapshot {
        if bucket.is_empty() {
            continue;
        }
        object.insert(
            profile_id.clone(),
            serde_json::to_value(bucket).map_err(|error| ProfileError::json(&error))?,
        );
    }
    let value = serde_json::Value::Object(object);
    serde_json::to_vec(&value).map_err(|error| ProfileError::json(&error))
}

/// Upstream `saveColdBuffers` with the production (fault-free) write seam.
pub fn save_snapshot(path: &Path, snapshot: &Snapshot) -> Result<(), ProfileError> {
    save_snapshot_with(path, snapshot, &WriteFaults::default())
}

/// Upstream `saveColdBuffers` with an injected write-fault seam.
///
/// Only the byte-forwarding behaviour of the write loop is faulted; the tmp
/// `create_new` + 0600 open, the fsync, the atomic rename and the
/// unlink-on-failure cleanup are the real production path.
pub fn save_snapshot_with(
    path: &Path,
    snapshot: &Snapshot,
    faults: &WriteFaults,
) -> Result<(), ProfileError> {
    let payload = serialize_snapshot(snapshot)?;
    let tmp_path = tmp_path_for(path);

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    // Mode 0600 is applied ATOMICALLY at create time (upstream `openSync(tmp,
    // "wx", 0o600)`): a post-create chmod would leave a umask-derived window.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = match options.open(&tmp_path) {
        Ok(file) => file,
        Err(error) => {
            return Err(save_failed(path, &error.to_string()));
        }
    };

    let outcome: Result<(), String> = {
        let handle = &mut file;
        write_all(handle, &payload, faults).and_then(|()| {
            // fsync before publish: a deliberate strict hardening of
            // upstream's close-only publish (D-V5/Step-6). The renamed file
            // must be durable, not merely visible.
            handle.sync_all().map_err(|error| error.to_string())
        })
    };
    drop(file);

    match outcome {
        Ok(()) => std::fs::rename(&tmp_path, path).map_err(|error| {
            let _ = std::fs::remove_file(&tmp_path);
            save_failed(path, &error.to_string())
        }),
        Err(message) => {
            let _ = std::fs::remove_file(&tmp_path);
            Err(save_failed(path, &message))
        }
    }
}

/// Upstream `save failed ({path}): {src}`.
fn save_failed(path: &Path, source: &str) -> ProfileError {
    ProfileError::ColdBuffer(format!(
        "profile cold buffer: save failed ({}): {source}",
        path.display()
    ))
}

/// `{cold-buffer.json}.{token}.tmp` (upstream `randomUUID()` suffix).
fn tmp_path_for(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.{}.tmp", path.display(), next_token()))
}

/// The tmp file's mode is applied atomically by the open (see
/// [`save_snapshot_with`]); non-Unix platforms have no mode to set.
/// The write-all loop.
///
/// Loop invariant: at the top of each iteration, bytes `payload[0..written)`
/// have been forwarded to `writer` and `written <= payload.len()`.
/// Termination: each iteration either advances `written` by ≥ 1 or returns Err.
pub fn write_all<W: Write>(
    writer: &mut W,
    payload: &[u8],
    faults: &WriteFaults,
) -> Result<(), String> {
    let mut written = 0usize;
    while written < payload.len() {
        let remaining = payload.len() - written;

        if faults.zero_at == Some(written) {
            return Err(stalled(written));
        }
        if faults.fail_at.is_some_and(|fail_at| written >= fail_at) {
            return Err("simulated EIO mid-payload".to_string());
        }

        let mut chunk = remaining;
        if let Some(limit) = faults.chunk_limit {
            chunk = chunk.min(limit);
        }
        if let Some(fail_at) = faults.fail_at
            && fail_at > written
        {
            chunk = chunk.min(fail_at - written);
        }
        if chunk == 0 {
            // A zero-length forward would loop forever: terminate instead.
            return Err(stalled(written));
        }

        match writer.write(&payload[written..written + chunk]) {
            Ok(forwarded) if forwarded > 0 => written += forwarded,
            Ok(_) => return Err(stalled(written)),
            Err(error) => return Err(error.to_string()),
        }
    }
    Ok(())
}

/// Upstream `profile cold buffer: write stalled after ${written} bytes`.
fn stalled(written: usize) -> String {
    format!("profile cold buffer: write stalled after {written} bytes")
}

// ---------------------------------------------------------------------------
// Private-item unit tests (house law: `#[cfg(test)]` covers private items;
// integration tests assert through the public interface)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile_store::ProfileItem;

    /// Unique temp dir under `std::env::temp_dir()` (never the user home).
    fn temp_dir(prefix: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let counter = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "mem-profile-w31-unit-{prefix}-{}-{counter}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn clean(dir: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    /// True when the process bypasses DAC (euid 0 / CAP_DAC_OVERRIDE), which
    /// makes permission-dependent cases unprovable.
    fn process_bypasses_file_modes() -> bool {
        #[cfg(unix)]
        {
            // SAFETY: `geteuid` has no preconditions and no side effects.
            unsafe { libc::geteuid() == 0 }
        }
        #[cfg(not(unix))]
        {
            true
        }
    }

    fn item(description: &str) -> ProfileItem {
        ProfileItem {
            description: description.to_string(),
            category: Some("style".to_string()),
            ..Default::default()
        }
    }

    fn snapshot_with(profile_id: &str, descriptions: &[&str]) -> Snapshot {
        let mut bucket = Bucket::new();
        bucket.preferences = descriptions.iter().map(|d| item(d)).collect();
        let mut snapshot = Snapshot::new();
        snapshot.insert(profile_id.to_string(), bucket);
        snapshot
    }

    fn tmp_leftovers(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().to_string())
                    .filter(|name| name.contains(".tmp"))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn test_load_missing_file_is_an_empty_snapshot() {
        let dir = temp_dir("missing");
        let snapshot = load_snapshot(&dir.join(COLD_BUFFER_FILE_NAME)).expect("missing file is ok");
        assert!(snapshot.is_empty());
        clean(&dir);
    }

    #[test]
    fn test_load_blank_forms_are_corrupt_json_refusals() {
        let dir = temp_dir("blank");
        for raw in ["", "   ", "\n\t"] {
            let path = dir.join(COLD_BUFFER_FILE_NAME);
            std::fs::write(&path, raw).expect("write blank");
            let error = load_snapshot(&path).expect_err("blank is corrupt");
            assert!(
                error
                    .to_string()
                    .starts_with("profile cold buffer: corrupt JSON"),
                "raw {raw:?} produced {error}"
            );
            assert_eq!(
                std::fs::read_to_string(&path).expect("read"),
                raw,
                "a refused load must never touch the bytes"
            );
        }
        clean(&dir);
    }

    #[test]
    fn test_load_corrupt_json_reports_and_leaves_bytes() {
        let dir = temp_dir("corrupt");
        let path = dir.join(COLD_BUFFER_FILE_NAME);
        std::fs::write(&path, "{not valid json").expect("write corrupt");
        let error = load_snapshot(&path).expect_err("corrupt is refused");
        assert!(error.to_string().contains("refusing to overwrite"));
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            "{not valid json"
        );
        clean(&dir);
    }

    #[test]
    fn test_load_non_object_root_is_unexpected_schema() {
        let dir = temp_dir("schema");
        for raw in ["null", "[]", "42", "\"text\""] {
            let path = dir.join(COLD_BUFFER_FILE_NAME);
            std::fs::write(&path, raw).expect("write");
            let error = load_snapshot(&path).expect_err("non-object root refused");
            assert!(
                error.to_string().contains("unexpected schema"),
                "raw {raw} produced {error}"
            );
        }
        clean(&dir);
    }

    #[test]
    fn test_load_valid_legacy_flat_file_is_dropped_not_rejected() {
        let dir = temp_dir("legacy");
        for raw in [
            r#"{"preferences":[],"patterns":[],"workflows":[]}"#,
            r#"{"preferences":[{"description":"legacy item"}]}"#,
        ] {
            let path = dir.join(COLD_BUFFER_FILE_NAME);
            std::fs::write(&path, raw).expect("write");
            let snapshot = load_snapshot(&path).expect("legacy accepts");
            assert!(snapshot.is_empty(), "legacy rows are dropped, raw {raw}");
        }
        clean(&dir);
    }

    #[test]
    fn test_load_legacy_keys_with_unknown_shape_are_rejected() {
        let dir = temp_dir("legacy-mixed");
        for raw in [
            r#"{"preferences":[],"profile_N":{"patterns":[]}}"#,
            r#"{"preferences":"not-an-array"}"#,
            r#"{"workflows":{},"patterns":[]}"#,
        ] {
            let path = dir.join(COLD_BUFFER_FILE_NAME);
            std::fs::write(&path, raw).expect("write");
            let error = load_snapshot(&path).expect_err("unknown shape refused");
            assert!(
                error.to_string().contains("legacy keys with unknown shape"),
                "raw {raw} produced {error}"
            );
        }
        clean(&dir);
    }

    #[test]
    fn test_load_invalid_bucket_shapes_report_the_exact_sentences() {
        let cases: [(&str, &str); 6] = [
            (
                r#"{"profile_N":null}"#,
                "bucket \"profile_N\" is not an object",
            ),
            (
                r#"{"profile_N":"oops"}"#,
                "bucket \"profile_N\" is not an object",
            ),
            (
                r#"{"profile_N":{"preferences":"nope"}}"#,
                "bucket \"profile_N.preferences\" is not an array",
            ),
            (
                r#"{"profile_N":{"preferences":[null]}}"#,
                "bucket \"profile_N.preferences\" holds a non-object item",
            ),
            (
                r#"{"profile_N":{"patterns":[42]}}"#,
                "bucket \"profile_N.patterns\" holds a non-object item",
            ),
            (
                r#"{"profile_N":{"workflows":[{"category":"x"}]}}"#,
                "bucket \"profile_N.workflows\" holds an item without a string description",
            ),
        ];
        let dir = temp_dir("shapes");
        for (raw, expected) in cases {
            let path = dir.join(COLD_BUFFER_FILE_NAME);
            std::fs::write(&path, raw).expect("write");
            let error = load_snapshot(&path).expect_err("shape refused");
            let message = error.to_string();
            assert!(
                message.contains(expected),
                "raw {raw} produced {message:?}, expected {expected:?}"
            );
            assert!(message.contains("refusing to overwrite"));
            assert_eq!(std::fs::read_to_string(&path).expect("read"), raw);
            assert!(tmp_leftovers(&dir).is_empty());
        }
        clean(&dir);
    }

    #[test]
    fn test_load_unreadable_file_is_refused_when_dac_applies() {
        if process_bypasses_file_modes() {
            return;
        }
        let dir = temp_dir("unreadable");
        let path = dir.join(COLD_BUFFER_FILE_NAME);
        std::fs::write(&path, r#"{"profile_X":{"preferences":[]}}"#).expect("write");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).expect("chmod");
        }
        let error = load_snapshot(&path).expect_err("unreadable is refused");
        assert!(
            error.to_string().contains("unreadable"),
            "unexpected message {error}"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        clean(&dir);
    }

    #[test]
    fn test_load_valid_snapshot_round_trips_through_save() {
        let dir = temp_dir("roundtrip");
        let path = dir.join(COLD_BUFFER_FILE_NAME);
        let mut snapshot = snapshot_with("profile_A", &["first", "second"]);
        snapshot.insert(
            "__unattributed__".to_string(),
            Bucket {
                workflows: vec![item("a workflow")],
                ..Default::default()
            },
        );

        save_snapshot(&path, &snapshot).expect("save");
        let reloaded = load_snapshot(&path).expect("reload");
        assert_eq!(reloaded, snapshot);
        clean(&dir);
    }

    #[test]
    fn test_save_omits_empty_buckets_and_writes_an_object() {
        let dir = temp_dir("empty-buckets");
        let path = dir.join(COLD_BUFFER_FILE_NAME);

        let mut snapshot = Snapshot::new();
        snapshot.insert("profile_EMPTY".to_string(), Bucket::new());
        save_snapshot(&path, &snapshot).expect("save empty");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "{}");

        let mut mixed = snapshot_with("profile_KEEP", &["kept"]);
        mixed.insert("profile_EMPTY".to_string(), Bucket::new());
        save_snapshot(&path, &mixed).expect("save mixed");
        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
        assert!(parsed.get("profile_KEEP").is_some());
        assert!(parsed.get("profile_EMPTY").is_none());
        clean(&dir);
    }

    #[test]
    fn test_save_with_chunk_limits_is_byte_accurate_for_multibyte_descriptions() {
        let dir = temp_dir("chunked");
        let path = dir.join(COLD_BUFFER_FILE_NAME);
        let description = "用户偏好：简洁中文回答并附带代码示例";
        let snapshot = snapshot_with("profile_SW", &[description]);

        for limit in [1usize, 3, 7, usize::MAX] {
            let faults = WriteFaults {
                chunk_limit: Some(limit),
                ..Default::default()
            };
            save_snapshot_with(&path, &snapshot, &faults).expect("chunked save completes");
            let reloaded = load_snapshot(&path).expect("reload");
            assert_eq!(
                reloaded["profile_SW"].preferences[0].description, description,
                "chunk limit {limit} must round-trip the exact description"
            );
            assert!(tmp_leftovers(&dir).is_empty(), "no tmp leftovers");
        }
        clean(&dir);
    }

    #[test]
    fn test_save_zero_byte_write_stalls_and_preserves_the_target() {
        let dir = temp_dir("zero");
        let path = dir.join(COLD_BUFFER_FILE_NAME);
        let pre_existing = r#"{"profile_PRE":{"preferences":[{"description":"previous"}]}}"#;
        std::fs::write(&path, pre_existing).expect("seed");

        let error = save_snapshot_with(
            &path,
            &snapshot_with("profile_SW", &["cannot be written"]),
            &WriteFaults {
                zero_at: Some(0),
                ..Default::default()
            },
        )
        .expect_err("a stalled write must fail loudly");

        let message = error.to_string();
        assert!(message.contains("write stalled after 0 bytes"), "{message}");
        assert!(message.contains("save failed"));
        assert_eq!(std::fs::read_to_string(&path).expect("read"), pre_existing);
        assert!(tmp_leftovers(&dir).is_empty(), "tmp must be unlinked");
        clean(&dir);
    }

    #[test]
    fn test_save_mid_payload_failure_preserves_target_and_clears_tmp() {
        let dir = temp_dir("half");
        let path = dir.join(COLD_BUFFER_FILE_NAME);
        let pre_existing = r#"{"profile_PRE":{"preferences":[{"description":"previous"}]}}"#;
        std::fs::write(&path, pre_existing).expect("seed");

        let snapshot = snapshot_with(
            "profile_SW",
            &["entry that fails halfway through the payload"],
        );
        let payload_len = serialize_snapshot(&snapshot).expect("serialize").len();

        for fail_at in [0usize, payload_len / 2, payload_len - 1] {
            let error = save_snapshot_with(
                &path,
                &snapshot,
                &WriteFaults {
                    fail_at: Some(fail_at),
                    ..Default::default()
                },
            )
            .expect_err("a mid-payload failure must propagate");
            assert!(error.to_string().contains("save failed"), "{error}");
            assert_eq!(
                std::fs::read_to_string(&path).expect("read"),
                pre_existing,
                "fail_at {fail_at} must leave the target bytes untouched"
            );
            assert!(
                tmp_leftovers(&dir).is_empty(),
                "fail_at {fail_at} must unlink the tmp file"
            );
        }
        clean(&dir);
    }

    #[test]
    fn test_save_failure_on_unwritable_dir_preserves_target_when_dac_applies() {
        if process_bypasses_file_modes() {
            return;
        }
        let dir = temp_dir("readonly-dir");
        let path = dir.join(COLD_BUFFER_FILE_NAME);
        std::fs::write(&path, "{}").expect("seed");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).expect("chmod");
        }

        let result = save_snapshot(&path, &snapshot_with("profile_T", &["cannot save"]));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }

        let error = result.expect_err("an unwritable dir must propagate");
        assert!(error.to_string().contains("save failed"), "{error}");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "{}");
        assert!(tmp_leftovers(&dir).is_empty());
        clean(&dir);
    }

    #[test]
    fn test_write_all_advances_strictly_or_errors() {
        let payload = b"abcdefghij";
        let mut sink: Vec<u8> = Vec::new();
        write_all(&mut sink, payload, &WriteFaults::default()).expect("plain write");
        assert_eq!(sink, payload);

        let mut sink: Vec<u8> = Vec::new();
        write_all(
            &mut sink,
            payload,
            &WriteFaults {
                chunk_limit: Some(3),
                ..Default::default()
            },
        )
        .expect("chunked write");
        assert_eq!(sink, payload);

        // chunk_limit 0 can never make progress: the loop must terminate.
        let mut sink: Vec<u8> = Vec::new();
        let error = write_all(
            &mut sink,
            payload,
            &WriteFaults {
                chunk_limit: Some(0),
                ..Default::default()
            },
        )
        .expect_err("zero-length forwarding must terminate");
        assert_eq!(error, "profile cold buffer: write stalled after 0 bytes");
    }

    #[test]
    fn test_published_snapshot_is_owner_only() {
        // The tmp file is opened `create_new` + 0600 atomically and the rename
        // preserves that mode, so the published snapshot must never be
        // group/world readable (security: a cold buffer can hold prompt text).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let dir = temp_dir("mode");
            let path = dir.join(COLD_BUFFER_FILE_NAME);
            save_snapshot(&path, &snapshot_with("profile_M", &["owner only"])).expect("save");
            let mode = std::fs::metadata(&path)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "published snapshot mode must be 0600");
            clean(&dir);
        }
    }

    #[test]
    fn test_empty_payload_writes_an_empty_file() {
        let dir = temp_dir("empty-payload");
        let path = dir.join(COLD_BUFFER_FILE_NAME);
        save_snapshot(&path, &Snapshot::new()).expect("save empty snapshot");
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "{}");
        clean(&dir);
    }
}
