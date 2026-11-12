//! Shard consolidation — the composition-root consolidator (parent blueprint
//! Steps 4–10, rev-11 §A–§L).
//!
//! The surface built here is the offline, all-or-nothing plan path: a private
//! plan JSON enumerates every source shard and its pinned target identity, and
//! `--dry-run` answers a versioned [`PlanReport`] without writing anything.
//! Every refusal is typed (exit 64/65/71/72/73/77) and names its own event, so
//! a caller never has to parse prose.
//!
//! This module never opens the live storage root for writing: the census opens
//! each candidate shard read-only (rev-11 §F), and the dry-run path performs no
//! mutation at all — the tree digest is unchanged before and after.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use mem_store::config::StoreConfig;
use mem_store::connection::ConnectionManager;
use mem_store::db::{
    ConnectFactory, ConnectOptions, Db, DbHandle, TursoConnectFactory, Value as DbValue, query_one,
};
use mem_store::platform::{Platform, SystemProcessProbe};
use mem_store::scope::Scope;
use mem_store::serde_json::{self, Value, json};
use mem_store::shard_manager::ShardManager;

/// The plan format this build accepts. A future format is a refusal, never a
/// best-effort read.
pub const PLAN_FORMAT_VERSION: u32 = 1;

/// The report format this build emits.
pub const REPORT_FORMAT_VERSION: u32 = 1;

/// Exit code: usage/plan validation (existing family, rev-11 §E.4).
pub const EXIT_PLAN_INVALID: i32 = 64;
/// Exit code: unsupported schema / encoding / tier.
pub const EXIT_UNSUPPORTED_SHAPE: i32 = 65;
/// Exit code: divergent same-ID payload abort.
pub const EXIT_DIVERGENT_ROWS: i32 = 71;
/// Exit code: identity/encoding refusal (collision, canonical mismatch).
pub const EXIT_IDENTITY_REFUSAL: i32 = 72;
/// Exit code: embedding incompatibility.
pub const EXIT_EMBEDDING_REFUSAL: i32 = 73;
/// Exit code: store-or-IO failure outside a single refusal (existing family).
pub const EXIT_STORE_OR_IO: i32 = 74;
/// Exit code: custody/snapshot/manifest verification failure.
pub const EXIT_CUSTODY_REFUSAL: i32 = 77;

/// The pre-consolidation host prefix and its replacement, applied at PATH
/// COMPONENT boundaries only (rule B.4 — `/home/vaz2` is untouched).
const OLD_HOST_ROOT: &str = "/home/vaz";
const NEW_HOST_ROOT: &str = "/home/opencode";

/// `memories` v1 columns, in DDL order (`crates/mem-store/src/sql.rs`).
const MEMORIES_V1: [&str; 17] = [
    "id",
    "content",
    "vector",
    "tags_vector",
    "container_tag",
    "tags",
    "type",
    "created_at",
    "updated_at",
    "metadata",
    "display_name",
    "user_name",
    "user_email",
    "project_path",
    "project_name",
    "git_repo_url",
    "is_pinned",
];

/// The pinned TagInfo values every association column normalizes to (§B.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetAssociation {
    /// The target `container_tag`.
    pub container_tag: String,
    /// The target `project_path`.
    pub project_path: String,
    /// The target `project_name`.
    pub project_name: Option<String>,
    /// The target `display_name`.
    pub display_name: Option<String>,
    /// The target `git_repo_url`.
    pub git_repo_url: Option<String>,
}

/// One plan row: a source shard and the identity it consolidates onto
/// (parent Step 4 field names, verbatim).
#[derive(Debug, Clone)]
pub struct IdentityMap {
    /// The source shard's identity hash (`project_<hash>_shard_<n>.db`).
    pub source_hash: String,
    /// The source's canonical identity string (`.opencode-mem-project` /
    /// `git-common:` / `remote:` / `path:`).
    pub source_identity: String,
    /// The source shard file.
    pub source_path: PathBuf,
    /// The target identity hash.
    pub target_hash: String,
    /// The target's canonical identity string.
    pub target_identity: String,
    /// The target shard file path.
    pub target_path: PathBuf,
    /// The pinned association values this source normalizes to.
    pub target_association: TargetAssociation,
    /// The explicit opt-in a linked source needs (`hard link` / `symlink`).
    pub allow_linked_source: bool,
}

/// The private plan document (§B.2): hashes are pinned, row content never is.
#[derive(Debug, Clone)]
pub struct ConsolidationPlan {
    /// [`PLAN_FORMAT_VERSION`].
    pub format_version: u32,
    /// The storage root the plan describes.
    pub storage_root: PathBuf,
    /// The custody snapshot the plan was derived from.
    pub source_snapshot_sha256: String,
    /// Every mapping row.
    pub mapping: Vec<IdentityMap>,
    /// The schema manifest digest.
    pub schema_manifest_sha256: String,
    /// The baseline manifest digest.
    pub baseline_manifest_sha256: String,
}

/// A typed refusal: an exit code plus a distinct event name.
#[derive(Debug, Clone)]
pub struct Refusal {
    /// The exit code the process answers.
    pub code: i32,
    /// The structured event name (`consolidation.*`).
    pub event: &'static str,
    /// A human-readable detail (never row content).
    pub message: String,
}

impl Refusal {
    fn new(code: i32, event: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            event,
            message: message.into(),
        }
    }
}

/// The census shape of one source shard (§A ladder acceptance).
#[derive(Debug, Clone)]
struct Census {
    user_version: i64,
    columns: Vec<String>,
}

/// What the dry-run learned; serialized as the versioned `PlanReport`.
#[derive(Debug, Clone)]
struct Preflight {
    plan_sha256: String,
    sources: Vec<PreflightSource>,
    /// §I.3: shard files the plan does not claim but the registry references —
    /// carried into the staged tree verbatim and never mutated.
    carried: Vec<String>,
    aux: Vec<AuxRewrite>,
}

#[derive(Debug, Clone)]
struct PreflightSource {
    source_hash: String,
    target_hash: String,
    target_identity: String,
    schema_version: i64,
    columns: BTreeSet<String>,
}

/// Entry point for `--mode=consolidate-shards --plan <json> [--custody <abs-dir>] [--dry-run]`.
///
/// Returns the process exit code; a refusal has already been emitted on
/// stderr by the time this returns.
///
/// Apply is a CUSTODY-REQUIRED mode (§C.5 rev 13): the auxiliary sidecar is a
/// custody artifact and `apply_plan` cannot derive the custody dir
/// (`staged_path` puts the staged tree beside the parent). Every dry run stays
/// custody-FREE, so the measured zero-write property is untouched.
pub fn run_plan(
    plan_path: &Path,
    storage_root: &Path,
    config: &mem_core::config::Config,
    custody: Option<&Path>,
    dry_run: bool,
) -> i32 {
    let outcome = if dry_run {
        plan_and_preflight(plan_path, storage_root, &config.container_tag_prefix)
    } else {
        match custody {
            Some(path) if path.is_absolute() => apply_plan(plan_path, storage_root, config, path),
            _ => Err(Refusal::new(
                EXIT_PLAN_INVALID,
                "consolidation.custody_required",
                "apply requires --custody <absolute-dir>",
            )),
        }
    };
    match outcome {
        Ok(report) => {
            println!("{report}");
            0
        }
        Err(refusal) => emit_refusal(&refusal),
    }
}

/// Emits the typed refusal line and exits with its code.
pub fn emit_refusal(refusal: &Refusal) -> ! {
    mem_core::diagnostics::structured_stderr(
        refusal.event,
        "error",
        "consolidation",
        json!({ "message": refusal.message, "code": refusal.code }),
    );
    std::process::exit(refusal.code);
}

fn plan_and_preflight(
    plan_path: &Path,
    storage_root: &Path,
    prefix: &str,
) -> Result<String, Refusal> {
    let plan = load_plan(plan_path)?;
    let preflight = preflight(&plan, storage_root, prefix)?;
    Ok(render_report(&preflight, true))
}

// ---------------------------------------------------------------------------
// Plan loading
// ---------------------------------------------------------------------------

fn load_plan(path: &Path) -> Result<ConsolidationPlan, Refusal> {
    let text = std::fs::read_to_string(path).map_err(|error| {
        Refusal::new(
            EXIT_PLAN_INVALID,
            "consolidation.plan_unreadable",
            format!("the plan cannot be read: {error}"),
        )
    })?;
    let value: Value = serde_json::from_str(&text).map_err(|error| {
        Refusal::new(
            EXIT_PLAN_INVALID,
            "consolidation.plan_unparsable",
            format!("the plan is not valid JSON: {error}"),
        )
    })?;
    plan_from_json(&value)
}

fn plan_from_json(value: &Value) -> Result<ConsolidationPlan, Refusal> {
    let object = value.as_object().ok_or_else(|| {
        Refusal::new(
            EXIT_PLAN_INVALID,
            "consolidation.plan_unparsable",
            "the plan is not a JSON object",
        )
    })?;
    let format_version = object
        .get("format_version")
        .and_then(Value::as_u64)
        .ok_or_else(|| plan_invalid("format_version is missing"))? as u32;
    if format_version != PLAN_FORMAT_VERSION {
        return Err(Refusal::new(
            EXIT_UNSUPPORTED_SHAPE,
            "consolidation.plan_format_unsupported",
            format!("plan format_version {format_version} is not {PLAN_FORMAT_VERSION}"),
        ));
    }
    let mapping = object
        .get("mapping")
        .and_then(Value::as_array)
        .ok_or_else(|| plan_invalid("mapping is missing"))?
        .iter()
        .map(identity_map_from_json)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(ConsolidationPlan {
        format_version,
        storage_root: PathBuf::from(required_string(object, "storage_root")?),
        source_snapshot_sha256: required_string(object, "source_snapshot_sha256")?,
        mapping,
        schema_manifest_sha256: required_string(object, "schema_manifest_sha256")?,
        baseline_manifest_sha256: required_string(object, "baseline_manifest_sha256")?,
    })
}

