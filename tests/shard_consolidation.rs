//! Public CLI test seam for shard consolidation (parent blueprint Steps 4–10
//! test table). All consolidation test names live in this one binary; the
//! pinned runner is `--test-threads=1` and every named run uses
//! `cargo test --offline --locked --test shard_consolidation <NAME> --
//!  --exact --nocapture --test-threads=1`.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mem_store::config::StoreConfig;
use mem_store::connection::ConnectionManager;
use mem_store::db::{ConnectFactory, ConnectOptions, TursoConnectFactory, Value};
use mem_store::platform::{Platform, SystemProcessProbe};
use mem_store::scope::Scope;
use mem_store::serde_json;
use mem_store::shard_manager::ShardManager;
use mem_store::vector::{self, MemoryInsert};

/// The reservation file name, at the PARENT of the storage root
/// (`crates/mem-store/src/maintenance_reservation.rs`). Mirrored here so a
/// test that renames the constant fails loudly instead of silently fencing
/// nothing.
const RESERVATION_FILE: &str = ".opencode-mem-maintenance.lock";

/// Fresh unique temp dir (hermeticity: `std::env::temp_dir()` only,
/// docs/agents/testing.md).
fn unique_temp_dir(prefix: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "shard-consolidation-{prefix}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Spawns the real engine binary in stdio mode against `root`, with the
/// process-global file log redirected into the fixture parent (never the
/// developer's real log).
fn spawn_engine_stdio(root: &Path, log_file: &Path) -> Child {
    Command::new(env!("CARGO_BIN_EXE_opencode-agents-memory"))
        .arg(format!("--storage-root={}", root.display()))
        .arg("--mode=stdio")
        .env("OPENCODE_MEM_LOG_FILE", log_file)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("engine child spawns")
}

/// Waits for exit within `bound`; kills + reaps on the deadline so a hung
/// fence can never wedge the suite.
fn wait_for_exit(child: &mut Child, bound: Duration) -> std::process::ExitStatus {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait().expect("try_wait") {
            return status;
        }
        if started.elapsed() >= bound {
            let _ = child.kill();
            panic!("engine child did not exit within {bound:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn child_stderr(child: &mut Child) -> String {
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    stderr
}

/// The exit-76 structured line carries ONLY `operation` + `root_basename` —
/// the F11 leak rule: nonce, start_time token and pid never leave the body.
fn assert_reservation_refusal_line(stderr: &str, root_basename: &str, operation: &str) {
    let line = stderr
        .lines()
        .find(|line| line.contains("maintenance.reservation_held"))
        .unwrap_or_else(|| panic!("no maintenance.reservation_held line in {stderr}"));
    let payload: serde_json::Value =
        serde_json::from_str(line).unwrap_or_else(|error| panic!("refusal line not JSON: {error}"));
    let object = payload.as_object().expect("refusal line is a JSON object");
    for key in object.keys() {
        assert!(
            matches!(
                key.as_str(),
                "component" | "event" | "level" | "operation" | "root_basename"
            ),
            "the exit-76 line leaked field {key:?}: {line}"
        );
    }
    assert_eq!(object["component"], "maintenance");
    assert_eq!(object["operation"], operation);
    assert_eq!(object["root_basename"], root_basename);
}

/// The fence (Step 2): a held reservation at the PARENT of the storage root
/// refuses engine startup with exit 76 BEFORE `acquire_engine_lock`'s
/// `create_dir_all` — the storage directory is never created, the foreign
/// reservation is never unlinked, and the structured line leaks nothing.
///
/// Mutant killed: the check placed after `acquire_outcome` — the directory
/// would exist by the time the child exits, failing the `!root.exists()`
/// assertion. C2 folds: a 0-byte reservation body is fail-closed Held, and
/// `acquire` creates the body at mode 0600.
#[test]
fn test_reservation_checked_before_directory_creation() {
    // Case 1 — parseable foreign holder (live pid: this test process).
    let parent = unique_temp_dir("reservation-fence");
    let root = parent.join("data");
    let reservation = parent.join(RESERVATION_FILE);
    let body = serde_json::json!({
        "pid": std::process::id(),
        "start_time": 1_u64,
        "operation": "maintenance",
        "root_basename": "data",
        "nonce": "AAAAAAAAAAAAAAAAAAAAAA==",
        "created_utc": "2026-11-12T00:00:00+00:00",
        "retake_seq": null,
        "s_dev": 1_u64,
        "s_ino": 1_u64,
    });
    std::fs::write(&reservation, body.to_string()).expect("write reservation body");

    let log_file = parent.join("engine.log");
    let mut child = spawn_engine_stdio(&root, &log_file);
    let status = wait_for_exit(&mut child, Duration::from_secs(30));
    let stderr = child_stderr(&mut child);
    assert_eq!(
        status.code(),
        Some(mem_store::maintenance_reservation::RESERVATION_HELD),
        "a held reservation must refuse with 76, got {status}; stderr: {stderr}"
    );
    assert!(
        !root.exists(),
        "the fence must fire before acquire_engine_lock's create_dir_all"
    );
    assert!(
        reservation.exists(),
        "a foreign reservation is never unlinked"
    );
    assert_reservation_refusal_line(&stderr, "data", "engine");
    let _ = std::fs::remove_dir_all(&parent);

    // Case 2 (C2 fold) — a 0-byte reservation body is fail-closed Held:
    // presence alone bars startup, nothing auto-reclaims an unreadable body.
    let parent = unique_temp_dir("reservation-empty-body");
    let root = parent.join("data");
    std::fs::write(parent.join(RESERVATION_FILE), "").expect("write empty body");
    let mut child = spawn_engine_stdio(&root, &parent.join("engine.log"));
    let status = wait_for_exit(&mut child, Duration::from_secs(30));
    assert_eq!(
        status.code(),
        Some(mem_store::maintenance_reservation::RESERVATION_HELD),
        "a 0-byte reservation body must be fail-closed Held, got {status}"
    );
    assert!(
        !root.exists(),
        "the 0-byte body still fences directory creation"
    );
    let _ = std::fs::remove_dir_all(&parent);

    // Case 3 (C2 fold) — acquire writes the body at mode 0600 with the pinned
    // shape (maintenance operation, own pid/token, 16-byte base64 nonce, the
    // storage root's dev+ino identity).
    let parent = unique_temp_dir("reservation-mode");
    let root = parent.join("data");
    std::fs::create_dir_all(&root).expect("storage root for acquire");
    let acquired = mem_store::maintenance_reservation::acquire(&root).expect("acquire");
    assert_eq!(
        acquired.path(),
        parent.join(RESERVATION_FILE),
        "the reservation lives at the parent of the storage root"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(acquired.path())
            .expect("reservation metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "the reservation body is mode 0600");
    }
    let body: mem_store::maintenance_reservation::ReservationBody = serde_json::from_str(
        &std::fs::read_to_string(acquired.path()).expect("read acquired body"),
    )
    .expect("the acquired body parses back");
    assert_eq!(body.operation, "maintenance");
    assert_eq!(body.pid, std::process::id());
    assert_eq!(body.root_basename, "data");
    assert_eq!(
        body.nonce.len(),
        24,
        "16 bytes in standard base64 with padding"
    );
    assert_eq!(
        body.start_time,
        mem_store::platform::process_start_time(std::process::id()),
        "the body carries the holder's start-time token"
    );
    let metadata = std::fs::metadata(&root).expect("root metadata");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(body.s_dev, Some(metadata.dev()));
        assert_eq!(body.s_ino, Some(metadata.ino()));
    }
    // Held-by-self pass-through (F9): the holder's own check proceeds.
    assert_eq!(
        mem_store::maintenance_reservation::check(&root),
        mem_store::maintenance_reservation::ReservationVerdict::HeldBySelf,
        "the holder reads its own reservation as HeldBySelf"
    );
    drop(acquired);
    let _ = std::fs::remove_dir_all(&parent);

    // Case 4 — the four CLI writer dispositions consult the same fence BEFORE
    // their first store open: a held reservation refuses `reap-shards` with 76
    // and names the refusing operation. Mutant killed: a `src/main.rs`
    // dispatch without `refuse_if_reservation_held` (the writer would run
    // against the root and answer some other code).
    let parent = unique_temp_dir("reservation-cli-writer");
    let root = parent.join("data");
    std::fs::write(parent.join(RESERVATION_FILE), "{}").expect("write truncated body");
    let mut child = Command::new(env!("CARGO_BIN_EXE_opencode-agents-memory"))
        .arg(format!("--storage-root={}", root.display()))
        .arg("--mode=reap-shards")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("CLI writer child spawns");
    let status = wait_for_exit(&mut child, Duration::from_secs(30));
    let stderr = child_stderr(&mut child);
    assert_eq!(
        status.code(),
        Some(mem_store::maintenance_reservation::RESERVATION_HELD),
        "a held reservation must refuse the reap-shards writer with 76, got {status}; stderr: {stderr}"
    );
    assert!(!root.exists(), "the refused CLI writer creates nothing");
    assert_reservation_refusal_line(&stderr, "data", "reap-shards");
    let _ = std::fs::remove_dir_all(&parent);
}

// ---------------------------------------------------------------------------
// Fixture harness — Steps 3–10 plan/preflight surface
// ---------------------------------------------------------------------------

fn run_async<F: std::future::Future>(future: F) -> F::Output {
    mem_store::tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build the test runtime")
        .block_on(future)
}

fn fixture_config(root: &Path) -> StoreConfig {
    StoreConfig {
        storage_path: root.to_path_buf(),
        embedding_dimensions: 2,
        embedding_model: "test-model".to_string(),
        max_vectors_per_shard: 1000,
        container_tag_prefix: "opencode".to_string(),
        database_encryption_enabled: false,
        database_encryption_cipher: "aes256gcm".to_string(),
        database_encryption_key: None,
        allow_unguarded_open: false,
    }
}

struct Fixture {
    manager: ShardManager,
}

fn fixture(root: &Path) -> Fixture {
    let config = fixture_config(root);
    let connections = Arc::new(ConnectionManager::new(
        root.to_path_buf(),
        config.clone(),
        Platform::current(),
        Arc::new(TursoConnectFactory),
    ));
    let manager = ShardManager::new(
        root.to_path_buf(),
        config,
        connections,
        Arc::new(SystemProcessProbe),
        None,
    );
    Fixture { manager }
}

fn record(id: &str, tag: &str) -> MemoryInsert {
    MemoryInsert {
        id: id.to_string(),
        content: format!("content {id}"),
        vector: vec![0.1, 0.2],
        tags_vector: None,
        container_tag: tag.to_string(),
        tags: None,
        memory_type: Some("note".to_string()),
        created_at: 1_700_000_000_000,
        updated_at: 1_700_000_000_000,
        metadata: None,
        session_id: None,
        display_name: None,
        user_name: None,
        user_email: None,
        project_path: None,
        project_name: None,
        git_repo_url: None,
        source_prompt_created_at: None,
    }
}

impl Fixture {
    /// One v4 shard (the ladder's top rung) with one row, registered in
    /// `metadata.db` exactly as the engine would leave it.
    fn seed(&self, hash: &str, row_id: &str) {
        let tag = format!("opencode_project_{hash}");
        run_async(async {
            let shard = self
                .manager
                .create_shard(Scope::Project, hash, 0)
                .await
                .expect("create the shard");
            let db = self
                .manager
                .open_shard(&shard.db_path)
                .await
                .expect("open the shard");
            vector::insert_vector(db.as_ref(), &record(row_id, &tag))
                .await
                .expect("insert the row");
            drop(db);
        });
    }
}

/// `sha256(canonical identity)[..16]` — the production TagResolver shorthand,
/// recomputed here from the pinned algorithm rather than trusted from output.
fn identity_hash(identity: &str) -> String {
    mem_core::sha256::hex(&mem_core::sha256::digest(identity.as_bytes()))[..16].to_string()
}

fn plan_row(
    root: &Path,
    source_hash: &str,
    source_identity: &str,
    target_hash: &str,
    target_identity: &str,
    allow_linked_source: bool,
) -> serde_json::Value {
    serde_json::json!({
        "source_hash": source_hash,
        "source_identity": source_identity,
        "source_path": root.join("projects").join(format!("project_{source_hash}_shard_0.db")),
        "target_hash": target_hash,
        "target_identity": target_identity,
        "target_path": root.join("projects").join(format!("project_{target_hash}_shard_0.db")),
        "target_association": {
            "container_tag": format!("opencode_project_{target_hash}"),
            "project_path": target_identity.split_once(':').map(|(_, rest)| rest).unwrap_or_default(),
            "project_name": "FixtureProject",
            "display_name": null,
            "git_repo_url": null,
        },
        "allow_linked_source": allow_linked_source,
    })
}

/// One valid row: the target hash is derived from the target identity (never
/// the source hash — a source hash is a registry key, not an identity hash).
fn valid_row(
    root: &Path,
    source_hash: &str,
    source_identity: &str,
    target_identity: &str,
    allow_linked_source: bool,
) -> serde_json::Value {
    plan_row(
        root,
        source_hash,
        source_identity,
        &identity_hash(target_identity),
        target_identity,
        allow_linked_source,
    )
}

fn write_plan(dir: &Path, root: &Path, mapping: Vec<serde_json::Value>) -> PathBuf {
    std::fs::create_dir_all(dir).expect("plan dir");
    let plan = serde_json::json!({
        "format_version": 1,
        "storage_root": root,
        "source_snapshot_sha256": "0".repeat(64),
        "mapping": mapping,
        "schema_manifest_sha256": "0".repeat(64),
        "baseline_manifest_sha256": "0".repeat(64),
    });
    let path = dir.join("plan.json");
    std::fs::write(&path, plan.to_string()).expect("write the plan");
    path
}

/// Runs one `consolidate-shards` invocation on the REAL binary with a scratch
/// HOME, returning (exit code, stdout, stderr).
///
/// `custody` is the rev-13 APPLY requirement (§C.5): the auxiliary sidecar is a
/// custody artifact. Every dry run stays custody-FREE, so `run_dry_run` passes
/// `None` and a dry run can never be made custody-dependent by accident.
fn run_consolidate(
    root: &Path,
    plan: &Path,
    dry_run: bool,
    custody: Option<&Path>,
) -> (Option<i32>, String, String) {
    let home = root.parent().unwrap_or(root).join("home");
    std::fs::create_dir_all(&home).expect("scratch home");
    if !home.join(".config/opencode/opencode-mem.jsonc").exists() {
        write_fixture_config(&home, 1000);
    }
    let mut command = Command::new(env!("CARGO_BIN_EXE_opencode-agents-memory"));
    command
        .arg(format!("--storage-root={}", root.display()))
        .arg("--mode=consolidate-shards")
        .arg(format!("--plan={}", plan.display()))
        .env("HOME", &home)
        .stdin(Stdio::null());
    if dry_run {
        command.arg("--dry-run");
    } else if let Some(custody) = custody {
        command.arg(format!("--custody={}", custody.display()));
    }
    let output = command.output().expect("consolidate child runs");
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

/// The fixture custody dir an APPLY must be given (§C.5 rev 13). Deliberately
/// not created here: the consolidator creates it at 0700, and a pre-existing
/// dir at any other mode is an EV-M 77.
fn fixture_custody(parent: &Path) -> PathBuf {
    parent.join("recovery").join("consolidation-fixture")
}

/// Runs `--mode=consolidate-shards --plan <plan> --dry-run`.
fn run_dry_run(root: &Path, plan: &Path) -> (Option<i32>, String, String) {
    run_consolidate(root, plan, true, None)
}

fn tree_digest(root: &Path) -> BTreeMap<String, String> {
    let mut digest = BTreeMap::new();
    collect_digest(root, root, &mut digest);
    digest
}

/// `(mode, kind)` per path under `root`, the root itself keyed `""`. This is
/// the oracle for restore fidelity: bytes alone cannot see a mode regression.
fn tree_modes(root: &Path) -> BTreeMap<String, (u32, &'static str)> {
    use std::os::unix::fs::MetadataExt;
    let mut modes = BTreeMap::new();
    let root_meta = std::fs::symlink_metadata(root).expect("fixture root metadata");
    modes.insert(String::new(), (root_meta.mode() & 0o7777, "dir"));
    collect_modes(root, root, &mut modes);
    modes
}

fn collect_modes(root: &Path, dir: &Path, modes: &mut BTreeMap<String, (u32, &'static str)>) {
    use std::os::unix::fs::MetadataExt;
    for entry in std::fs::read_dir(dir).expect("read fixture dir") {
        let path = entry.expect("fixture entry").path();
        let relative = path
            .strip_prefix(root)
            .expect("relative path")
            .to_string_lossy()
            .to_string();
        let metadata = std::fs::symlink_metadata(&path).expect("fixture metadata");
        let kind = if metadata.is_dir() {
            "dir"
        } else if metadata.file_type().is_symlink() {
            "link"
        } else {
            "file"
        };
        modes.insert(relative, (metadata.mode() & 0o7777, kind));
        if metadata.is_dir() {
            collect_modes(root, &path, modes);
        }
    }
}

fn collect_digest(root: &Path, dir: &Path, digest: &mut BTreeMap<String, String>) {
    let entries = std::fs::read_dir(dir).expect("read fixture dir");
    for entry in entries {
        let path = entry.expect("fixture entry").path();
        let relative = path
            .strip_prefix(root)
            .expect("relative path")
            .to_string_lossy()
            .to_string();
        let metadata = std::fs::symlink_metadata(&path).expect("fixture metadata");
        if metadata.is_dir() {
            digest.insert(format!("{relative}/"), "dir".to_string());
            collect_digest(root, &path, digest);
        } else if metadata.file_type().is_symlink() {
            let target = std::fs::read_link(&path).expect("symlink target");
            digest.insert(relative, format!("symlink:{}", target.display()));
        } else {
            let bytes = std::fs::read(&path).expect("fixture bytes");
            digest.insert(
                relative,
                mem_core::sha256::hex(&mem_core::sha256::digest(&bytes)),
            );
        }
    }
}

fn report_json(stdout: &str, stderr: &str) -> serde_json::Value {
    serde_json::from_str(stdout.trim()).unwrap_or_else(|error| {
        panic!("the plan report is not JSON ({error}); stdout: {stdout}; stderr: {stderr}")
    })
}

/// The refusal line: a typed exit code plus a `consolidation.*` structured
/// line naming the refusal. The message is asserted to be specific, never a
/// generic failure.
fn assert_refusal(code: Option<i32>, expected: i32, stderr: &str, needle: &str) {
    assert_eq!(
        code,
        Some(expected),
        "expected exit {expected}; stderr: {stderr}"
    );
    assert!(
        stderr.contains(needle),
        "the refusal must name {needle:?}; stderr: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// Step 3 — mapping and preflight
// ---------------------------------------------------------------------------

/// The plan's source-hash SET must be exactly the shard set on disk (count
/// equality is not enough), and a source reachable only through a link needs
/// the explicit per-row opt-in.
///
/// Mutants killed: a count-only comparison (an unrelated hash swapped in while
/// the count holds is accepted) and an opt-in that is never consulted.
#[test]
fn test_plan_requires_exact_identity_set() {
    const HASH_A: &str = "aaaaaaaaaaaaaaaa";
    const HASH_B: &str = "bbbbbbbbbbbbbbbb";
    const HASH_C: &str = "cccccccccccccccc";

    let parent = unique_temp_dir("plan-exact-set");
    let root = parent.join("data");
    std::fs::create_dir_all(&root).expect("fixture root");
    let rows;
    {
        let fx = fixture(&root);
        fx.seed(HASH_A, "row-a");
        fx.seed(HASH_B, "row-b");
        fx.seed(HASH_C, "row-c");
        rows = vec![
            valid_row(&root, HASH_A, "path:/fixture/a", "path:/fixture/a", false),
            valid_row(&root, HASH_B, "path:/fixture/b", "path:/fixture/b", false),
            valid_row(&root, HASH_C, "path:/fixture/c", "path:/fixture/c", false),
        ];
    }

    let plan = write_plan(&parent.join("plans"), &root, rows.clone());
    let (code, stdout, stderr) = run_dry_run(&root, &plan);
    assert_eq!(
        code,
        Some(0),
        "a complete plan must dry-run clean; stderr: {stderr}"
    );
    let report = report_json(&stdout, &stderr);
    assert_eq!(report["format_version"], 1);
    let sources = report["sources"].as_array().expect("sources array");
    assert_eq!(sources.len(), 3);
    let reported: Vec<&str> = sources
        .iter()
        .map(|source| source["source_hash"].as_str().expect("source_hash"))
        .collect();
    assert!(reported.contains(&HASH_A) && reported.contains(&HASH_B) && reported.contains(&HASH_C));

    // Subcase C — §I.3: a shard file the plan does not claim, but the registry
    // DOES reference, is CARRIED VERBATIM — never refused (the "refuse every
    // unplanned shard" mutant answers 72 here) and never mutated.
    let carried = root
        .join("projects")
        .join(format!("project_{HASH_C}_shard_0.db"));
    let carried_bytes = std::fs::read(&carried).expect("carried shard bytes");
    let mut claimed_only = rows.clone();
    claimed_only.truncate(2);
    let plan = write_plan(&parent.join("plans-carried"), &root, claimed_only.clone());
    let before = tree_digest(&root);
    let (code, stdout, stderr) = run_dry_run(&root, &plan);
    assert_eq!(
        code,
        Some(0),
        "a registry-referenced unplanned shard is carried, not refused; stderr: {stderr}"
    );
    let report = report_json(&stdout, &stderr);
    assert_eq!(report["source_count"], 2);
    let carried_report: Vec<&str> = report["carried_verbatim"]
        .as_array()
        .expect("carried_verbatim array")
        .iter()
        .map(|hash| hash.as_str().expect("carried hash"))
        .collect();
    assert_eq!(
        carried_report,
        vec![HASH_C],
        "the unplanned-but-registered shard is reported as carried"
    );
    assert_eq!(
        before,
        tree_digest(&root),
        "the carried shard is not mutated"
    );

    // The same file reaches the staged tree byte-identically: carried, never
    // rewritten, never dropped (a mutant that skips it loses unrelated data).
    let (code, _stdout, stderr) =
        run_consolidate(&root, &plan, false, Some(&fixture_custody(&parent)));
    assert_eq!(
        code,
        Some(0),
        "a carried shard never blocks apply; stderr: {stderr}"
    );
    let staged = staged_root(&parent);
    assert_eq!(
        std::fs::read(
            staged
                .join("projects")
                .join(format!("project_{HASH_C}_shard_0.db"))
        )
        .expect("staged carried shard"),
        carried_bytes,
        "the carried shard is staged byte-identically"
    );

    // Subcase A — one source replaced by an unrelated hash, count still three.
    let mut swapped = rows.clone();
    swapped[2] = valid_row(
        &root,
        "ffffffffffffffff",
        "path:/fixture/c",
        "path:/fixture/c",
        false,
    );
    let before = tree_digest(&root);
    let plan = write_plan(&parent.join("plans-swapped"), &root, swapped);
    let (code, _stdout, stderr) = run_dry_run(&root, &plan);
    assert_refusal(code, 72, &stderr, "consolidation.source_set_mismatch");
    assert_eq!(
        before,
        tree_digest(&root),
        "a refused dry-run mutates nothing"
    );

    // Subcase B — a linked source without the explicit opt-in.
    let links = parent.join("links");
    std::fs::create_dir_all(&links).expect("link dir");
    let original = root
        .join("projects")
        .join(format!("project_{HASH_C}_shard_0.db"));
    let copy = links.join("shard-c.db");
    std::fs::copy(&original, &copy).expect("copy shard");
    std::fs::remove_file(&original).expect("remove original shard");
    std::os::unix::fs::symlink(&copy, &original).expect("symlink shard");
    let before = tree_digest(&root);
    let plan = write_plan(&parent.join("plans-linked"), &root, rows.clone());
    let (code, _stdout, stderr) = run_dry_run(&root, &plan);
    assert_refusal(code, 72, &stderr, "consolidation.linked_source_not_allowed");
    assert_eq!(before, tree_digest(&root), "the refusal writes nothing");

    let mut opted_in = rows;
    opted_in[2] = valid_row(&root, HASH_C, "path:/fixture/c", "path:/fixture/c", true);
    let plan = write_plan(&parent.join("plans-opted-in"), &root, opted_in);
    let (code, _stdout, stderr) = run_dry_run(&root, &plan);
    assert_eq!(
        code,
        Some(0),
        "the explicit opt-in admits the linked source; stderr: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&parent);
}

/// A target with a `git-common:` canonical identity and no on-disk directory
/// keeps that explicit identity — never a runtime `path:` fallback — and no
/// directory is created for it.
#[test]
fn test_missing_git_identity_never_falls_back() {
    const HASH_A: &str = "aaaaaaaaaaaaaaaa";
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);

    let parent = unique_temp_dir("plan-git-identity");
    let root = parent.join("data");
    std::fs::create_dir_all(&root).expect("fixture root");
    let absent_repo = format!("/home/opencode/absent-repo-{nanos}");
    let source_identity = format!("git-common:/home/vaz/absent-repo-{nanos}/.git");
    let target_identity = format!("git-common:{absent_repo}/.git");
    let target_hash = identity_hash(&target_identity);
    {
        let fx = fixture(&root);
        fx.seed(HASH_A, "row-a");
    }
    let rows = vec![plan_row(
        &root,
        HASH_A,
        &source_identity,
        &target_hash,
        &target_identity,
        false,
    )];
    // (`target_hash` is the recomputed identity hash above, not a source hash.)
    let plan = write_plan(&parent.join("plans"), &root, rows);
    let (code, stdout, stderr) = run_dry_run(&root, &plan);
    assert_eq!(
        code,
        Some(0),
        "the explicit git-common identity plans clean; stderr: {stderr}"
    );
    let report = report_json(&stdout, &stderr);
    let source = &report["sources"][0];
    assert_eq!(source["target_identity"], target_identity);
    assert!(
        !source["target_identity"]
            .as_str()
            .expect("identity")
            .starts_with("path:")
    );
    assert!(
        !Path::new(&format!("{absent_repo}/.git")).exists(),
        "no identity directory is ever created"
    );

    // Mutant kill: the same row with the `path:` fallback's hash is refused.
    let fallback_hash = identity_hash(&format!("path:{absent_repo}"));
    let rows = vec![plan_row(
        &root,
        HASH_A,
        &source_identity,
        &fallback_hash,
        &target_identity,
        false,
    )];
    let plan = write_plan(&parent.join("plans-fallback"), &root, rows);
    let (code, _stdout, stderr) = run_dry_run(&root, &plan);
    assert_refusal(code, 72, &stderr, "consolidation.target_hash_mismatch");

    let _ = std::fs::remove_dir_all(&parent);
}

/// Rule B.4 as ruled 2026-11-12: plain component-boundary swap for every
/// `/home/vaz` identity, no exceptions. The former special `56c3912b7371f31b`
/// pin to `/home/opencode/Code/opencode` is superseded; the `SPECIAL` row now
/// resolves to `/home/opencode/Code/Personal/JS/opencode`, `/home/vaz2/...` is
/// left alone.
///
/// Mutants killed: the retired special arm (the special source becomes
/// `/home/opencode/Code/opencode`) and a non-component-boundary prefix rewrite
/// (`/home/vaz2` becomes `/home/opencode2`).
#[test]
fn test_special_opencode_target_and_boundary_mapping() {
    const SPECIAL: &str = "56c3912b7371f31b";
    const HASH_B: &str = "bbbbbbbbbbbbbbbb";
    const HASH_C: &str = "cccccccccccccccc";

    let parent = unique_temp_dir("plan-boundary");
    let root = parent.join("data");
    std::fs::create_dir_all(&root).expect("fixture root");
    {
        let fx = fixture(&root);
        fx.seed(SPECIAL, "row-special");
        fx.seed(HASH_B, "row-b");
        fx.seed(HASH_C, "row-c");
    }

    let special_target = "path:/home/opencode/Code/Personal/JS/opencode";
    let rows = vec![
        plan_row(
            &root,
            SPECIAL,
            "path:/home/vaz/Code/Personal/JS/opencode",
            &identity_hash(special_target),
            special_target,
            false,
        ),
        // `/home/vaz2` shares the textual prefix but not a path component.
        plan_row(
            &root,
            HASH_B,
            "path:/home/vaz2/work",
            &identity_hash("path:/home/vaz2/work"),
            "path:/home/vaz2/work",
            false,
        ),
        // A plain component-boundary rewrite.
        plan_row(
            &root,
            HASH_C,
            "path:/home/vaz/Code/Personal/Rust/thing",
            &identity_hash("path:/home/opencode/Code/Personal/Rust/thing"),
            "path:/home/opencode/Code/Personal/Rust/thing",
            false,
        ),
    ];
    let plan = write_plan(&parent.join("plans"), &root, rows);
    let (code, stdout, stderr) = run_dry_run(&root, &plan);
    assert_eq!(
        code,
        Some(0),
        "the pinned associations plan clean; stderr: {stderr}"
    );
    let report = report_json(&stdout, &stderr);
    let sources = report["sources"].as_array().expect("sources array");
    let by_hash = |hash: &str| -> &serde_json::Value {
        sources
            .iter()
            .find(|source| source["source_hash"] == hash)
            .expect("reported source")
    };
    assert_eq!(by_hash(SPECIAL)["target_identity"], special_target);
    assert_eq!(by_hash(HASH_B)["target_identity"], "path:/home/vaz2/work");
    assert_eq!(
        by_hash(HASH_C)["target_identity"],
        "path:/home/opencode/Code/Personal/Rust/thing"
    );

    let _ = std::fs::remove_dir_all(&parent);
}

/// A supplied target hash that disagrees with its canonical string, a target
/// hash collision with unequal canonical strings, and a traversing target path
/// each refuse before any write.
#[test]
fn test_identity_collision_refuses_before_write() {
    const HASH_A: &str = "aaaaaaaaaaaaaaaa";
    const HASH_B: &str = "bbbbbbbbbbbbbbbb";

    let parent = unique_temp_dir("plan-collision");
    let root = parent.join("data");
    std::fs::create_dir_all(&root).expect("fixture root");
    {
        let fx = fixture(&root);
        fx.seed(HASH_A, "row-a");
        fx.seed(HASH_B, "row-b");
    }
    let target_a = "path:/fixture/a";
    let good = vec![
        valid_row(&root, HASH_A, target_a, target_a, false),
        valid_row(&root, HASH_B, "path:/fixture/b", "path:/fixture/b", false),
    ];
    let before = tree_digest(&root);

    // Subcase A — supplied target hash disagrees with the canonical string.
    let mut wrong_hash = good.clone();
    wrong_hash[0] = plan_row(&root, HASH_A, target_a, "0123456789abcdef", target_a, false);
    let plan = write_plan(&parent.join("plans-wrong-hash"), &root, wrong_hash);
    let (code, _stdout, stderr) = run_dry_run(&root, &plan);
    assert_refusal(code, 72, &stderr, "consolidation.target_hash_mismatch");
    assert_eq!(
        before,
        tree_digest(&root),
        "the refused plan mutates nothing"
    );

    // Subcase B — two rows claim one target hash under unequal canonical
    // strings (the trust-the-supplied-hash mutant).
    let mut collision = good.clone();
    collision[1] = plan_row(
        &root,
        HASH_B,
        "path:/fixture/b",
        &identity_hash(target_a),
        "path:/fixture/b",
        false,
    );
    let plan = write_plan(&parent.join("plans-collision"), &root, collision);
    let (code, _stdout, stderr) = run_dry_run(&root, &plan);
    assert_refusal(code, 72, &stderr, "consolidation.target_hash_collision");
    assert_eq!(
        before,
        tree_digest(&root),
        "the refused plan mutates nothing"
    );

    // Subcase C — a traversing target path never escapes the root.
    let outside = parent.join("outside.db");
    std::fs::write(&outside, b"sentinel-bytes").expect("outside sentinel");
    let mut traversal = good.clone();
    let mut row = valid_row(&root, HASH_A, target_a, target_a, false);
    row["target_path"] = serde_json::json!(root.join("projects").join("..").join("outside.db"));
    traversal[0] = row;
    let plan = write_plan(&parent.join("plans-traversal"), &root, traversal);
    let (code, _stdout, stderr) = run_dry_run(&root, &plan);
    assert_refusal(code, 72, &stderr, "consolidation.target_path_outside_root");
    assert_eq!(
        std::fs::read(&outside).expect("outside bytes"),
        b"sentinel-bytes",
        "nothing outside the root is ever touched"
    );
    assert_eq!(
        before,
        tree_digest(&root),
        "the refused plan mutates nothing"
    );

    let _ = std::fs::remove_dir_all(&parent);
}

// ---------------------------------------------------------------------------
// §A.1 memory-shard ladder fixtures (rev-12): ONE base shape, TEN CLI subcases
// ---------------------------------------------------------------------------

/// The `deleted_at` column and its non-partial index — the accepted shape's one
/// deviation from the v3 rung.
const DELETED_AT_COLUMN: &str = "ALTER TABLE memories ADD COLUMN deleted_at INTEGER";
const DELETED_AT_INDEX: &str = "CREATE INDEX idx_deleted_at ON memories(deleted_at)";

/// The production ladder DDL for one rung, from the shipped constants (never a
/// hand-copied schema): v1 17 columns, v2 + `session_id`, v3 + `shard_stats`,
/// v4 + `source_prompt_created_at`, each with its own rung's exact index set.
fn ladder_statements(version: i64) -> Vec<String> {
    let mut statements = vec![
        mem_store::sql::memories_ddl(2),
        mem_store::sql::shard_metadata_ddl().to_string(),
    ];
    statements.extend(
        mem_store::sql::MEMORIES_INDEXES
            .iter()
            .map(|sql| sql.to_string()),
    );
    if version >= 2 {
        statements.push(mem_store::sql::MEMORY_SHARD_V2_ALTER.to_string());
        statements.push(mem_store::sql::MEMORY_SHARD_V2_INDEX.to_string());
    }
    if version >= 3 {
        statements.push(mem_store::shard_facts::SHARD_STATS_DDL.to_string());
    }
    if version >= 4 {
        statements.push(mem_store::shard_retention_sql::RETENTION_STAMP_ALTER.to_string());
    }
    statements.push(format!("PRAGMA user_version = {version}"));
    statements
}

/// The §A.1 base shape, defined ONCE: v3's 18 `memories` names + nullable
/// `deleted_at INTEGER`, `shard_metadata` + `shard_stats`, the exact SIX
/// indexes, and the embedding identity rows a populated shard carries.
fn accepted_shape_statements() -> Vec<String> {
    let mut statements = ladder_statements(3);
    statements.insert(statements.len() - 1, DELETED_AT_COLUMN.to_string());
    statements.insert(statements.len() - 1, DELETED_AT_INDEX.to_string());
    statements.push(
        "INSERT INTO shard_metadata (key, value) VALUES \
         ('embedding_dimensions', '2'), ('embedding_model', 'test-model')"
            .to_string(),
    );
    statements
}

/// One row of the base shape, every NOT NULL column filled, `deleted_at` NULL.
fn accepted_shape_row(id: &str) -> String {
    format!(
        "INSERT INTO memories (id, content, vector, container_tag, created_at, updated_at, deleted_at) \
         VALUES ('{id}', 'content {id}', vector32('[0.1, 0.2]'), 'opencode_project_aaaaaaaaaaaaaaaa', \
         1700000000000, 1700000000000, NULL)"
    )
}

/// The same row with a non-NULL `deleted_at` (subcase (a)).
fn deleted_at_set_row(id: &str) -> String {
    accepted_shape_row(id).replace("1700000000000, NULL)", "1700000000000, 1700000000001)")
}

/// Replaces the seeded v4 shard with a hand-built shape: the statements in
/// order (DDL, then rows). Every subcase pins its OWN shape this way.
fn build_shape(root: &Path, hash: &str, statements: &[String]) {
    let shard = root
        .join("projects")
        .join(format!("project_{hash}_shard_0.db"));
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(PathBuf::from(format!("{}{suffix}", shard.display())));
    }
    let config = fixture_config(root);
    run_async(async {
        let options = ConnectOptions::for_config(&config, Platform::current());
        let handle = TursoConnectFactory
            .connect(shard.clone(), options)
            .await
            .expect("create the pinned-shape shard");
        for sql in statements {
            handle
                .execute(sql, Vec::new())
                .await
                .unwrap_or_else(|error| panic!("fixture DDL failed ({sql}): {error}"));
        }
    });
}

/// The ACCEPT sibling of [`shape_case`]: a dry-run over the mutated fixture
/// exits 0 and writes nothing.
fn shape_accept_dry(case: &str, mutate: impl FnOnce(&Path, &Path)) {
    const HASH: &str = "aaaaaaaaaaaaaaaa";

    let parent = unique_temp_dir(&format!("plan-census-{case}"));
    let root = parent.join("data");
    std::fs::create_dir_all(&root).expect("fixture root");
    {
        let fx = fixture(&root);
        fx.seed(HASH, &format!("row-{case}"));
    }
    mutate(&root, &parent);

    let identity = "path:/fixture/a";
    let rows = vec![valid_row(&root, HASH, identity, identity, false)];
    let plan = write_plan(&parent.join("plans"), &root, rows);
    let before = tree_digest(&root);
    let (code, _stdout, stderr) = run_dry_run(&root, &plan);
    assert_eq!(
        code,
        Some(0),
        "{case}: the accepted shape preflights clean; stderr: {stderr}"
    );
    assert_eq!(
        before,
        tree_digest(&root),
        "{case}: an accepted dry-run mutates nothing"
    );
    let _ = std::fs::remove_dir_all(&parent);
}

/// The applied ACCEPT sibling of [`shape_case`]: the plan merges clean, and the
/// staged destination's `(ids, memories columns)` come back for the row-carry
/// and projection assertions.
fn shape_accept_applied(
    case: &str,
    mutate: impl FnOnce(&Path, &Path),
) -> (Vec<String>, Vec<String>) {
    const HASH: &str = "aaaaaaaaaaaaaaaa";

    let parent = unique_temp_dir(&format!("plan-census-{case}"));
    let root = parent.join("data");
    std::fs::create_dir_all(&root).expect("fixture root");
    {
        let fx = fixture(&root);
        fx.seed(HASH, &format!("row-{case}"));
    }
    mutate(&root, &parent);

    let identity = "path:/fixture/a";
    let rows = vec![valid_row(&root, HASH, identity, identity, false)];
    let plan = write_plan(&parent.join("plans"), &root, rows);
    let (code, _stdout, stderr) =
        run_consolidate(&root, &plan, false, Some(&fixture_custody(&parent)));
    assert_eq!(
        code,
        Some(0),
        "{case}: the accepted shape merges clean; stderr: {stderr}"
    );
    let staged = staged_root(&parent);
    let destination = staged_shard(&staged, &identity_hash(identity), 0);
    let destination_ids = ids(&read_rows(&destination));
    let destination_columns = shard_columns(&destination);
    let _ = std::fs::remove_dir_all(&parent);
    (destination_ids, destination_columns)
}

/// One refused-shape case: a valid plan over a fixture root that `mutate`
/// perturbs, refusing 65 with its own message and writing nothing.
fn shape_case(case: &str, needle: &str, mutate: impl FnOnce(&Path, &Path)) {
    const HASH: &str = "aaaaaaaaaaaaaaaa";

    let parent = unique_temp_dir(&format!("plan-census-{case}"));
    let root = parent.join("data");
    std::fs::create_dir_all(&root).expect("fixture root");
    {
        let fx = fixture(&root);
        fx.seed(HASH, &format!("row-{case}"));
    }
    mutate(&root, &parent);

    let identity = "path:/fixture/a";
    let rows = vec![valid_row(&root, HASH, identity, identity, false)];
    let plan = write_plan(&parent.join("plans"), &root, rows);
    let before = tree_digest(&root);
    let (code, _stdout, stderr) = run_dry_run(&root, &plan);
    assert_refusal(code, 65, &stderr, needle);
    assert_eq!(
        before,
        tree_digest(&root),
        "{case}: a refused plan mutates nothing"
    );
    let _ = std::fs::remove_dir_all(&parent);
}

/// Every unsupported shape and every newly-present tier refuses 65 with its
/// OWN message: an off-ladder `memories`, an unknown table, an extra payload
/// column, an unsupported ID encoding, an empty facts/vault tier, an orphan
/// shard file and a non-conforming registry row.
#[test]
fn test_unknown_schema_encoding_and_tiers_refuse() {
    const HASH: &str = "aaaaaaaaaaaaaaaa";

    // §A.1 (e) ACCEPT — the BASE SHAPE defined ONCE above (`accepted_shape_statements`):
    // v3's 18 names + nullable `deleted_at`, `shard_metadata` + `shard_stats`,
    // the exact SIX indexes, one all-NULL row. This IS the re-pointed `ladder`
    // subcase: it applies clean, the destination carries the all-NULL row, and
    // its `memories` never receives a `deleted_at` column.
    let (destination_ids, destination_columns) = shape_accept_applied("ladder", |root, _| {
        let mut statements = accepted_shape_statements();
        statements.push(accepted_shape_row("shape-row"));
        build_shape(root, HASH, &statements);
    });
    assert_eq!(
        destination_ids,
        vec!["shape-row".to_string()],
        "the all-NULL row is carried onto the destination"
    );
    assert!(
        !destination_columns.iter().any(|name| name == "deleted_at"),
        "the destination never receives the vestigial column: {destination_columns:?}"
    );

    // §A.1 (a) REFUSE — the base shape plus one row with a NON-NULL
    // `deleted_at` (the all-NULL property is enforced by code, never trusted).
    shape_case("deleted-at-set", "non-NULL deleted_at", |root, _| {
        let mut statements = accepted_shape_statements();
        statements.push(deleted_at_set_row("shape-row"));
        build_shape(root, HASH, &statements);
    });

    // §A.1 (b) REFUSE — the base shape with `idx_deleted_at` DROPPED: only the
    // index predicate can see this (a column-only census passes (a) and (e)).
    shape_case("deleted-at-no-index", "index set outside", |root, _| {
        let mut statements = accepted_shape_statements();
        statements.retain(|sql| sql.as_str() != DELETED_AT_INDEX);
        statements.push(accepted_shape_row("shape-row"));
        build_shape(root, HASH, &statements);
    });

    // §A.1 (c) REFUSE — a SEVENTH index on a NON-`memories` table: a
    // `pragma_index_list('memories')`-scoped predicate sees 6 == 6 here.
    shape_case("seventh-index", "index set outside", |root, _| {
        let mut statements = accepted_shape_statements();
        statements.push("CREATE INDEX idx_extra ON shard_metadata(key)".to_string());
        statements.push(accepted_shape_row("shape-row"));
        build_shape(root, HASH, &statements);
    });

    // §A.1 (d1) REFUSE — a RUNG-CORRECT **v2** shard carrying `deleted_at`:
    // 18 names + `deleted_at` (19), `{memories, shard_metadata}`, the five
    // ladder indexes and NO `idx_deleted_at`. Rung-correct on purpose: the
    // column NAME SET is then the only possible refuser, so folding the shape
    // into the shared `2 | 3` ladder arm cannot survive this fixture.
    shape_case("deleted-at-v2", "memories shape outside", |root, _| {
        let mut statements = ladder_statements(2);
        statements.insert(statements.len() - 1, DELETED_AT_COLUMN.to_string());
        build_shape(root, HASH, &statements);
    });

    // §A.1 (d2) REFUSE — a RUNG-CORRECT **v4** shard carrying `deleted_at`
    // (20 names, three tables, the five ladder indexes).
    shape_case("deleted-at-v4", "memories shape outside", |root, _| {
        let mut statements = ladder_statements(4);
        statements.insert(statements.len() - 1, DELETED_AT_COLUMN.to_string());
        build_shape(root, HASH, &statements);
    });

    // §A.1 (f) ACCEPT — a plain v1 shard: 17 names, `{memories, shard_metadata}`,
    // the FOUR v1 indexes. Kills a five-for-all index rule (v1 has no
    // `session_id` column and cannot carry `idx_session_id`).
    shape_accept_dry("ladder-v1", |root, _| {
        let mut statements = ladder_statements(1);
        statements.push(
            "INSERT INTO memories (id, content, vector, container_tag, created_at, updated_at) \
             VALUES ('shape-row', 'content', vector32('[0.1, 0.2]'), \
             'opencode_project_aaaaaaaaaaaaaaaa', 1700000000000, 1700000000000)"
                .to_string(),
        );
        build_shape(root, HASH, &statements);
    });

    // §A.1 (g) REFUSE — the base shape with ONE ladder index (`idx_type`)
    // dropped: kills a "no extras + `idx_deleted_at` present" mutant.
    shape_case("ladder-index-dropped", "index set outside", |root, _| {
        let mut statements = accepted_shape_statements();
        statements.retain(|sql| sql.as_str() != mem_store::sql::MEMORIES_INDEXES[1]);
        statements.push(accepted_shape_row("shape-row"));
        build_shape(root, HASH, &statements);
    });

    // §A.1 (i) REFUSE — name-set vs column COUNT: 19 columns, `deleted_at`
    // present, the exact six indexes, both extra tables present, all rows NULL,
    // but one ladder column (`metadata`) replaced by `rogue` at the SAME count.
    shape_case("rogue-column", "memories shape outside", |root, _| {
        let mut statements = accepted_shape_statements();
        let base = statements
            .iter()
            .position(|sql| sql.contains("CREATE TABLE IF NOT EXISTS memories"))
            .expect("the memories DDL is part of the base shape");
        statements[base] = statements[base].replace("metadata TEXT", "rogue INTEGER");
        statements.push(accepted_shape_row("shape-row"));
        build_shape(root, HASH, &statements);
    });

    // §A.1 (j1) REFUSE — table-set SUPERSET: the accepted shape plus one EXTRA
    // table carrying a row (the census used to refuse only on `tables.is_empty()`).
    shape_case("extra-table", "table set outside", |root, _| {
        let mut statements = accepted_shape_statements();
        statements.push("CREATE TABLE extra_notes (id TEXT PRIMARY KEY)".to_string());
        statements.push("INSERT INTO extra_notes (id) VALUES ('orphan-row')".to_string());
        statements.push(accepted_shape_row("shape-row"));
        build_shape(root, HASH, &statements);
    });

    // §A.1 (j2) REFUSE — table-set SUBSET: the base shape with `shard_stats`
    // DROPPED (the v3 rung requires it).
    shape_case("missing-shard-stats", "table set outside", |root, _| {
        let mut statements = accepted_shape_statements();
        statements.retain(|sql| sql.as_str() != mem_store::shard_facts::SHARD_STATS_DDL);
        statements.push(accepted_shape_row("shape-row"));
        build_shape(root, HASH, &statements);
    });

    shape_case(
        "unknown-table",
        "consolidation.auxiliary_table_unknown",
        |root, _| {
            seed_episodic(root);
            fixture_exec(
                root,
                &root.join("episodic.db"),
                "CREATE TABLE legacy_notes (id TEXT PRIMARY KEY)",
                Vec::new(),
            );
        },
    );

    shape_case(
        "payload-column",
        "consolidation.auxiliary_column_unknown",
        |root, _| {
            seed_episodic(root);
            fixture_exec(
                root,
                &root.join("episodic.db"),
                "ALTER TABLE episodic_raw_context ADD COLUMN transcript BLOB",
                Vec::new(),
            );
        },
    );

    shape_case(
        "encoding",
        "consolidation.auxiliary_encoding_unsupported",
        |root, _| {
            seed_episodic(root);
            fixture_exec(
                root,
                &root.join("episodic.db"),
                "INSERT INTO episodic_raw_context (project_id, session_id, agent_id,
                   raw_context, byte_len, captured_at)
                 VALUES (x'2f686f6d652f76617a', 's1', 'main', 'raw', 3, 1)",
                Vec::new(),
            );
        },
    );

    shape_case("facts", "consolidation.facts_tier_present", |root, _| {
        fixture_exec(
            root,
            &root.join("facts.db"),
            "CREATE TABLE facts (id TEXT PRIMARY KEY)",
            Vec::new(),
        );
    });

    shape_case("vault", "consolidation.vault_present", |root, _| {
        std::fs::create_dir_all(root.join("vault")).expect("vault dir");
    });

    shape_case(
        "vault-keys",
        "consolidation.vault_keys_present",
        |root, _| {
            std::fs::create_dir_all(root.join("vault-keys")).expect("vault-keys dir");
        },
    );

    shape_case("orphan", "consolidation.orphan_shard_file", |root, _| {
        std::fs::copy(
            root.join("projects")
                .join(format!("project_{HASH}_shard_0.db")),
            root.join("projects")
                .join("project_ffffffffffffffff_shard_0.db"),
        )
        .expect("orphan shard file");
    });

    shape_case(
        "registry",
        "consolidation.registry_row_nonconforming",
        |root, _| {
            fixture_exec(
                root,
                &root.join("metadata.db"),
                "INSERT INTO shards (scope, scope_hash, shard_index, db_path, vector_count,
                   is_active, created_at)
                 VALUES ('project', 'ffffffffffffffff', 0, 'projects/../outside.db', 0, 1, 1)",
                Vec::new(),
            );
        },
    );

    // F-1: a rotated SOURCE (`shard_1` with distinct rows) must refuse 65, never
    // drop the rotated rows silently. Today the plan is accepted (exit 0) and
    // `shard_1` is neither merged nor copied. The rotated shard is created by
    // the production manager, so the fixture is a store shape the product
    // itself produces (`rotate_shard`).
    shape_case(
        "multi-shard",
        "consolidation.multi_shard_claimed",
        |root, _| {
            let fx = fixture(root);
            let tag = format!("opencode_project_{HASH}");
            run_async(async {
                let active = fx
                    .manager
                    .get_active_shard(Scope::Project, HASH)
                    .await
                    .expect("read the active shard")
                    .expect("shard 0 is active");
                let shard = fx
                    .manager
                    .rotate_shard(Scope::Project, HASH, active.id, 1)
                    .await
                    .expect("rotate to shard 1");
                let db = fx
                    .manager
                    .open_shard(&shard.db_path)
                    .await
                    .expect("open shard 1");
                vector::insert_vector(db.as_ref(), &record("rotated-only", &tag))
                    .await
                    .expect("rotated row");
                drop(db);
            });
        },
    );

    // F-1 target side: a claimed TARGET hash with a rotated `shard_1` is copied
    // unregistered today (no registry row for it ⇒ unreachable rows). Must
    // refuse 65.
    shape_case(
        "multi-shard-target",
        "consolidation.multi_shard_claimed",
        |root, _| {
            let target = identity_hash("path:/fixture/a");
            let fx = fixture(root);
            run_async(async {
                fx.manager
                    .create_shard(Scope::Project, &target, 0)
                    .await
                    .expect("target shard 0");
                let active = fx
                    .manager
                    .get_active_shard(Scope::Project, &target)
                    .await
                    .expect("read the target's active shard")
                    .expect("target shard 0 is active");
                fx.manager
                    .rotate_shard(Scope::Project, &target, active.id, 1)
                    .await
                    .expect("rotate the target to shard 1");
            });
        },
    );
}

// ---------------------------------------------------------------------------
// Step 4 — typed lossless merge (staged output, never install)
// ---------------------------------------------------------------------------

/// Every row of one shard as a typed cell map. Read-only typed open: the
/// staged output is inspected the way the engine would read it, never through
/// a report flag.
fn read_rows(shard: &Path) -> Vec<BTreeMap<String, Value>> {
    run_async(async {
        let mut options = ConnectOptions::default();
        options.read_only = true;
        let handle = TursoConnectFactory
            .connect(shard.to_path_buf(), options)
            .await
            .unwrap_or_else(|error| {
                let listing: Vec<String> = std::fs::read_dir(shard.parent().unwrap_or(shard))
                    .map(|entries| {
                        entries
                            .filter_map(|entry| entry.ok())
                            .map(|entry| entry.file_name().to_string_lossy().to_string())
                            .collect()
                    })
                    .unwrap_or_default();
                panic!(
                    "open {} read-only: {error}; siblings: {listing:?}",
                    shard.display()
                )
            });
        let rows = handle
            .query("SELECT * FROM memories", Vec::new())
            .await
            .unwrap_or_else(|error| panic!("select {}: {error}", shard.display()));
        let mut typed: Vec<BTreeMap<String, Value>> = rows
            .iter()
            .map(|row| {
                row.column_names()
                    .iter()
                    .filter_map(|name| row.get(name).map(|value| (name.clone(), value.clone())))
                    .collect()
            })
            .collect();
        typed.sort_by_key(|cells| cells.get("id").and_then(Value::as_text).map(str::to_string));
        typed
    })
}

fn cell<'a>(cells: &'a BTreeMap<String, Value>, name: &str) -> &'a Value {
    cells
        .get(name)
        .unwrap_or_else(|| panic!("the row has no {name} cell"))
}

/// `memories` column names of one shard, read-only.
fn shard_columns(shard: &Path) -> Vec<String> {
    run_async(async {
        let mut options = ConnectOptions::default();
        options.read_only = true;
        let handle = TursoConnectFactory
            .connect(shard.to_path_buf(), options)
            .await
            .expect("open the destination shard");
        handle
            .query("PRAGMA table_info(memories)", Vec::new())
            .await
            .expect("destination columns")
            .iter()
            .filter_map(|row| row.text("name"))
            .collect()
    })
}

fn text_cell<'a>(cells: &'a BTreeMap<String, Value>, name: &str) -> &'a str {
    cell(cells, name)
        .as_text()
        .unwrap_or_else(|| panic!("{name} is not text: {:?}", cell(cells, name)))
}