fn identity_map_from_json(value: &Value) -> Result<IdentityMap, Refusal> {
    let object = value
        .as_object()
        .ok_or_else(|| plan_invalid("a mapping row is not an object"))?;
    let association = object
        .get("target_association")
        .and_then(Value::as_object)
        .ok_or_else(|| plan_invalid("target_association is missing"))?;
    Ok(IdentityMap {
        source_hash: required_string(object, "source_hash")?,
        source_identity: required_string(object, "source_identity")?,
        source_path: PathBuf::from(required_string(object, "source_path")?),
        target_hash: required_string(object, "target_hash")?,
        target_identity: required_string(object, "target_identity")?,
        target_path: PathBuf::from(required_string(object, "target_path")?),
        target_association: TargetAssociation {
            container_tag: required_string(association, "container_tag")?,
            project_path: required_string(association, "project_path")?,
            project_name: optional_string(association, "project_name"),
            display_name: optional_string(association, "display_name"),
            git_repo_url: optional_string(association, "git_repo_url"),
        },
        allow_linked_source: object
            .get("allow_linked_source")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn plan_invalid(message: &str) -> Refusal {
    Refusal::new(EXIT_PLAN_INVALID, "consolidation.plan_invalid", message)
}

fn required_string(object: &serde_json::Map<String, Value>, key: &str) -> Result<String, Refusal> {
    object
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| plan_invalid(&format!("{key} is missing or not a string")))
}

fn optional_string(object: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    object.get(key).and_then(Value::as_str).map(str::to_string)
}

// ---------------------------------------------------------------------------
// Preflight (§B.2, §B.3, §B.4, §A ladder, §I.3)
// ---------------------------------------------------------------------------

fn preflight(
    plan: &ConsolidationPlan,
    storage_root: &Path,
    prefix: &str,
) -> Result<Preflight, Refusal> {
    let planned_root = canonical(storage_root);
    let plan_root = canonical(&plan.storage_root);
    if planned_root != plan_root {
        return Err(Refusal::new(
            EXIT_PLAN_INVALID,
            "consolidation.plan_root_mismatch",
            format!(
                "the plan describes {} but the command targets {}",
                plan.storage_root.display(),
                storage_root.display()
            ),
        ));
    }
    if plan.mapping.is_empty() {
        return Err(plan_invalid("the plan maps no sources"));
    }
    if plan.source_snapshot_sha256.is_empty()
        || plan.schema_manifest_sha256.is_empty()
        || plan.baseline_manifest_sha256.is_empty()
    {
        return Err(plan_invalid("the plan is missing a pinned digest"));
    }

    let disk = disk_sources(storage_root)?;
    let planned: BTreeSet<&str> = plan
        .mapping
        .iter()
        .map(|row| row.source_hash.as_str())
        .collect();
    if planned.len() != plan.mapping.len() {
        return Err(Refusal::new(
            EXIT_IDENTITY_REFUSAL,
            "consolidation.source_set_duplicate",
            "one source hash appears twice in the mapping",
        ));
    }
    let on_disk: BTreeSet<&str> = disk.keys().map(String::as_str).collect();
    // §I.3 three-way reconciliation of registry rows against files. A shard is
    // CLAIMED when the plan names it — as a source or as a target. A shard the
    // plan does not claim but the registry references is UNRELATED: it is
    // carried into the staged tree verbatim and never mutated (§C.6). A shard
    // neither claimed nor registered is an ORPHAN and refuses 65.
    let mut claimed = planned.clone();
    claimed.extend(plan.mapping.iter().map(|row| row.target_hash.as_str()));
    // F-1 (opsec-2): the plan schema names ONE shard file per identity
    // (`IdentityMap.source_path`), and the merge reads exactly that file while
    // the staged tree drops every index of a planned hash. A claimed hash with
    // more than one shard file would therefore lose the other indices silently
    // (source side) or publish them unregistered (target side), so it refuses
    // 65 here — before the orphan check, the census, and any mutation.
    // Multi-shard merge support is a plan-schema change (rev-12), not this wave.
    for hash in &claimed {
        if let Some(files) = disk.get(*hash)
            && files.len() != 1
        {
            return Err(Refusal::new(
                EXIT_UNSUPPORTED_SHAPE,
                "consolidation.multi_shard_claimed",
                format!(
                    "claimed hash {hash} has {} shard files; multi-shard sources and targets are not supported",
                    files.len()
                ),
            ));
        }
    }
    let registered = registry_project_hashes(storage_root)?;
    let mut carried = Vec::new();
    for hash in on_disk.difference(&claimed) {
        if registered.contains(*hash) {
            carried.push((*hash).to_string());
        } else {
            return Err(Refusal::new(
                EXIT_UNSUPPORTED_SHAPE,
                "consolidation.orphan_shard_file",
                format!("shard file for {hash} is on disk, unplanned and unregistered"),
            ));
        }
    }
    // A claimed SOURCE with no file is a plan defect — never a carried shard.
    let absent: Vec<&str> = planned.difference(&on_disk).copied().collect();
    if !absent.is_empty() {
        return Err(Refusal::new(
            EXIT_IDENTITY_REFUSAL,
            "consolidation.source_set_mismatch",
            format!("the plan names sources with no shard file on disk: {absent:?}"),
        ));
    }

    let mut targets: BTreeMap<&str, &str> = BTreeMap::new();
    let mut sources = Vec::with_capacity(plan.mapping.len());
    for row in &plan.mapping {
        let disk_path = disk
            .get(&row.source_hash)
            .and_then(|files| files.first())
            .ok_or_else(|| {
                Refusal::new(
                    EXIT_IDENTITY_REFUSAL,
                    "consolidation.source_set_mismatch",
                    format!("source {} has no shard file", row.source_hash),
                )
            })?;
        if &row.source_path != disk_path {
            return Err(Refusal::new(
                EXIT_IDENTITY_REFUSAL,
                "consolidation.source_path_mismatch",
                format!(
                    "source {} names {} but the shard is {}",
                    row.source_hash,
                    row.source_path.display(),
                    disk_path.display()
                ),
            ));
        }
        if is_linked(disk_path) && !row.allow_linked_source {
            return Err(Refusal::new(
                EXIT_IDENTITY_REFUSAL,
                "consolidation.linked_source_not_allowed",
                format!(
                    "source {} is reached through a link and the plan does not opt in",
                    row.source_hash
                ),
            ));
        }
        if let Some(expected) = constrained_target(&row.source_identity)
            && expected != row.target_identity
        {
            return Err(Refusal::new(
                EXIT_IDENTITY_REFUSAL,
                "consolidation.target_identity_mismatch",
                format!(
                    "source {} maps to {:?} but the pinned transform yields {expected:?}",
                    row.source_hash, row.target_identity
                ),
            ));
        }
        // The collision check runs BEFORE the per-row hash agreement: one hash
        // naming two canonical strings is a plan-level contradiction, and a
        // mutant that trusts supplied hashes must die here, not later.
        if let Some(previous) =
            targets.insert(row.target_hash.as_str(), row.target_identity.as_str())
            && previous != row.target_identity
        {
            return Err(Refusal::new(
                EXIT_IDENTITY_REFUSAL,
                "consolidation.target_hash_collision",
                format!(
                    "target hash {} names two canonical identities ({previous:?}, {:?})",
                    row.target_hash, row.target_identity
                ),
            ));
        }
        let recomputed = identity_hash(&row.target_identity);
        if recomputed != row.target_hash {
            return Err(Refusal::new(
                EXIT_IDENTITY_REFUSAL,
                "consolidation.target_hash_mismatch",
                format!(
                    "target hash {} for {:?} disagrees with the recomputed {recomputed}",
                    row.target_hash, row.target_identity
                ),
            ));
        }
        check_target_path(storage_root, row)?;
        let census = census_shard(disk_path)?;
        sources.push(PreflightSource {
            source_hash: row.source_hash.clone(),
            target_hash: row.target_hash.clone(),
            target_identity: row.target_identity.clone(),
            schema_version: census.user_version,
            columns: census.columns.into_iter().collect(),
        });
    }
    sources.sort_by(|left, right| left.source_hash.cmp(&right.source_hash));
    Ok(Preflight {
        plan_sha256: identity_hash(&render_plan_digest(plan)),
        sources,
        carried,
        aux: auxiliary_closure(plan, storage_root, prefix)?,
    })
}

fn render_plan_digest(plan: &ConsolidationPlan) -> String {
    let mut digest = String::new();
    digest.push_str(&format!("{}\n", plan.format_version));
    digest.push_str(&format!("{}\n", plan.storage_root.display()));
    digest.push_str(&format!("{}\n", plan.source_snapshot_sha256));
    digest.push_str(&format!("{}\n", plan.schema_manifest_sha256));
    digest.push_str(&format!("{}\n", plan.baseline_manifest_sha256));
    for row in &plan.mapping {
        digest.push_str(&format!(
            "{}|{}|{}|{}|{}|{}|{}\n",
            row.source_hash,
            row.source_identity,
            row.source_path.display(),
            row.target_hash,
            row.target_identity,
            row.target_path.display(),
            row.allow_linked_source
        ));
    }
    digest
}

/// The source shards present under `<root>/projects`, keyed by identity hash.
///
/// EVERY shard index of a hash is retained and the per-hash list is sorted, so
/// the preflight's view never depends on `read_dir` order (opsec-2 F-1: a single
/// path per hash made the same plan pass or refuse across runs).
fn disk_sources(storage_root: &Path) -> Result<BTreeMap<String, Vec<PathBuf>>, Refusal> {
    let dir = storage_root.join("projects");
    let entries = std::fs::read_dir(&dir).map_err(|error| {
        Refusal::new(
            EXIT_UNSUPPORTED_SHAPE,
            "consolidation.projects_dir_unreadable",
            format!("{} cannot be listed: {error}", dir.display()),
        )
    })?;
    let mut sources = BTreeMap::new();
    for entry in entries {
        let path = entry
            .map_err(|error| {
                Refusal::new(
                    EXIT_UNSUPPORTED_SHAPE,
                    "consolidation.projects_dir_unreadable",
                    format!("{} cannot be listed: {error}", dir.display()),
                )
            })?
            .path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(rest) = name.strip_prefix("project_") else {
            continue;
        };
        let Some((hash, shard)) = rest.split_once("_shard_") else {
            continue;
        };
        let Some(index) = shard.strip_suffix(".db") else {
            continue;
        };
        if index.is_empty() || !index.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        sources
            .entry(hash.to_string())
            .or_insert_with(Vec::new)
            .push(path.clone());
    }
    for files in sources.values_mut() {
        files.sort();
    }
    Ok(sources)
}

/// A source file reachable only through a link needs the explicit opt-in.
fn is_linked(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
}

/// Rule B.4: plain component-boundary host replacement `/home/vaz` →
/// `/home/opencode`, with NO exceptions (user ruling 2026-11-12). `/home/vaz2`
/// is NOT a `/home/vaz` component.
///
/// `Some` when the source's own canonical identity fixes the target: any
/// pre-migration `/home/vaz`-rooted identity. `None` for a source outside that
/// root (a marker root, an already-migrated path) — those fold onto a target the
/// plan pins, so no derivation applies.
fn constrained_target(source_identity: &str) -> Option<String> {
    let (scheme, path) = source_identity.split_once(':')?;
    if path == OLD_HOST_ROOT {
        return Some(format!("{scheme}:{NEW_HOST_ROOT}"));
    }
    let prefix = format!("{OLD_HOST_ROOT}/");
    path.strip_prefix(&prefix)
        .map(|rest| format!("{scheme}:{NEW_HOST_ROOT}/{rest}"))
}

/// `sha256(canonical identity)[..16]` — the production TagResolver shorthand
/// (`crates/mem-core/src/tags/mod.rs`), recomputed here rather than trusted.
fn identity_hash(identity: &str) -> String {
    mem_core::sha256::hex(&mem_core::sha256::digest(identity.as_bytes()))[..16].to_string()
}

/// The target must be a shard path inside `<root>/projects`, lexically free of
/// traversal, and named for its own hash.
fn check_target_path(storage_root: &Path, row: &IdentityMap) -> Result<(), Refusal> {
    let projects = storage_root.join("projects");
    let lexical = row
        .target_path
        .components()
        .all(|component| !matches!(component, Component::ParentDir | Component::CurDir));
    let inside = if lexical {
        canonical(row.target_path.parent().unwrap_or(Path::new(""))) == canonical(&projects)
            && row.target_path.is_absolute()
    } else {
        // An absolute path with parents is checked below by the lexical rule;
        // a relative one can never be inside the root.
        false
    };
    if !inside {
        return Err(Refusal::new(
            EXIT_IDENTITY_REFUSAL,
            "consolidation.target_path_outside_root",
            format!(
                "target {} is not a shard path directly under {}",
                row.target_path.display(),
                projects.display()
            ),
        ));
    }
    let expected = format!("project_{}_shard_", row.target_hash);
    let named = row
        .target_path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(&expected) && name.ends_with(".db"));
    if !named {
        return Err(Refusal::new(
            EXIT_IDENTITY_REFUSAL,
            "consolidation.target_path_invalid",
            format!(
                "target {} is not named for target hash {}",
                row.target_path.display(),
                row.target_hash
            ),
        ));
    }
    Ok(())
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

// ---------------------------------------------------------------------------
// Census (§A ladder, read-only)
// ---------------------------------------------------------------------------

fn census_shard(path: &Path) -> Result<Census, Refusal> {
    let unsupported = |detail: String| {
        Refusal::new(
            EXIT_UNSUPPORTED_SHAPE,
            "consolidation.shard_shape_unsupported",
            detail,
        )
    };
    let handle = open_read_only(path)?;
    let user_version =
        block_on(async { query_one(handle.as_ref(), "PRAGMA user_version", Vec::new()).await })
            .map_err(|error| {
                Refusal::new(
                    EXIT_UNSUPPORTED_SHAPE,
                    "consolidation.shard_unreadable",
                    format!("{} has no readable user_version: {error}", path.display()),
                )
            })?
            .and_then(|row| row.i64("user_version"))
            .unwrap_or(0);
    let rows =
        block_on(handle.query("PRAGMA table_info(memories)", Vec::new())).map_err(|error| {
            unsupported(format!(
                "{} has no readable memories table: {error}",
                path.display()
            ))
        })?;
    let columns: Vec<String> = rows.iter().filter_map(|row| row.text("name")).collect();
    let tables = block_on(handle.query(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
        Vec::new(),
    ))
    .map_err(|error| {
        unsupported(format!(
            "{} has no readable sqlite_master: {error}",
            path.display()
        ))
    })?;
    let table_names: Vec<String> = tables.iter().filter_map(|row| row.text("name")).collect();
    let actual_tables: BTreeSet<&str> = table_names.iter().map(String::as_str).collect();
    // The accepted shape is keyed on BOTH `user_version == 3` and the presence
    // of the vestigial column; a v2 or v4 shard carrying it still refuses.
    let deleted_at = columns.iter().any(|name| name == "deleted_at");
    let expected = ladder_columns(user_version, deleted_at).ok_or_else(|| {
        unsupported(format!(
            "{} is user_version {user_version}, outside the pinned 1..=4 ladder",
            path.display()
        ))
    })?;
    let actual: BTreeSet<&str> = columns.iter().map(String::as_str).collect();
    if actual != expected {
        let unexpected: Vec<&&str> = actual.difference(&expected).collect();
        let missing: Vec<&&str> = expected.difference(&actual).collect();
        return Err(unsupported(format!(
            "{} has a memories shape outside the ladder (unexpected: {unexpected:?}; missing: {missing:?})",
            path.display()
        )));
    }
    let expected_tables = ladder_tables(user_version);
    if actual_tables != expected_tables {
        let unexpected: Vec<&&str> = actual_tables.difference(&expected_tables).collect();
        let missing: Vec<&&str> = expected_tables.difference(&actual_tables).collect();
        return Err(unsupported(format!(
            "{} has a table set outside the pinned ladder (unexpected: {unexpected:?}; missing: {missing:?})",
            path.display()
        )));
    }
    let accepted = user_version == 3 && deleted_at;
    let expected_indexes = ladder_indexes(user_version, accepted);
    let actual_indexes = census_indexes(&handle, path)?;
    if actual_indexes != expected_indexes {
        let unexpected: Vec<&IndexShape> = actual_indexes.difference(&expected_indexes).collect();
        let missing: Vec<&IndexShape> = expected_indexes.difference(&actual_indexes).collect();
        return Err(unsupported(format!(
            "{} has an index set outside the pinned ladder (unexpected: {unexpected:?}; missing: {missing:?})",
            path.display()
        )));
    }
    // §A.1 seam (i): the all-NULL property is enforced by code, never trusted
    // from the census snapshot. Only the accepted shape can carry the column, so
    // the predicate is conditional by construction.
    if accepted {
        let non_null = block_on(query_one(
            handle.as_ref(),
            "SELECT COUNT(*) AS non_null FROM memories WHERE deleted_at IS NOT NULL",
            Vec::new(),
        ))
        .map_err(|error| {
            unsupported(format!(
                "{} cannot count its non-NULL deleted_at rows: {error}",
                path.display()
            ))
        })?
        .and_then(|row| row.i64("non_null"))
        .ok_or_else(|| {
            unsupported(format!(
                "{} cannot read its non-NULL deleted_at count",
                path.display()
            ))
        })?;
        if non_null != 0 {
            return Err(unsupported(format!(
                "{} holds {non_null} memories rows with a non-NULL deleted_at; the accepted v3+deleted_at shape is all-NULL",
                path.display()
            )));
        }
    }
    Ok(Census {
        user_version,
        columns,
    })
}

/// One explicit index, compared on the pinned 5-tuple: name, table, indexed
/// column list in key order (each with its sort direction) and partial-ness.
/// A name-only comparison would accept a partial or mis-targeted index under a
/// ladder name, and without sort order a DESC↔ASC swap would pass.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct IndexShape {
    name: String,
    table: String,
    columns: Vec<(String, bool)>,
    partial: bool,
}

/// The exact `memories` index set per rung: v1 carries the four
/// `MEMORIES_INDEXES`, v2+ adds `idx_session_id`, and the accepted shape adds
/// the non-partial ascending `idx_deleted_at`.
const LADDER_INDEXES: [(&str, &str, bool); 5] = [
    ("idx_container_tag", "container_tag", false),
    ("idx_type", "type", false),
    ("idx_created_at", "created_at", true),
    ("idx_is_pinned", "is_pinned", false),
    ("idx_session_id", "session_id", false),
];

fn ladder_indexes(version: i64, accepted: bool) -> BTreeSet<IndexShape> {
    let mut expected: BTreeSet<IndexShape> = LADDER_INDEXES
        .iter()
        .take(if version == 1 { 4 } else { 5 })
        .map(|(name, column, descending)| IndexShape {
            name: (*name).to_string(),
            table: "memories".to_string(),
            columns: vec![((*column).to_string(), *descending)],
            partial: false,
        })
        .collect();
    if accepted {
        expected.insert(IndexShape {
            name: "idx_deleted_at".to_string(),
            table: "memories".to_string(),
            columns: vec![("deleted_at".to_string(), false)],
            partial: false,
        });
    }
    expected
}

/// The exact table set per rung (`sqlite_%` internals excluded): v1/v2 carry
/// `shard_metadata`, v3/v4 and the accepted shape add `shard_stats`. An unknown
/// table is a refusal — the merge rebuilds each planned shard from `memories`
/// rows only, so accepting one would drop its rows silently.
fn ladder_tables(version: i64) -> BTreeSet<&'static str> {
    let mut tables: BTreeSet<&'static str> = ["memories", "shard_metadata"].into_iter().collect();
    if version >= 3 {
        tables.insert("shard_stats");
    }
    tables
}

/// Every explicit index in the shard, read from the catalog so an index on ANY
/// table is seen (a `memories`-scoped scan would accept a seventh index on
/// `shard_metadata`). `sqlite_autoindex%` entries are excluded: they are
/// PRIMARY KEY/UNIQUE artifacts, not ladder indexes.
fn census_indexes(handle: &DbHandle, path: &Path) -> Result<BTreeSet<IndexShape>, Refusal> {
    let unsupported = |detail: String| {
        Refusal::new(
            EXIT_UNSUPPORTED_SHAPE,
            "consolidation.shard_shape_unsupported",
            detail,
        )
    };
    let listed = block_on(handle.query(
        "SELECT name, tbl_name FROM sqlite_master \
         WHERE type = 'index' AND name NOT LIKE 'sqlite_autoindex%'",
        Vec::new(),
    ))
    .map_err(|error| {
        unsupported(format!(
            "{} has no readable index catalog: {error}",
            path.display()
        ))
    })?;
    let mut partials: BTreeMap<String, BTreeMap<String, bool>> = BTreeMap::new();
    let mut shapes = BTreeSet::new();
    for row in &listed {
        let (Some(name), Some(table)) = (row.text("name"), row.text("tbl_name")) else {
            return Err(unsupported(format!(
                "{} lists an index without a name or table",
                path.display()
            )));
        };
        if !partials.contains_key(&table) {
            let rows = block_on(handle.query(
                &format!("PRAGMA index_list('{}')", sql_string(&table)),
                Vec::new(),
            ))
            .map_err(|error| {
                unsupported(format!(
                    "{} cannot list the indexes of {table}: {error}",
                    path.display()
                ))
            })?;
            let map: BTreeMap<String, bool> = rows
                .iter()
                .filter_map(|row| Some((row.text("name")?, row.i64("partial")? != 0)))
                .collect();
            partials.insert(table.clone(), map);
        }
        let partial = partials
            .get(&table)
            .and_then(|by_name| by_name.get(&name))
            .copied();
        let Some(partial) = partial else {
            return Err(unsupported(format!(
                "{} lists index {name} but PRAGMA index_list({table}) does not",
                path.display()
            )));
        };
        let xinfo = block_on(handle.query(
            &format!("PRAGMA index_xinfo('{}')", sql_string(&name)),
            Vec::new(),
        ))
        .map_err(|error| {
            unsupported(format!(
                "{} cannot read the columns of index {name}: {error}",
                path.display()
            ))
        })?;
        let mut keyed: Vec<(i64, String, bool)> = xinfo
            .iter()
            .filter(|row| row.i64("key") == Some(1))
            .filter_map(|row| Some((row.i64("seqno")?, row.text("name")?, row.i64("desc")? != 0)))
            .collect();
        keyed.sort_by_key(|(seqno, _, _)| *seqno);
        let columns = keyed
            .into_iter()
            .map(|(_, column, descending)| (column, descending))
            .collect();
        shapes.insert(IndexShape {
            name,
            table,
            columns,
            partial,
        });
    }
    Ok(shapes)
}

/// SQL string-literal quoting for the pragma arguments (names come from the
/// shard's own catalog).
fn sql_string(value: &str) -> String {
    value.replace('\'', "''")
}