fn ids(rows: &[BTreeMap<String, Value>]) -> Vec<String> {
    rows.iter()
        .map(|cells| text_cell(cells, "id").to_string())
        .collect()
}

/// The one staged tree the consolidator built under `parent`.
fn staged_root(parent: &Path) -> PathBuf {
    let mut found: Vec<PathBuf> = std::fs::read_dir(parent)
        .expect("fixture parent lists")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("consolidation-staged-"))
        })
        .collect();
    assert_eq!(
        found.len(),
        1,
        "exactly one staged tree under {}: {found:?}",
        parent.display()
    );
    found.remove(0)
}

fn staged_shard(staged: &Path, hash: &str, index: u32) -> PathBuf {
    staged
        .join("projects")
        .join(format!("project_{hash}_shard_{index}.db"))
}

/// `(shard_index, vector_count, is_active)` for one registry tuple, ordered by
/// index — the §C.6 staged-registry assertion reads the DB, never the report.
fn registry_rows(staged: &Path, hash: &str) -> Vec<(i64, i64, i64)> {
    let registry = staged.join("metadata.db");
    run_async(async {
        let mut options = ConnectOptions::default();
        options.read_only = true;
        let handle = TursoConnectFactory
            .connect(registry, options)
            .await
            .expect("open the staged registry");
        let rows = handle
            .query(
                "SELECT shard_index, vector_count, is_active FROM shards \
                 WHERE scope = 'project' AND scope_hash = ? ORDER BY shard_index",
                vec![Value::Text(hash.to_string())],
            )
            .await
            .expect("select staged registry rows");
        rows.iter()
            .map(|row| {
                (
                    row.i64("shard_index").unwrap_or(-1),
                    row.i64("vector_count").unwrap_or(-1),
                    row.i64("is_active").unwrap_or(-1),
                )
            })
            .collect()
    })
}

/// Runs `sql` against a fixture DB with the normal writable open (fixture
/// mutation only, never staged output).
fn fixture_exec(root: &Path, db: &Path, sql: &str, params: Vec<Value>) {
    let config = fixture_config(root);
    run_async(async {
        let handle = TursoConnectFactory
            .connect(
                db.to_path_buf(),
                ConnectOptions::for_config(&config, Platform::current()),
            )
            .await
            .unwrap_or_else(|error| panic!("open {}: {error}", db.display()));
        handle
            .execute(sql, params)
            .await
            .unwrap_or_else(|error| panic!("{sql} on {}: {error}", db.display()));
    });
}

/// Writes the fixture's config into the scratch HOME: the same embedding
/// identity the fixture shards were built with, plus the capacity under test.
fn write_fixture_config(home: &Path, capacity: u32) {
    let dir = home.join(".config").join("opencode");
    std::fs::create_dir_all(&dir).expect("config dir");
    std::fs::write(
        dir.join("opencode-mem.jsonc"),
        format!(
            "{{\"embeddingModel\":\"test-model\",\"embeddingDimensions\":2,\"maxVectorsPerShard\":{capacity}}}"
        ),
    )
    .expect("write fixture config");
}

/// One rich row: nonunit vectors, integer-width timestamps, historical host
/// paths inside content and metadata, a private row that exists nowhere else.
fn rich_row(id: &str, tag: &str) -> MemoryInsert {
    let mut row = record(id, tag);
    row.content = format!("/home/vaz/Code/Personal/Rust/thing remembers {id}");
    row.vector = vec![0.5, -2.25];
    row.tags_vector = Some(vec![1.0, 0.125]);
    row.tags = Some("alpha,beta".to_string());
    row.metadata = Some("{\"cwd\":\"/home/vaz/Code/Personal/Rust/thing\"}".to_string());
    row.session_id = Some(format!("session-{id}"));
    row.display_name = Some(format!("display {id}"));
    row.user_name = Some("vaz".to_string());
    row.user_email = Some("vaz@example.com".to_string());
    row.project_path = Some("/home/vaz/Code/Personal/Rust/thing".to_string());
    row.project_name = Some("thing".to_string());
    row.git_repo_url = Some("git@example.com:vaz/thing.git".to_string());
    row.created_at = 1_700_000_000_000;
    row.updated_at = 1_600_000_000_123;
    row.source_prompt_created_at = Some(1_500_000_000_456);
    row
}

impl Fixture {
    fn seed_row(&self, hash: &str, row: &MemoryInsert) {
        run_async(async {
            let shard = self
                .manager
                .create_shard(Scope::Project, hash, 0)
                .await
                .expect("create the shard");
            let db = self
                .manager
                .open_shard(&shard.db_path)
                .await
                .expect("open the shard");
            vector::insert_vector(db.as_ref(), row)
                .await
                .expect("insert the row");
            drop(db);
        });
    }
}

/// A fully-private and a mixed row survive byte-for-byte: only the five
/// association columns normalize, every other typed cell, NULL/empty
/// distinction, integer width, timestamp, raw vector/tags-vector blob and
/// historical `/home/vaz` string stay verbatim.
///
/// Mutants killed: an export/import roundtrip (typed cells and blob bytes
/// re-encoded), a NULL coercion (NULL read back as empty), and a naive path
/// replacement that rewrites history inside `content`/`metadata`.
#[test]
fn test_full_typed_raw_merge_preserves_history() {
    const TARGET: &str = "1111111111111111";
    const SOURCE: &str = "2222222222222222";

    let parent = unique_temp_dir("merge-preserve");
    let root = parent.join("data");
    std::fs::create_dir_all(&root).expect("fixture root");
    let target_tag = format!("opencode_project_{TARGET}");
    let source_tag = format!("opencode_project_{SOURCE}");
    {
        let fx = fixture(&root);
        let mut private_target = record("private-target", &target_tag);
        private_target.vector = vec![-0.75, 0.5];
        private_target.tags_vector = Some(vec![0.25, -0.5]);
        fx.seed_row(TARGET, &private_target);
        fx.seed_row(SOURCE, &rich_row("history", &source_tag));
        let mut nulls = record("nulls", &source_tag);
        nulls.tags = None;
        nulls.metadata = None;
        nulls.display_name = None;
        nulls.user_name = None;
        nulls.user_email = None;
        nulls.project_path = None;
        nulls.project_name = None;
        nulls.git_repo_url = None;
        nulls.session_id = None;
        fx.seed_row(SOURCE, &nulls);
        let mut empties = record("empties", &source_tag);
        empties.tags = Some(String::new());
        empties.metadata = Some(String::new());
        empties.display_name = Some(String::new());
        empties.project_name = Some(String::new());
        empties.project_path = Some(String::new());
        fx.seed_row(SOURCE, &empties);
    }
    // Pin one row the way the API does (pin flag is a typed cell, not content).
    let source_shard = root
        .join("projects")
        .join(format!("project_{SOURCE}_shard_0.db"));
    fixture_exec(
        &root,
        &source_shard,
        "UPDATE memories SET is_pinned = 1 WHERE id = 'history'",
        Vec::new(),
    );

    let source_rows = read_rows(&source_shard);
    let history = source_rows
        .iter()
        .find(|cells| text_cell(cells, "id") == "history")
        .expect("the history row exists");

    let target_identity = "path:/home/opencode/Code/Personal/Rust/thing";
    let target_hash = identity_hash(target_identity);
    let rows = vec![
        valid_row(&root, TARGET, target_identity, target_identity, false),
        valid_row(
            &root,
            SOURCE,
            "path:/home/vaz/Code/Personal/Rust/thing",
            target_identity,
            false,
        ),
    ];
    let plan = write_plan(&parent.join("plans"), &root, rows);
    let (code, stdout, stderr) =
        run_consolidate(&root, &plan, false, Some(&fixture_custody(&parent)));
    assert_eq!(code, Some(0), "the merge applies clean; stderr: {stderr}");
    let report = report_json(&stdout, &stderr);
    assert_eq!(report["dry_run"], false);

    let staged = staged_root(&parent);
    let merged = read_rows(&staged_shard(&staged, &target_hash, 0));
    assert_eq!(
        ids(&merged),
        vec!["empties", "history", "nulls", "private-target"],
        "every survivor is present exactly once"
    );

    let by_id = |id: &str| -> &BTreeMap<String, Value> {
        merged
            .iter()
            .find(|cells| text_cell(cells, "id") == id)
            .expect("survivor")
    };

    // Association normalization is the ONLY change.
    let normalized_tag = format!("opencode_project_{target_hash}");
    for survivor in &merged {
        assert_eq!(text_cell(survivor, "container_tag"), normalized_tag);
        assert_eq!(
            text_cell(survivor, "project_path"),
            "/home/opencode/Code/Personal/Rust/thing"
        );
        assert_eq!(text_cell(survivor, "project_name"), "FixtureProject");
        assert!(cell(survivor, "display_name").is_null());
        assert!(cell(survivor, "git_repo_url").is_null());
    }

    // Everything else is verbatim, compared against the SOURCE cells.
    let merged_history = by_id("history");
    for name in [
        "content",
        "tags",
        "type",
        "created_at",
        "updated_at",
        "metadata",
        "user_name",
        "user_email",
        "is_pinned",
        "session_id",
        "source_prompt_created_at",
    ] {
        assert_eq!(
            cell(merged_history, name),
            cell(history, name),
            "the {name} cell must survive verbatim"
        );
    }
    assert_eq!(
        cell(merged_history, "vector"),
        cell(history, "vector"),
        "raw vector blob bytes are preserved"
    );
    assert_eq!(
        cell(merged_history, "tags_vector"),
        cell(history, "tags_vector"),
        "raw tags-vector blob bytes are preserved"
    );
    assert!(
        text_cell(merged_history, "content").contains("/home/vaz/"),
        "history inside content is never rewritten"
    );
    assert!(
        text_cell(merged_history, "metadata").contains("/home/vaz/"),
        "history inside metadata is never rewritten"
    );
    assert_eq!(cell(merged_history, "is_pinned"), &Value::Integer(1));

    // NULL stays NULL; empty stays empty.
    assert!(cell(by_id("nulls"), "tags").is_null());
    assert!(cell(by_id("nulls"), "metadata").is_null());
    assert!(cell(by_id("nulls"), "session_id").is_null());
    assert_eq!(cell(by_id("empties"), "tags"), &Value::Text(String::new()));
    assert_eq!(
        cell(by_id("empties"), "metadata"),
        &Value::Text(String::new())
    );

    assert_eq!(registry_rows(&staged, &target_hash), vec![(0, 4, 1)]);
    let _ = std::fs::remove_dir_all(&parent);
}