/// The exact `memories` column set per ladder version: v1 17, v2 18
/// (`session_id`), v3 18, v4 19 (`source_prompt_created_at`). The accepted
/// "v3 + vestigial `deleted_at`" shape is its OWN arm — folding it into `2 | 3`
/// would accept it on a v2 shard.
fn ladder_columns(version: i64, deleted_at: bool) -> Option<BTreeSet<&'static str>> {
    let mut columns: BTreeSet<&'static str> = MEMORIES_V1.into_iter().collect();
    match version {
        1 => Some(columns),
        2 => {
            columns.insert("session_id");
            Some(columns)
        }
        3 if deleted_at => {
            columns.insert("session_id");
            columns.insert("deleted_at");
            Some(columns)
        }
        3 | 4 => {
            columns.insert("session_id");
            if version == 4 {
                columns.insert("source_prompt_created_at");
            }
            Some(columns)
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Apply — typed lossless merge into a staged tree (Step 4; install is Step 7)
// ---------------------------------------------------------------------------

/// The staged tree name prefix under the storage root's PARENT (rev-11 §E.1).
pub const STAGED_DIR_PREFIX: &str = "consolidation-staged-";

/// Root-level paths never carried into a staged tree: the M1 members (§C.1b)
/// plus `.wal-mode`, which the first writable open regenerates.
const STAGED_ROOT_DROPS: [&str; 3] = ["status", "engine.lock", ".wal-mode"];

/// The `memories` v4 column set in DDL order — the columns the merge reads and
/// writes. A source shard on a lower rung reads its absent columns as NULL.
const MEMORIES_V4: [&str; 19] = [
    "id",
    "content",
    "vector",
    "tags_vector",
    "container_tag",
    "tags",
    "type",
    "created_at",
    "updated_at",
    "metadata",
    "display_name",
    "user_name",
    "user_email",
    "project_path",
    "project_name",
    "git_repo_url",
    "is_pinned",
    "session_id",
    "source_prompt_created_at",
];

const INSERT_MERGED: &str = r#"
  INSERT INTO memories (
    id, content, vector, tags_vector, container_tag, tags, type, created_at, updated_at,
    metadata, display_name, user_name, user_email, project_path, project_name, git_repo_url,
    is_pinned, session_id, source_prompt_created_at
  ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
"#;

/// One surviving row: every typed cell (raw blobs included) plus how many
/// source rows collapsed into it (§B.7 occurrence provenance).
#[derive(Debug, Clone)]
struct MergedRow {
    id: String,
    cells: BTreeMap<String, DbValue>,
    occurrences: u32,
}

/// One destination group's merge result.
#[derive(Debug, Clone)]
struct MergeGroup {
    target_hash: String,
    target_identity: String,
    survivors: Vec<MergedRow>,
}

impl MergeGroup {
    fn occurrences(&self) -> usize {
        self.survivors
            .iter()
            .map(|row| row.occurrences as usize)
            .sum()
    }
}

/// The shard split a group was written as: `(index, row count)`.
#[derive(Debug, Clone)]
struct GroupShards {
    shards: Vec<(u32, usize)>,
}

/// This build's embedding identity, resolved once (rule D.1).
struct EmbeddingExpectation {
    dimensions: u32,
    model: String,
    fingerprint: Option<String>,
}

fn apply_plan(
    plan_path: &Path,
    storage_root: &Path,
    config: &mem_core::config::Config,
    custody: &Path,
) -> Result<String, Refusal> {
    if config.max_vectors_per_shard == 0 {
        return Err(Refusal::new(
            EXIT_PLAN_INVALID,
            "consolidation.capacity_unusable",
            "max_vectors_per_shard is 0; no survivor can be placed",
        ));
    }
    let plan = load_plan(plan_path)?;
    let preflight = preflight(&plan, storage_root, &config.container_tag_prefix)?;
    // §C.5 rev 13: the audit sidecar is written immediately after `preflight`
    // returns and BEFORE `merge_groups`/`build_staged_tree`/`write_auxiliary`,
    // so no mutation precedes the record. A re-apply over an installed window
    // is refused first: the sidecar describes the PRE-install read of S, and
    // rewriting it from the installed tree would destroy the audit trail.
    refuse_installed_window(custody)?;
    write_aux_sidecar(custody, &aux_sidecar_bytes(&preflight.aux))?;
    let mut store_config = mem_store::config::from_core(config);
    store_config.storage_path = storage_root.to_path_buf();
    let expected = expected_embedding(config)?;

    // The whole merge is decided in memory first: any refusal leaves S
    // untouched AND builds no staged tree (all-or-nothing, §B.7).
    let groups = merge_groups(&plan, &expected)?;

    let staged = staged_path(storage_root, &preflight.plan_sha256);
    build_staged_tree(storage_root, &staged, &plan)?;
    let shards = write_groups(&staged, &store_config, config, &plan, &groups)?;
    write_auxiliary(&staged, &store_config, &preflight.aux)?;
    // The staged root's `.wal-mode` is regenerated by the first writable open
    // of the installed tree, never carried (§C.7).
    let _ = std::fs::remove_file(staged.join(".wal-mode"));
    Ok(render_apply_report(&preflight, &staged, &groups, &shards))
}

fn expected_embedding(config: &mem_core::config::Config) -> Result<EmbeddingExpectation, Refusal> {
    let identity = mem_core::embedding::EmbeddingIdentity::from_config(config);
    let fingerprint = identity.fingerprint().map_err(|error| {
        Refusal::new(
            EXIT_EMBEDDING_REFUSAL,
            "consolidation.embedding_mismatch",
            format!("this build cannot express its embedding identity: {error}"),
        )
    })?;
    Ok(EmbeddingExpectation {
        dimensions: identity.dimensions,
        model: identity.model,
        fingerprint: Some(fingerprint),
    })
}

fn merge_groups(
    plan: &ConsolidationPlan,
    expected: &EmbeddingExpectation,
) -> Result<Vec<MergeGroup>, Refusal> {
    let mut grouped: BTreeMap<&str, Vec<&IdentityMap>> = BTreeMap::new();
    for row in &plan.mapping {
        grouped
            .entry(row.target_hash.as_str())
            .or_default()
            .push(row);
    }
    let mut groups = Vec::with_capacity(grouped.len());
    for (target_hash, rows) in grouped {
        let association = &rows[0].target_association;
        for row in &rows {
            if &row.target_association != association {
                return Err(Refusal::new(
                    EXIT_IDENTITY_REFUSAL,
                    "consolidation.target_association_conflict",
                    format!(
                        "sources folding onto {target_hash} disagree on the pinned association"
                    ),
                ));
            }
        }
        let mut survivors: BTreeMap<String, MergedRow> = BTreeMap::new();
        for row in &rows {
            let source = read_source(&row.source_path)?;
            if !source.rows.is_empty() {
                check_embedding(&row.source_path, &source.identity, expected)?;
            }
            for mut cells in source.rows {
                normalize_association(&mut cells, association);
                let id = match cells.get("id") {
                    Some(DbValue::Text(id)) => id.clone(),
                    _ => {
                        return Err(Refusal::new(
                            EXIT_UNSUPPORTED_SHAPE,
                            "consolidation.shard_shape_unsupported",
                            format!(
                                "{} holds a row without a text id",
                                row.source_path.display()
                            ),
                        ));
                    }
                };
                match survivors.get_mut(&id) {
                    None => {
                        survivors.insert(
                            id.clone(),
                            MergedRow {
                                id,
                                cells,
                                occurrences: 1,
                            },
                        );
                    }
                    Some(existing) if existing.cells == cells => existing.occurrences += 1,
                    Some(existing) => {
                        let differing: Vec<&str> = MEMORIES_V4
                            .iter()
                            .copied()
                            .filter(|name| existing.cells.get(*name) != cells.get(*name))
                            .collect();
                        return Err(Refusal::new(
                            EXIT_DIVERGENT_ROWS,
                            "consolidation.rows_divergent",
                            format!(
                                "id {id} diverges across the sources of {target_hash} in {differing:?}; the whole plan aborts"
                            ),
                        ));
                    }
                }
            }
        }
        groups.push(MergeGroup {
            target_hash: target_hash.to_string(),
            target_identity: rows[0].target_identity.clone(),
            survivors: survivors.into_values().collect(),
        });
    }
    Ok(groups)
}

struct SourceRows {
    rows: Vec<BTreeMap<String, DbValue>>,
    identity: BTreeMap<String, String>,
}

fn read_source(path: &Path) -> Result<SourceRows, Refusal> {
    let handle = open_read_only(path)?;
    // §A.1 seam (ii): `SELECT *` is retained on purpose — an explicit column
    // list built from `ladder_columns(3)` would make `row.get("deleted_at")` a
    // silent no-op. The row-level refusal runs on the FETCHED row and BEFORE
    // the `MEMORIES_V4` projection: after the projection the column is absent
    // and a non-NULL row would be silently dropped (fail-open resurrection).
    // The predicate is conditional on the column existing — the ladder shards
    // carry no `deleted_at`.
    let rows = block_on(handle.query("SELECT * FROM memories", Vec::new())).map_err(|error| {
        Refusal::new(
            EXIT_UNSUPPORTED_SHAPE,
            "consolidation.shard_shape_unsupported",
            format!("{} cannot be read: {error}", path.display()),
        )
    })?;
    let mut typed: Vec<BTreeMap<String, DbValue>> = Vec::with_capacity(rows.len());
    for row in &rows {
        if let Some(value) = row.get("deleted_at")
            && !value.is_null()
        {
            return Err(Refusal::new(
                EXIT_UNSUPPORTED_SHAPE,
                "consolidation.shard_shape_unsupported",
                format!(
                    "{} holds a memories row with a non-NULL deleted_at; the accepted v3+deleted_at shape is all-NULL",
                    path.display()
                ),
            ));
        }
        typed.push(
            MEMORIES_V4
                .iter()
                .map(|name| {
                    (
                        (*name).to_string(),
                        row.get(name).cloned().unwrap_or(DbValue::Null),
                    )
                })
                .collect(),
        );
    }
    let metadata = block_on(handle.query("SELECT key, value FROM shard_metadata", Vec::new()))
        .map_err(|error| {
            Refusal::new(
                EXIT_UNSUPPORTED_SHAPE,
                "consolidation.shard_shape_unsupported",
                format!("{} has no readable shard_metadata: {error}", path.display()),
            )
        })?;
    let identity = metadata
        .iter()
        .filter_map(|row| Some((row.text("key")?, row.text("value")?)))
        .collect();
    Ok(SourceRows {
        rows: typed,
        identity,
    })
}

/// Rule D.1: a populated shard must carry this build's dimensions and model,
/// and any stored fingerprint must equal this build's.
fn check_embedding(
    path: &Path,
    identity: &BTreeMap<String, String>,
    expected: &EmbeddingExpectation,
) -> Result<(), Refusal> {
    let mismatch = |detail: String| {
        Refusal::new(
            EXIT_EMBEDDING_REFUSAL,
            "consolidation.embedding_mismatch",
            format!("{} cannot merge: {detail}", path.display()),
        )
    };
    match identity.get("embedding_dimensions") {
        Some(value) => {
            let dimensions: u32 = value
                .parse()
                .map_err(|_| mismatch(format!("embedding_dimensions {value:?} is not a size")))?;
            if dimensions != expected.dimensions {
                return Err(mismatch(format!(
                    "embedding_dimensions {dimensions} is not this build's {}",
                    expected.dimensions
                )));
            }
        }
        None => {
            return Err(mismatch(
                "a populated shard has no embedding_dimensions identity".to_string(),
            ));
        }
    }
    match identity.get("embedding_model") {
        Some(model) if model == &expected.model => {}
        Some(model) => {
            return Err(mismatch(format!(
                "embedding_model {model:?} is not this build's {:?}",
                expected.model
            )));
        }
        None => {
            return Err(mismatch(
                "a populated shard has no embedding_model identity".to_string(),
            ));
        }
    }
    if let Some(fingerprint) = identity.get("embedding_fingerprint")
        && Some(fingerprint.as_str()) != expected.fingerprint.as_deref()
    {
        return Err(mismatch(
            "the stored embedding fingerprint is not this build's".to_string(),
        ));
    }
    Ok(())
}

/// §B.7: the five association columns are the only content transform.
fn normalize_association(cells: &mut BTreeMap<String, DbValue>, association: &TargetAssociation) {
    cells.insert(
        "container_tag".to_string(),
        DbValue::Text(association.container_tag.clone()),
    );
    cells.insert(
        "project_path".to_string(),
        DbValue::Text(association.project_path.clone()),
    );
    for (name, value) in [
        ("project_name", &association.project_name),
        ("display_name", &association.display_name),
        ("git_repo_url", &association.git_repo_url),
    ] {
        cells.insert(
            name.to_string(),
            value.clone().map(DbValue::Text).unwrap_or(DbValue::Null),
        );
    }
}

/// The staged tree path: `<parent>/consolidation-staged-<nonce>` (§E.1).
fn staged_path(storage_root: &Path, plan_sha256: &str) -> PathBuf {
    let parent = storage_root.parent().unwrap_or(Path::new("."));
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let seed = format!("{plan_sha256}|{}|{nanos}", std::process::id());
    let nonce = mem_core::sha256::hex(&mem_core::sha256::digest(seed.as_bytes()))[..16].to_string();
    parent.join(format!("{STAGED_DIR_PREFIX}{nonce}"))
}

/// Copies S into the staged tree, dropping the M1 root paths, the regenerated
/// `.wal-mode`, and every shard file the plan consolidates away.
fn build_staged_tree(
    storage_root: &Path,
    staged: &Path,
    plan: &ConsolidationPlan,
) -> Result<(), Refusal> {
    let io =
        |detail: String| Refusal::new(EXIT_STORE_OR_IO, "consolidation.staged_tree_failed", detail);
    std::fs::create_dir(staged)
        .map_err(|error| io(format!("cannot create {}: {error}", staged.display())))?;
    let sources: BTreeSet<&str> = plan
        .mapping
        .iter()
        .map(|row| row.source_hash.as_str())
        .collect();
    copy_tree(storage_root, staged, &sources, true)
        .map_err(|error| io(format!("cannot copy {}: {error}", storage_root.display())))
}

fn copy_tree(
    from: &Path,
    to: &Path,
    sources: &BTreeSet<&str>,
    at_root: bool,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if at_root && STAGED_ROOT_DROPS.contains(&name.as_str()) {
            continue;
        }
        if planned_source_file(&name, sources) {
            continue;
        }
        let source = entry.path();
        let target = to.join(&name);
        if entry.file_type()?.is_dir() {
            std::fs::create_dir(&target)?;
            copy_tree(&source, &target, sources, false)?;
        } else {
            std::fs::copy(&source, &target)?;
        }
    }
    Ok(())
}

fn planned_source_file(name: &str, sources: &BTreeSet<&str>) -> bool {
    let Some(rest) = name.strip_prefix("project_") else {
        return false;
    };
    let Some(hash) = rest.split("_shard_").next() else {
        return false;
    };
    sources.contains(hash)
}

/// Writes every destination group into the staged tree: fresh v4 shards via
/// the production manager, registry rows replaced directly (§C.6).
fn write_groups(
    staged: &Path,
    store_config: &StoreConfig,
    config: &mem_core::config::Config,
    plan: &ConsolidationPlan,
    groups: &[MergeGroup],
) -> Result<Vec<GroupShards>, Refusal> {
    let registry = block_on(TursoConnectFactory.connect(
        staged.join("metadata.db"),
        ConnectOptions::for_config(store_config, Platform::current()),
    ))
    .map_err(|error| {
        Refusal::new(
            EXIT_STORE_OR_IO,
            "consolidation.staged_registry_failed",
            format!("the staged registry cannot be opened: {error}"),
        )
    })?;
    // Retired source rows are REPLACED, never retained (§C.6): a registry row
    // pointing at a file this plan consolidated away would re-arm reaping.
    let retired: BTreeSet<&str> = plan
        .mapping
        .iter()
        .map(|row| row.source_hash.as_str())
        .chain(groups.iter().map(|group| group.target_hash.as_str()))
        .collect();
    for hash in retired {
        block_on(registry.execute(
            "DELETE FROM shards WHERE scope = 'project' AND scope_hash = ?",
            vec![DbValue::Text(hash.to_string())],
        ))
        .map_err(|error| {
            Refusal::new(
                EXIT_STORE_OR_IO,
                "consolidation.staged_registry_failed",
                format!("the staged registry cannot drop {hash}: {error}"),
            )
        })?;
    }
    drop(registry);

    let connections = Arc::new(ConnectionManager::new(
        staged.to_path_buf(),
        store_config.clone(),
        Platform::current(),
        Arc::new(TursoConnectFactory),
    ));
    let manager = ShardManager::new(
        staged.to_path_buf(),
        store_config.clone(),
        connections,
        Arc::new(SystemProcessProbe),
        Some(mem_core::embedding::EmbeddingIdentity::from_config(config)),
    );
    let capacity = store_config.max_vectors_per_shard as usize;
    let mut written = Vec::with_capacity(groups.len());
    for group in groups {
        let mut shards = Vec::new();
        let mut previous: Option<mem_store::shard_manager::ShardInfo> = None;
        for (index, chunk) in group.survivors.chunks(capacity).enumerate() {
            let index = index as u32;
            let shard = match &previous {
                None => block_on(manager.create_shard(Scope::Project, &group.target_hash, index)),
                Some(retire) => block_on(manager.rotate_shard(
                    Scope::Project,
                    &group.target_hash,
                    retire.id,
                    index,
                )),
            }
            .map_err(|error| {
                Refusal::new(
                    EXIT_STORE_OR_IO,
                    "consolidation.staged_shard_failed",
                    format!(
                        "cannot build shard {index} for {}: {error}",
                        group.target_hash
                    ),
                )
            })?;
            let db = block_on(manager.open_shard(&shard.db_path)).map_err(|error| {
                Refusal::new(
                    EXIT_STORE_OR_IO,
                    "consolidation.staged_shard_failed",
                    format!("cannot open {}: {error}", shard.db_path.display()),
                )
            })?;
            for row in chunk {
                insert_row(db.as_ref(), row)?;
            }
            drop(db);
            block_on(manager.set_vector_count(shard.id, chunk.len() as u64)).map_err(|error| {
                Refusal::new(
                    EXIT_STORE_OR_IO,
                    "consolidation.staged_registry_failed",
                    format!(
                        "cannot record {} rows for shard {index}: {error}",
                        chunk.len()
                    ),
                )
            })?;
            shards.push((index, chunk.len()));
            previous = Some(shard);
        }
        written.push(GroupShards { shards });
    }
    Ok(written)
}

fn insert_row(db: &dyn Db, row: &MergedRow) -> Result<(), Refusal> {
    let params: Vec<DbValue> = MEMORIES_V4
        .iter()
        .map(|name| row.cells.get(*name).cloned().unwrap_or(DbValue::Null))
        .collect();
    block_on(db.execute(INSERT_MERGED, params))
        .map(|_| ())
        .map_err(|error| {
            Refusal::new(
                EXIT_STORE_OR_IO,
                "consolidation.staged_shard_failed",
                format!("row {} cannot be written: {error}", row.id),
            )
        })
}

fn render_apply_report(
    preflight: &Preflight,
    staged: &Path,
    groups: &[MergeGroup],
    shards: &[GroupShards],
) -> String {
    let groups: Vec<Value> = groups
        .iter()
        .zip(shards)
        .map(|(group, split)| {
            json!({
                "target_hash": group.target_hash,
                "target_identity": group.target_identity,
                "survivors": group.survivors.len(),
                "occurrences": group.occurrences(),
                "shards": split
                    .shards
                    .iter()
                    .map(|(index, rows)| json!({ "index": index, "rows": rows }))
                    .collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "format_version": REPORT_FORMAT_VERSION,
        "dry_run": false,
        "plan_sha256": preflight.plan_sha256,
        "staged_root": staged.display().to_string(),
        "source_count": preflight.sources.len(),
        "group_count": groups.len(),
        "groups": groups,
        "auxiliary": render_aux(&preflight.aux),
    })
    .to_string()
}

/// A read-only typed open (rev-11 §F: never a writable open on a source).
fn open_read_only(path: &Path) -> Result<DbHandle, Refusal> {
    let mut options = ConnectOptions::default();
    options.read_only = true;
    block_on(TursoConnectFactory.connect(path.to_path_buf(), options)).map_err(|error| {
        Refusal::new(
            EXIT_UNSUPPORTED_SHAPE,
            "consolidation.shard_unreadable",
            format!("{} cannot be opened read-only: {error}", path.display()),
        )
    })
}

// ---------------------------------------------------------------------------
// Auxiliary closure (§C.2–§C.5; read-only read, staged-tree write)
// ---------------------------------------------------------------------------

/// Tiers whose mere presence refuses (§C.5): never opened, never created.
const REFUSED_TIERS: [(&str, &str); 3] = [
    ("facts.db", "consolidation.facts_tier_present"),
    ("vault", "consolidation.vault_present"),
    ("vault-keys", "consolidation.vault_keys_present"),
];

/// One pinned auxiliary table: the column the closure remaps (if any), the
/// identity key sets a post-remap collision is evaluated on, and the pinned
/// column inventory (a subset is allowed: lower schema rungs are tolerated,
/// extra columns are not).
struct AuxTable {
    table: &'static str,
    remap: Option<&'static str>,
    /// The strict PATH domain (`true`): a value outside the closure is
    /// swap-and-carried. The tag domain (`false`): an unmapped tag passes
    /// unchanged, as it always has.
    strict: bool,
    keys: &'static [&'static [&'static str]],
    columns: &'static [&'static str],
}

const AGENT_SESSION_COLUMNS: [&str; 15] = [
    "id",
    "project_id",
    "session_id",
    "agent_id",
    "parent_session_id",
    "summary",
    "metadata",
    "embedding",
    "created_at",
    "updated_at",
    "deleted_at",
    "orphaned",
    "capture_id",
    "agent_role",
    "not_before",
];

const MILESTONE_COLUMNS: [&str; 10] = [
    "id",
    "project_id",
    "agent_id",
    "summary",
    "contributors",
    "source_count",
    "window_start",
    "window_end",
    "embedding",
    "created_at",
];

const RAW_CONTEXT_COLUMNS: [&str; 6] = [
    "project_id",
    "session_id",
    "agent_id",
    "raw_context",
    "byte_len",
    "captured_at",
];

const SEGMENT_COLUMNS: [&str; 8] = [
    "project_id",
    "session_id",
    "agent_id",
    "segment_index",
    "raw_content",
    "byte_offset",
    "byte_len",
    "created_at",
];

const CAPTURE_INBOX_COLUMNS: [&str; 10] = [
    "capture_id",
    "project_id",
    "session_id",
    "agent_id",
    "payload",
    "created_at",
    "attempts",
    "last_attempt_at",
    "status",
    "not_before",
];

const PROMPT_COLUMNS: [&str; 12] = [
    "id",
    "session_id",
    "message_id",
    "project_path",
    "content",
    "created_at",
    "captured",
    "user_learning_captured",
    "linked_memory_id",
    "capture_attempts",
    "provider_id",
    "model_id",
];

const SKILL_COLUMNS: [&str; 9] = [
    "id",
    "name",
    "description",
    "content",
    "autoinvoke",
    "enabled",
    "container_tag",
    "created_at",
    "updated_at",
];

const EPISODIC_TABLES: [AuxTable; 5] = [
    AuxTable {
        table: "agent_sessions",
        remap: Some("project_id"),
        strict: true,
        keys: &[&["id"], &["project_id", "session_id", "agent_id"]],
        columns: &AGENT_SESSION_COLUMNS,
    },
    AuxTable {
        table: "episodic_milestones",
        remap: Some("project_id"),
        strict: true,
        keys: &[&["id"]],
        columns: &MILESTONE_COLUMNS,
    },
    AuxTable {
        table: "episodic_raw_context",
        remap: Some("project_id"),
        strict: true,
        keys: &[&["project_id", "session_id", "agent_id"]],
        columns: &RAW_CONTEXT_COLUMNS,
    },
    AuxTable {
        table: "episodic_segments",
        remap: Some("project_id"),
        strict: true,
        keys: &[&["project_id", "session_id", "agent_id", "segment_index"]],
        columns: &SEGMENT_COLUMNS,
    },
    AuxTable {
        table: "capture_inbox",
        remap: Some("project_id"),
        strict: true,
        keys: &[&["capture_id"]],
        columns: &CAPTURE_INBOX_COLUMNS,
    },
];

const PROMPT_TABLES: [AuxTable; 1] = [AuxTable {
    table: "user_prompts",
    remap: Some("project_path"),
    strict: true,
    keys: &[&["id"]],
    columns: &PROMPT_COLUMNS,
}];

const SKILL_TABLES: [AuxTable; 1] = [AuxTable {
    table: "skills",
    remap: Some("container_tag"),
    strict: false,
    keys: &[&["id"]],
    columns: &SKILL_COLUMNS,
}];

/// One auxiliary DB, its remapped tables and the tables it may additionally
/// hold (known, carried verbatim, never remapped).
struct AuxDb {
    name: &'static str,
    tables: &'static [AuxTable],
    extra: &'static [&'static str],
}

const AUX_DBS: [AuxDb; 3] = [
    AuxDb {
        name: "episodic.db",
        tables: &EPISODIC_TABLES,
        extra: &["sweep_state"],
    },
    AuxDb {
        name: "user-prompts.db",
        tables: &PROMPT_TABLES,
        extra: &["sweep_state"],
    },
    AuxDb {
        name: "skills.db",
        tables: &SKILL_TABLES,
        extra: &[],
    },
];

/// The remap domain: mapped source host paths and scope tags, plus the set of
/// values that are already targets (they pass untouched).
struct AuxDomain {
    paths: BTreeMap<String, String>,
    tags: BTreeMap<String, String>,
    target_paths: BTreeSet<String>,
}

/// The three-state outcome of one auxiliary cell's decision (§C.5 rev 13).
///
/// Branch (a) is `Mapped` — a MAPPING rewrite, never a carry record; a literal
/// `Some`-means-carry reading would emit the divergent group's own discarded
/// row plus every other mapped rewrite and break the production 16/99 census by
/// construction. Branch (c) is `Carried`, tagged ALWAYS — including a swap that
/// changed nothing — because the ten no-op carries are indistinguishable from
/// branch (b) in the counter. `None` is branch (b) plus the identity-mapping
/// no-op: the cell is unchanged and no record exists.
enum Remap {
    Mapped(DbValue),
    Carried { original: DbValue, carried: DbValue },
}

/// A branch-(c) decision's captured pair: the value the source held and the
/// exact bytes the staged cell receives. Captured AT the decision, never
/// recomputed — `original` dies with `remap_cell`.
#[derive(Debug, Clone)]
struct AuxCarryTag {
    original: DbValue,
    carried: DbValue,
}

/// One `kind: "carry"` record: a distinct `(db, table, column, original)` tuple
/// with at least one STAGED SURVIVOR carrier (§C.5 rev 13, opsec C4/C16).
#[derive(Debug, Clone)]
struct AuxCarry {
    column: &'static str,
    original: DbValue,
    carried: DbValue,
    rows: usize,
}

/// One `kind: "divergent"` record: the collapsed key, the kept row and EVERY
/// recorded loser — both as the POST-remap rows exactly as the staged table
/// holds them.
#[derive(Debug, Clone)]
struct AuxDivergence {
    key_set: Vec<&'static str>,
    key: Vec<(&'static str, DbValue)>,
    kept: BTreeMap<String, DbValue>,
    discarded: Vec<BTreeMap<String, DbValue>>,
}

/// One table's decided rewrite: every surviving row (typed cells intact) and
/// how many rows were remapped, collapsed or discarded, plus the captured
/// records the sidecar renders.
#[derive(Debug, Clone)]
struct AuxRewrite {
    db: &'static str,
    table: &'static str,
    columns: Vec<String>,
    /// The pinned column order (`AuxTable::columns`) the canonical
    /// serialization and the sidecar's rows use.
    pinned: &'static [&'static str],
    rows: Vec<BTreeMap<String, DbValue>>,
    remapped: usize,
    collapsed: usize,
    discarded: usize,
    divergences: Vec<AuxDivergence>,
    carries: Vec<AuxCarry>,
}

/// The closure refusal set (§C.5) and the decided per-table rewrites, all
/// computed read-only BEFORE the staged tree exists: a refusal here mutates
/// nothing.
fn auxiliary_closure(
    plan: &ConsolidationPlan,
    storage_root: &Path,
    prefix: &str,
) -> Result<Vec<AuxRewrite>, Refusal> {
    for (name, event) in REFUSED_TIERS {
        if std::fs::symlink_metadata(storage_root.join(name)).is_ok() {
            return Err(Refusal::new(
                EXIT_UNSUPPORTED_SHAPE,
                event,
                format!("{name} is present under the storage root; this build has no rule for it"),
            ));
        }
    }
    let domain = auxiliary_domain(plan, prefix)?;
    check_registry_rows(storage_root)?;
    let mut rewrites = Vec::new();
    for db in &AUX_DBS {
        let path = storage_root.join(db.name);
        if std::fs::symlink_metadata(&path).is_err() {
            continue;
        }
        let handle = open_read_only(&path).map_err(|refusal| {
            Refusal::new(
                EXIT_UNSUPPORTED_SHAPE,
                "consolidation.auxiliary_unreadable",
                refusal.message,
            )
        })?;
        let present = aux_present_tables(&path, handle.as_ref())?;
        for name in &present {
            let known = db.tables.iter().any(|table| table.table == name)
                || db.extra.contains(&name.as_str());
            if !known {
                return Err(Refusal::new(
                    EXIT_UNSUPPORTED_SHAPE,
                    "consolidation.auxiliary_table_unknown",
                    format!("{} holds the unknown table {name:?}", path.display()),
                ));
            }
        }
        for table in db.tables {
            if !present.iter().any(|name| name == table.table) {
                continue;
            }
            rewrites.push(read_aux_table(
                &path,
                handle.as_ref(),
                db.name,
                table,
                &domain,
            )?);
        }
    }
    Ok(rewrites)
}

/// The remap domain: mapped source host paths and scope tags. A value that is
/// both a mapped source and a mapped target makes the rewrite undecidable.
fn auxiliary_domain(plan: &ConsolidationPlan, prefix: &str) -> Result<AuxDomain, Refusal> {
    let mut paths: BTreeMap<String, String> = BTreeMap::new();
    let mut tags: BTreeMap<String, String> = BTreeMap::new();
    let mut target_paths: BTreeSet<String> = BTreeSet::new();
    for row in &plan.mapping {
        if let Some(source) = host_path(&row.source_identity) {
            let target = host_path(&row.target_identity).ok_or_else(|| {
                Refusal::new(
                    EXIT_IDENTITY_REFUSAL,
                    "consolidation.identity_unsupported",
                    format!(
                        "source {} has a host path but its target {:?} has none",
                        row.source_hash, row.target_identity
                    ),
                )
            })?;
            if let Some(previous) = paths.insert(source.clone(), target.clone())
                && previous != target
            {
                return Err(auxiliary_ambiguous(&source));
            }
            target_paths.insert(target);
        }
        let source_tag = format!("{prefix}_project_{}", row.source_hash);
        let target_tag = row.target_association.container_tag.clone();
        if let Some(previous) = tags.insert(source_tag.clone(), target_tag.clone())
            && previous != target_tag
        {
            return Err(auxiliary_ambiguous(&source_tag));
        }
    }
    for (source, target) in &paths {
        if target != source && target_paths.contains(source) {
            return Err(auxiliary_ambiguous(source));
        }
    }
    Ok(AuxDomain {
        paths,
        tags,
        target_paths,
    })
}

/// The host DIRECTORY a canonical identity names (`path:/…`, `git-common:/…`).
///
/// A `git-common:` identity names the repository's common dir, so its project
/// directory is that dir's parent — the same rule `get_git_project_root` applies
/// (`crates/mem-core/src/tags/mod.rs:208-220`). Returning the `.git` path would
/// make §B.5's whole-string rule undecidable, because no wire `project_id` ever
/// equals a `.git` path (§B.1).
fn host_path(identity: &str) -> Option<String> {
    let (scheme, rest) = identity.split_once(':')?;
    if !rest.starts_with('/') {
        return None;
    }
    let path = match scheme {
        "git-common" => rest.strip_suffix("/.git").unwrap_or(rest),
        _ => rest,
    };
    Some(path.to_string())
}

fn auxiliary_ambiguous(value: &str) -> Refusal {
    Refusal::new(
        EXIT_IDENTITY_REFUSAL,
        "consolidation.identity_ambiguous",
        format!("{value:?} is both a mapped source and a mapped target"),
    )
}

/// The project scope hashes the registry references (empty when no registry).
fn registry_project_hashes(storage_root: &Path) -> Result<BTreeSet<String>, Refusal> {
    let path = storage_root.join("metadata.db");
    if std::fs::symlink_metadata(&path).is_err() {
        return Ok(BTreeSet::new());
    }
    let handle = open_read_only(&path).map_err(|refusal| {
        Refusal::new(
            EXIT_UNSUPPORTED_SHAPE,
            "consolidation.registry_unreadable",
            refusal.message,
        )
    })?;
    let rows = block_on(handle.query(
        "SELECT scope_hash FROM shards WHERE scope = 'project'",
        Vec::new(),
    ))
    .map_err(|error| {
        Refusal::new(
            EXIT_UNSUPPORTED_SHAPE,
            "consolidation.registry_unreadable",
            format!("{} cannot be read: {error}", path.display()),
        )
    })?;
    Ok(rows
        .iter()
        .filter_map(|row| row.text("scope_hash"))
        .collect())
}

/// §C.6/§I.3: every registry row keeps the `<scope>s/<basename>` shape.
fn check_registry_rows(storage_root: &Path) -> Result<(), Refusal> {
    let path = storage_root.join("metadata.db");
    if std::fs::symlink_metadata(&path).is_err() {
        return Ok(());
    }
    let handle = open_read_only(&path).map_err(|refusal| {
        Refusal::new(
            EXIT_UNSUPPORTED_SHAPE,
            "consolidation.registry_unreadable",
            refusal.message,
        )
    })?;
    let rows = block_on(handle.query("SELECT scope, db_path FROM shards", Vec::new())).map_err(
        |error| {
            Refusal::new(
                EXIT_UNSUPPORTED_SHAPE,
                "consolidation.registry_unreadable",
                format!("{} cannot be read: {error}", path.display()),
            )
        },
    )?;
    for row in rows.iter() {
        let (Some(scope), Some(db_path)) = (row.text("scope"), row.text("db_path")) else {
            return Err(Refusal::new(
                EXIT_UNSUPPORTED_SHAPE,
                "consolidation.registry_row_nonconforming",
                format!(
                    "{} has a shards row without a scope/db_path",
                    path.display()
                ),
            ));
        };
        let basename = db_path.rsplit('/').next().unwrap_or(db_path.as_str());
        if db_path != format!("{scope}s/{basename}") {
            return Err(Refusal::new(
                EXIT_UNSUPPORTED_SHAPE,
                "consolidation.registry_row_nonconforming",
                format!(
                    "{} stores db_path {db_path:?} for scope {scope:?}; the invariant is <scope>s/<basename>",
                    path.display()
                ),
            ));
        }
    }
    Ok(())
}

fn aux_present_tables(path: &Path, handle: &dyn Db) -> Result<BTreeSet<String>, Refusal> {
    let rows = block_on(handle.query(
        "SELECT name FROM sqlite_master WHERE type = 'table'",
        Vec::new(),
    ))
    .map_err(|error| {
        Refusal::new(
            EXIT_UNSUPPORTED_SHAPE,
            "consolidation.auxiliary_unreadable",
            format!("{} has no readable sqlite_master: {error}", path.display()),
        )
    })?;
    Ok(rows
        .iter()
        .filter_map(|row| row.text("name"))
        .filter(|name| !name.starts_with("sqlite_"))
        .collect())
}

fn read_aux_table(
    path: &Path,
    handle: &dyn Db,
    db_name: &'static str,
    table: &AuxTable,
    domain: &AuxDomain,
) -> Result<AuxRewrite, Refusal> {
    let unreadable = |detail: String| {
        Refusal::new(
            EXIT_UNSUPPORTED_SHAPE,
            "consolidation.auxiliary_unreadable",
            format!("{} {}: {detail}", path.display(), table.table),
        )
    };
    let info = block_on(handle.query(&format!("PRAGMA table_info({})", table.table), Vec::new()))
        .map_err(|error| unreadable(format!("no readable columns: {error}")))?;
    let columns: Vec<String> = info.iter().filter_map(|row| row.text("name")).collect();
    for column in &columns {
        if !table.columns.contains(&column.as_str()) {
            return Err(Refusal::new(
                EXIT_UNSUPPORTED_SHAPE,
                "consolidation.auxiliary_column_unknown",
                format!(
                    "{} {} carries the column {column:?}, outside the pinned inventory",
                    path.display(),
                    table.table
                ),
            ));
        }
    }
    let rows = block_on(handle.query(&format!("SELECT * FROM {}", table.table), Vec::new()))
        .map_err(|error| unreadable(format!("cannot be read: {error}")))?;
    let mut typed: Vec<BTreeMap<String, DbValue>> = rows
        .iter()
        .map(|row| {
            columns
                .iter()
                .filter_map(|name| row.get(name).map(|value| (name.clone(), value.clone())))
                .collect()
        })
        .collect();

    let mut remapped = 0;
    let mut tags: Vec<Option<AuxCarryTag>> = vec![None; typed.len()];
    for (index, row) in typed.iter_mut().enumerate() {
        let Some(column) = table.remap else {
            continue;
        };
        let Some(current) = row.get(column).cloned() else {
            continue;
        };
        match remap_cell(&current, domain, table.strict, path, table, column)? {
            None => {}
            Some(Remap::Mapped(value)) => {
                row.insert(column.to_string(), value);
                remapped += 1;
            }
            Some(Remap::Carried { original, carried }) => {
                // `remapped` counts REAL byte changes only, so the write-path
                // skip stays keyed on a change while a no-op carry still gets
                // its record (§C.5 rev 13).
                if carried != original {
                    row.insert(column.to_string(), carried.clone());
                    remapped += 1;
                }
                tags[index] = Some(AuxCarryTag { original, carried });
            }
        }
    }
    let outcome = collapse_aux_rows(typed, table, path)?;
    let carries = carry_records(table, &outcome.survivors, &tags);
    Ok(AuxRewrite {
        db: db_name,
        table: table.table,
        columns,
        pinned: table.columns,
        rows: outcome.rows,
        remapped,
        collapsed: outcome.collapsed,
        discarded: outcome.discarded,
        divergences: outcome.divergences,
        carries,
    })
}

/// §C.5 rev 13 (opsec C4/C16): one record per distinct `(column, original)`
/// tuple WITH at least one staged survivor carrier. `survivors` indexes the
/// PRE-collapse tag vector (gate-6 MOD 1d), so a loser's carrier is never
/// counted, two distinct originals sharing one `carried` produce two records,
/// and a zero-carrier tuple emits nothing.
fn carry_records(
    table: &AuxTable,
    survivors: &[usize],
    tags: &[Option<AuxCarryTag>],
) -> Vec<AuxCarry> {
    let Some(column) = table.remap else {
        return Vec::new();
    };
    let mut by_original: BTreeMap<String, AuxCarry> = BTreeMap::new();
    for index in survivors {
        let Some(Some(tag)) = tags.get(*index) else {
            continue;
        };
        let key = match &tag.original {
            DbValue::Text(text) => text.clone(),
            other => format!("{other:?}"),
        };
        match by_original.get_mut(&key) {
            Some(record) => record.rows += 1,
            None => {
                by_original.insert(
                    key,
                    AuxCarry {
                        column,
                        original: tag.original.clone(),
                        carried: tag.carried.clone(),
                        rows: 1,
                    },
                );
            }
        }
    }
    by_original.into_values().collect()
}

/// Rule B.5 + §C.5 rev 13: three outcomes, evaluated in this order — a mapped
/// source host path rewrites (`Mapped`), a mapped target (or an identity
/// mapping) passes unchanged (`None`), and anything else is SWAP-AND-CARRIED
/// (`Carried`), never refused. A non-text cell is an unsupported encoding.
fn remap_cell(
    value: &DbValue,
    domain: &AuxDomain,
    strict: bool,
    path: &Path,
    table: &AuxTable,
    column: &str,
) -> Result<Option<Remap>, Refusal> {
    match value {
        DbValue::Null => Ok(None),
        DbValue::Text(current) => {
            let mapping = if strict { &domain.paths } else { &domain.tags };
            if let Some(target) = mapping.get(current) {
                // The identity-mapping no-op: a hit that rewrites nothing.
                return if target == current {
                    Ok(None)
                } else {
                    Ok(Some(Remap::Mapped(DbValue::Text(target.clone()))))
                };
            }
            if !strict || domain.target_paths.contains(current) {
                return Ok(None);
            }
            Ok(Some(Remap::Carried {
                original: DbValue::Text(current.clone()),
                carried: DbValue::Text(swap_host_root(current)),
            }))
        }
        other => Err(Refusal::new(
            EXIT_UNSUPPORTED_SHAPE,
            "consolidation.auxiliary_encoding_unsupported",
            format!(
                "{} {}.{column} holds a {other:?} instead of text",
                path.display(),
                table.table
            ),
        )),
    }
}

/// §B.4's pinned swap applied to an auxiliary cell: the exact host root, or a
/// LEADING `OLD_HOST_ROOT/` prefix. A `/home/vaz` component further along the
/// string is content and stays — a mid-path replacement is never authorised.
fn swap_host_root(value: &str) -> String {
    if value == OLD_HOST_ROOT {
        return NEW_HOST_ROOT.to_string();
    }
    match value
        .strip_prefix(OLD_HOST_ROOT)
        .and_then(|rest| rest.strip_prefix('/'))
    {
        Some(rest) => format!("{NEW_HOST_ROOT}/{rest}"),
        None => value.to_string(),
    }
}

/// What one table's collapse decided (§C.2/§C.5 rev 13).
struct CollapseOutcome {
    rows: Vec<BTreeMap<String, DbValue>>,
    collapsed: usize,
    discarded: usize,
    divergences: Vec<AuxDivergence>,
    /// Survivor positions in the PRE-collapse row vector — the tag vector's
    /// index domain, never post-collapse ordinals.
    survivors: Vec<usize>,
}

/// §C.2/§C.3: every UNIQUE/PK is re-evaluated after the remap. Exact full-row
/// equality collapses with occurrence provenance in EVERY pass; a divergent
/// group keeps exactly ONE row (keep-newer) and records every loser.
fn collapse_aux_rows(
    rows: Vec<BTreeMap<String, DbValue>>,
    table: &AuxTable,
    path: &Path,
) -> Result<CollapseOutcome, Refusal> {
    if table.keys.is_empty() {
        let survivors = (0..rows.len()).collect();
        return Ok(CollapseOutcome {
            rows,
            collapsed: 0,
            discarded: 0,
            divergences: Vec::new(),
            survivors,
        });
    }
    let mut kept: Vec<usize> = (0..rows.len()).collect();
    let mut collapsed = 0;
    let mut discarded = 0;
    let mut divergences = Vec::new();
    for keys in table.keys {
        let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for index in &kept {
            groups
                .entry(aux_key(&rows[*index], keys))
                .or_default()
                .push(*index);
        }
        let mut survivors = Vec::with_capacity(groups.len());
        for group in groups.into_values() {
            // The equality branch runs in EVERY pass: a byte-identical
            // duplicate collapses and is NEVER recorded as a loser.
            if group.len() == 1 {
                survivors.push(group[0]);
                continue;
            }
            if group.iter().all(|index| rows[*index] == rows[group[0]]) {
                collapsed += group.len() - 1;
                survivors.push(group[0]);
                continue;
            }
            check_created_at(&rows, &group, table, path, keys)?;
            let mut best = group[0];
            for index in &group[1..] {
                if keep_newer(&rows[*index], &rows[best], table) == Ordering::Greater {
                    best = *index;
                }
            }
            let mut losers = Vec::new();
            for index in &group {
                if *index == best {
                    continue;
                }
                if rows[*index] == rows[best] {
                    collapsed += 1;
                } else {
                    losers.push(*index);
                }
            }
            if !losers.is_empty() {
                discarded += losers.len();
                divergences.push(AuxDivergence {
                    key_set: keys.to_vec(),
                    key: keys
                        .iter()
                        .map(|name| {
                            (
                                *name,
                                rows[best].get(*name).cloned().unwrap_or(DbValue::Null),
                            )
                        })
                        .collect(),
                    kept: rows[best].clone(),
                    discarded: losers.iter().map(|index| rows[*index].clone()).collect(),
                });
            }
            survivors.push(best);
        }
        kept = survivors;
    }
    Ok(CollapseOutcome {
        rows: kept.iter().map(|index| rows[*index].clone()).collect(),
        collapsed,
        discarded,
        divergences,
        survivors: kept,
    })
}

/// §C.5 rev 13 (gate-1 M5): a divergent group whose table HAS `created_at` and
/// holds ANY non-Integer value there refuses 71 BEFORE any comparison, so NULL
/// and NaN never reach an ordering.
fn check_created_at(
    rows: &[BTreeMap<String, DbValue>],
    group: &[usize],
    table: &AuxTable,
    path: &Path,
    keys: &[&str],
) -> Result<(), Refusal> {
    if !table.columns.contains(&"created_at") {
        return Ok(());
    }
    for index in group {
        if !matches!(rows[*index].get("created_at"), Some(DbValue::Integer(_))) {
            return Err(Refusal::new(
                EXIT_DIVERGENT_ROWS,
                "consolidation.auxiliary_rows_divergent",
                format!(
                    "{} {} holds a divergent group for {keys:?} whose created_at is not an integer; newer is undecidable",
                    path.display(),
                    table.table
                ),
            ));
        }
    }
    Ok(())
}

/// The TOTAL keep-newer order: greatest `created_at` as an Integer, then the
/// lexicographically greatest text `id`, then the greatest canonical row
/// serialization — which decides the tables with neither column and any
/// residual tie.
fn keep_newer(
    candidate: &BTreeMap<String, DbValue>,
    current: &BTreeMap<String, DbValue>,
    table: &AuxTable,
) -> Ordering {
    if table.columns.contains(&"created_at")
        && let (Some(DbValue::Integer(left)), Some(DbValue::Integer(right))) =
            (candidate.get("created_at"), current.get("created_at"))
        && left != right
    {
        return left.cmp(right);
    }
    if table.columns.contains(&"id")
        && let (Some(DbValue::Text(left)), Some(DbValue::Text(right))) =
            (candidate.get("id"), current.get("id"))
        && left != right
    {
        return left.cmp(right);
    }
    canonical_row(candidate, table.columns).cmp(&canonical_row(current, table.columns))
}

/// Every cell in the PINNED column order, `{:?}`-rendered, compared as bytes.
fn canonical_row(row: &BTreeMap<String, DbValue>, columns: &[&str]) -> String {
    let mut out = String::new();
    for column in columns {
        out.push_str(&format!("{:?}\u{1}", row.get(*column)));
    }
    out
}

fn aux_key(row: &BTreeMap<String, DbValue>, keys: &[&str]) -> String {
    let mut key = String::new();
    for name in keys {
        key.push_str(&format!("{:?}\u{1}", row.get(*name)));
    }
    key
}

/// Writes the decided rewrites into the STAGED copies (never S): the whole
/// table is replaced by its survivors, typed cells intact.
fn write_auxiliary(
    staged: &Path,
    store_config: &StoreConfig,
    rewrites: &[AuxRewrite],
) -> Result<(), Refusal> {
    for rewrite in rewrites {
        // §C.5 rev 13 (gate-2 C8): a divergent group whose survivor already
        // holds the target string needs no table rewrite, so the discard count
        // joins the predicate — the old two-term skip would leave the loser in
        // the staged table.
        if rewrite.remapped == 0 && rewrite.collapsed == 0 && rewrite.discarded == 0 {
            continue;
        }
        let path = staged.join(rewrite.db);
        let io = |detail: String| {
            Refusal::new(
                EXIT_STORE_OR_IO,
                "consolidation.auxiliary_write_failed",
                detail,
            )
        };
        let handle = block_on(TursoConnectFactory.connect(
            path.clone(),
            ConnectOptions::for_config(store_config, Platform::current()),
        ))
        .map_err(|error| io(format!("{} cannot be opened: {error}", path.display())))?;
        block_on(handle.execute(&format!("DELETE FROM {}", rewrite.table), Vec::new()))
            .map_err(|error| io(format!("{} cannot be cleared: {error}", path.display())))?;
        let columns = rewrite.columns.join(", ");
        let marks = vec!["?"; rewrite.columns.len()].join(", ");
        let insert = format!("INSERT INTO {} ({columns}) VALUES ({marks})", rewrite.table);
        for row in &rewrite.rows {
            let values: Vec<DbValue> = rewrite
                .columns
                .iter()
                .map(|column| row.get(column).cloned().unwrap_or(DbValue::Null))
                .collect();
            block_on(handle.execute(&insert, values)).map_err(|error| {
                io(format!(
                    "{} {} cannot be rewritten: {error}",
                    path.display(),
                    rewrite.table
                ))
            })?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

/// The auxiliary closure as reported: per DB, per table, counts only.
fn render_aux(aux: &[AuxRewrite]) -> Value {
    let mut by_db: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
    for rewrite in aux {
        by_db.entry(rewrite.db).or_default().push(json!({
            "table": rewrite.table,
            "rows": rewrite.rows.len(),
            "remapped": rewrite.remapped,
            "collapsed": rewrite.collapsed,
        }));
    }
    json!(by_db)
}

/// §C.5 rev 13: the sidecar's JSONL body. `kind` first, one record per line —
/// `divergent` lines (the kept row and EVERY recorded loser, POST-remap
/// exactly as the staged table holds them) then `carry` lines (one per
/// distinct original with at least one surviving carrier). Deterministic: two
/// applies over the same snapshot produce byte-identical bytes.
fn aux_sidecar_bytes(aux: &[AuxRewrite]) -> Vec<u8> {
    let mut out = String::new();
    for rewrite in aux {
        for divergence in &rewrite.divergences {
            out.push_str(&format!(
                "{{\"kind\":\"divergent\",\"db\":{},\"table\":{},\"key_set\":{},\"key\":{},\"kept\":{},\"discarded\":[{}],\"reason\":\"keep-newer\"}}\n",
                cell_json(&DbValue::Text(rewrite.db.to_string())),
                cell_json(&DbValue::Text(rewrite.table.to_string())),
                json_array(divergence.key_set.iter().map(|name| DbValue::Text(name.to_string()))),
                // §C.5 rev 13: `key` is "the collapsed key's cells" — the
                // `key_set` members ONLY. Rendering it over the full pinned
                // column list would emit a null for every non-key cell, and
                // reviewer check (2) reads `key` as a predicate (`"id":null`
                // would become `id IS NULL` against a table where `id` is
                // never null). `kept`/`discarded` stay full rows by spec.
                json_object(
                    &divergence.key_set,
                    &divergence
                        .key
                        .iter()
                        .map(|(name, value)| ((*name).to_string(), value.clone()))
                        .collect(),
                ),
                row_json(&divergence.kept, rewrite.pinned),
                divergence
                    .discarded
                    .iter()
                    .map(|row| row_json(row, rewrite.pinned))
                    .collect::<Vec<_>>()
                    .join(","),
            ));
        }
        for carry in &rewrite.carries {
            out.push_str(&format!(
                "{{\"kind\":\"carry\",\"db\":{},\"table\":{},\"column\":{},\"original\":{},\"carried\":{},\"rows\":{}}}\n",
                cell_json(&DbValue::Text(rewrite.db.to_string())),
                cell_json(&DbValue::Text(rewrite.table.to_string())),
                cell_json(&DbValue::Text(carry.column.to_string())),
                cell_json(&carry.original),
                cell_json(&carry.carried),
                carry.rows,
            ));
        }
    }
    out.into_bytes()
}

/// One cell, verbatim for text/integer/real/null; a blob is `{"sha256","len"}`
/// — the bytes are never lost (this design never mutates the source snapshot),
/// so the digest is the checkable handle on them.
fn cell_json(value: &DbValue) -> String {
    match value {
        DbValue::Null => "null".to_string(),
        DbValue::Integer(number) => number.to_string(),
        // A REAL can only reach a sidecar outside `created_at` (a non-Integer
        // `created_at` in a divergent group refuses 71 first). ponytail: a
        // non-finite REAL renders as `null` (serde_json has no NaN literal);
        // no pinned aux column is REAL, so add a typed encoding only if one
        // ever becomes one.
        DbValue::Real(number) => Value::from(*number).to_string(),
        DbValue::Text(text) => Value::String(text.clone()).to_string(),
        DbValue::Blob(bytes) => format!(
            "{{\"sha256\":\"{}\",\"len\":{}}}",
            sha256_hex(bytes),
            bytes.len()
        ),
    }
}

/// A JSON array of cells (the divergent record's `key_set`).
fn json_array(values: impl Iterator<Item = DbValue>) -> String {
    let cells: Vec<String> = values.map(|value| cell_json(&value)).collect();
    format!("[{}]", cells.join(","))
}

/// One full row as a JSON object in the PINNED column order.
fn row_json(row: &BTreeMap<String, DbValue>, columns: &[&str]) -> String {
    json_object(columns, row)
}

/// `{column: cell}` in the pinned order; a cell the row lacks is `null`.
fn json_object(columns: &[&str], row: &BTreeMap<String, DbValue>) -> String {
    let cells: Vec<String> = columns
        .iter()
        .map(|column| {
            format!(
                "{}:{}",
                Value::String((*column).to_string()),
                cell_json(row.get(*column).unwrap_or(&DbValue::Null))
            )
        })
        .collect();
    format!("{{{}}}", cells.join(","))
}

fn render_report(preflight: &Preflight, dry_run: bool) -> String {
    let groups: BTreeSet<&str> = preflight
        .sources
        .iter()
        .map(|source| source.target_hash.as_str())
        .collect();
    let sources: Vec<Value> = preflight
        .sources
        .iter()
        .map(|source| {
            json!({
                "source_hash": source.source_hash,
                "target_hash": source.target_hash,
                "target_identity": source.target_identity,
                "schema_version": source.schema_version,
                "columns": source.columns.iter().collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "format_version": REPORT_FORMAT_VERSION,
        "dry_run": dry_run,
        "plan_sha256": preflight.plan_sha256,
        "source_count": preflight.sources.len(),
        "group_count": groups.len(),
        "sources": sources,
        "carried_verbatim": preflight.carried,
        "auxiliary": render_aux(&preflight.aux),
    })
    .to_string()
}

// ---------------------------------------------------------------------------
// Runtime helper
// ---------------------------------------------------------------------------

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    match mem_store::tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime.block_on(future),
        Err(error) => {
            mem_core::diagnostics::structured_stderr(
                "consolidation.runtime_failed",
                "error",
                "consolidation",
                json!({ "message": format!("the consolidator runtime cannot start: {error}") }),
            );
            std::process::exit(EXIT_STORE_OR_IO);
        }
    }
}

// ---------------------------------------------------------------------------
// Step 6 — verified snapshot before native recovery (§G S3, C2 premise → 77)
// ---------------------------------------------------------------------------

/// The M1 exclusion set, relative to the storage root (rev-11 §C.1). Exact
/// root-level match only; the `.excluded` sidecar records what it covered.
///
/// Two-directional construction-time pin (MOD 2, blueprint :125): the length is
/// type-pinned, so adding a third member (widening) or dropping one (narrowing)
/// is a compile error, and the content check makes substituting a member a
/// compile error too. No runtime assert is needed.
const M1_EXCLUDED: [&str; 2] = ["status", "engine.lock"];
const _: () = assert!(
    M1_EXCLUDED.len() == 2
        && const_str_eq(M1_EXCLUDED[0], "status")
        && const_str_eq(M1_EXCLUDED[1], "engine.lock")
);

/// Const-evaluable `==` for `&str` (`str::eq` is not const).
const fn const_str_eq(left: &str, right: &str) -> bool {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    if left.len() != right.len() {
        return false;
    }
    let mut index = 0;
    while index < left.len() {
        if left[index] != right[index] {
            return false;
        }
        index += 1;
    }
    true
}
const MODE_TABLE: &str = "mode-table";
const BASELINE: &str = "baseline.sha256";
const EXCLUDED: &str = "manifest.sha256.excluded";
const SNAP_DATA: &str = "data";

fn custody_refusal(message: impl Into<String>) -> Refusal {
    Refusal::new(
        EXIT_CUSTODY_REFUSAL,
        "consolidation.snapshot_refused",
        message,
    )
}

/// Entry point for `--mode=consolidate-snapshot --custody <abs-dir>`.
pub fn run_snapshot(root: &Path, custody: &Path) -> i32 {
    match snapshot(root, custody) {
        Ok(()) => 0,
        Err(refusal) => emit_refusal(&refusal),
    }
}

/// Entry point for `--mode=consolidate-verify-snapshot --custody <abs-dir>`.
pub fn run_verify_snapshot(custody: &Path) -> i32 {
    match verify_snapshot(custody) {
        Ok(()) => 0,
        Err(refusal) => emit_refusal(&refusal),
    }
}

/// C2 premise: the storage root is a plain directory with owner write, no
/// group/other write, and no set-id or sticky bit. Checked before any tree
/// derivation.
fn premise_check(root: &Path) -> Result<(), Refusal> {
    use std::os::unix::fs::MetadataExt;
    let meta = std::fs::symlink_metadata(root)
        .map_err(|error| custody_refusal(format!("storage root unreadable: {error}")))?;
    if !meta.is_dir() {
        return Err(custody_refusal("storage root is not a plain directory"));
    }
    let mode = meta.mode() & 0o7777;
    if mode & 0o200 == 0 {
        return Err(custody_refusal("storage root has no owner write"));
    }
    if mode & 0o022 != 0 {
        return Err(custody_refusal("storage root has group or other write"));
    }
    if mode & 0o7000 != 0 {
        return Err(custody_refusal("storage root has a set-id or sticky bit"));
    }
    // Per path, not only the root (§C.1b / §G S3 premise): every entry must be
    // restorable by `u+w` alone, so each one is checked before any derivation.
    for (rel, entry) in tree_of(root)? {
        let mode = entry.mode() & 0o7777;
        if mode & 0o200 == 0 {
            return Err(custody_refusal(format!("{rel} has no owner write")));
        }
        if mode & 0o022 != 0 {
            return Err(custody_refusal(format!("{rel} has group or other write")));
        }
        if mode & 0o7000 != 0 {
            return Err(custody_refusal(format!("{rel} has a set-id or sticky bit")));
        }
    }
    Ok(())
}

/// Walks `dir` recursively, recording every entry's lstat metadata keyed by
/// its path relative to `root`. Any non-directory, non-regular, non-symlink
/// entry refuses.
fn walk_tree(
    root: &Path,
    dir: &Path,
    out: &mut BTreeMap<String, std::fs::Metadata>,
) -> Result<(), Refusal> {
    let entries = std::fs::read_dir(dir)
        .map_err(|error| custody_refusal(format!("read {}: {error}", dir.display())))?;
    for entry in entries {
        let path = entry
            .map_err(|error| custody_refusal(format!("read entry: {error}")))?
            .path();
        let meta = std::fs::symlink_metadata(&path)
            .map_err(|error| custody_refusal(format!("stat {}: {error}", path.display())))?;
        let rel = path
            .strip_prefix(root)
            .map_err(|_| custody_refusal("entry outside root"))?
            .to_string_lossy()
            .into_owned();
        let is_dir = meta.is_dir();
        let plain = meta.is_file() || meta.file_type().is_symlink();
        if !is_dir && !plain {
            return Err(custody_refusal(format!("non-regular entry {rel}")));
        }
        out.insert(rel, meta);
        if is_dir {
            walk_tree(root, &path, out)?;
        }
    }
    Ok(())
}

fn tree_of(root: &Path) -> Result<BTreeMap<String, std::fs::Metadata>, Refusal> {
    let mut tree = BTreeMap::new();
    walk_tree(root, root, &mut tree)?;
    Ok(tree)
}

/// M1-filtered manifest: one line per entry, `dir <rel>`, `file <sha256> <rel>`
/// or `link <target> <rel>`. Root-level `status` and `engine.lock` are skipped.
fn manifest_of(root: &Path) -> Result<String, Refusal> {
    let mut out = String::new();
    for (rel, meta) in tree_of(root)? {
        if M1_EXCLUDED.contains(&rel.as_str()) {
            continue;
        }
        let path = root.join(&rel);
        if meta.is_dir() {
            out.push_str(&format!("dir {rel}\n"));
        } else if meta.file_type().is_symlink() {
            let target = std::fs::read_link(&path)
                .map_err(|error| custody_refusal(format!("readlink {rel}: {error}")))?;
            out.push_str(&format!("link {} {rel}\n", target.display()));
        } else {
            let bytes = std::fs::read(&path)
                .map_err(|error| custody_refusal(format!("read {rel}: {error}")))?;
            let digest = mem_core::sha256::hex(&mem_core::sha256::digest(&bytes));
            out.push_str(&format!("file {digest} {rel}\n"));
        }
    }
    Ok(out)
}

/// Presence and kind of each M1-excluded path. A non-regular excluded path
/// refuses 77.
fn excluded_sidecar_of(root: &Path) -> Result<String, Refusal> {
    let mut out = String::new();
    for rel in M1_EXCLUDED {
        match std::fs::symlink_metadata(root.join(rel)) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                out.push_str(&format!("absent {rel}\n"));
            }
            Err(error) => return Err(custody_refusal(format!("stat {rel}: {error}"))),
            Ok(meta) if meta.is_file() => out.push_str(&format!("file {rel}\n")),
            Ok(_) => return Err(custody_refusal(format!("excluded {rel} is not regular"))),
        }
    }
    Ok(out)
}

/// The pre-chmod `(mode, kind)` table, one line per path under the root, the
/// root itself FIRST as the lone `.` path (so a restore can reproduce the S
/// root's mode — §H requires it to equal this table's root value).
fn mode_table_of(
    root: &Path,
    tree: &BTreeMap<String, std::fs::Metadata>,
) -> Result<String, Refusal> {
    use std::os::unix::fs::MetadataExt;
    fn kind_of(meta: &std::fs::Metadata) -> &'static str {
        if meta.is_dir() {
            "dir"
        } else if meta.file_type().is_symlink() {
            "link"
        } else {
            "file"
        }
    }
    let root_meta = std::fs::symlink_metadata(root)
        .map_err(|error| custody_refusal(format!("mode-table root: {error}")))?;
    let mut out = format!(
        "{:o} {} .\n",
        root_meta.mode() & 0o7777,
        kind_of(&root_meta)
    );
    for (rel, meta) in tree {
        out.push_str(&format!(
            "{:o} {} {rel}\n",
            meta.mode() & 0o7777,
            kind_of(meta)
        ));
    }
    Ok(out)
}

/// Entry kind of one captured mode-table line.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TableKind {
    Dir,
    File,
    Link,
}

/// Parses a captured mode table into `rel -> (mode, kind)` with the root keyed
/// `""`. Two on-disk formats exist and BOTH are legitimate: this module's
/// writer emits `<octal> <kind-word> <bare-rel>` with the root as the lone `.`
/// line, while the external window script emits `<octal> <kind-letter>
/// <./-prefixed-path>`. A leading `./` and a lone `.` normalize to the root;
/// `d`/`f`/`l` are accepted beside `dir`/`file`/`link`. A root-less table (an
/// artifact written before the root line existed) still parses.
fn parse_mode_table(contents: &str) -> Result<BTreeMap<String, (u32, TableKind)>, Refusal> {
    let mut table = BTreeMap::new();
    for (number, line) in contents.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let mut fields = line.split_whitespace();
        let mode = fields
            .next()
            .ok_or_else(|| custody_refusal(format!("mode-table line {} is empty", number + 1)))?;
        let kind = fields.next().ok_or_else(|| {
            custody_refusal(format!("mode-table line {} has no kind", number + 1))
        })?;
        let raw = fields.collect::<Vec<_>>().join(" ");
        if raw.is_empty() {
            return Err(custody_refusal(format!(
                "mode-table line {} has no path",
                number + 1
            )));
        }
        let mode = u32::from_str_radix(mode, 8).map_err(|_| {
            custody_refusal(format!("mode-table line {} mode {mode:?}", number + 1))
        })?;
        let kind = match kind {
            "d" | "dir" => TableKind::Dir,
            "f" | "file" => TableKind::File,
            "l" | "link" => TableKind::Link,
            other => {
                return Err(custody_refusal(format!(
                    "mode-table line {} kind {other:?}",
                    number + 1
                )));
            }
        };
        let mut rel = raw.as_str();
        while let Some(rest) = rel.strip_prefix("./") {
            rel = rest;
        }
        if rel == "." || rel.is_empty() {
            rel = "";
        }
        table.insert(rel.to_string(), (mode & 0o7777, kind));
    }
    Ok(table)
}

/// Applies a captured `(mode, kind)` table to a freshly built tree. Every
/// recorded path must exist with the recorded kind (otherwise 77 — the tree is
/// not the one the table describes); symlink modes are not meaningful on Linux
/// and are left alone. Deepest first, so a directory is never made read-only
/// before its children are populated.
fn apply_mode_table(
    root: &Path,
    table: &BTreeMap<String, (u32, TableKind)>,
) -> Result<(), Refusal> {
    use std::os::unix::fs::PermissionsExt;
    for (rel, (mode, kind)) in table.iter().rev() {
        let path = if rel.is_empty() {
            root.to_path_buf()
        } else {
            root.join(rel)
        };
        let meta = std::fs::symlink_metadata(&path)
            .map_err(|error| custody_refusal(format!("mode-table stat {rel:?}: {error}")))?;
        let actual = if meta.is_dir() {
            TableKind::Dir
        } else if meta.file_type().is_symlink() {
            TableKind::Link
        } else {
            TableKind::File
        };
        if actual != *kind {
            return Err(custody_refusal(format!(
                "mode-table kind mismatch at {rel:?}"
            )));
        }
        if *kind == TableKind::Link {
            continue;
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(*mode))
            .map_err(|error| custody_refusal(format!("mode-table chmod {rel:?}: {error}")))?;
    }
    Ok(())
}

/// The live `(mode, kind)` map of a tree, in the parser's shape. Used to decide
/// whether a restore is already complete.
fn mode_map_of(root: &Path) -> Result<BTreeMap<String, (u32, TableKind)>, Refusal> {
    use std::os::unix::fs::MetadataExt;
    let mut table = BTreeMap::new();
    let root_meta = std::fs::symlink_metadata(root)
        .map_err(|error| custody_refusal(format!("mode-map root: {error}")))?;
    table.insert(String::new(), (root_meta.mode() & 0o7777, TableKind::Dir));
    for (rel, meta) in tree_of(root)? {
        let kind = if meta.is_dir() {
            TableKind::Dir
        } else if meta.file_type().is_symlink() {
            TableKind::Link
        } else {
            TableKind::File
        };
        table.insert(rel, (meta.mode() & 0o7777, kind));
    }
    Ok(table)
}

/// `sha256` hex of a byte slice (one spelling for the new artifact code).
fn sha256_hex(bytes: &[u8]) -> String {
    mem_core::sha256::hex(&mem_core::sha256::digest(bytes))
}

/// Exclusive, owner-only artifact write (0600, never follows a symlink).
fn write_artifact(path: &Path, contents: &[u8]) -> Result<(), Refusal> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| custody_refusal(format!("create {}: {error}", path.display())))?;
    file.write_all(contents)
        .map_err(|error| custody_refusal(format!("write {}: {error}", path.display())))
}