/// An exact duplicate is compared AFTER association normalization: one
/// survivor, three provenance occurrences.
///
/// Mutants killed: comparing before normalization (every duplicate "diverges")
/// and failing every duplicate group instead of collapsing it.
#[test]
fn test_exact_duplicates_account_occurrences() {
    const TARGET: &str = "1111111111111111";
    const SOURCE_A: &str = "2222222222222222";
    const SOURCE_B: &str = "3333333333333333";

    let parent = unique_temp_dir("merge-duplicates");
    let root = parent.join("data");
    std::fs::create_dir_all(&root).expect("fixture root");
    {
        let fx = fixture(&root);
        for (hash, description) in [
            (TARGET, "target association"),
            (SOURCE_A, "source a association"),
            (SOURCE_B, "source b association"),
        ] {
            // Unequal associations, identical payload: metadata records the
            // origin so a pre-normalization compare would diverge.
            let mut row = record("dup", &format!("opencode_project_{hash}"));
            row.vector = vec![0.5, -1.5];
            row.metadata = Some(format!("{{\"origin\":\"{description}\"}}"));
            row.project_path = Some(format!("/fixture/{hash}"));
            fx.seed_row(hash, &row);
        }
    }
    // Make the payloads identical except the association columns.
    for hash in [TARGET, SOURCE_A, SOURCE_B] {
        let shard = root
            .join("projects")
            .join(format!("project_{hash}_shard_0.db"));
        fixture_exec(
            &root,
            &shard,
            "UPDATE memories SET metadata = '{\"origin\":\"same\"}'",
            Vec::new(),
        );
    }

    let target_identity = "path:/home/opencode/Code/thing";
    let target_hash = identity_hash(target_identity);
    let rows = vec![
        valid_row(&root, TARGET, target_identity, target_identity, false),
        valid_row(
            &root,
            SOURCE_A,
            "path:/home/vaz/Code/thing",
            target_identity,
            false,
        ),
        valid_row(
            &root,
            SOURCE_B,
            "path:/fixture/marker-root",
            target_identity,
            false,
        ),
    ];
    let plan = write_plan(&parent.join("plans"), &root, rows);
    let (code, stdout, stderr) =
        run_consolidate(&root, &plan, false, Some(&fixture_custody(&parent)));
    assert_eq!(code, Some(0), "duplicates collapse clean; stderr: {stderr}");
    let report = report_json(&stdout, &stderr);
    let groups = report["groups"].as_array().expect("groups array");
    assert_eq!(groups.len(), 1, "one destination group");
    assert_eq!(groups[0]["survivors"], 1, "ONE survivor");
    assert_eq!(groups[0]["occurrences"], 3, "THREE provenance occurrences");

    let staged = staged_root(&parent);
    let merged = read_rows(&staged_shard(&staged, &target_hash, 0));
    assert_eq!(ids(&merged), vec!["dup"]);
    assert_eq!(registry_rows(&staged, &target_hash), vec![(0, 1, 1)]);
    let _ = std::fs::remove_dir_all(&parent);
}

/// Divergence under one ID aborts the WHOLE plan before any write: no
/// latest-wins, no content-only comparison.
///
/// Mutants killed: latest-timestamp-wins and a compare that ignores the pin
/// flag or `metadata`.
#[test]
fn test_divergent_ids_abort_whole_plan() {
    const TARGET: &str = "1111111111111111";
    const SOURCE: &str = "2222222222222222";

    let parent = unique_temp_dir("merge-divergent");
    let root = parent.join("data");
    std::fs::create_dir_all(&root).expect("fixture root");
    {
        let fx = fixture(&root);
        fx.seed_row(
            TARGET,
            &record("clash", &format!("opencode_project_{TARGET}")),
        );
        fx.seed_row(
            SOURCE,
            &record("clash", &format!("opencode_project_{SOURCE}")),
        );
        fx.seed_row(
            SOURCE,
            &record("only-source", &format!("opencode_project_{SOURCE}")),
        );
    }
    let target_identity = "path:/home/opencode/Code/thing";
    let rows = vec![
        valid_row(&root, TARGET, target_identity, target_identity, false),
        valid_row(
            &root,
            SOURCE,
            "path:/home/vaz/Code/thing",
            target_identity,
            false,
        ),
    ];
    let plan = write_plan(&parent.join("plans"), &root, rows);

    // Subcase A — one metadata cell diverges.
    let source_shard = root
        .join("projects")
        .join(format!("project_{SOURCE}_shard_0.db"));
    fixture_exec(
        &root,
        &source_shard,
        "UPDATE memories SET metadata = '{\"branch\":\"b\"}' WHERE id = 'clash'",
        Vec::new(),
    );
    fixture_exec(
        &root,
        &root
            .join("projects")
            .join(format!("project_{TARGET}_shard_0.db")),
        "UPDATE memories SET metadata = '{\"branch\":\"a\"}' WHERE id = 'clash'",
        Vec::new(),
    );
    let before = tree_digest(&root);
    let (code, _stdout, stderr) =
        run_consolidate(&root, &plan, false, Some(&fixture_custody(&parent)));
    assert_refusal(code, 71, &stderr, "consolidation.rows_divergent");
    assert_eq!(before, tree_digest(&root), "an aborted plan writes nothing");
    assert!(
        staged_root_opt(&parent).is_none(),
        "no staged tree is built for an aborted plan"
    );

    // Subcase B — identical payloads except the pin flag.
    fixture_exec(
        &root,
        &source_shard,
        "UPDATE memories SET metadata = NULL WHERE id = 'clash'",
        Vec::new(),
    );
    fixture_exec(
        &root,
        &root
            .join("projects")
            .join(format!("project_{TARGET}_shard_0.db")),
        "UPDATE memories SET metadata = NULL WHERE id = 'clash'",
        Vec::new(),
    );
    fixture_exec(
        &root,
        &source_shard,
        "UPDATE memories SET is_pinned = 1 WHERE id = 'clash'",
        Vec::new(),
    );
    let before = tree_digest(&root);
    let (code, _stdout, stderr) =
        run_consolidate(&root, &plan, false, Some(&fixture_custody(&parent)));
    assert_refusal(code, 71, &stderr, "consolidation.rows_divergent");
    assert_eq!(before, tree_digest(&root), "an aborted plan writes nothing");
    let _ = std::fs::remove_dir_all(&parent);
}

fn staged_root_opt(parent: &Path) -> Option<PathBuf> {
    std::fs::read_dir(parent)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("consolidation-staged-"))
        })
}

/// A populated shard whose embedded identity is unknown or unequal refuses
/// with 73 BEFORE any byte changes.
///
/// Mutants killed: relabelling an incompatible fingerprint, and treating a
/// missing required identity key as adoptable.
#[test]
fn test_embedding_identity_mismatch_refuses() {
    const TARGET: &str = "1111111111111111";
    const SOURCE: &str = "2222222222222222";

    let parent = unique_temp_dir("merge-embedding");
    let root = parent.join("data");
    std::fs::create_dir_all(&root).expect("fixture root");
    {
        let fx = fixture(&root);
        fx.seed_row(
            TARGET,
            &record("row-target", &format!("opencode_project_{TARGET}")),
        );
        fx.seed_row(
            SOURCE,
            &record("row-source", &format!("opencode_project_{SOURCE}")),
        );
    }
    let target_identity = "path:/home/opencode/Code/thing";
    let rows = vec![
        valid_row(&root, TARGET, target_identity, target_identity, false),
        valid_row(
            &root,
            SOURCE,
            "path:/home/vaz/Code/thing",
            target_identity,
            false,
        ),
    ];
    let plan = write_plan(&parent.join("plans"), &root, rows);
    let target_shard = root
        .join("projects")
        .join(format!("project_{TARGET}_shard_0.db"));

    // Subcase A — a populated fingerprint that differs from the build's.
    fixture_exec(
        &root,
        &target_shard,
        "INSERT OR REPLACE INTO shard_metadata (key, value) VALUES ('embedding_fingerprint', 'not-this-build')",
        Vec::new(),
    );
    let before = tree_digest(&root);
    let (code, _stdout, stderr) =
        run_consolidate(&root, &plan, false, Some(&fixture_custody(&parent)));
    assert_refusal(code, 73, &stderr, "consolidation.embedding_mismatch");
    assert_eq!(
        before,
        tree_digest(&root),
        "the refused plan mutates nothing"
    );
    assert!(
        staged_root_opt(&parent).is_none(),
        "no staged tree is built"
    );

    // Subcase B — a populated shard missing a required identity key.
    fixture_exec(
        &root,
        &target_shard,
        "DELETE FROM shard_metadata WHERE key = 'embedding_fingerprint'",
        Vec::new(),
    );
    fixture_exec(
        &root,
        &target_shard,
        "DELETE FROM shard_metadata WHERE key = 'embedding_model'",
        Vec::new(),
    );
    let before = tree_digest(&root);
    let (code, _stdout, stderr) =
        run_consolidate(&root, &plan, false, Some(&fixture_custody(&parent)));
    assert_refusal(code, 73, &stderr, "consolidation.embedding_mismatch");
    assert_eq!(
        before,
        tree_digest(&root),
        "the refused plan mutates nothing"
    );
    let _ = std::fs::remove_dir_all(&parent);
}

/// Capacity splits survivors across shards (2 + 1), only the highest shard is
/// active, and the staged registry carries the real counts.
///
/// Mutants killed: a cached configured capacity, activating every shard, and a
/// registry that keeps retired source rows.
#[test]
fn test_capacity_and_registry_counts() {
    const TARGET: &str = "1111111111111111";
    const SOURCE_A: &str = "2222222222222222";
    const SOURCE_B: &str = "3333333333333333";

    let parent = unique_temp_dir("merge-capacity");
    let root = parent.join("data");
    std::fs::create_dir_all(&root).expect("fixture root");
    let home = parent.join("home");
    write_fixture_config(&home, 2);
    {
        let fx = fixture(&root);
        fx.seed_row(TARGET, &record("a", &format!("opencode_project_{TARGET}")));
        fx.seed_row(
            SOURCE_A,
            &record("b", &format!("opencode_project_{SOURCE_A}")),
        );
        fx.seed_row(
            SOURCE_B,
            &record("c", &format!("opencode_project_{SOURCE_B}")),
        );
        // One exact duplicate (same payload after normalization) rides along.
        fx.seed_row(
            SOURCE_B,
            &record("b", &format!("opencode_project_{SOURCE_B}")),
        );
    }
    // Normalize the duplicate's payload so only the association differs.
    let duplicate_shard = root
        .join("projects")
        .join(format!("project_{SOURCE_B}_shard_0.db"));
    fixture_exec(
        &root,
        &duplicate_shard,
        "UPDATE memories SET content = 'content b', vector = vector32('[0.1,0.2]'), \
         created_at = 1700000000000, updated_at = 1700000000000 \
         WHERE id = 'b'",
        Vec::new(),
    );

    let target_identity = "path:/home/opencode/Code/thing";
    let target_hash = identity_hash(target_identity);
    let rows = vec![
        valid_row(&root, TARGET, target_identity, target_identity, false),
        valid_row(
            &root,
            SOURCE_A,
            "path:/home/vaz/Code/thing",
            target_identity,
            false,
        ),
        valid_row(
            &root,
            SOURCE_B,
            "path:/fixture/marker-root",
            target_identity,
            false,
        ),
    ];
    let plan = write_plan(&parent.join("plans"), &root, rows);
    let (code, stdout, stderr) =
        run_consolidate(&root, &plan, false, Some(&fixture_custody(&parent)));
    assert_eq!(
        code,
        Some(0),
        "the capacity split applies clean; stderr: {stderr}"
    );
    let report = report_json(&stdout, &stderr);
    assert_eq!(report["groups"][0]["survivors"], 3);

    let staged = staged_root(&parent);
    assert_eq!(
        ids(&read_rows(&staged_shard(&staged, &target_hash, 0))),
        vec!["a", "b"],
        "the first shard holds capacity rows"
    );
    assert_eq!(
        ids(&read_rows(&staged_shard(&staged, &target_hash, 1))),
        vec!["c"],
        "the continuation shard holds the rest"
    );
    assert_eq!(
        registry_rows(&staged, &target_hash),
        vec![(0, 2, 0), (1, 1, 1)],
        "actual counts, only the highest shard active"
    );
    for hash in [TARGET, SOURCE_A, SOURCE_B] {
        assert!(
            registry_rows(&staged, hash).len() <= 2,
            "retired source rows are replaced, never kept"
        );
    }
    assert!(
        registry_rows(&staged, SOURCE_A).is_empty(),
        "a consolidated-away source has no registry row"
    );
    assert!(
        registry_rows(&staged, SOURCE_B).is_empty(),
        "a consolidated-away source has no registry row"
    );

    // Capacity 0 refuses (an unusable setting, never a silent single shard).
    let parent = unique_temp_dir("merge-capacity-zero");
    let root = parent.join("data");
    std::fs::create_dir_all(&root).expect("fixture root");
    write_fixture_config(&parent.join("home"), 0);
    {
        let fx = fixture(&root);
        fx.seed_row(TARGET, &record("a", &format!("opencode_project_{TARGET}")));
    }
    let rows = vec![valid_row(
        &root,
        TARGET,
        target_identity,
        target_identity,
        false,
    )];
    let plan = write_plan(&parent.join("plans"), &root, rows);
    let before = tree_digest(&root);
    let (code, _stdout, stderr) =
        run_consolidate(&root, &plan, false, Some(&fixture_custody(&parent)));
    assert_refusal(code, 64, &stderr, "consolidation.capacity_unusable");
    assert_eq!(before, tree_digest(&root), "the refusal writes nothing");
    let _ = std::fs::remove_dir_all(&parent);
}

/// A foreign reservation blocks apply before any write (F11: the refusal line
/// names only `operation` + `root_basename`), and the staged tree is never
/// built under a held root.
#[test]
fn test_foreign_writer_and_reservation_block_startup() {
    const HASH: &str = "1111111111111111";

    let parent = unique_temp_dir("merge-foreign-holder");
    let root = parent.join("data");
    std::fs::create_dir_all(&root).expect("fixture root");
    {
        let fx = fixture(&root);
        fx.seed_row(HASH, &record("row", &format!("opencode_project_{HASH}")));
    }
    let identity = "path:/fixture/self";
    let rows = vec![valid_row(&root, HASH, identity, identity, false)];
    let plan = write_plan(&parent.join("plans"), &root, rows);

    // A foreign holder: this pid with a different start-time token (F9).
    let body = serde_json::json!({
        "pid": std::process::id(),
        "start_time": 1_u64,
        "operation": "maintenance",
        "root_basename": "data",
        "nonce": "AAAAAAAAAAAAAAAAAAAAAA==",
        "created_utc": "2026-11-12T00:00:00+00:00",
        "retake_seq": null,
        "s_dev": 1_u64,
        "s_ino": 1_u64,
    });
    std::fs::write(parent.join(RESERVATION_FILE), body.to_string()).expect("write reservation");

    let before = tree_digest(&root);
    let (code, _stdout, stderr) =
        run_consolidate(&root, &plan, false, Some(&fixture_custody(&parent)));
    assert_eq!(
        code,
        Some(mem_store::maintenance_reservation::RESERVATION_HELD),
        "a held reservation refuses apply with 76; stderr: {stderr}"
    );
    assert_reservation_refusal_line(&stderr, "data", "consolidate-shards");
    assert!(
        staged_root_opt(&parent).is_none(),
        "no staged tree is built"
    );
    assert_eq!(
        before,
        tree_digest(&root),
        "the refused apply mutates nothing"
    );
    let _ = std::fs::remove_dir_all(&parent);
}

// ---------------------------------------------------------------------------
// Step 5 — auxiliary closure (§C.2–§C.5)
// ---------------------------------------------------------------------------

/// The episodic tier exactly as the engine creates it (five tables, dims 2).
const EPISODIC_FIXTURE_DDL: [&str; 5] = [
    "CREATE TABLE IF NOT EXISTS agent_sessions (
       id TEXT PRIMARY KEY, project_id TEXT NOT NULL, session_id TEXT NOT NULL,
       agent_id TEXT NOT NULL, parent_session_id TEXT, summary TEXT NOT NULL,
       metadata TEXT NOT NULL, embedding F32_BLOB(2) NOT NULL, created_at INTEGER NOT NULL,
       updated_at INTEGER NOT NULL, deleted_at INTEGER, orphaned INTEGER NOT NULL DEFAULT 0,
       capture_id TEXT, agent_role TEXT NOT NULL DEFAULT 'main',
       UNIQUE (project_id, session_id, agent_id))",
    "CREATE TABLE IF NOT EXISTS episodic_milestones (
       id TEXT PRIMARY KEY, project_id TEXT NOT NULL, agent_id TEXT NOT NULL,
       summary TEXT NOT NULL, contributors TEXT NOT NULL, source_count INTEGER NOT NULL,
       window_start INTEGER NOT NULL, window_end INTEGER NOT NULL,
       embedding F32_BLOB(2) NOT NULL, created_at INTEGER NOT NULL)",
    "CREATE TABLE IF NOT EXISTS episodic_raw_context (
       project_id TEXT NOT NULL, session_id TEXT NOT NULL, agent_id TEXT NOT NULL,
       raw_context TEXT NOT NULL, byte_len INTEGER NOT NULL, captured_at INTEGER NOT NULL,
       PRIMARY KEY (project_id, session_id, agent_id))",
    "CREATE TABLE IF NOT EXISTS episodic_segments (
       project_id TEXT NOT NULL, session_id TEXT NOT NULL, agent_id TEXT NOT NULL,
       segment_index INTEGER NOT NULL, raw_content TEXT NOT NULL, byte_offset INTEGER NOT NULL,
       byte_len INTEGER NOT NULL, created_at INTEGER NOT NULL,
       PRIMARY KEY (project_id, session_id, agent_id, segment_index))",
    "CREATE TABLE IF NOT EXISTS capture_inbox (
       capture_id TEXT PRIMARY KEY, project_id TEXT NOT NULL, session_id TEXT NOT NULL,
       agent_id TEXT NOT NULL, payload TEXT NOT NULL, created_at INTEGER NOT NULL,
       attempts INTEGER NOT NULL DEFAULT 0, last_attempt_at INTEGER,
       status TEXT NOT NULL DEFAULT 'queued', not_before INTEGER)",
];

/// Creates the five pinned episodic tables under `root` (fixture mutation only).
fn seed_episodic(root: &Path) {
    let db = root.join("episodic.db");
    for statement in EPISODIC_FIXTURE_DDL {
        fixture_exec(root, &db, statement, Vec::new());
    }
}

/// Every row of one auxiliary table as a typed cell map (read-only open).
fn aux_rows(db: &Path, table: &str, order_by: &str) -> Vec<BTreeMap<String, Value>> {
    run_async(async {
        let mut options = ConnectOptions::default();
        options.read_only = true;
        let handle = TursoConnectFactory
            .connect(db.to_path_buf(), options)
            .await
            .unwrap_or_else(|error| panic!("open {} read-only: {error}", db.display()));
        let rows = handle
            .query(
                &format!("SELECT * FROM {table} ORDER BY {order_by}"),
                Vec::new(),
            )
            .await
            .unwrap_or_else(|error| panic!("select {table} from {}: {error}", db.display()));
        rows.iter()
            .map(|row| {
                row.column_names()
                    .iter()
                    .filter_map(|name| row.get(name).map(|value| (name.clone(), value.clone())))
                    .collect()
            })
            .collect()
    })
}

/// The rev-13 auxiliary audit sidecar's records, in file order. Its absence is
/// a failure here: an apply always writes it, empty only when neither kind
/// exists.
fn aux_sidecar_lines(custody: &Path) -> Vec<serde_json::Value> {
    let path = custody.join("journal.json.aux-divergent");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|error| panic!("sidecar line {line:?}: {error}"))
        })
        .collect()
}

/// One auxiliary table's report entry.
fn aux_entry<'a>(report: &'a serde_json::Value, db: &str, table: &str) -> &'a serde_json::Value {
    report["auxiliary"][db]
        .as_array()
        .unwrap_or_else(|| panic!("no auxiliary entry for {db}: {report}"))
        .iter()
        .find(|entry| entry["table"] == table)
        .unwrap_or_else(|| panic!("no auxiliary entry for {table}: {report}"))
}

/// The persisted auxiliary closure: five episodic tables, prompts and skills
/// remapped by whole-string equality, profiles/coordination byte-verbatim.
/// Rev 13: a cell outside the closure is SWAP-AND-CARRIED, never refused — a
/// LEADING `/home/vaz/` prefix is rewritten, a `/home/vaz2` sibling and a
/// MID-PATH `/home/vaz` component are carried unchanged, and each is recorded
/// as one `carry` line in the custody sidecar.
#[test]
fn test_auxiliary_persisted_closure() {
    const SOURCE_A: &str = "4444444444444444";
    const SOURCE_B: &str = "5555555555555555";
    let target_identity = "path:/home/opencode/Code/thing";
    let source_a_identity = "path:/home/vaz/Code/thing";
    let source_b_identity = "path:/fixture/marker-root";
    let target_hash = identity_hash(target_identity);
    let target_tag = format!("opencode_project_{target_hash}");

    let parent = unique_temp_dir("aux-closure");
    let root = parent.join("data");
    std::fs::create_dir_all(&root).expect("fixture root");
    {
        let fx = fixture(&root);
        fx.seed_row(
            SOURCE_A,
            &record("row-a", &format!("opencode_project_{SOURCE_A}")),
        );
        fx.seed_row(
            SOURCE_B,
            &record("row-b", &format!("opencode_project_{SOURCE_B}")),
        );
    }

    seed_episodic(&root);
    let episodic = root.join("episodic.db");
    // §A pins `episodic.db` as "5 tables + sweep_state": the sweep stamp is a
    // known table, carried verbatim — never an unknown-table refusal.
    fixture_exec(
        &root,
        &episodic,
        "CREATE TABLE sweep_state (tier TEXT PRIMARY KEY, last_swept_unix_ms INTEGER NOT NULL)",
        Vec::new(),
    );
    fixture_exec(
        &root,
        &episodic,
        "INSERT INTO sweep_state (tier, last_swept_unix_ms) VALUES ('episodic', 1700000000000)",
        Vec::new(),
    );
    fixture_exec(
        &root,
        &episodic,
        "INSERT INTO agent_sessions (id, project_id, session_id, agent_id, summary, metadata,
           embedding, created_at, updated_at, deleted_at, orphaned)
         VALUES ('as-1', ?1, 's1', 'main', 'summary one', '{}', vector32('[0.5,-1.5]'),
           1700000000000, 1600000000123, 1650000000000, 1)",
        vec![Value::Text(source_a_identity[5..].to_string())],
    );
    fixture_exec(
        &root,
        &episodic,
        "INSERT INTO episodic_milestones (id, project_id, agent_id, summary, contributors,
           source_count, window_start, window_end, embedding, created_at)
         VALUES ('m-1', ?1, 'main', 'milestone', '[]', 1, 1, 2, vector32('[1.0,0.25]'), 3)",
        vec![Value::Text(source_b_identity[5..].to_string())],
    );
    // Two rows citing DIFFERENT source hosts, identical payload: both remap onto
    // the same compound key and must collapse into one survivor.
    for identity in [source_a_identity, source_b_identity] {
        fixture_exec(
            &root,
            &episodic,
            "INSERT INTO episodic_raw_context (project_id, session_id, agent_id, raw_context,
               byte_len, captured_at) VALUES (?1, 's1', 'main', 'raw bytes', 9, 10)",
            vec![Value::Text(identity[5..].to_string())],
        );
    }
    fixture_exec(
        &root,
        &episodic,
        "INSERT INTO episodic_segments (project_id, session_id, agent_id, segment_index,
           raw_content, byte_offset, byte_len, created_at)
         VALUES (?1, 's1', 'main', 0, 'seg', 0, 3, 4)",
        vec![Value::Text(source_a_identity[5..].to_string())],
    );
    fixture_exec(
        &root,
        &episodic,
        "INSERT INTO capture_inbox (capture_id, project_id, session_id, agent_id, payload,
           created_at, not_before) VALUES ('c-1', ?1, 's1', 'main', '{}', 5, NULL)",
        vec![Value::Text(source_b_identity[5..].to_string())],
    );

    let prompts = root.join("user-prompts.db");
    fixture_exec(
        &root,
        &prompts,
        "CREATE TABLE user_prompts (
           id TEXT PRIMARY KEY, session_id TEXT NOT NULL, message_id TEXT NOT NULL,
           project_path TEXT, content TEXT NOT NULL, created_at INTEGER NOT NULL,
           captured INTEGER DEFAULT 0, user_learning_captured BOOLEAN DEFAULT 0,
           linked_memory_id TEXT, capture_attempts INTEGER DEFAULT 0,
           provider_id TEXT, model_id TEXT)",
        Vec::new(),
    );
    for (id, project, created) in [
        ("p-1", Some(source_a_identity), 1_i64),
        ("p-2", None, 2_i64),
        ("p-3", Some(target_identity), 3_i64),
    ] {
        fixture_exec(
            &root,
            &prompts,
            "INSERT INTO user_prompts (id, session_id, message_id, project_path, content,
               created_at) VALUES (?1, 's1', ?1, ?2, 'prompt', ?3)",
            vec![
                Value::Text(id.to_string()),
                project.map_or(Value::Null, |value| Value::Text(value[5..].to_string())),
                Value::Integer(created),
            ],
        );
    }
    fixture_exec(
        &root,
        &prompts,
        "CREATE TABLE sweep_state (tier TEXT PRIMARY KEY, last_swept_unix_ms INTEGER NOT NULL)",
        Vec::new(),
    );

    let skills = root.join("skills.db");
    fixture_exec(
        &root,
        &skills,
        "CREATE TABLE skills (
           id TEXT PRIMARY KEY, name TEXT NOT NULL, description TEXT NOT NULL DEFAULT '',
           content TEXT NOT NULL, autoinvoke INTEGER NOT NULL DEFAULT 1,
           enabled INTEGER NOT NULL DEFAULT 1, container_tag TEXT,
           created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL)",
        Vec::new(),
    );
    for (id, tag) in [
        ("k-1", Some(format!("opencode_project_{SOURCE_A}"))),
        ("k-2", None),
    ] {
        fixture_exec(
            &root,
            &skills,
            "INSERT INTO skills (id, name, content, container_tag, created_at, updated_at)
             VALUES (?1, ?1, 'body', ?2, 1, 2)",
            vec![
                Value::Text(id.to_string()),
                tag.map_or(Value::Null, Value::Text),
            ],
        );
    }

    // Carried verbatim: no project binding, never opened by the merge.
    for (name, table_ddl, row_insert) in [
        (
            "user-profiles.db",
            "CREATE TABLE user_profiles (id TEXT PRIMARY KEY, name TEXT NOT NULL)",
            "INSERT INTO user_profiles (id, name) VALUES ('u-1', 'vaz')",
        ),
        (
            ".profile-learning-coordination.db",
            "CREATE TABLE profile_learning_lock (id INTEGER PRIMARY KEY, holder TEXT)",
            "INSERT INTO profile_learning_lock (id, holder) VALUES (1, 'session')",
        ),
    ] {
        let db = root.join(name);
        fixture_exec(&root, &db, table_ddl, Vec::new());
        fixture_exec(&root, &db, row_insert, Vec::new());
    }
    let profiles_before = std::fs::read(root.join("user-profiles.db")).expect("profiles bytes");
    let coordination_before =
        std::fs::read(root.join(".profile-learning-coordination.db")).expect("coordination bytes");

    let rows = vec![
        valid_row(&root, SOURCE_A, source_a_identity, target_identity, false),
        valid_row(&root, SOURCE_B, source_b_identity, target_identity, false),
    ];
    let plan = write_plan(&parent.join("plans"), &root, rows);
    let (code, stdout, stderr) =
        run_consolidate(&root, &plan, false, Some(&fixture_custody(&parent)));
    assert_eq!(
        code,
        Some(0),
        "the auxiliary closure applies clean; stderr: {stderr}"
    );
    let report = report_json(&stdout, &stderr);
    let auxiliary = aux_entry(&report, "episodic.db", "episodic_raw_context");
    assert_eq!(
        auxiliary["collapsed"], 1,
        "the exact-duplicate collapse is reported: {report}"
    );

    let staged = staged_root(&parent);
    let staged_episodic = staged.join("episodic.db");
    let expected = "/home/opencode/Code/thing";
    let sessions = aux_rows(&staged_episodic, "agent_sessions", "id");
    assert_eq!(sessions.len(), 1, "one session row survives");
    assert_eq!(text_cell(&sessions[0], "project_id"), expected);
    assert_eq!(text_cell(&sessions[0], "summary"), "summary one");
    assert_eq!(text_cell(&sessions[0], "metadata"), "{}");
    assert_eq!(
        cell(&sessions[0], "created_at").as_i64(),
        Some(1700000000000),
        "integer width and timestamps survive verbatim"
    );
    assert_eq!(
        cell(&sessions[0], "deleted_at").as_i64(),
        Some(1650000000000)
    );
    assert_eq!(cell(&sessions[0], "orphaned").as_i64(), Some(1));

    let milestones = aux_rows(&staged_episodic, "episodic_milestones", "id");
    assert_eq!(milestones.len(), 1);
    assert_eq!(text_cell(&milestones[0], "project_id"), expected);
    assert_eq!(text_cell(&milestones[0], "contributors"), "[]");

    let raw = aux_rows(&staged_episodic, "episodic_raw_context", "session_id");
    assert_eq!(
        raw.len(),
        1,
        "equal payloads collapse onto the compound key"
    );
    assert_eq!(text_cell(&raw[0], "project_id"), expected);
    assert_eq!(text_cell(&raw[0], "raw_context"), "raw bytes");

    let segments = aux_rows(&staged_episodic, "episodic_segments", "segment_index");
    assert_eq!(segments.len(), 1);
    assert_eq!(text_cell(&segments[0], "project_id"), expected);
    assert_eq!(text_cell(&segments[0], "raw_content"), "seg");

    let inbox = aux_rows(&staged_episodic, "capture_inbox", "capture_id");
    assert_eq!(inbox.len(), 1);
    assert_eq!(text_cell(&inbox[0], "project_id"), expected);
    assert_eq!(text_cell(&inbox[0], "status"), "queued");

    let sweep = aux_rows(&staged_episodic, "sweep_state", "tier");
    assert_eq!(
        sweep.len(),
        1,
        "the pinned sweep_state row is carried, not dropped or refused"
    );
    assert_eq!(text_cell(&sweep[0], "tier"), "episodic");
    assert_eq!(
        cell(&sweep[0], "last_swept_unix_ms").as_i64(),
        Some(1700000000000)
    );

    let staged_prompts = staged.join("user-prompts.db");
    let prompt_rows = aux_rows(&staged_prompts, "user_prompts", "id");
    assert_eq!(prompt_rows.len(), 3, "no prompt row is invented or lost");
    assert_eq!(text_cell(&prompt_rows[0], "project_path"), expected);
    assert_eq!(
        cell(&prompt_rows[1], "project_path"),
        &Value::Null,
        "NULL project_path stays NULL"
    );
    assert_eq!(
        text_cell(&prompt_rows[2], "project_path"),
        expected,
        "a target-equal row passes untouched"
    );
    assert_eq!(
        aux_rows(&staged_prompts, "sweep_state", "tier").len(),
        0,
        "a known carried table is not remapped"
    );

    let skill_rows = aux_rows(&staged.join("skills.db"), "skills", "id");
    assert_eq!(skill_rows.len(), 2);
    assert_eq!(text_cell(&skill_rows[0], "container_tag"), target_tag);
    assert_eq!(
        cell(&skill_rows[1], "container_tag"),
        &Value::Null,
        "a global skill stays global"
    );

    assert_eq!(
        std::fs::read(staged.join("user-profiles.db")).expect("staged profiles bytes"),
        profiles_before,
        "user-profiles.db carries byte-verbatim"
    );
    assert_eq!(
        std::fs::read(staged.join(".profile-learning-coordination.db"))
            .expect("staged coordination bytes"),
        coordination_before,
        "the coordination DB carries byte-verbatim"
    );

    // Rev 13: the branch decision is taken on the ORIGINAL value, and a value
    // outside the closure is SWAP-AND-CARRIED, never refused. Three cases, each
    // the only input that separates the pinned rule from a mutant: a LEADING
    // prefix is swapped; a `/home/vaz2` sibling is not a `/home/vaz` component;
    // and a `/home/vaz` further along the string is CONTENT and stays. All
    // three run on the APPLY path, because "exit 0" alone is satisfied by a
    // mutant that drops the row or carries it unswapped.
    for (case, outside, staged_value) in [
        (
            "subpath",
            "/home/vaz/Code/thing/sub",
            "/home/opencode/Code/thing/sub",
        ),
        ("sibling", "/home/vaz2/Code/thing", "/home/vaz2/Code/thing"),
        ("mid-path", "/tmp/x/home/vaz/y", "/tmp/x/home/vaz/y"),
    ] {
        let parent = unique_temp_dir(&format!("aux-outside-{case}"));
        let root = parent.join("data");
        std::fs::create_dir_all(&root).expect("fixture root");
        {
            let fx = fixture(&root);
            fx.seed_row(
                SOURCE_A,
                &record("row-a", &format!("opencode_project_{SOURCE_A}")),
            );
        }
        seed_episodic(&root);
        fixture_exec(
            &root,
            &root.join("episodic.db"),
            "INSERT INTO episodic_raw_context (project_id, session_id, agent_id, raw_context,
               byte_len, captured_at) VALUES (?1, 's1', 'main', 'raw', 3, 1)",
            vec![Value::Text(outside.to_string())],
        );
        let rows = vec![valid_row(
            &root,
            SOURCE_A,
            source_a_identity,
            target_identity,
            false,
        )];
        let plan = write_plan(&parent.join("plans"), &root, rows);
        let custody = fixture_custody(&parent);
        let before = tree_digest(&root);
        let (code, _stdout, stderr) = run_consolidate(&root, &plan, false, Some(&custody));
        assert_eq!(
            code,
            Some(0),
            "{case}: a carried cell never refuses; stderr: {stderr}"
        );
        assert_eq!(
            before,
            tree_digest(&root),
            "{case}: the apply mutates no S byte"
        );
        let staged = staged_root(&parent);
        let raw = aux_rows(
            &staged.join("episodic.db"),
            "episodic_raw_context",
            "session_id",
        );
        assert_eq!(raw.len(), 1, "{case}: the carried row is never dropped");
        assert_eq!(
            text_cell(&raw[0], "project_id"),
            staged_value,
            "{case}: the staged cell is the swap of the ORIGINAL value"
        );
        let lines = aux_sidecar_lines(&custody);
        assert_eq!(lines.len(), 1, "{case}: exactly one carry line: {lines:?}");
        assert_eq!(lines[0]["kind"], "carry");
        assert_eq!(lines[0]["db"], "episodic.db");
        assert_eq!(lines[0]["table"], "episodic_raw_context");
        assert_eq!(lines[0]["column"], "project_id");
        assert_eq!(lines[0]["original"], outside);
        assert_eq!(lines[0]["carried"], staged_value);
        assert_eq!(lines[0]["rows"], 1);
        let _ = std::fs::remove_dir_all(&parent);
    }

    // Rev 13, branch (a) beside branch (c) IN THE SAME TABLE: a cell that equals
    // a mapped source EXACTLY is a MAPPING rewrite (`Mapped`) and records
    // NOTHING; a cell outside the closure is `Carried` and is ALWAYS recorded.
    // A `Some`-means-carry reading tags both, so the oracle is the SIDECAR's
    // carry-line set — the staged row alone cannot tell the two branches apart.
    {
        let parent = unique_temp_dir("aux-mapped-vs-carried");
        let root = parent.join("data");
        std::fs::create_dir_all(&root).expect("fixture root");
        {
            let fx = fixture(&root);
            fx.seed_row(
                SOURCE_A,
                &record("row-a", &format!("opencode_project_{SOURCE_A}")),
            );
        }
        seed_episodic(&root);
        for value in ["/home/vaz/Code/thing", "/home/vaz/Code/thing/sub"] {
            fixture_exec(
                &root,
                &root.join("episodic.db"),
                "INSERT INTO episodic_raw_context (project_id, session_id, agent_id, raw_context,
                   byte_len, captured_at) VALUES (?1, 's1', 'main', 'raw', 3, 1)",
                vec![Value::Text(value.to_string())],
            );
        }
        let rows = vec![valid_row(
            &root,
            SOURCE_A,
            source_a_identity,
            target_identity,
            false,
        )];
        let plan = write_plan(&parent.join("plans"), &root, rows);
        let custody = fixture_custody(&parent);
        let (code, _stdout, stderr) = run_consolidate(&root, &plan, false, Some(&custody));
        assert_eq!(
            code,
            Some(0),
            "branch (a) and branch (c) both apply clean; {stderr}"
        );
        let staged = staged_root(&parent);
        let raw = aux_rows(
            &staged.join("episodic.db"),
            "episodic_raw_context",
            "project_id",
        );
        assert_eq!(raw.len(), 2, "neither row is dropped");
        assert_eq!(
            text_cell(&raw[0], "project_id"),
            "/home/opencode/Code/thing",
            "branch (a) rewrites the exact mapped source to its target"
        );
        assert_eq!(
            text_cell(&raw[1], "project_id"),
            "/home/opencode/Code/thing/sub",
            "branch (c) carries the swapped subpath"
        );
        let lines = aux_sidecar_lines(&custody);
        let carries: Vec<&serde_json::Value> = lines
            .iter()
            .filter(|line| line["kind"] == "carry")
            .collect();
        assert_eq!(
            carries.len(),
            1,
            "a Mapped cell contributes NO carry line (Some-means-carry mutant): {lines:?}"
        );
        assert_eq!(carries[0]["original"], "/home/vaz/Code/thing/sub");
        assert_eq!(carries[0]["carried"], "/home/opencode/Code/thing/sub");
        assert_eq!(carries[0]["rows"], 1);
        assert!(
            !lines
                .iter()
                .any(|line| line["original"] == "/home/vaz/Code/thing"),
            "branch (a) is a mapping rewrite, never a carry record: {lines:?}"
        );
        let _ = std::fs::remove_dir_all(&parent);
    }

    // §B.5 host-path rule for a `git-common:` group: the project directory is
    // the REPO ROOT (TagResolver's `get_git_project_root`), never the `.git`
    // directory — no wire `project_id` is ever a `.git` path (§B.1).
    {
        const HASH: &str = "6666666666666666";
        let source_identity = "git-common:/home/vaz/Code/repo/.git";
        let target_identity = "git-common:/home/opencode/Code/repo/.git";
        let target_hash = identity_hash(target_identity);
        let parent = unique_temp_dir("aux-git-common-host");
        let root = parent.join("data");
        std::fs::create_dir_all(&root).expect("fixture root");
        {
            let fx = fixture(&root);
            fx.seed(HASH, "row-git");
        }
        seed_episodic(&root);
        fixture_exec(
            &root,
            &root.join("episodic.db"),
            "INSERT INTO agent_sessions (id, project_id, session_id, agent_id, summary, metadata,
               embedding, created_at, updated_at, deleted_at, orphaned)
             VALUES ('as-git', '/home/vaz/Code/repo', 's1', 'main', 's', '{}',
               vector32('[0.5,-1.5]'), 1, 2, NULL, 0)",
            Vec::new(),
        );
        let rows = vec![plan_row(
            &root,
            HASH,
            source_identity,
            &target_hash,
            target_identity,
            false,
        )];
        let plan = write_plan(&parent.join("plans"), &root, rows);
        let (code, _stdout, stderr) =
            run_consolidate(&root, &plan, false, Some(&fixture_custody(&parent)));
        assert_eq!(
            code,
            Some(0),
            "a git-common group's episodic closure plans clean; stderr: {stderr}"
        );
        let staged = staged_root(&parent);
        let sessions = aux_rows(&staged.join("episodic.db"), "agent_sessions", "id");
        assert_eq!(sessions.len(), 1);
        assert_eq!(
            text_cell(&sessions[0], "project_id"),
            "/home/opencode/Code/repo",
            "the remap target is the repo root, not the .git directory"
        );
        let _ = std::fs::remove_dir_all(&parent);
    }

    let _ = std::fs::remove_dir_all(&parent);
}