/// Copies the S tree into `to` byte-for-byte. Directories 0700, files 0600,
/// symlinks recreated with their target; nothing is followed.
fn copy_snapshot_tree(
    from: &Path,
    to: &Path,
    tree: &BTreeMap<String, std::fs::Metadata>,
) -> Result<(), Refusal> {
    use std::os::unix::fs::{DirBuilderExt, symlink};
    let io = |error: std::io::Error| custody_refusal(format!("copy: {error}"));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(to)
        .map_err(io)?;
    // BTreeMap order puts every parent before its children.
    for (rel, meta) in tree {
        let src = from.join(rel);
        let dst = to.join(rel);
        if meta.is_dir() {
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&dst)
                .map_err(io)?;
        } else if meta.file_type().is_symlink() {
            let target = std::fs::read_link(&src).map_err(io)?;
            symlink(target, &dst).map_err(io)?;
        } else {
            let bytes = std::fs::read(&src).map_err(io)?;
            write_artifact(&dst, &bytes)?;
        }
    }
    Ok(())
}

/// `chmod -R a-w` over the staged data tree only (F43). Symlinks are skipped.
fn freeze_tree(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let meta = std::fs::symlink_metadata(path)?;
    if meta.is_dir() {
        for entry in std::fs::read_dir(path)? {
            freeze_tree(&entry?.path())?;
        }
    }
    if !meta.file_type().is_symlink() {
        std::fs::set_permissions(
            path,
            std::fs::Permissions::from_mode(meta.mode() & 0o7777 & !0o222),
        )?;
    }
    Ok(())
}