/// Two or more auxiliary rows collapsing onto ONE identity key with divergent
/// payloads resolve KEEP-NEWER — never 71 — and every loser is recorded (§C.5
/// rev 13). Discriminators: the byte-LESSER row is the NEWER one, so tiers (1)
/// and (3) disagree and a serialization-only mutant dies; the three-row group
/// records TWO losers (`discarded` is an array, not a single row); a group
/// whose ONLY rewrite trigger is the discard term must still rewrite the staged
/// table (the old two-term skip leaves the loser behind); and a non-`Integer`
/// `created_at` in a divergent group still refuses 71 before any comparison.
#[test]
fn test_auxiliary_collision_aborts() {
    const SOURCE_A: &str = "4444444444444444";
    const SOURCE_B: &str = "5555555555555555";
    const SOURCE_C: &str = "7777777777777777";
    let target_identity = "path:/home/opencode/Code/thing";
    let source_a_identity = "path:/home/vaz/Code/thing";
    let source_b_identity = "path:/fixture/marker-root";
    let source_c_identity = "path:/fixture/third-root";
    let target = "/home/opencode/Code/thing";

    // (1) keep-newer with TWO recorded losers. Three rows, three DIFFERENT
    // pre-remap project_ids (so the fixture's PK/UNIQUE hold), ONE compound key
    // after the remap. `created_at` descends while the canonical serialization
    // ascends, so only tier (1) keeps `seg one`.
    let parent = unique_temp_dir("aux-collision");
    let root = parent.join("data");
    std::fs::create_dir_all(&root).expect("fixture root");
    {
        let fx = fixture(&root);
        for (hash, name) in [
            (SOURCE_A, "row-a"),
            (SOURCE_B, "row-b"),
            (SOURCE_C, "row-c"),
        ] {
            fx.seed_row(hash, &record(name, &format!("opencode_project_{hash}")));
        }
    }
    seed_episodic(&root);
    for (identity, raw, created) in [
        (source_a_identity, "seg one", 30_i64),
        (source_b_identity, "seg two", 20_i64),
        (source_c_identity, "seg three", 10_i64),
    ] {
        fixture_exec(
            &root,
            &root.join("episodic.db"),
            "INSERT INTO episodic_segments (project_id, session_id, agent_id, segment_index,
               raw_content, byte_offset, byte_len, created_at)
             VALUES (?1, 's2', 'main', 0, ?2, 0, ?3, ?4)",
            vec![
                Value::Text(identity[5..].to_string()),
                Value::Text(raw.to_string()),
                Value::Integer(raw.len() as i64),
                Value::Integer(created),
            ],
        );
    }
    let rows = vec![
        valid_row(&root, SOURCE_A, source_a_identity, target_identity, false),
        valid_row(&root, SOURCE_B, source_b_identity, target_identity, false),
        valid_row(&root, SOURCE_C, source_c_identity, target_identity, false),
    ];
    let plan = write_plan(&parent.join("plans"), &root, rows);
    let custody = fixture_custody(&parent);
    let before = tree_digest(&root);
    let (code, _stdout, stderr) = run_consolidate(&root, &plan, false, Some(&custody));
    assert_eq!(
        code,
        Some(0),
        "rev 13 resolves a divergent auxiliary group instead of aborting; {stderr}"
    );
    assert_eq!(before, tree_digest(&root), "the apply mutates no S byte");

    let staged = staged_root(&parent);
    let segments = aux_rows(
        &staged.join("episodic.db"),
        "episodic_segments",
        "segment_index",
    );
    assert_eq!(segments.len(), 1, "one row survives the three-row group");
    assert_eq!(text_cell(&segments[0], "project_id"), target);
    assert_eq!(
        text_cell(&segments[0], "raw_content"),
        "seg one",
        "keep-newer picks the greatest created_at, not the greatest bytes"
    );
    assert_eq!(cell(&segments[0], "created_at").as_i64(), Some(30));

    let lines = aux_sidecar_lines(&custody);
    let divergent: Vec<&serde_json::Value> = lines
        .iter()
        .filter(|line| line["kind"] == "divergent")
        .collect();
    assert_eq!(
        divergent.len(),
        1,
        "one divergent line per collapsed key: {lines:?}"
    );
    let line = divergent[0];
    assert_eq!(line["db"], "episodic.db");
    assert_eq!(line["table"], "episodic_segments");
    assert_eq!(line["reason"], "keep-newer");
    assert_eq!(
        line["key_set"],
        serde_json::json!(["project_id", "session_id", "agent_id", "segment_index"])
    );
    assert_eq!(line["key"]["project_id"], target);
    assert_eq!(line["kept"]["raw_content"], "seg one");
    let discarded = line["discarded"]
        .as_array()
        .expect("`discarded` is an ARRAY of every loser");
    assert_eq!(discarded.len(), 2, "the three-row group records TWO losers");
    let mut recorded: Vec<&str> = discarded
        .iter()
        .map(|row| row["raw_content"].as_str().expect("discarded raw_content"))
        .collect();
    recorded.sort_unstable();
    assert_eq!(recorded, vec!["seg three", "seg two"]);
    assert_eq!(
        before,
        tree_digest(&root),
        "a resolved collision still mutates no S byte"
    );
    let _ = std::fs::remove_dir_all(&parent);

    // (2) a group whose ONLY rewrite trigger is the discard term: two
    // `agent_sessions` rows already holding the TARGET project_id
    // (`remapped == 0`), not byte-equal (`collapsed == 0`), colliding in pass 2
    // on `(project_id, session_id, agent_id)`. The old two-term skip leaves the
    // loser in the staged table, so the STAGED ROW SET is the assertion.
    //
    // The fixture's table omits `UNIQUE (project_id, session_id, agent_id)`: a
    // duplicate triple is unreachable while that constraint holds, and SQLite
    // cannot drop a UNIQUE constraint, so this row set is the legacy
    // pre-constraint shape the collapse exists to resolve. Every column is the
    // pinned one; only the constraint is absent.
    let parent = unique_temp_dir("aux-discard-only");
    let root = parent.join("data");
    std::fs::create_dir_all(&root).expect("fixture root");
    {
        let fx = fixture(&root);
        fx.seed_row(
            SOURCE_A,
            &record("row-a", &format!("opencode_project_{SOURCE_A}")),
        );
    }
    let episodic = root.join("episodic.db");
    fixture_exec(
        &root,
        &episodic,
        "CREATE TABLE agent_sessions (
           id TEXT PRIMARY KEY, project_id TEXT NOT NULL, session_id TEXT NOT NULL,
           agent_id TEXT NOT NULL, parent_session_id TEXT, summary TEXT NOT NULL,
           metadata TEXT NOT NULL, embedding F32_BLOB(2) NOT NULL, created_at INTEGER NOT NULL,
           updated_at INTEGER NOT NULL, deleted_at INTEGER, orphaned INTEGER NOT NULL DEFAULT 0,
           capture_id TEXT, agent_role TEXT NOT NULL DEFAULT 'main')",
        Vec::new(),
    );
    for (id, created) in [("as-old", 10_i64), ("as-new", 20_i64)] {
        fixture_exec(
            &root,
            &episodic,
            "INSERT INTO agent_sessions (id, project_id, session_id, agent_id, summary, metadata,
               embedding, created_at, updated_at, deleted_at, orphaned)
             VALUES (?1, ?2, 's3', 'main', ?1, '{}', vector32('[0.5,-1.5]'), ?3, 1, NULL, 0)",
            vec![
                Value::Text(id.to_string()),
                Value::Text(target.to_string()),
                Value::Integer(created),
            ],
        );
    }
    let rows = vec![valid_row(
        &root,
        SOURCE_A,
        source_a_identity,
        target_identity,
        false,
    )];
    let plan = write_plan(&parent.join("plans"), &root, rows);
    let custody = fixture_custody(&parent);
    let (code, _stdout, stderr) = run_consolidate(&root, &plan, false, Some(&custody));
    assert_eq!(code, Some(0), "the discard-only group resolves; {stderr}");
    let staged = staged_root(&parent);
    let sessions = aux_rows(&staged.join("episodic.db"), "agent_sessions", "id");
    assert_eq!(
        ids(&sessions),
        vec!["as-new".to_string()],
        "the discard term alone must trigger the staged rewrite (old-skip mutant)"
    );
    let lines = aux_sidecar_lines(&custody);
    let line = lines
        .iter()
        .find(|line| line["table"] == "agent_sessions")
        .unwrap_or_else(|| panic!("a divergent line for agent_sessions: {lines:?}"));
    assert_eq!(line["kept"]["id"], "as-new");
    assert_eq!(line["discarded"][0]["id"], "as-old");

    // Rev 13's other pinned refusals, hosted here because this is the one place
    // a valid apply fixture and a REAL custody dir already coexist:
    // (2a) a DIFFERING, NON-EMPTY sidecar at a resume → 77 mismatch;
    // (2b) a journal at InstalledUnverified AND at Verified → 77
    //      window_not_applyable (deleting that guard would silently rewrite the
    //      audit record from the installed tree);
    // (2c) apply without `--custody` → 64 custody_required.
    // Each asserts the EVENT NAME: 77 is shared by every custody refusal.
    use std::os::unix::fs::PermissionsExt;
    let sidecar = custody.join("journal.json.aux-divergent");
    let planted = b"{\"kind\":\"carry\",\"db\":\"sentinel\",\"table\":\"t\",\"column\":\"c\",\"original\":\"o\",\"carried\":\"o\",\"rows\":1}\n";
    std::fs::write(&sidecar, planted).expect("plant a differing sidecar");
    std::fs::set_permissions(&sidecar, std::fs::Permissions::from_mode(0o600))
        .expect("sidecar mode");
    let before = tree_digest(&root);
    let (code, _stdout, stderr) = run_consolidate(&root, &plan, false, Some(&custody));
    assert_refusal(
        code,
        77,
        &stderr,
        "consolidation.auxiliary_sidecar_mismatch",
    );
    assert_eq!(
        before,
        tree_digest(&root),
        "a sidecar mismatch mutates no S byte"
    );
    assert_eq!(
        std::fs::read(&sidecar).expect("sidecar bytes"),
        planted,
        "the existing audit record is never rewritten"
    );

    let journal = custody.join("journal.json");
    for phase in ["InstalledUnverified", "Verified"] {
        std::fs::write(
            &journal,
            format!(
                "{{\"phase\":\"{phase}\",\"seq\":1,\"prev_hash\":\"\",\"record_hash\":\"aa\"}}\n"
            ),
        )
        .expect("journal");
        std::fs::set_permissions(&journal, std::fs::Permissions::from_mode(0o600))
            .expect("journal mode");
        let (code, _stdout, stderr) = run_consolidate(&root, &plan, false, Some(&custody));
        assert_refusal(code, 77, &stderr, "consolidation.window_not_applyable");
        assert_eq!(
            before,
            tree_digest(&root),
            "{phase}: a refused re-apply mutates no S byte"
        );
        assert_eq!(
            std::fs::read(&sidecar).expect("sidecar bytes"),
            planted,
            "{phase}: the audit record is never rewritten"
        );
    }

    let (code, _stdout, stderr) = run_consolidate(&root, &plan, false, None);
    assert_refusal(code, 64, &stderr, "consolidation.custody_required");
    let _ = std::fs::remove_dir_all(&parent);

    // (3) a non-`Integer` `created_at` in a divergent group refuses 71 BEFORE
    // any comparison, so NULL/NaN never reach an ordering (§C.5, gate-1 M5).
    let parent = unique_temp_dir("aux-created-at");
    let root = parent.join("data");
    std::fs::create_dir_all(&root).expect("fixture root");
    {
        let fx = fixture(&root);
        fx.seed_row(
            SOURCE_A,
            &record("row-a", &format!("opencode_project_{SOURCE_A}")),
        );
        fx.seed_row(
            SOURCE_B,
            &record("row-b", &format!("opencode_project_{SOURCE_B}")),
        );
    }
    seed_episodic(&root);
    for (identity, raw, created) in [
        (source_a_identity, "seg one", Value::Integer(1)),
        (
            source_b_identity,
            "seg two",
            Value::Text("soon".to_string()),
        ),
    ] {
        fixture_exec(
            &root,
            &root.join("episodic.db"),
            "INSERT INTO episodic_segments (project_id, session_id, agent_id, segment_index,
               raw_content, byte_offset, byte_len, created_at)
             VALUES (?1, 's2', 'main', 0, ?2, 0, ?3, ?4)",
            vec![
                Value::Text(identity[5..].to_string()),
                Value::Text(raw.to_string()),
                Value::Integer(raw.len() as i64),
                created,
            ],
        );
    }
    let rows = vec![
        valid_row(&root, SOURCE_A, source_a_identity, target_identity, false),
        valid_row(&root, SOURCE_B, source_b_identity, target_identity, false),
    ];
    let plan = write_plan(&parent.join("plans"), &root, rows);
    let before = tree_digest(&root);
    let (code, _stdout, stderr) =
        run_consolidate(&root, &plan, false, Some(&fixture_custody(&parent)));
    assert_refusal(code, 71, &stderr, "consolidation.auxiliary_rows_divergent");
    assert_eq!(
        before,
        tree_digest(&root),
        "the refused plan writes nothing at all"
    );
    let _ = std::fs::remove_dir_all(&parent);

    // (4) a DIVERGENT LOSER that carries a TAGGED cell: the loser's
    // `(column, original)` emits NO carry line and the survivor's `rows` counts
    // SURVIVORS only (gate-6 MOD 1d). `user_prompts` is the table whose key set
    // (`["id"]`) does not contain the remapped column, and its legacy shape (no
    // `id` PRIMARY KEY) is what lets one id appear twice — the exact row set the
    // collapse exists to resolve. Both rows are branch (c), so a count taken
    // over every tag index rather than the survivor set emits TWO carry lines.
    let parent = unique_temp_dir("aux-discarded-carrier");
    let root = parent.join("data");
    std::fs::create_dir_all(&root).expect("fixture root");
    {
        let fx = fixture(&root);
        fx.seed_row(
            SOURCE_A,
            &record("row-a", &format!("opencode_project_{SOURCE_A}")),
        );
    }
    let prompts = root.join("user-prompts.db");
    fixture_exec(
        &root,
        &prompts,
        "CREATE TABLE user_prompts (
           id TEXT, session_id TEXT NOT NULL, message_id TEXT NOT NULL,
           project_path TEXT, content TEXT NOT NULL, created_at INTEGER NOT NULL,
           captured INTEGER DEFAULT 0, user_learning_captured BOOLEAN DEFAULT 0,
           linked_memory_id TEXT, capture_attempts INTEGER DEFAULT 0,
           provider_id TEXT, model_id TEXT)",
        Vec::new(),
    );
    for (project, content, created) in [
        ("/home/vaz/Code/thing/sub", "older", 10_i64),
        ("/tmp/elsewhere", "newer", 20_i64),
    ] {
        fixture_exec(
            &root,
            &prompts,
            "INSERT INTO user_prompts (id, session_id, message_id, project_path, content,
               created_at) VALUES ('p-dup', 's9', 'm1', ?1, ?2, ?3)",
            vec![
                Value::Text(project.to_string()),
                Value::Text(content.to_string()),
                Value::Integer(created),
            ],
        );
    }
    let rows = vec![valid_row(
        &root,
        SOURCE_A,
        source_a_identity,
        target_identity,
        false,
    )];
    let plan = write_plan(&parent.join("plans"), &root, rows);
    let custody = fixture_custody(&parent);
    let (code, _stdout, stderr) = run_consolidate(&root, &plan, false, Some(&custody));
    assert_eq!(code, Some(0), "the discarded carrier resolves; {stderr}");
    let staged = staged_root(&parent);
    let kept = aux_rows(&staged.join("user-prompts.db"), "user_prompts", "id");
    assert_eq!(kept.len(), 1, "one prompt survives the id collision");
    assert_eq!(
        text_cell(&kept[0], "project_path"),
        "/tmp/elsewhere",
        "keep-newer keeps the greater created_at"
    );
    let lines = aux_sidecar_lines(&custody);
    let carries: Vec<&serde_json::Value> = lines
        .iter()
        .filter(|line| line["kind"] == "carry")
        .collect();
    assert_eq!(
        carries.len(),
        1,
        "the LOSER's carrier is never counted (all-tags mutant): {lines:?}"
    );
    assert_eq!(carries[0]["original"], "/tmp/elsewhere");
    assert_eq!(carries[0]["rows"], 1, "rows counts survivors only");
    assert!(
        !lines
            .iter()
            .any(|line| line["original"] == "/home/vaz/Code/thing/sub"),
        "a discarded row's carrier emits no carry line: {lines:?}"
    );
    let divergent: Vec<&serde_json::Value> = lines
        .iter()
        .filter(|line| line["kind"] == "divergent")
        .collect();
    assert_eq!(divergent.len(), 1, "one divergent line: {lines:?}");
    assert_eq!(
        divergent[0]["discarded"][0]["project_path"], "/home/opencode/Code/thing/sub",
        "the discarded row is recorded POST-remap, with its carried cell"
    );
    let _ = std::fs::remove_dir_all(&parent);
}

// ---------------------------------------------------------------------------
// Step 6 — reservation custody: release gate + pre-journal adopt (§E.2)
// ---------------------------------------------------------------------------

use mem_store::maintenance_reservation as reservation;

/// Writes a reservation body at `<parent>/RESERVATION_FILE` for `root`.
fn write_reservation_body(parent: &Path, pid: u32, start_time: Option<u64>, nonce: &str) {
    let body = serde_json::json!({
        "pid": pid,
        "start_time": start_time,
        "operation": "maintenance",
        "root_basename": "data",
        "nonce": nonce,
        "created_utc": "2026-11-12T00:00:00+00:00",
        "retake_seq": null,
        "s_dev": 1_u64,
        "s_ino": 1_u64,
    });
    std::fs::write(parent.join(RESERVATION_FILE), body.to_string()).expect("write reservation");
}

/// A pid that cannot name a live process on this host.
fn dead_pid() -> u32 {
    // Above the kernel's pid_max on every supported host; never reused here.
    4_194_000
}

#[test]
fn test_reservation_release_requires_nonce_and_dead_pid() {
    let parent = unique_temp_dir("release-gate");
    let root = parent.join("data");
    std::fs::create_dir_all(&root).expect("fixture root");
    let nonce = "AAAAAAAAAAAAAAAAAAAAAA==";
    let own_token = mem_store::platform::process_start_time(std::process::id());
    let own = std::process::id();

    // (1) wrong nonce refuses, even with a dead holder.
    write_reservation_body(&parent, dead_pid(), Some(7), nonce);
    let phase = reservation::ReleasePhase::Verified;
    assert!(
        reservation::release(&root, "BBBBBBBBBBBBBBBBBBBBBB==", phase).is_err(),
        "wrong nonce must refuse"
    );
    assert!(
        parent.join(RESERVATION_FILE).exists(),
        "refused release keeps the file"
    );

    // (2) a live holder refuses, even with the right nonce.
    write_reservation_body(&parent, own, own_token, nonce);
    assert!(
        reservation::release(&root, nonce, phase).is_err(),
        "a live holder must refuse"
    );

    // (3) a pid-reused holder (token differs from the live process) releases.
    write_reservation_body(
        &parent,
        own,
        Some(own_token.unwrap_or(0).wrapping_add(1)),
        nonce,
    );
    assert!(
        reservation::release(&root, nonce, phase).is_ok(),
        "a token-differing (pid-reused) holder releases"
    );
    assert!(!parent.join(RESERVATION_FILE).exists());

    // (4) a start_time-less body with a present pid refuses.
    write_reservation_body(&parent, own, None, nonce);
    assert!(
        reservation::release(&root, nonce, phase).is_err(),
        "start_time-less body with a present pid must refuse"
    );

    // (5) effective phase outside {Verified, RolledBack} refuses.
    write_reservation_body(&parent, dead_pid(), Some(7), nonce);
    assert!(
        reservation::release(&root, nonce, reservation::ReleasePhase::Installing).is_err(),
        "a non-terminal journal phase must refuse"
    );
    assert!(parent.join(RESERVATION_FILE).exists());

    let _ = std::fs::remove_dir_all(&parent);
}

#[test]
fn test_reservation_abandoned_before_journal_recovers_as_rolledback() {
    let parent = unique_temp_dir("pre-journal-adopt");
    let root = parent.join("data");
    std::fs::create_dir_all(&root).expect("fixture root");
    std::fs::write(root.join("sentinel.txt"), b"unchanged").expect("sentinel");
    let nonce = "AAAAAAAAAAAAAAAAAAAAAA==";

    // Orphaned reservation: dead holder, journal ABSENT (no custody dir).
    write_reservation_body(&parent, dead_pid(), Some(7), nonce);
    let before = tree_digest(&root);

    let adopted = reservation::adopt_pre_journal(&root, nonce)
        .expect("orphan + absent journal + dead holder adopts");
    assert_eq!(
        adopted.record_kind(),
        "RolledBack",
        "synthesized one-record journal is RolledBack{{pre_journal_abandon}}"
    );
    assert_eq!(adopted.reason(), "pre_journal_abandon");
    assert!(
        reservation::release(&root, nonce, reservation::ReleasePhase::RolledBack).is_ok(),
        "the adopted orphan releases"
    );
    assert!(!parent.join(RESERVATION_FILE).exists());
    assert_eq!(before, tree_digest(&root), "S is byte-unchanged");

    // Pid-reuse-refuted variant: a live pid with a foreign token is also dead.
    let own = std::process::id();
    let reused = mem_store::platform::process_start_time(own)
        .unwrap_or(0)
        .wrapping_add(1);
    write_reservation_body(&parent, own, Some(reused), nonce);
    assert!(
        reservation::adopt_pre_journal(&root, nonce).is_ok(),
        "a pid-reused (token-refuted) holder is dead and adopts"
    );

    let _ = std::fs::remove_dir_all(&parent);
}

// ---------------------------------------------------------------------------
// Step 6 — verified snapshot before native recovery (§G S3, C2 premise→77)
// ---------------------------------------------------------------------------

/// Runs the consolidator's snapshot step against `root` under `custody`, via
/// the CLI seam: `--mode=consolidate-snapshot --custody <dir>`.
fn run_snapshot(root: &Path, custody: &Path) -> (Option<i32>, String, String) {
    let home = root.parent().unwrap_or(root).join("home");
    std::fs::create_dir_all(&home).expect("scratch home");
    if !home.join(".config/opencode/opencode-mem.jsonc").exists() {
        write_fixture_config(&home, 1000);
    }
    let output = Command::new(env!("CARGO_BIN_EXE_opencode-agents-memory"))
        .arg(format!("--storage-root={}", root.display()))
        .arg("--mode=consolidate-snapshot")
        .arg(format!("--custody={}", custody.display()))
        .env("HOME", &home)
        .stdin(Stdio::null())
        .output()
        .expect("snapshot child runs");
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

/// Plants committed WAL-only rows: a checkpoint-free connection writes rows
/// and is dropped, leaving them in `-wal` (the native recovery input).
fn plant_wal_only_rows(db: &Path) {
    fixture_exec(
        db.parent().expect("db parent"),
        db,
        "CREATE TABLE IF NOT EXISTS wal_probe (id TEXT PRIMARY KEY, body TEXT)",
        Vec::new(),
    );
    for index in 0..3 {
        fixture_exec(
            db.parent().expect("db parent"),
            db,
            "INSERT INTO wal_probe (id, body) VALUES (?1, ?2)",
            vec![
                Value::Text(format!("wal-{index}")),
                Value::Text("committed-in-wal".to_string()),
            ],
        );
    }
}

#[test]
fn test_snapshot_precedes_native_recovery() {
    let parent = unique_temp_dir("snapshot-native");
    let root = parent.join("data");
    std::fs::create_dir_all(&root).expect("fixture root");
    let db = root.join("episodic.db");
    plant_wal_only_rows(&db);
    std::fs::write(root.join("sentinel.txt"), b"sentinel").expect("sentinel");

    // Exact pre-open bytes of the live database and its sidecars (the backup
    // must equal these, never a checkpointed rewrite).
    let pre_open = tree_digest(&root);
    let wal = root.join("episodic.db-wal");
    assert!(wal.exists(), "fixture must leave committed rows in -wal");

    let custody = parent.join("recovery").join("consolidation-test-nonce");
    let (code, _stdout, stderr) = run_snapshot(&root, &custody);
    assert_eq!(
        code,
        Some(0),
        "a complete snapshot succeeds; stderr: {stderr}"
    );

    // Backup = exact pre-open bytes, sidecars included.
    let snap = custody.join("data");
    let snap_digest = tree_digest(&snap);
    for (rel, digest) in &pre_open {
        if rel == "status" || rel == "engine.lock" {
            continue;
        }
        assert_eq!(
            snap_digest.get(rel),
            Some(digest),
            "the snapshot holds the exact pre-open bytes of {rel}"
        );
    }
    assert!(
        snap.join("episodic.db-wal").exists(),
        "the -wal sidecar is captured (omitted-sidecar mutant dies here)"
    );

    // The source tree is untouched by the snapshot.
    assert_eq!(pre_open, tree_digest(&root), "S is byte-unchanged");

    // Rows survive the staged native reopen of the snapshot clone.
    let staged_clone = parent.join("clone-reopen");
    copy_tree_for_test(&snap, &staged_clone);
    make_writable_for_test(&staged_clone);
    assert_eq!(
        wal_probe_count(&staged_clone.join("episodic.db")),
        3,
        "WAL-only committed rows survive native recovery on the clone"
    );

    // Incomplete snapshot refuses: a missing sidecar in the custody copy.
    // The frozen snapshot stays a-w (F43); the sidecar is dropped from a
    // writable clone of the custody, never from the oracle.
    let broken = parent.join("recovery").join("consolidation-broken-nonce");
    let (code, _stdout, stderr) = run_snapshot(&root, &broken);
    assert_eq!(code, Some(0));
    let broken_clone = parent.join("recovery").join("consolidation-broken-clone");
    copy_tree_for_test(&broken, &broken_clone);
    make_writable_for_test(&broken_clone);
    std::fs::remove_file(broken_clone.join("data").join("episodic.db-wal"))
        .expect("drop sidecar from clone");
    let verdict = run_verify_snapshot(&broken_clone);
    assert_eq!(
        verdict,
        Some(77),
        "an incomplete snapshot refuses 77; stderr: {stderr}"
    );

    // Subcase (premise per path, opsec-1 folded): the premise is checked on
    // every source path, not only the root. A child with group write refuses 77.
    {
        use std::os::unix::fs::PermissionsExt;
        let parent = unique_temp_dir("premise-per-path");
        let root = parent.join("data");
        std::fs::create_dir_all(root.join("projects")).expect("fixture tree");
        std::fs::write(root.join("projects").join("project_x_shard_0.db"), b"x").expect("child");
        // A child file with a group write bit: the root is clean, the path is not.
        std::fs::set_permissions(
            root.join("projects").join("project_x_shard_0.db"),
            std::fs::Permissions::from_mode(0o664),
        )
        .expect("child mode");
        let custody = parent.join("recovery").join("consolidation-premise-nonce");
        let (code, _stdout, stderr) = run_snapshot(&root, &custody);
        assert_eq!(
            code,
            Some(77),
            "a path with group write refuses 77 before any tree derivation; stderr: {stderr}"
        );
        assert!(
            !custody.join("data").exists(),
            "no snapshot tree is derived when the premise fails"
        );
        let _ = std::fs::remove_dir_all(&parent);
    }

    let _ = std::fs::remove_dir_all(&parent);
}

/// Recursively copies a directory tree (test helper, no symlink follow).
fn copy_tree_for_test(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("copy dest");
    for entry in std::fs::read_dir(from).expect("copy src") {
        let entry = entry.expect("entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("type").is_dir() {
            copy_tree_for_test(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy file");
        }
    }
}

/// Grants owner write (`u+w`) on a DERIVED copy only (rev-11 derived-tree
/// writability rule). Never applied to the frozen oracle.
fn make_writable_for_test(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::symlink_metadata(path).expect("clone meta");
    if meta.file_type().is_symlink() {
        return;
    }
    if meta.is_dir() {
        for entry in std::fs::read_dir(path).expect("clone dir") {
            make_writable_for_test(&entry.expect("clone entry").path());
        }
    }
    let mode = meta.permissions().mode() & 0o7777;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode | 0o200))
        .expect("clone chmod u+w");
}