fn snapshot(root: &Path, custody: &Path) -> Result<(), Refusal> {
    use std::os::unix::fs::DirBuilderExt;
    premise_check(root)?;
    let parent = custody
        .parent()
        .ok_or_else(|| custody_refusal("custody has no parent"))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| custody_refusal(format!("custody parent: {error}")))?;
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(custody)
        .map_err(|error| custody_refusal(format!("custody create: {error}")))?;

    // Mode table is the FIRST artifact, captured before any copy or chmod.
    let tree = tree_of(root)?;
    write_artifact(
        &custody.join(MODE_TABLE),
        mode_table_of(root, &tree)?.as_bytes(),
    )?;

    let baseline = manifest_of(root)?;
    write_artifact(&custody.join(BASELINE), baseline.as_bytes())?;
    write_artifact(
        &custody.join(EXCLUDED),
        excluded_sidecar_of(root)?.as_bytes(),
    )?;

    let data = custody.join(SNAP_DATA);
    copy_snapshot_tree(root, &data, &tree)?;
    if manifest_of(&data)? != baseline {
        return Err(custody_refusal("snapshot rehash differs from baseline"));
    }
    freeze_tree(&data).map_err(|error| custody_refusal(format!("freeze: {error}")))?;
    Ok(())
}

fn verify_snapshot(custody: &Path) -> Result<(), Refusal> {
    let baseline = std::fs::read_to_string(custody.join(BASELINE))
        .map_err(|error| custody_refusal(format!("baseline unreadable: {error}")))?;
    for artifact in [MODE_TABLE, EXCLUDED] {
        if !custody.join(artifact).is_file() {
            return Err(custody_refusal(format!("{artifact} missing")));
        }
    }
    let rehash = manifest_of(&custody.join(SNAP_DATA))?;
    if rehash != baseline {
        return Err(custody_refusal("snapshot incomplete or altered"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Step 7 — journal write primitive (§L L2/L3). ONE fixed journal filename
// (OQ3, informational): `journal.json` in the custody dir.
// ---------------------------------------------------------------------------

/// The single journal filename inside the custody dir (rev-11 OQ3 pin).
pub const JOURNAL_FILE: &str = "journal.json";

/// The rev-13 auxiliary audit sidecar (custody artifact, never inside S):
/// one JSON object per line, `kind` first.
pub const AUX_SIDECAR: &str = "journal.json.aux-divergent";

/// EV-M, then L2's directory creation: `recovery/` 0700 (EEXIST tolerated) and
/// the custody dir 0700 when absent. Shared by the journal and the sidecar
/// writers — both are custody artifacts created before any mutation.
fn ensure_custody(custody: &Path) -> Result<(), Refusal> {
    use std::os::unix::fs::DirBuilderExt;
    let io = |detail: String| Refusal::new(EXIT_STORE_OR_IO, "consolidation.journal_io", detail);
    // EV-M (blueprint :125, rev-11 §H.1): the custody-mode preflight runs before
    // EVERY first journal write, not only install/recover. An absent custody dir
    // passes, so the create-if-absent branch below still yields a valid 0700 dir;
    // a pre-existing dir of any other mode refuses 77.
    ev_m_preflight(custody)?;
    // L2: `recovery/` is created 0700 (EEXIST tolerated); the custody dir too.
    if let Some(recovery) = custody.parent() {
        match std::fs::DirBuilder::new().mode(0o700).create(recovery) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(io(format!("recovery dir: {error}"))),
        }
    }
    if !custody.is_dir() {
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(custody)
            .map_err(|error| io(format!("custody dir: {error}")))?;
    }
    Ok(())
}

/// §C.5 rev 13: writes `<custody>/journal.json.aux-divergent` through the §L
/// L2/L3 form — fixed `.tmp` sibling (`O_CREAT|O_TRUNC|O_NOFOLLOW`, 0600) →
/// `fsync(tmp)` → PLAIN atomic rename over → `fsync(custody)`. No NOREPLACE: a
/// resume must replace its own prior sidecar. An EXISTING sidecar is compared
/// BYTE-FOR-BYTE with the freshly computed bytes first — equal continues (the
/// replace writes the same bytes), different refuses 77.
fn write_aux_sidecar(custody: &Path, bytes: &[u8]) -> Result<(), Refusal> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let io = |detail: String| Refusal::new(EXIT_STORE_OR_IO, "consolidation.journal_io", detail);
    ensure_custody(custody)?;
    let path = custody.join(AUX_SIDECAR);
    if path.exists() {
        let existing = std::fs::read(&path)
            .map_err(|error| io(format!("read {}: {error}", path.display())))?;
        if existing != bytes {
            return Err(Refusal::new(
                EXIT_CUSTODY_REFUSAL,
                "consolidation.auxiliary_sidecar_mismatch",
                format!(
                    "{} holds {} bytes that differ from this run's {}; the record would no longer describe the tree it audits",
                    path.display(),
                    existing.len(),
                    bytes.len()
                ),
            ));
        }
    }
    let tmp = custody.join(format!("{AUX_SIDECAR}.tmp"));
    // Exclusive mode at creation, never widened; O_NOFOLLOW so a planted
    // symlink is an error, not a write outside the custody dir.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&tmp)
        .map_err(|error| io(format!("create {}: {error}", tmp.display())))?;
    file.write_all(bytes)
        .map_err(|error| io(format!("write {}: {error}", tmp.display())))?;
    file.sync_all()
        .map_err(|error| io(format!("fsync {}: {error}", tmp.display())))?;
    drop(file);
    std::fs::rename(&tmp, &path)
        .map_err(|error| io(format!("rename into {}: {error}", path.display())))?;
    std::fs::File::open(custody)
        .and_then(|dir| dir.sync_all())
        .map_err(|error| io(format!("fsync custody: {error}")))
}

/// The journal's EFFECTIVE phase: the last record that carries a `phase`
/// field, skipping bookkeeping records (`Retaken`, `TailDiscarded`).
fn effective_phase(custody: &Path) -> Result<Option<String>, Refusal> {
    let journal = custody.join(JOURNAL_FILE);
    if !journal.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&journal)
        .map_err(|error| custody_refusal(format!("journal read: {error}")))?;
    for line in text.lines().rev().filter(|line| !line.trim().is_empty()) {
        let record: Value = serde_json::from_str(line)
            .map_err(|error| custody_refusal(format!("journal unparseable: {error}")))?;
        if let Some(phase) = record.get("phase").and_then(Value::as_str) {
            return Ok(Some(phase.to_string()));
        }
    }
    Ok(None)
}

/// §C.5 rev 13: a re-apply over a window whose journal reached
/// `InstalledUnverified` or `Verified` is REFUSED 77 — the sidecar is not
/// hashed into any manifest, so this is what stops a re-apply from silently
/// rewriting the audit record from the installed tree. In-design an in-flight
/// re-apply exits 76 (reservation held); this 77 is the out-of-design-release
/// defence.
fn refuse_installed_window(custody: &Path) -> Result<(), Refusal> {
    match effective_phase(custody)?.as_deref() {
        Some(PHASE_INSTALLED_UNVERIFIED) | Some(PHASE_VERIFIED) => Err(Refusal::new(
            EXIT_CUSTODY_REFUSAL,
            "consolidation.window_not_applyable",
            "the journal already records an installed window; the audit record is never rewritten",
        )),
        _ => Ok(()),
    }
}

/// Entry point for `--mode=consolidate-journal --custody <abs-dir>`. Appends
/// one PHASE record with L3 write-ahead: `<journal>.tmp` created exclusively
/// at mode 0600 (set at creation, never widened later), written, fsynced,
/// renamed over the journal, custody dir fsynced.
pub fn run_journal_write(custody: &Path, phase: &str) -> i32 {
    match journal_write(custody, phase) {
        Ok(()) => 0,
        Err(refusal) => emit_refusal(&refusal),
    }
}

fn journal_write(custody: &Path, phase: &str) -> Result<(), Refusal> {
    journal_write_record(custody, &json!({ "phase": phase }))
}

fn journal_write_record(custody: &Path, record: &Value) -> Result<(), Refusal> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let io = |detail: String| Refusal::new(EXIT_STORE_OR_IO, "consolidation.journal_io", detail);
    ensure_custody(custody)?;
    // The journal carries its whole history: append one chained line and
    // rewrite the file through the tmp + rename (L3). seq and prev_hash link
    // each line to the one before it.
    let journal = custody.join(JOURNAL_FILE);
    let history = std::fs::read_to_string(&journal).unwrap_or_default();
    let (seq, prev_hash) = match history.lines().rev().find(|line| !line.trim().is_empty()) {
        Some(last) => {
            let last: Value = serde_json::from_str(last)
                .map_err(|error| io(format!("history unparseable: {error}")))?;
            (
                last.get("seq").and_then(Value::as_u64).unwrap_or(0) + 1,
                last.get("record_hash")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )
        }
        None => (1, String::new()),
    };
    let mut body = record.clone();
    body["seq"] = json!(seq);
    body["prev_hash"] = json!(prev_hash);
    let line = body.to_string();
    let record_hash = mem_core::sha256::hex(&mem_core::sha256::digest(line.as_bytes()));
    body["record_hash"] = json!(record_hash);
    let tmp = custody.join(format!("{JOURNAL_FILE}.tmp"));
    // Exclusive create at 0600: the mode is set at creation, never widened.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(|error| io(format!("create {}: {error}", tmp.display())))?;
    let mut text = history;
    text.push_str(&format!("{body}\n"));
    file.write_all(text.as_bytes())
        .map_err(|error| io(format!("write {}: {error}", tmp.display())))?;
    file.sync_all()
        .map_err(|error| io(format!("fsync {}: {error}", tmp.display())))?;
    drop(file);
    // Test-only kill seam: the `.tmp` creation window, before the rename.
    kill_at("journal-tmp");
    std::fs::rename(&tmp, &journal)
        .map_err(|error| io(format!("rename into {}: {error}", journal.display())))?;
    std::fs::File::open(custody)
        .and_then(|dir| dir.sync_all())
        .map_err(|error| io(format!("fsync custody: {error}")))
}

// ---------------------------------------------------------------------------
// Step 7 — install (L4/L5) and recovery (§H.1, EV-M). Row 15.
// ---------------------------------------------------------------------------

/// The journal-phase string for a prepared window: the auxiliary sidecar MAY
/// be absent there, and that absence is NOT a 77.
const PHASE_PREPARED: &str = "Prepared";
/// The journal-phase string after the snapshot is verified: still before
/// `preflight` returns, so the sidecar MAY be absent.
const PHASE_SNAPSHOT_VERIFIED: &str = "SnapshotVerified";
/// The journal-phase string for a durable install intent (§L L5 licence).
const PHASE_INSTALLING: &str = "Installing";
/// The journal-phase string after L5 completes.
const PHASE_INSTALLED_UNVERIFIED: &str = "InstalledUnverified";
/// The journal-phase string for a verified staged tree: the install licence
/// (R2) and the operand settlement compares the installed S against.
const PHASE_STAGED_VERIFIED: &str = "StagedVerified";
/// The journal-phase string after settlement accepts the installed tree.
const PHASE_VERIFIED: &str = "Verified";
/// The journal-phase string for an operator-authorised restore in progress.
const PHASE_ROLLING_BACK: &str = "RollingBack";
/// The journal-phase string for a completed restore (terminal).
const PHASE_ROLLED_BACK: &str = "RolledBack";