/// Native-open count of `wal_probe` rows on a cloned DB (WAL recovery runs).
fn wal_probe_count(db: &Path) -> i64 {
    run_async(async {
        let handle = TursoConnectFactory
            .connect(
                db.to_path_buf(),
                ConnectOptions::for_config(
                    &fixture_config(db.parent().expect("parent")),
                    Platform::current(),
                ),
            )
            .await
            .expect("native reopen recovers the WAL");
        let rows = handle
            .query("SELECT COUNT(*) FROM wal_probe", Vec::new())
            .await
            .expect("count");
        match rows.first().and_then(|row| row.get("COUNT(*)")) {
            Some(Value::Integer(count)) => *count,
            other => panic!("unexpected count cell: {other:?}"),
        }
    })
}

/// Runs the snapshot verifier over `custody` (`--mode=consolidate-verify-snapshot`);
/// returns the exit code.
fn run_verify_snapshot(custody: &Path) -> Option<i32> {
    let parent = custody
        .parent()
        .and_then(Path::parent)
        .expect("custody parent");
    let root = parent.join("data");
    let output = Command::new(env!("CARGO_BIN_EXE_opencode-agents-memory"))
        .arg(format!("--storage-root={}", root.display()))
        .arg("--mode=consolidate-verify-snapshot")
        .arg(format!("--custody={}", custody.display()))
        .env("HOME", parent.join("home"))
        .stdin(Stdio::null())
        .output()
        .expect("verify child runs");
    output.status.code()
}

// ---------------------------------------------------------------------------
// Step 7 — journal write primitive (§L L2/L3, C2 fold: `<journal>.tmp` 0600)
// ---------------------------------------------------------------------------