/// EV-M (rev 11, F43): the custody dir must be 0700 and every journal artifact
/// 0600 before any journal write. Divergence refuses 77 — never a silent proceed
/// on a wedged dir, and the reservation is never auto-reclaimed.
fn ev_m_preflight(custody: &Path) -> Result<(), Refusal> {
    use std::os::unix::fs::PermissionsExt;
    if !custody.is_dir() {
        return Ok(());
    }
    let dir_mode = std::fs::metadata(custody)
        .map_err(|error| custody_refusal(format!("custody stat: {error}")))?
        .permissions()
        .mode()
        & 0o7777;
    if dir_mode != 0o700 {
        return Err(custody_refusal(format!(
            "custody dir mode {dir_mode:o} is not 0700 (EV-M)"
        )));
    }
    let journal = custody.join(JOURNAL_FILE);
    if journal.exists() {
        let mode = std::fs::metadata(&journal)
            .map_err(|error| custody_refusal(format!("journal stat: {error}")))?
            .permissions()
            .mode()
            & 0o7777;
        if mode != 0o600 {
            return Err(custody_refusal(format!(
                "journal mode {mode:o} is not 0600 (EV-M)"
            )));
        }
    }
    // §C.5 rev 13: the auxiliary sidecar joins the enumerated artifact list,
    // but its presence is mandatory only from `StagedVerified` onward — it is
    // written after `preflight` returns, so a journal still at `Prepared` or
    // `SnapshotVerified` may legitimately have none. The phase read is LENIENT
    // here: a corrupt journal is the recover path's own failure to archive, not
    // an EV-M mode refusal.
    if let Some(phase) = effective_phase(custody).ok().flatten()
        && phase != PHASE_PREPARED
        && phase != PHASE_SNAPSHOT_VERIFIED
    {
        let sidecar = custody.join(AUX_SIDECAR);
        let mode = std::fs::metadata(&sidecar)
            .map_err(|error| {
                custody_refusal(format!(
                    "aux sidecar {AUX_SIDECAR} unreadable at phase {phase}: {error} (EV-M)"
                ))
            })?
            .permissions()
            .mode()
            & 0o7777;
        if mode != 0o600 {
            return Err(custody_refusal(format!(
                "aux sidecar mode {mode:o} is not 0600 at phase {phase} (EV-M)"
            )));
        }
    }
    Ok(())
}

/// The test-only kill seam: `OPENCODE_MEM_CONSOLIDATE_KILL_AT=<boundary>`
/// aborts the process at that named boundary (SIGABRT). Never a CLI surface.
fn kill_at(boundary: &str) {
    if std::env::var("OPENCODE_MEM_CONSOLIDATE_KILL_AT").as_deref() == Ok(boundary) {
        std::process::abort();
    }
}

/// `--mode=consolidate-install --custody <dir> --staged <dir>`: L3 `Installing`
/// durable → L4 retire S → (kill seam `after-l4`) → L5 staged→S → fsync.
pub fn run_install(storage_root: &Path, custody: &Path, staged: &Path) -> i32 {
    match install(storage_root, custody, staged) {
        Ok(()) => 0,
        Err(refusal) => emit_refusal(&refusal),
    }
}

fn install(storage_root: &Path, custody: &Path, staged: &Path) -> Result<(), Refusal> {
    let io = |detail: String| Refusal::new(EXIT_STORE_OR_IO, "consolidation.install_io", detail);
    ev_m_preflight(custody)?;
    // R2 (§H.1): an install is licensed ONLY by a durable `StagedVerified`
    // record naming THIS staged tree and THIS root, and the staged manifest that
    // record's digest covers must still match the tree on disk.
    let record = last_journal_record(custody)?
        .ok_or_else(|| custody_refusal("install requires a StagedVerified journal record"))?;
    check_journal_paths(&record, custody)?;
    if record.get("phase").and_then(Value::as_str) != Some(PHASE_STAGED_VERIFIED) {
        return Err(custody_refusal(
            "install requires the effective phase StagedVerified",
        ));
    }
    let recorded_root = record
        .get("storage_root")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if Path::new(recorded_root) != storage_root {
        return Err(custody_refusal(
            "the staged-verify record names a different storage_root",
        ));
    }
    let recorded_staged = record
        .get("staged")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if Path::new(recorded_staged) != staged {
        return Err(custody_refusal(
            "the staged-verify record names a different staged tree",
        ));
    }
    let recorded_digest = record
        .get("staged_manifest_sha256")
        .and_then(Value::as_str)
        .ok_or_else(|| custody_refusal("staged-verify record names no staged manifest digest"))?
        .to_string();
    // §C.1b install two-side rule: the verified staged manifest is M1-built and
    // stored beside the custody (0600, never hashed into any manifest), and its
    // digest is durable in the `Installing` record BEFORE L4 — so settlement can
    // compare the installed S against what was verified, never against the
    // pre-consolidation snapshot (opsec-2 F-2a).
    let staged_name = staged
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io("staged tree has no name".to_string()))?;
    let staged_manifest = manifest_of(staged)?;
    let staged_digest = sha256_hex(staged_manifest.as_bytes());
    if staged_digest != recorded_digest {
        return Err(custody_refusal(
            "the staged tree changed since it was verified",
        ));
    }
    journal_install_intent(custody, storage_root, staged, &staged_digest)?;
    write_artifact(
        &custody.join(format!("{staged_name}.manifest.sha256")),
        staged_manifest.as_bytes(),
    )?;
    write_artifact(
        &custody.join(format!("{staged_name}.manifest.sha256.excluded")),
        excluded_sidecar_of(staged)?.as_bytes(),
    )?;

    // L4: retire the live S under the recovery dir (no-replace: the target is
    // a fresh `consolidation-retired-*` name; existence refuses).
    let recovery = custody
        .parent()
        .ok_or_else(|| io("custody has no parent".to_string()))?;
    let retired = recovery.join(format!(
        "consolidation-retired-{}",
        custody
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("nonce")
    ));
    if retired.exists() {
        return Err(custody_refusal("retired target already exists"));
    }
    std::fs::rename(storage_root, &retired).map_err(|error| io(format!("L4 retire S: {error}")))?;
    kill_at("after-l4");

    // L5: install the staged tree as S (licensed by the durable `Installing`).
    std::fs::rename(staged, storage_root)
        .map_err(|error| io(format!("L5 install staged: {error}")))?;
    journal_write_record(
        custody,
        &json!({
            "phase": PHASE_INSTALLED_UNVERIFIED,
            "staged_manifest_sha256": staged_digest,
            "staged": staged.to_string_lossy(),
            "storage_root": storage_root.to_string_lossy(),
        }),
    )
}

/// `--mode=consolidate-staged-verify --custody <dir> --staged <dir>`: computes
/// the staged tree's M1 manifest and records `StagedVerified` with its digest.
/// This is the licence the install requires (R2) and the operand settlement
/// compares the installed S against.
pub fn run_staged_verify(storage_root: &Path, custody: &Path, staged: &Path) -> i32 {
    match staged_verify(storage_root, custody, staged) {
        Ok(()) => 0,
        Err(refusal) => emit_refusal(&refusal),
    }
}

fn staged_verify(storage_root: &Path, custody: &Path, staged: &Path) -> Result<(), Refusal> {
    if !staged.is_dir() {
        return Err(custody_refusal("staged tree is not a directory"));
    }
    let digest = sha256_hex(manifest_of(staged)?.as_bytes());
    journal_write_record(
        custody,
        &json!({
            "phase": PHASE_STAGED_VERIFIED,
            "storage_root": storage_root.to_string_lossy(),
            "staged": staged.to_string_lossy(),
            "staged_manifest_sha256": digest,
        }),
    )
}

/// The last non-empty journal record, chain-verified; `None` when no journal
/// exists. A broken chain refuses here rather than being read as a guess.
fn last_journal_record(custody: &Path) -> Result<Option<Value>, Refusal> {
    let journal = custody.join(JOURNAL_FILE);
    if !journal.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&journal)
        .map_err(|error| custody_refusal(format!("journal read: {error}")))?;
    verify_record_chain(&text)?;
    let Some(last) = text.lines().rev().find(|line| !line.trim().is_empty()) else {
        return Err(custody_refusal("journal is empty"));
    };
    let record = serde_json::from_str(last)
        .map_err(|error| custody_refusal(format!("journal unparseable: {error}")))?;
    Ok(Some(record))
}

/// `--mode=consolidate-recover --custody <dir>`: EV-M first; a journal at
/// `Installing` with S absent and the staged tree present completes L5 and
/// records `InstalledUnverified`. Nothing else is guessed.
pub fn run_recover(custody: &Path) -> i32 {
    match recover(custody) {
        Ok(()) => 0,
        Err(refusal) => emit_refusal(&refusal),
    }
}

fn recover(custody: &Path) -> Result<(), Refusal> {
    ev_m_preflight(custody)?;
    let journal = custody.join(JOURNAL_FILE);
    if !journal.exists() {
        return Ok(());
    }
    let text = std::fs::read_to_string(&journal)
        .map_err(|error| custody_refusal(format!("journal read: {error}")))?;
    if let Err(refusal) = verify_record_chain(&text) {
        archive_corrupt_journal(custody)?;
        return Err(refusal);
    }
    let last = text
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default();
    let record: Value = match serde_json::from_str(last) {
        Ok(record) => record,
        Err(error) => {
            archive_corrupt_journal(custody)?;
            return Err(custody_refusal(format!("journal unparseable: {error}")));
        }
    };
    if let (Some(recorded), Ok(expected)) = (
        record.get("baseline_sha256").and_then(Value::as_str),
        std::fs::read_to_string(custody.join(BASELINE)),
    ) && recorded != expected.trim()
    {
        return Err(custody_refusal(
            "stale baseline: journal baseline_sha256 differs from baseline.sha256",
        ));
    }
    check_journal_paths(&record, custody)?;
    if record.get("phase").and_then(Value::as_str) == Some(PHASE_INSTALLED_UNVERIFIED) {
        return Ok(());
    }
    if record.get("phase").and_then(Value::as_str) == Some(PHASE_INSTALLING) {
        // L5 is licensed by the durable `Installing` record, which names the
        // exact staged and storage paths. Complete it only when S is absent and
        // the staged tree is present (the kill-between-renames shape).
        let storage = record
            .get("storage_root")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .ok_or_else(|| custody_refusal("Installing record names no storage_root"))?;
        let staged = record
            .get("staged")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .ok_or_else(|| custody_refusal("Installing record names no staged tree"))?;
        if storage.exists() || !staged.exists() {
            return Err(custody_refusal("Installing shape is not the L4-only state"));
        }
        let digest = record
            .get("staged_manifest_sha256")
            .and_then(Value::as_str)
            .ok_or_else(|| custody_refusal("Installing record names no staged manifest digest"))?
            .to_string();
        std::fs::rename(&staged, &storage)
            .map_err(|error| custody_refusal(format!("recover L5: {error}")))?;
        return journal_write_record(
            custody,
            &json!({
                "phase": PHASE_INSTALLED_UNVERIFIED,
                "staged_manifest_sha256": digest,
                "staged": record["staged"],
                "storage_root": record["storage_root"],
            }),
        );
    }
    if record.get("phase").and_then(Value::as_str) == Some(PHASE_ROLLING_BACK) {
        // F-2c: classify a restore in progress from disk evidence only.
        let storage = record
            .get("storage_root")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .ok_or_else(|| custody_refusal("RollingBack record names no storage_root"))?;
        let table = std::fs::read_to_string(custody.join(MODE_TABLE))
            .map_err(|error| custody_refusal(format!("mode-table unreadable: {error}")))?;
        let modes = parse_mode_table(&table)?;
        let rolled_back = json!({
            "phase": PHASE_ROLLED_BACK,
            "staged_manifest_sha256": record["staged_manifest_sha256"],
            "staged": record["staged"],
            "storage_root": record["storage_root"],
        });
        // (ζ) the restore completed: S is byte/mode-equal to the snapshot.
        if restore_matches_snapshot(&storage, custody, &modes)? {
            return journal_write_record(custody, &rolled_back);
        }
        // (γ) the crash landed between the two renames: S is retired and the
        // restore tree holds the snapshot. Install it (no-replace), then record.
        let restore = storage.with_extension("restore-tmp");
        if !storage.exists()
            && restore.is_dir()
            && manifest_of(&restore)? == manifest_of(&custody.join(SNAP_DATA))?
        {
            std::fs::rename(&restore, &storage)
                .map_err(|error| custody_refusal(format!("recover restore: {error}")))?;
            return journal_write_record(custody, &rolled_back);
        }
        return Err(custody_refusal(
            "RollingBack shape is not classifiable from disk",
        ));
    }
    Err(custody_refusal(
        "journal phase requires an operator decision",
    ))
}

/// L3 write-ahead for the install: the `Installing` record names the exact
/// storage and staged paths that L5 will rename, and the verified staged
/// manifest's digest, so recovery and settlement never guess.
fn journal_install_intent(
    custody: &Path,
    storage: &Path,
    staged: &Path,
    staged_digest: &str,
) -> Result<(), Refusal> {
    journal_write_record(
        custody,
        &json!({
            "phase": PHASE_INSTALLING,
            "storage_root": storage.to_string_lossy(),
            "staged": staged.to_string_lossy(),
            "staged_manifest_sha256": staged_digest,
        }),
    )
}

/// Moves an unusable journal aside to `recovery/consolidation-corrupt-<UTC>-<nonce>`
/// so it is never trusted or re-read, and keeps the custody held (77 follows).
fn archive_corrupt_journal(custody: &Path) -> Result<(), Refusal> {
    let recovery = custody
        .parent()
        .ok_or_else(|| custody_refusal("custody has no parent"))?;
    let nonce = custody
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("nonce");
    // Epoch seconds: unique and sortable; no clock crate is added to the root.
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let target = recovery.join(format!("consolidation-corrupt-{stamp}-{nonce}"));
    std::fs::rename(custody.join(JOURNAL_FILE), &target)
        .map_err(|error| custody_refusal(format!("archive-aside: {error}")))
}

/// Interior record-chain check: every line is a record whose `seq` increments
/// from 1 and whose `prev_hash` equals the previous line's `record_hash`.
fn verify_record_chain(text: &str) -> Result<(), Refusal> {
    let mut prev_hash = String::new();
    for (index, line) in text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .enumerate()
    {
        let record: Value = serde_json::from_str(line)
            .map_err(|error| custody_refusal(format!("chain record unparseable: {error}")))?;
        if record.get("seq").and_then(Value::as_u64) != Some(index as u64 + 1) {
            return Err(custody_refusal("journal chain seq break"));
        }
        if record
            .get("prev_hash")
            .and_then(Value::as_str)
            .unwrap_or_default()
            != prev_hash
        {
            return Err(custody_refusal("journal chain prev_hash break"));
        }
        prev_hash = record
            .get("record_hash")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
    }
    Ok(())
}

/// Every path a journal record names must sit inside the storage root family
/// (its parent). An escaping path refuses 77 and is never followed.
fn check_journal_paths(record: &Value, custody: &Path) -> Result<(), Refusal> {
    let family = custody
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| custody_refusal("custody has no root family"))?;
    for key in ["storage_root", "staged"] {
        if let Some(path) = record.get(key).and_then(Value::as_str) {
            let path = Path::new(path);
            let inside = path.is_absolute()
                && path
                    .components()
                    .all(|component| !matches!(component, Component::ParentDir))
                && path.starts_with(family);
            if !inside {
                return Err(custody_refusal(format!("{key} escapes the root family")));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Step 8 — rollback (§H.2 restore) and settlement verify (§J). W7 rows 17–19.
// ---------------------------------------------------------------------------

/// `--mode=consolidate-rollback --custody <dir>`: rebuild S from the frozen
/// snapshot. The snapshot is rehashed against the baseline BEFORE any rename;
/// the current (partial) S is moved aside under `recovery/`, never deleted.
pub fn run_rollback(storage_root: &Path, custody: &Path) -> i32 {
    match rollback(storage_root, custody) {
        Ok(()) => 0,
        Err(refusal) => emit_refusal(&refusal),
    }
}

fn rollback(storage_root: &Path, custody: &Path) -> Result<(), Refusal> {
    // Verify the frozen snapshot against its baseline before touching S.
    verify_snapshot(custody)?;
    // R1-i: a restore is licensed only by a journal whose effective phase is an
    // install in flight or a restore already begun. An absent journal, `Verified`
    // and any other phase refuse 77 — a rollback row is never invented.
    let record = last_journal_record(custody)?
        .ok_or_else(|| custody_refusal("rollback requires a journal"))?;
    check_journal_paths(&record, custody)?;
    let phase = record
        .get("phase")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if phase != PHASE_INSTALLED_UNVERIFIED
        && phase != PHASE_ROLLING_BACK
        && phase != PHASE_ROLLED_BACK
    {
        return Err(custody_refusal(
            "rollback requires an install in flight or a begun restore",
        ));
    }
    let recorded_root = record
        .get("storage_root")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if Path::new(recorded_root) != storage_root {
        return Err(custody_refusal(
            "the journal names a different storage_root",
        ));
    }
    let data = custody.join(SNAP_DATA);
    // The clone-time `(mode, kind)` table is the restore oracle for modes: the
    // frozen tree carries post-`a-w` modes, so it can never be the source.
    let table = std::fs::read_to_string(custody.join(MODE_TABLE))
        .map_err(|error| custody_refusal(format!("mode-table unreadable: {error}")))?;
    let modes = parse_mode_table(&table)?;
    let carried = |next: &str| {
        json!({
            "phase": next,
            "staged_manifest_sha256": record["staged_manifest_sha256"],
            "staged": record["staged"],
            "storage_root": record["storage_root"],
        })
    };
    // Idempotent: S already equals the snapshot in bytes and in every mode the
    // captured table records — the root included, since the table's first line
    // is the S root itself. Nothing is restored, so the journal is only brought
    // to its terminal phase (and a completed restore stays an accepted no-op).
    if restore_matches_snapshot(storage_root, custody, &modes)? {
        if phase == PHASE_ROLLED_BACK {
            return Ok(());
        }
        return journal_write_record(custody, &carried(PHASE_ROLLED_BACK));
    }
    let recovery = custody
        .parent()
        .ok_or_else(|| custody_refusal("custody has no parent"))?;
    let nonce = custody
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("nonce");
    let failed = recovery.join(format!("consolidation-failed-{nonce}"));
    if failed.exists() {
        return Err(custody_refusal("failed-tree target already exists"));
    }
    // Build the restore tree beside S from the snapshot, then swap it in.
    let restore = storage_root.with_extension("restore-tmp");
    if restore.exists() {
        return Err(custody_refusal("restore staging path already exists"));
    }
    let tree = tree_of(&data)?;
    copy_snapshot_tree(&data, &restore, &tree)?;
    apply_mode_table(&restore, &modes)?;
    if manifest_of(&restore)? != manifest_of(&data)? {
        return Err(custody_refusal("restored tree differs from snapshot"));
    }
    // F-2c: write-ahead BEFORE the first rename, so a crash between the renames
    // is classified by recovery from disk evidence instead of guessed.
    journal_write_record(custody, &carried(PHASE_ROLLING_BACK))?;
    if storage_root.exists() {
        std::fs::rename(storage_root, &failed)
            .map_err(|error| custody_refusal(format!("keep failed tree: {error}")))?;
    }
    kill_at("rollback-after-failed");
    std::fs::rename(&restore, storage_root)
        .map_err(|error| custody_refusal(format!("install restore: {error}")))?;
    journal_write_record(custody, &carried(PHASE_ROLLED_BACK))
}

/// True when `tree` matches the frozen snapshot in bytes and in every `(mode,
/// kind)` the clone-time table records: the shape a completed restore has.
fn restore_matches_snapshot(
    tree: &Path,
    custody: &Path,
    modes: &BTreeMap<String, (u32, TableKind)>,
) -> Result<bool, Refusal> {
    if !tree.is_dir() || manifest_of(tree)? != manifest_of(&custody.join(SNAP_DATA))? {
        return Ok(false);
    }
    let live = mode_map_of(tree)?;
    Ok(modes.iter().all(|(rel, want)| live.get(rel) == Some(want)))
}

/// `--mode=consolidate-verify --custody <dir>`: settlement. Recomputes every
/// final artifact from disk; a persisted flag or vector is never trusted.
pub fn run_verify(storage_root: &Path, custody: &Path) -> i32 {
    match settle(storage_root, custody) {
        Ok(()) => 0,
        Err(refusal) => emit_refusal(&refusal),
    }
}

fn settle(storage_root: &Path, custody: &Path) -> Result<(), Refusal> {
    // A persisted final vector, when present, must equal a recomputation from
    // the snapshot, never be trusted as written.
    let vector = custody.join("final.vector");
    if vector.exists() {
        let recomputed = mem_core::sha256::hex(&mem_core::sha256::digest(
            manifest_of(&custody.join(SNAP_DATA))?.as_bytes(),
        ));
        let persisted = std::fs::read_to_string(&vector)
            .map_err(|error| custody_refusal(format!("vector unreadable: {error}")))?;
        if persisted.trim() != recomputed {
            return Err(custody_refusal(
                "persisted vector differs from recomputed artifact",
            ));
        }
    }
    verify_snapshot(custody)?;
    // With NO journal the live tree must still equal the snapshot: that is the
    // pre-install post-manifest-edit check.
    let Some(record) = last_journal_record(custody)? else {
        if storage_root.is_dir()
            && manifest_of(storage_root)? != manifest_of(&custody.join(SNAP_DATA))?
        {
            return Err(custody_refusal(
                "post-manifest source edit: live tree differs from baseline",
            ));
        }
        return Ok(());
    };
    // With a journal, settlement compares the installed S against the VERIFIED
    // STAGED manifest recorded at install — never against the pre-consolidation
    // snapshot, which a genuine install changes by design (opsec-2 F-2b).
    check_journal_paths(&record, custody)?;
    let recorded_root = record
        .get("storage_root")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if Path::new(recorded_root) != storage_root {
        return Err(custody_refusal("journal names a different storage_root"));
    }
    let phase = record
        .get("phase")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if phase == PHASE_INSTALLING {
        return Err(custody_refusal("install not complete: L5 has not run"));
    }
    if phase != PHASE_INSTALLED_UNVERIFIED && phase != PHASE_VERIFIED {
        return Err(custody_refusal("journal is not in a settled phase"));
    }
    let digest = record
        .get("staged_manifest_sha256")
        .and_then(Value::as_str)
        .ok_or_else(|| custody_refusal("record names no staged manifest digest"))?
        .to_string();
    let staged_name = record
        .get("staged")
        .and_then(Value::as_str)
        .map(Path::new)
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .ok_or_else(|| custody_refusal("record names no staged tree"))?;
    let staged_manifest =
        std::fs::read_to_string(custody.join(format!("{staged_name}.manifest.sha256")))
            .map_err(|error| custody_refusal(format!("staged manifest unreadable: {error}")))?;
    if sha256_hex(staged_manifest.as_bytes()) != digest {
        return Err(custody_refusal(
            "staged manifest differs from its recorded digest",
        ));
    }
    if !storage_root.is_dir() {
        return Err(custody_refusal("installed storage root is absent"));
    }
    if manifest_of(storage_root)? != staged_manifest {
        if phase == PHASE_INSTALLED_UNVERIFIED {
            // §H.1: fail → `RollingBack`. The restore itself stays an explicit
            // operator command; settlement never rolls back on its own.
            journal_write_record(
                custody,
                &json!({
                    "phase": PHASE_ROLLING_BACK,
                    "staged_manifest_sha256": digest,
                    "staged": record["staged"],
                    "storage_root": record["storage_root"],
                }),
            )?;
        }
        return Err(custody_refusal(
            "post-install source edit: installed S differs from the verified staged manifest",
        ));
    }
    if phase == PHASE_INSTALLED_UNVERIFIED {
        journal_write_record(
            custody,
            &json!({
                "phase": PHASE_VERIFIED,
                "staged_manifest_sha256": digest,
                "staged": record["staged"],
                "storage_root": record["storage_root"],
            }),
        )?;
    }
    Ok(())
}

/// C3 (opsec carry-forward, Step 10 harness-side): a rehearsal run's
/// `storage_root` must resolve inside its scratch tree. A mis-aimed root
/// refuses here, exit 64, before any reservation or dispatch.
pub fn rehearsal_scratch_assert(scratch: &Path, storage_root: &Path) {
    let inside = match (
        std::fs::canonicalize(scratch),
        std::fs::canonicalize(storage_root),
    ) {
        (Ok(scratch), Ok(root)) => root.starts_with(&scratch),
        // An absent root cannot be inside the scratch tree either way.
        _ => false,
    };
    if !inside {
        eprintln!(
            "consolidate-shards: storage_root is outside the rehearsal scratch tree; refusing"
        );
        std::process::exit(EXIT_PLAN_INVALID);
    }
}

#[cfg(test)]
mod mode_table_format_tests {
    use super::{TableKind, parse_mode_table};

    /// Both on-disk formats parse to the same map: the production writer's
    /// (`dir`/`file`, bare paths, root line as the lone `.`) and the external
    /// window script's (`d`/`f`, `./`-prefixed paths, root line `.`).
    #[test]
    fn both_mode_table_formats_parse_to_one_map() {
        let written = "750 dir .\n644 file keep.txt\n600 file .auth-token\n";
        let window = "755 d .\n755 d ./projects\n644 f ./keep.txt\n600 f ./.auth-token\n";
        let from_writer = parse_mode_table(written).expect("writer format parses");
        let from_window = parse_mode_table(window).expect("window format parses");
        // The writer's first line is the S root itself.
        assert_eq!(from_writer.get(""), Some(&(0o750, TableKind::Dir)));
        assert_eq!(from_writer.get("keep.txt"), Some(&(0o644, TableKind::File)));
        assert_eq!(from_window.get(""), Some(&(0o755, TableKind::Dir)));
        assert_eq!(from_window.get("projects"), Some(&(0o755, TableKind::Dir)));
        assert_eq!(from_window.get("keep.txt"), Some(&(0o644, TableKind::File)));
        assert_eq!(
            from_window.get(".auth-token"),
            Some(&(0o600, TableKind::File))
        );
        // A root-less table (an artifact written before the root line) still
        // parses; the root simply has no recorded mode.
        let legacy = parse_mode_table("644 file keep.txt\n").expect("legacy parses");
        assert_eq!(legacy.get(""), None);
        assert!(
            parse_mode_table("755 x .\n").is_err(),
            "unknown kind refuses"
        );
        assert!(parse_mode_table("nope f .\n").is_err(), "bad octal refuses");
    }
}

#[cfg(test)]
mod shard_shape_tests {
    use super::{EXIT_UNSUPPORTED_SHAPE, block_on, read_source};
    use mem_store::db::{ConnectFactory, ConnectOptions, TursoConnectFactory};
    use std::path::Path;

    /// The accepted shape's DDL — v3's 18 names plus the nullable `deleted_at`,
    /// the six indexes, `shard_metadata` (with the embedding identity rows a
    /// populated shard carries) and `shard_stats` — and ONE row whose
    /// `deleted_at` the caller pins. This is the minimum `read_source` needs to
    /// reach its `SELECT *` row loop.
    fn shape_statements(deleted_at: &str) -> Vec<String> {
        let mut statements: Vec<String> = vec![
            mem_store::sql::memories_ddl(2),
            mem_store::sql::shard_metadata_ddl().to_string(),
        ];
        statements.extend(
            mem_store::sql::MEMORIES_INDEXES
                .iter()
                .map(|sql| sql.to_string()),
        );
        statements.push(mem_store::sql::MEMORY_SHARD_V2_ALTER.to_string());
        statements.push(mem_store::sql::MEMORY_SHARD_V2_INDEX.to_string());
        statements.push(mem_store::shard_facts::SHARD_STATS_DDL.to_string());
        statements.push("ALTER TABLE memories ADD COLUMN deleted_at INTEGER".to_string());
        statements.push("CREATE INDEX idx_deleted_at ON memories(deleted_at)".to_string());
        statements.push(
            "INSERT INTO shard_metadata (key, value) VALUES \
             ('embedding_dimensions', '2'), ('embedding_model', 'test-model')"
                .to_string(),
        );
        statements.push(format!(
            "INSERT INTO memories (id, content, vector, container_tag, created_at, updated_at, \
             deleted_at) VALUES ('carried-row', 'content', vector32('[0.1, 0.2]'), \
             'opencode_project_fixture', 1700000000000, 1700000000000, {deleted_at})"
        ));
        statements.push("PRAGMA user_version = 3".to_string());
        statements
    }

    fn build_shard(path: &Path, deleted_at: &str) {
        let handle =
            block_on(TursoConnectFactory.connect(path.to_path_buf(), ConnectOptions::default()))
                .expect("create the fixture shard");
        for sql in shape_statements(deleted_at) {
            block_on(handle.execute(&sql, Vec::new()))
                .unwrap_or_else(|error| panic!("fixture DDL failed ({sql}): {error}"));
        }
    }

    /// §A.1 (h) — the ONLY fixture that reaches seam (ii). `read_source` must
    /// refuse a FETCHED row carrying a non-NULL `deleted_at` BEFORE the
    /// `MEMORIES_V4` projection (after it the column is absent forever and the
    /// row is silently dropped), and must carry its all-NULL counterpart with no
    /// `deleted_at` key in the typed row. Both shards carry `shard_metadata`
    /// rows, so the refusal cannot come from a missing identity table.
    #[test]
    fn test_read_source_refuses_non_null_deleted_at() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or(0);
        let dir =
            std::env::temp_dir().join(format!("shard-read-source-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("fixture dir");

        let refused = dir.join("refused.db");
        build_shard(&refused, "1700000000001");
        let refusal = match read_source(&refused) {
            Ok(_) => panic!("a non-NULL deleted_at must refuse"),
            Err(refusal) => refusal,
        };
        assert_eq!(refusal.code, EXIT_UNSUPPORTED_SHAPE);
        assert_eq!(refusal.event, "consolidation.shard_shape_unsupported");
        assert!(
            refusal.message.contains("non-NULL deleted_at"),
            "distinct message: {}",
            refusal.message
        );

        let carried = dir.join("carried.db");
        build_shard(&carried, "NULL");
        let source = read_source(&carried).expect("an all-NULL deleted_at carries");
        assert_eq!(source.rows.len(), 1, "the row is carried");
        assert!(
            !source.rows[0].contains_key("deleted_at"),
            "the typed row is projected onto MEMORIES_V4 and holds no deleted_at key"
        );
        assert_eq!(
            source.identity.get("embedding_model").map(String::as_str),
            Some("test-model"),
            "the shard_metadata identity is read beside the rows"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