/// Writes one journal record through the CLI seam (`--mode=consolidate-journal`).
fn run_journal_write(custody: &Path) -> (Option<i32>, String, String) {
    let parent = custody
        .parent()
        .and_then(Path::parent)
        .expect("custody parent");
    let root = parent.join("data");
    std::fs::create_dir_all(&root).expect("fixture root");
    let home = parent.join("home");
    std::fs::create_dir_all(&home).expect("scratch home");
    write_fixture_config(&home, 1000);
    let output = Command::new(env!("CARGO_BIN_EXE_opencode-agents-memory"))
        .arg(format!("--storage-root={}", root.display()))
        .arg("--mode=consolidate-journal")
        .arg(format!("--custody={}", custody.display()))
        .env("HOME", &home)
        .stdin(Stdio::null())
        .output()
        .expect("journal child runs");
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

// ---------------------------------------------------------------------------
// Step 7 — install crash boundaries (§H.1, L4/L5, EV-M). Row 15.
// ---------------------------------------------------------------------------

/// Runs `--mode=consolidate-recover --custody <dir>` and returns the exit code.
fn run_recover(custody: &Path) -> (Option<i32>, String) {
    let parent = custody
        .parent()
        .and_then(Path::parent)
        .expect("custody parent");
    let root = parent.join("data");
    let home = parent.join("home");
    std::fs::create_dir_all(&home).expect("scratch home");
    write_fixture_config(&home, 1000);
    let output = Command::new(env!("CARGO_BIN_EXE_opencode-agents-memory"))
        .arg(format!("--storage-root={}", root.display()))
        .arg("--mode=consolidate-recover")
        .arg(format!("--custody={}", custody.display()))
        .env("HOME", &home)
        .stdin(Stdio::null())
        .output()
        .expect("recover child runs");
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

/// The fixture install: a live S with one sentinel, and a staged tree with a
/// different sentinel, both under one parent so L4/L5 renames are same-fs.
fn install_fixture(parent: &Path) -> (PathBuf, PathBuf) {
    let live = parent.join("data");
    let staged = parent.join("consolidation-staged-nonce0001");
    std::fs::create_dir_all(&live).expect("live S");
    std::fs::create_dir_all(&staged).expect("staged");
    std::fs::write(live.join("sentinel.txt"), b"old-live").expect("live sentinel");
    std::fs::write(staged.join("sentinel.txt"), b"new-staged").expect("staged sentinel");
    (live, staged)
}

/// EV-M (rev 13) makes the auxiliary sidecar mandatory from `StagedVerified`
/// onward. These custody fixtures never run an apply, so they must plant the
/// artifact a real window already holds; an empty body is the correct one for
/// a fixture with no auxiliary records.
fn write_fixture_aux_sidecar(custody: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let path = custody.join("journal.json.aux-divergent");
    std::fs::write(&path, b"").expect("write the fixture aux sidecar");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .expect("chmod the fixture aux sidecar");
}

/// The last non-empty record of a custody journal.
fn last_journal_record(custody: &Path) -> serde_json::Value {
    let text = std::fs::read_to_string(custody.join("journal.json")).expect("journal readable");
    let last = text
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .expect("the journal has a record");
    serde_json::from_str(last).expect("the last record parses")
}

#[test]
fn test_install_crash_boundaries_recover() {
    let parent = unique_temp_dir("install-crash");
    let custody = parent
        .join("recovery")
        .join("consolidation-20261112T000000Z-nonce0001");
    let (live, staged) = install_fixture(&parent);
    let before_live = tree_digest(&live);
    let before_staged = tree_digest(&staged);

    // C2 fold (was test_journal_tmp_is_0600_at_creation): every durable journal
    // record this test drives is 0600. Checked after the first record, after
    // the kill (the persisted `Installing` record), and after recovery.
    let assert_journal_0600 = |dir: &Path, when: &str| {
        use std::os::unix::fs::PermissionsExt;
        let journal = dir.join("journal.json");
        if journal.exists() {
            let mode = std::fs::metadata(&journal)
                .expect("journal metadata")
                .permissions()
                .mode()
                & 0o7777;
            assert_eq!(mode, 0o600, "the durable journal is 0600 {when}");
        }
    };
    // First record on a SIBLING custody, so the install chain below is untouched.
    let first_custody = parent
        .join("recovery")
        .join("consolidation-20261112T000000Z-firstrec");
    let (first, _first_out, first_err) = run_journal_write(&first_custody);
    assert_eq!(first, Some(0), "a first journal record writes; {first_err}");
    assert_journal_0600(&first_custody, "after the first record");

    // Creation window (C2): the `.tmp` is 0600 BEFORE the rename. The kill seam
    // aborts after the tmp is written and closed, so the tmp survives on disk.
    let window_custody = parent
        .join("recovery")
        .join("consolidation-20261112T000000Z-window");
    std::fs::create_dir_all(&window_custody).expect("window custody");
    // EV-M (blueprint :125): every first journal write refuses a custody dir
    // that is not 0700, so the window fixture must be a valid custody dir — the
    // `create_dir_all` default (0755) would (correctly) refuse 77 instead of
    // reaching the creation-window seam.
    std::fs::set_permissions(
        &window_custody,
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .expect("window custody is 0700 (EV-M)");
    let window_tmp = {
        let home = parent.join("home-window");
        std::fs::create_dir_all(&home).expect("window home");
        write_fixture_config(&home, 1000);
        let root = parent.join("data-window");
        std::fs::create_dir_all(&root).expect("window root");
        let status = Command::new(env!("CARGO_BIN_EXE_opencode-agents-memory"))
            .arg(format!("--storage-root={}", root.display()))
            .arg("--mode=consolidate-journal")
            .arg(format!("--custody={}", window_custody.display()))
            .env("OPENCODE_MEM_CONSOLIDATE_KILL_AT", "journal-tmp")
            .env("HOME", &home)
            .stdin(Stdio::null())
            .output()
            .expect("window child runs");
        assert_eq!(
            status.status.code(),
            None,
            "the child aborts at journal-tmp"
        );
        window_custody.join("journal.json.tmp")
    };
    use std::os::unix::fs::PermissionsExt;
    let tmp_mode = std::fs::metadata(&window_tmp)
        .expect("the .tmp survives the abort")
        .permissions()
        .mode()
        & 0o7777;
    assert_eq!(
        tmp_mode, 0o600,
        "the journal .tmp is 0600 at creation, before the rename"
    );

    // Kill boundary A: between the two install renames (L4 done, L5 not).
    // The durable `Installing` record licenses L4 and L5; a kill between them
    // leaves S absent and the staged tree still at its staged name.
    // A verified snapshot precedes the install (settlement re-verifies it on
    // every path), and the staged tree is verified before the install intent.
    let (snap, _snap_out, snap_err) = run_snapshot(&live, &custody);
    assert_eq!(
        snap,
        Some(0),
        "the snapshot precedes the install; {snap_err}"
    );
    let (staged_verified, staged_err) = run_staged_verify(&live, &custody, &staged);
    assert_eq!(
        staged_verified,
        Some(0),
        "the staged tree is verified before the install; {staged_err}"
    );
    write_fixture_aux_sidecar(&custody);
    let (code, stderr) = run_install_step(&custody, &live, &staged, "after-l4");
    assert_eq!(
        code, None,
        "the owned child aborts (signal) between the renames; {stderr}"
    );
    assert!(!live.exists(), "L4 renamed S away");
    assert!(staged.exists(), "L5 has not run: staged still at its name");
    assert_journal_0600(&custody, "after the kill at after-l4");

    // F-2a (opsec-2): the durable `Installing` record names the staged manifest
    // digest, and the verified staged manifest (M1-built) is stored beside the
    // custody at 0600 with its `.excluded` sidecar — the two-side rule's
    // verified operand, recorded BEFORE L4.
    let installing = last_journal_record(&custody);
    assert_eq!(
        installing["phase"].as_str(),
        Some("Installing"),
        "the durable record at the kill boundary is the install intent"
    );
    let recorded_digest = installing["staged_manifest_sha256"]
        .as_str()
        .expect("the Installing record names the staged manifest digest")
        .to_string();
    let staged_name = staged
        .file_name()
        .and_then(|name| name.to_str())
        .expect("staged name");
    let staged_manifest_path = custody.join(format!("{staged_name}.manifest.sha256"));
    let staged_manifest = std::fs::read_to_string(&staged_manifest_path)
        .expect("the verified staged manifest is stored beside the custody");
    assert_eq!(
        mem_core::sha256::hex(&mem_core::sha256::digest(staged_manifest.as_bytes())),
        recorded_digest,
        "the recorded digest is the stored staged manifest's digest"
    );
    assert!(
        custody
            .join(format!("{staged_name}.manifest.sha256.excluded"))
            .is_file(),
        "the staged manifest has its .excluded sidecar"
    );
    assert_eq!(
        std::fs::metadata(&staged_manifest_path)
            .expect("staged manifest metadata")
            .permissions()
            .mode()
            & 0o7777,
        0o600,
        "the verified staged manifest is 0600"
    );

    // Recovery converges to EXACTLY ONE complete tree at S, never mixed.
    let (recovered, stderr) = run_recover_expect(&custody);
    assert_eq!(
        recovered,
        Some(0),
        "recover completes the install; {stderr}"
    );
    assert_eq!(
        tree_digest(&live),
        before_staged,
        "S is exactly the staged tree, never a mix of old and new"
    );
    assert!(
        !live.join("sentinel.txt").exists() || tree_digest(&live) != before_live,
        "the old live tree is retired, not merged"
    );
    assert_journal_0600(&custody, "after recovery");

    // EV-M wedge discriminator: a custody dir at 0500 refuses 77 BEFORE any
    // journal write, instead of silently proceeding on a wedged dir.
    let wedged = parent
        .join("recovery")
        .join("consolidation-20261112T000000Z-wedged");
    std::fs::create_dir_all(&wedged).expect("wedged custody");
    std::fs::set_permissions(&wedged, std::os::unix::fs::PermissionsExt::from_mode(0o500))
        .expect("wedge the mode");
    let (wedge_code, wedge_stderr) = run_recover_expect(&wedged);
    assert_eq!(
        wedge_code,
        Some(77),
        "EV-M: a 0500 custody dir refuses 77 before any journal write; {wedge_stderr}"
    );
    std::fs::set_permissions(&wedged, std::os::unix::fs::PermissionsExt::from_mode(0o700))
        .expect("restore for cleanup");

    // F-3 (review-4, blueprint :125): EV-M fences `--mode=consolidate-journal`
    // itself, not only install/recover. A PRE-EXISTING 0755 custody dir must
    // refuse 77 and create NO journal. The discriminating mutant is the one
    // that drops `ev_m_preflight` from `journal_write_record`: it writes a
    // journal into the 0755 dir and exits 0.
    let loose = parent
        .join("recovery")
        .join("consolidation-20261112T000000Z-loose");
    std::fs::create_dir_all(&loose).expect("loose custody");
    std::fs::set_permissions(&loose, std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .expect("loose custody is 0755");
    let (loose_code, _loose_out, loose_err) = run_journal_write(&loose);
    assert_eq!(
        loose_code,
        Some(77),
        "EV-M: consolidate-journal refuses a 0755 custody dir; {loose_err}"
    );
    assert!(
        !loose.join("journal.json").exists(),
        "no journal is created in a 0755 custody dir"
    );

    // F-2b (opsec-2): settlement compares the installed S with the VERIFIED
    // STAGED manifest recorded at install — never with the pre-consolidation
    // snapshot — and records `Verified`, the phase the reservation release gate
    // requires. A genuine install changes S by design.
    let (verified, verified_err) = run_custody_mode("consolidate-verify", &live, &custody);
    assert_eq!(
        verified,
        Some(0),
        "settlement accepts the installed tree against its verified staged manifest; {verified_err}"
    );
    assert_eq!(
        last_journal_record(&custody)["phase"].as_str(),
        Some("Verified"),
        "settlement records the Verified phase"
    );
    // Tampering with the installed tree after a successful settlement is
    // refused, never passed as a settled window.
    std::fs::write(live.join("sentinel.txt"), b"edited").expect("tamper the installed tree");
    let (tampered, tampered_err) = run_custody_mode("consolidate-verify", &live, &custody);
    assert_eq!(
        tampered,
        Some(77),
        "a post-install source edit is refused; {tampered_err}"
    );
    let _ = std::fs::remove_dir_all(&parent);
}

/// Runs `--mode=consolidate-staged-verify --custody <dir> --staged <dir>`: the
/// staged-verify writer the install's `StagedVerified` precondition requires.
fn run_staged_verify(root: &Path, custody: &Path, staged: &Path) -> (Option<i32>, String) {
    let parent = custody
        .parent()
        .and_then(Path::parent)
        .expect("custody parent");
    let home = parent.join("home");
    std::fs::create_dir_all(&home).expect("scratch home");
    write_fixture_config(&home, 1000);
    let output = Command::new(env!("CARGO_BIN_EXE_opencode-agents-memory"))
        .arg(format!("--storage-root={}", root.display()))
        .arg("--mode=consolidate-staged-verify")
        .arg(format!("--custody={}", custody.display()))
        .arg(format!("--staged={}", staged.display()))
        .env("HOME", &home)
        .stdin(Stdio::null())
        .output()
        .expect("staged-verify child runs");
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

/// Runs the owned child that performs the install and is killed at `boundary`.
fn run_install_step(
    custody: &Path,
    live: &Path,
    staged: &Path,
    boundary: &str,
) -> (Option<i32>, String) {
    run_install_child(custody, live, staged, boundary)
}

/// Runs the owned child that performs the install with no kill seam.
fn run_install_mode(custody: &Path, live: &Path, staged: &Path) -> (Option<i32>, String) {
    run_install_child(custody, live, staged, "")
}

fn run_install_child(
    custody: &Path,
    live: &Path,
    staged: &Path,
    boundary: &str,
) -> (Option<i32>, String) {
    let parent = custody
        .parent()
        .and_then(Path::parent)
        .expect("custody parent");
    let home = parent.join("home");
    std::fs::create_dir_all(&home).expect("scratch home");
    write_fixture_config(&home, 1000);
    let output = Command::new(env!("CARGO_BIN_EXE_opencode-agents-memory"))
        .arg(format!("--storage-root={}", live.display()))
        .arg("--mode=consolidate-install")
        .arg(format!("--custody={}", custody.display()))
        .arg(format!("--staged={}", staged.display()))
        // Test-only kill seam: an env var, never a CLI flag, so no production
        // surface exists. The child aborts at the named boundary (SIGABRT).
        .env("OPENCODE_MEM_CONSOLIDATE_KILL_AT", boundary)
        .env("HOME", &home)
        .stdin(Stdio::null())
        .output()
        .expect("install child runs");
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

/// Every phase string the custody journal records, in order.
fn journal_phases(custody: &Path) -> Vec<String> {
    std::fs::read_to_string(custody.join("journal.json"))
        .expect("journal readable")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let record: serde_json::Value = serde_json::from_str(line).expect("record parses");
            record
                .get("phase")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// `run_recover` with the exit code only, for the convergence assertions.
fn run_recover_expect(custody: &Path) -> (Option<i32>, String) {
    run_recover(custody)
}

// ---------------------------------------------------------------------------
// Step 7 — corrupt journal fails closed (§H, archive-aside). Row 16.
// ---------------------------------------------------------------------------

/// Writes raw bytes as the custody journal and runs recover; returns the exit,
/// the stderr, and the fixture parent.
fn recover_with_journal(name: &str, bytes: &[u8]) -> (Option<i32>, String, PathBuf) {
    let parent = unique_temp_dir(name);
    let custody = parent
        .join("recovery")
        .join("consolidation-20261112T000000Z-corrupt");
    std::fs::create_dir_all(&custody).expect("custody");
    std::fs::set_permissions(
        &custody,
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .expect("custody 0700");
    let journal = custody.join("journal.json");
    std::fs::write(&journal, bytes).expect("corrupt journal");
    std::fs::set_permissions(
        &journal,
        std::os::unix::fs::PermissionsExt::from_mode(0o600),
    )
    .expect("journal 0600");
    // A real window past `SnapshotVerified` already holds the rev-13 sidecar;
    // plant it so EV-M's presence rule never masks the check under test.
    write_fixture_aux_sidecar(&custody);
    let (code, stderr) = run_recover(&custody);
    (code, stderr, parent)
}

#[test]
fn test_corrupt_journal_fails_closed() {
    // (1) truncated JSON: refuse 77 for the unparseable record, archive the
    // journal aside, never guess a state from a torn tail.
    let (code, stderr, parent) =
        recover_with_journal("corrupt-truncated", br#"{"seq":1,"phase":"Insta"#);
    assert_eq!(code, Some(77), "truncated journal refuses 77; {stderr}");
    assert!(
        stderr.contains("unparseable"),
        "refusal names the unparseable record; {stderr}"
    );
    let recovery = parent.join("recovery");
    assert!(
        !recovery
            .join("consolidation-20261112T000000Z-corrupt")
            .join("journal.json")
            .exists(),
        "the corrupt journal is archived aside"
    );
    assert!(
        std::fs::read_dir(&recovery).expect("list").any(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .starts_with("consolidation-corrupt-")
        }),
        "archive-aside creates consolidation-corrupt-<UTC>-<nonce>"
    );
    let _ = std::fs::remove_dir_all(&parent);

    // (2) escaping path: a valid single record whose storage_root lies outside
    // the root family. Never followed; nothing outside is touched.
    let outside = unique_temp_dir("corrupt-outside-target");
    std::fs::create_dir_all(outside.join("staged")).expect("outside staged");
    let escaping = format!(
        r#"{{"phase":"Installing","storage_root":"{}","staged":"{}","seq":1,"prev_hash":"","record_hash":"aa"}}"#,
        outside.join("victim").display(),
        outside.join("staged").display()
    );
    let (code, stderr, parent) = recover_with_journal("corrupt-escape", escaping.as_bytes());
    assert_eq!(code, Some(77), "an escaping path refuses 77; {stderr}");
    assert!(
        stderr.contains("escapes the root family"),
        "refusal is the escape guard, not another check; {stderr}"
    );
    assert!(
        outside.join("staged").exists(),
        "no outside write: staged untouched"
    );
    assert!(
        !outside.join("victim").exists(),
        "no outside write: victim not created"
    );
    let _ = std::fs::remove_dir_all(&parent);
    let _ = std::fs::remove_dir_all(&outside);

    // (3) stale baseline: a valid chain whose recorded baseline differs from
    // baseline.sha256 in the custody dir must refuse, never proceed.
    let stale = br#"{"phase":"Prepared","baseline_sha256":"0000","seq":1,"prev_hash":"","record_hash":"aa"}"#;
    let (code, stderr, parent) = recover_with_journal("corrupt-stale", stale);
    let custody = parent
        .join("recovery")
        .join("consolidation-20261112T000000Z-corrupt");
    std::fs::write(custody.join("baseline.sha256"), "1111\n").expect("baseline");
    let (code2, stderr2) = run_recover(&custody);
    assert_eq!(code2, Some(77), "a stale baseline refuses 77; {stderr2}");
    assert!(
        stderr2.contains("stale baseline"),
        "refusal is the baseline check; {stderr2}"
    );
    let _ = (code, stderr);
    let _ = std::fs::remove_dir_all(&parent);

    // (4) interior chain break: the second record's prev_hash does not equal
    // the first record's record_hash. Refuse 77 on the chain, archive aside.
    let broken = b"{\"phase\":\"Prepared\",\"seq\":1,\"prev_hash\":\"\",\"record_hash\":\"aaaa\"}\n{\"phase\":\"Installing\",\"seq\":2,\"prev_hash\":\"bbbb\",\"record_hash\":\"cccc\"}\n";
    let (code, stderr, parent) = recover_with_journal("corrupt-chain", broken);
    assert_eq!(
        code,
        Some(77),
        "an interior chain break refuses 77; {stderr}"
    );
    assert!(
        stderr.contains("prev_hash break"),
        "refusal is the chain check; {stderr}"
    );
    let _ = std::fs::remove_dir_all(&parent);
}

// ---------------------------------------------------------------------------
// Step 8 — rollback, idempotency, settlement (§H, §J). W7 rows 17–19.
// ---------------------------------------------------------------------------

/// Runs `--mode=<mode> --custody <dir>` on the real binary; returns exit + stderr.
fn run_custody_mode(mode: &str, root: &Path, custody: &Path) -> (Option<i32>, String) {
    run_custody_child(mode, root, custody, "")
}

/// `run_custody_mode` with the test-only kill seam armed at `boundary`.
fn run_custody_mode_killed(
    mode: &str,
    root: &Path,
    custody: &Path,
    boundary: &str,
) -> (Option<i32>, String) {
    run_custody_child(mode, root, custody, boundary)
}

fn run_custody_child(
    mode: &str,
    root: &Path,
    custody: &Path,
    boundary: &str,
) -> (Option<i32>, String) {
    let parent = custody
        .parent()
        .and_then(Path::parent)
        .expect("custody parent");
    let home = parent.join("home");
    std::fs::create_dir_all(&home).expect("scratch home");
    write_fixture_config(&home, 1000);
    let output = Command::new(env!("CARGO_BIN_EXE_opencode-agents-memory"))
        .arg(format!("--storage-root={}", root.display()))
        .arg(format!("--mode={mode}"))
        .arg(format!("--custody={}", custody.display()))
        .env("OPENCODE_MEM_CONSOLIDATE_KILL_AT", boundary)
        .env("HOME", &home)
        .stdin(Stdio::null())
        .output()
        .expect("custody child runs");
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

#[test]
fn test_restore_is_whole_root_and_idempotent() {
    use std::os::unix::fs::PermissionsExt;
    // A partial apply: a sentinel and a new DB were added, an unrelated file
    // mutated, and two modes widened. Restore from the frozen snapshot must
    // remove the additions and restore every original `(mode, kind)` tuple.
    let parent = unique_temp_dir("restore-whole-root");
    let root = parent.join("data");
    std::fs::create_dir_all(root.join("projects")).expect("projects");
    std::fs::write(root.join("keep.txt"), b"original").expect("keep");
    std::fs::write(
        root.join("projects").join("project_x_shard_0.db"),
        b"orig-db",
    )
    .expect("db");
    // Non-default modes, so a restore that sources modes from the post-freeze
    // snapshot (or from the copy's 0700/0600 defaults) cannot match. The root's
    // mode is set explicitly so the root-line assertion cannot go vacuous under
    // a umask that would already make it 0700.
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o750)).expect("root mode");
    std::fs::set_permissions(
        root.join("keep.txt"),
        std::fs::Permissions::from_mode(0o640),
    )
    .expect("keep mode");
    std::fs::set_permissions(
        root.join("projects"),
        std::fs::Permissions::from_mode(0o750),
    )
    .expect("projects mode");
    let original_modes = tree_modes(&root);
    let custody = parent.join("recovery").join("consolidation-restore-nonce");
    let (snap, _out, snap_err) = run_snapshot(&root, &custody);
    assert_eq!(
        snap,
        Some(0),
        "the snapshot precedes the partial apply; {snap_err}"
    );
    let original = tree_digest(&root);

    // R1-i: a restore is licensed by a journal, so the fixture builds the real
    // install chain — verify the staged tree, install it, then damage S.
    let staged = parent.join("consolidation-staged-restore");
    copy_tree_for_test(&custody.join("data"), &staged);
    // The snapshot is frozen `a-w`; the derived install copy must be writable,
    // because the damage below happens in the installed S.
    make_writable_for_test(&staged);
    let (verified, verify_err) = run_staged_verify(&root, &custody, &staged);
    assert_eq!(
        verified,
        Some(0),
        "the staged tree verifies before the install; {verify_err}"
    );
    write_fixture_aux_sidecar(&custody);
    let (installed, install_err) = run_install_mode(&custody, &root, &staged);
    assert_eq!(
        installed,
        Some(0),
        "the staged tree installs before the damage; {install_err}"
    );

    // Partial apply: additions, a content mutation, and widened modes.
    std::fs::write(root.join("sentinel.partial"), b"added").expect("sentinel");
    std::fs::write(root.join("projects").join("project_new_shard_9.db"), b"new").expect("new db");
    std::fs::write(root.join("keep.txt"), b"mutated").expect("mutate");
    std::fs::set_permissions(
        root.join("keep.txt"),
        std::fs::Permissions::from_mode(0o644),
    )
    .expect("widen keep");
    std::fs::set_permissions(
        root.join("projects"),
        std::fs::Permissions::from_mode(0o755),
    )
    .expect("widen projects");

    let (first, first_err) = run_custody_mode("consolidate-rollback", &root, &custody);
    assert_eq!(
        first,
        Some(0),
        "rollback restores the whole root; {first_err}"
    );
    assert_eq!(
        tree_digest(&root),
        original,
        "restore removes additions and restores every original tuple"
    );
    // Per-path `(mode, kind)` equality against the captured clone-time table,
    // the ROOT included: the writer records the S root as the lone `.` path, so
    // a restore that leaves the root at the copy default 0700 fails here.
    let restored_modes = tree_modes(&root);
    let expected_modes = original_modes.clone();
    assert_eq!(
        restored_modes, expected_modes,
        "restored (mode, kind) equals the clone-time mode table, root included"
    );
    let keep_mode = std::fs::metadata(root.join("keep.txt"))
        .expect("restored keep")
        .permissions()
        .mode()
        & 0o7777;
    assert_eq!(
        keep_mode, 0o640,
        "restore reproduces the clone-time file mode"
    );
    let projects_mode = std::fs::metadata(root.join("projects"))
        .expect("restored projects")
        .permissions()
        .mode()
        & 0o7777;
    assert_eq!(
        projects_mode, 0o750,
        "restore reproduces the clone-time directory mode"
    );
    assert!(
        !root.join("sentinel.partial").exists(),
        "the addition is removed"
    );
    // F-2c: the restore is journalled — a write-ahead `RollingBack` before the
    // renames and a terminal `RolledBack` after them.
    let phases = journal_phases(&custody);
    assert!(
        phases.iter().any(|phase| phase == "RollingBack"),
        "the restore is journalled before it renames S: {phases:?}"
    );
    assert_eq!(
        phases.last().map(String::as_str),
        Some("RolledBack"),
        "the completed restore is journalled: {phases:?}"
    );

    // Idempotent: a second restore leaves the manifest identical.
    let after_first = tree_digest(&root);
    let after_first_modes = tree_modes(&root);
    let (second, second_err) = run_custody_mode("consolidate-rollback", &root, &custody);
    assert_eq!(
        second,
        Some(0),
        "a second restore is accepted; {second_err}"
    );
    assert_eq!(
        tree_digest(&root),
        after_first,
        "the second manifest is identical"
    );
    assert_eq!(
        tree_modes(&root),
        after_first_modes,
        "the second restore leaves the modes unchanged"
    );
    assert_eq!(
        journal_phases(&custody),
        phases,
        "a repeat restore on a completed one writes no new record"
    );

    // R1-i: a snapshot alone never licenses a restore. No journal → 77.
    let no_journal_parent = unique_temp_dir("restore-no-journal");
    let no_journal_root = no_journal_parent.join("data");
    std::fs::create_dir_all(&no_journal_root).expect("no-journal root");
    std::fs::write(no_journal_root.join("keep.txt"), b"original").expect("no-journal keep");
    let no_journal_custody = no_journal_parent
        .join("recovery")
        .join("consolidation-nojournal-nonce");
    let (snap, _out, snap_err) = run_snapshot(&no_journal_root, &no_journal_custody);
    assert_eq!(
        snap,
        Some(0),
        "the no-journal fixture snapshots; {snap_err}"
    );
    assert!(
        !no_journal_custody.join("journal.json").exists(),
        "a snapshot writes no journal"
    );
    let (refused, refused_err) = run_custody_mode(
        "consolidate-rollback",
        &no_journal_root,
        &no_journal_custody,
    );
    assert_eq!(
        refused,
        Some(77),
        "a rollback without a journal refuses; {refused_err}"
    );
    let _ = std::fs::remove_dir_all(&no_journal_parent);

    // F-2c recovery: a crash between the two renames leaves the journal at
    // `RollingBack`, S retired and the restore tree holding the snapshot.
    // Recovery classifies that shape from disk and converges to `RolledBack`.
    let crash_parent = unique_temp_dir("restore-crash");
    let crash_root = crash_parent.join("data");
    std::fs::create_dir_all(crash_root.join("projects")).expect("crash projects");
    std::fs::write(crash_root.join("keep.txt"), b"original").expect("crash keep");
    let crash_custody = crash_parent
        .join("recovery")
        .join("consolidation-crash-nonce");
    let (snap, _out, snap_err) = run_snapshot(&crash_root, &crash_custody);
    assert_eq!(snap, Some(0), "the crash fixture snapshots; {snap_err}");
    let crash_original = tree_digest(&crash_root);
    let crash_staged = crash_parent.join("consolidation-staged-crash");
    copy_tree_for_test(&crash_custody.join("data"), &crash_staged);
    make_writable_for_test(&crash_staged);
    let (verified, verify_err) = run_staged_verify(&crash_root, &crash_custody, &crash_staged);
    assert_eq!(
        verified,
        Some(0),
        "the crash fixture verifies; {verify_err}"
    );
    write_fixture_aux_sidecar(&crash_custody);
    let (installed, install_err) = run_install_mode(&crash_custody, &crash_root, &crash_staged);
    assert_eq!(
        installed,
        Some(0),
        "the crash fixture installs; {install_err}"
    );
    std::fs::write(crash_root.join("sentinel.partial"), b"added").expect("crash sentinel");
    let (killed, _killed_err) = run_custody_mode_killed(
        "consolidate-rollback",
        &crash_root,
        &crash_custody,
        "rollback-after-failed",
    );
    assert_eq!(killed, None, "the child aborts between the two renames");
    assert!(
        !crash_root.exists(),
        "S is retired before the restore tree is installed"
    );
    let crash_phases = journal_phases(&crash_custody);
    assert_eq!(
        crash_phases.last().map(String::as_str),
        Some("RollingBack"),
        "the write-ahead record precedes the renames: {crash_phases:?}"
    );
    let (recovered, recover_err) = run_recover(&crash_custody);
    assert_eq!(recovered, Some(0), "recovery converges; {recover_err}");
    assert_eq!(
        tree_digest(&crash_root),
        crash_original,
        "recovery installs the restore tree as S"
    );
    assert_eq!(
        journal_phases(&crash_custody).last().map(String::as_str),
        Some("RolledBack"),
        "recovery journals the completed restore"
    );

    let _ = std::fs::remove_dir_all(&parent);
    let _ = std::fs::remove_dir_all(&crash_parent);
}

#[test]
fn test_verified_repeat_apply_is_noop() {
    // A verified apply followed by an identical second invocation must not
    // touch bytes, mtimes, or the backup set. A changed plan refuses.
    let parent = unique_temp_dir("repeat-apply-noop");
    let custody = parent.join("recovery").join("consolidation-repeat-nonce");
    let root = parent.join("data");
    std::fs::create_dir_all(root.join("projects")).expect("root");
    std::fs::write(
        root.join("projects").join("project_x_shard_0.db"),
        b"verified",
    )
    .expect("db");
    let (snap, _out, snap_err) = run_snapshot(&root, &custody);
    assert_eq!(
        snap,
        Some(0),
        "the snapshot precedes the verified apply; {snap_err}"
    );
    // A settled journal is what a verified apply leaves behind: the real
    // journal writer records the install-verified phase.
    // Settle through the real install: L3 Installing, L4 retire, L5 install,
    // then InstalledUnverified. The staged tree is the snapshot-equal tree.
    let staged = parent.join("consolidation-staged-repeat");
    std::fs::create_dir_all(&staged).expect("staged");
    let snapshot_tree = custody.join("data");
    copy_tree_for_test(&snapshot_tree, &staged);
    let (verified, verify_err) = run_staged_verify(&root, &custody, &staged);
    assert_eq!(
        verified,
        Some(0),
        "the staged tree verifies before the install; {verify_err}"
    );
    write_fixture_aux_sidecar(&custody);
    let settled = Command::new(env!("CARGO_BIN_EXE_opencode-agents-memory"))
        .arg(format!("--storage-root={}", root.display()))
        .arg("--mode=consolidate-install")
        .arg(format!("--custody={}", custody.display()))
        .arg(format!("--staged={}", staged.display()))
        .env("HOME", parent.join("home"))
        .stdin(Stdio::null())
        .output()
        .expect("install child runs");
    assert_eq!(
        settled.status.code(),
        Some(0),
        "the settled install completes; {}",
        String::from_utf8_lossy(&settled.stderr)
    );
    let (code, stderr) = run_custody_mode("consolidate-verify", &root, &custody);
    assert_eq!(code, Some(0), "a verified custody is accepted; {stderr}");
    let before = tree_digest(&custody);
    let mtimes_before = std::fs::metadata(custody.join("journal.json"))
        .expect("journal")
        .modified()
        .expect("mtime");

    let (again, again_err) = run_custody_mode("consolidate-verify", &root, &custody);
    assert_eq!(again, Some(0), "the repeat is accepted; {again_err}");
    assert_eq!(tree_digest(&custody), before, "bytes and set unchanged");
    let mtimes_after = std::fs::metadata(custody.join("journal.json"))
        .expect("journal")
        .modified()
        .expect("mtime");
    assert_eq!(
        mtimes_after, mtimes_before,
        "no journal rewrite on a verified repeat"
    );

    // R1-i: a verified install is terminal for rollback — restoring it is an
    // operator decision, never an automatic one.
    let (refused, refused_err) = run_custody_mode("consolidate-rollback", &root, &custody);
    assert_eq!(
        refused,
        Some(77),
        "a verified install refuses a rollback; {refused_err}"
    );

    // Changed plan / incomplete journal refuses: a journal truncated to a
    // non-terminal phase must not be treated as verified.
    std::fs::write(
        custody.join("journal.json"),
        b"{\"phase\":\"Prepared\",\"seq\":1,\"prev_hash\":\"\",\"record_hash\":\"aa\"}\n",
    )
    .expect("incomplete journal");
    let (incomplete, incomplete_err) = run_custody_mode("consolidate-verify", &root, &custody);
    assert_eq!(
        incomplete,
        Some(77),
        "an incomplete journal refuses; {incomplete_err}"
    );

    // A broken interior chain must refuse on the chain itself, not pass as a
    // settled phase: the chain check runs in settlement, not only in recovery.
    std::fs::write(
        custody.join("journal.json"),
        b"{\"phase\":\"InstalledUnverified\",\"seq\":1,\"prev_hash\":\"\",\"record_hash\":\"aa\"}\n{\"phase\":\"InstalledUnverified\",\"seq\":2,\"prev_hash\":\"zz\",\"record_hash\":\"bb\"}\n",
    )
    .expect("broken chain");
    let (chained, chained_err) = run_custody_mode("consolidate-verify", &root, &custody);
    assert_eq!(chained, Some(77), "a broken chain refuses; {chained_err}");
    assert!(
        chained_err.contains("prev_hash break"),
        "refusal is the chain check; {chained_err}"
    );
    let _ = std::fs::remove_dir_all(&parent);
}

#[test]
fn test_settlement_recomputes_final_artifacts() {
    // Settlement must recompute the final artifacts from disk, never trust a
    // self-written flag. A flipped persisted vector byte and a post-manifest
    // source edit must each reject.
    let parent = unique_temp_dir("settlement-recompute");
    let root = parent.join("data");
    std::fs::create_dir_all(root.join("projects")).expect("projects");
    std::fs::write(
        root.join("projects").join("project_x_shard_0.db"),
        b"settled",
    )
    .expect("db");
    let custody = parent.join("recovery").join("consolidation-settle-nonce");
    let (snap, _out, snap_err) = run_snapshot(&root, &custody);
    assert_eq!(snap, Some(0), "the snapshot is taken; {snap_err}");

    // Subcase (i): a flipped persisted vector byte after success is rejected.
    let vector = custody.join("final.vector");
    std::fs::write(&vector, b"0000").expect("persisted vector");
    let (flipped, flipped_err) = run_custody_mode("consolidate-verify", &root, &custody);
    assert_eq!(
        flipped,
        Some(77),
        "a flipped persisted vector byte rejects; {flipped_err}"
    );
    assert!(
        flipped_err.contains("recomputed") || flipped_err.contains("vector"),
        "the refusal recomputes the artifact, not the flag; {flipped_err}"
    );
    let _ = std::fs::remove_file(&vector);

    // Subcase (ii): a post-manifest source edit rejects.
    std::fs::write(
        root.join("projects").join("project_x_shard_0.db"),
        b"edited-after",
    )
    .expect("edit");
    let (edited, edited_err) = run_custody_mode("consolidate-verify", &root, &custody);
    assert_eq!(
        edited,
        Some(77),
        "a post-manifest source edit rejects; {edited_err}"
    );
    let _ = std::fs::remove_dir_all(&parent);
}

// ---------------------------------------------------------------------------
// Step 10 — C3 harness-side scratch assert (opsec carry-forward). W8 row 23.
// ---------------------------------------------------------------------------

/// The rehearsal harness refuses a plan whose `storage_root` does not resolve
/// inside its scratch tree. It must refuse BEFORE any reservation exists, so a
/// mis-aimed harness never creates a reservation under the live root.
#[test]
fn test_rehearsal_refuses_mis_aimed_storage_root() {
    let scratch = unique_temp_dir("c3-scratch");
    let rehearsal = scratch.join("rehearsal-20261112T000000Z");
    std::fs::create_dir_all(&rehearsal).expect("rehearsal tree");
    // The mis-aimed root is a FULLY VALID store (same fixture and plan as
    // `test_plan_requires_exact_identity_set`), so a dry-run exits 0 unless
    // containment refuses it. The refusal is therefore containment, not a
    // missing `projects/` directory or a placeholder-hash mismatch.
    let mis_aimed = scratch.join("elsewhere").join("data");
    std::fs::create_dir_all(&mis_aimed).expect("existing mis-aimed root");
    const HASH_A: &str = "aaaaaaaaaaaaaaaa";
    let rows = {
        let fx = fixture(&mis_aimed);
        fx.seed(HASH_A, "row-a");
        vec![valid_row(
            &mis_aimed,
            HASH_A,
            "path:/fixture/a",
            "path:/fixture/a",
            false,
        )]
    };
    let plan = write_plan(&scratch.join("plans"), &mis_aimed, rows);

    // The rehearsal scratch assert is the consolidator's dispatch in rehearsal
    // mode: `--rehearsal <scratch-dir>` makes the plan's `storage_root` a
    // precondition, checked before any reservation or dispatch. `--dry-run`
    // keeps the accept-all mutant from writing into the mis-aimed root.
    let output = Command::new(env!("CARGO_BIN_EXE_opencode-agents-memory"))
        .arg(format!("--storage-root={}", mis_aimed.display()))
        .arg("--mode=consolidate-shards")
        .arg(format!("--plan={}", plan.display()))
        .arg(format!("--rehearsal={}", rehearsal.display()))
        .arg("--dry-run")
        .env("HOME", scratch.join("home"))
        .stdin(Stdio::null())
        .output()
        .expect("harness runs");
    assert_ne!(
        output.status.code(),
        Some(0),
        "a mis-aimed storage_root must not succeed"
    );
    assert_eq!(
        output.status.code(),
        Some(64),
        "containment refuses with exit 64"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("outside the rehearsal scratch"),
        "refusal names the scratch assert; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // The reservation lives at the PARENT of the storage root (acquire test
    // above), so that is the path to check.
    assert!(
        !mis_aimed
            .parent()
            .expect("mis-aimed parent")
            .join(RESERVATION_FILE)
            .exists(),
        "no reservation is created beside the mis-aimed root"
    );
    let _ = std::fs::remove_dir_all(&scratch);
}
