//! Issue 53 · G2 — the **vault** surface under encryption × experimental
//! multiprocess WAL.
//!
//! The vault is the surface where the pair is *forced*, not opt-in:
//! `VaultStore::ready` refuses to open when `database_encryption_enabled` is off
//! (`crates/mem-vault/src/store.rs:112`) and its per-scope manager sets
//! `database_encryption_enabled = true` unconditionally (`:271`), while
//! `ConnectOptions::for_config` adds multiprocess WAL on every non-Windows
//! platform. This probe measures that exact open, on the vault's own scope
//! directory and its own minted key — not a synthetic database.
//!
//! **Measurement harness, not a suite addition.** Both tests are `#[ignore]`d:
//!
//! ```sh
//! cargo test -p mem-vault --test issue53_vault_probe -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Shape (the same as `crates/mem-store/tests/issue53_probe.rs`, the G1 harness):
//! a parent `VaultStore` writes one content-addressed entry and keeps its handle
//! live; a second process opens the vault **read-only** with the scope's own key
//! (read from `vault-keys/{scope}.key`) and reads it; a third process opens the
//! vault **writable** through `VaultStore::ready` and `put`s a second entry; the
//! parent then `put`s a third, drops its store, opens a fresh `VaultStore`, reads
//! every entry back by hash, and asks the engine for `PRAGMA integrity_check`.
//!
//! Non-vacuity: the keyless read at the end must fail, or the "encrypted" cell
//! measured a plaintext database. The literal protocol lines and the verdict are
//! quoted verbatim in `docs/evidence/issue-53-encryption-multiprocess/`.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use mem_store::config::StoreConfig;
use mem_store::db::{
    ConnectFactory, ConnectOptions, Db, DbHandle, EncryptionOptions, turso_connect_factory,
};
use mem_store::platform::Platform;
use mem_vault::{VaultEntry, VaultStore};

static COUNTER: AtomicU64 = AtomicU64::new(0);

const SCOPE: &str = "0123456789abcdef";
const CIPHER: &str = "aes256gcm";
const PROBE_TIMEOUT: Duration = Duration::from_secs(120);

const ENV_CHILD: &str = "MEM53_VAULT_CHILD";
const ENV_ROOT: &str = "MEM53_VAULT_ROOT";
const ENV_MODE: &str = "MEM53_VAULT_MODE";
const ENV_CONTENT: &str = "MEM53_VAULT_CONTENT";
const ENV_REPORT: &str = "MEM53_REPORT";

fn scratch_root(label: &str) -> PathBuf {
    let counter = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "mem-vault-issue53-{label}-{}-{counter}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create the scratch root");
    dir
}

/// The vault's config: encryption **on**, no caller key (the vault ignores it and
/// mints per scope), exactly what `VaultStore::build_manager` sees.
fn vault_config(root: &Path) -> StoreConfig {
    StoreConfig {
        storage_path: root.to_path_buf(),
        embedding_dimensions: 8,
        embedding_model: "test-model".to_string(),
        max_vectors_per_shard: 1000,
        container_tag_prefix: "opencode".to_string(),
        database_encryption_enabled: true,
        database_encryption_cipher: CIPHER.to_string(),
        database_encryption_key: None,
        allow_unguarded_open: false,
    }
}

fn entry(content: &str) -> VaultEntry {
    VaultEntry {
        content: content.as_bytes().to_vec(),
        mime_type: Some("text/plain".to_string()),
        // A short ref: `put` rejects a `source_ref` over 256 bytes, and the 4 KiB
        // filler contents would trip that if they were folded into it.
        source_ref: "issue53:probe".to_string(),
        created_by: "issue53-probe".to_string(),
    }
}

/// Filler so the vault's WAL spans many pages before the second process joins: a
/// three-row vault fits a handful of frames, which would make a backfill trivial.
const VAULT_FILLER_ENTRIES: usize = 16;
const VAULT_FILLER_BYTES: usize = 4096;

fn vault_key(root: &Path) -> String {
    let path = root.join("vault-keys").join(format!("{SCOPE}.key"));
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read the vault key {}: {error}", path.display()))
        .trim()
        .to_string()
}

fn vault_db_path(root: &Path) -> PathBuf {
    root.join("vault").join(SCOPE).join("vault.db")
}

/// A read-only open of the vault database with the scope's own key, through the
/// production factory. This is the only way a *second* process can read the vault
/// without `VaultStore::ready` creating/writing anything.
async fn open_vault_read_only(root: &Path) -> Result<DbHandle, String> {
    let config = vault_config(root);
    let db_path = vault_db_path(root);
    let mut options = ConnectOptions::for_config(&config, Platform::current());
    options.encryption = Some(EncryptionOptions {
        cipher: CIPHER.to_string(),
        hexkey: vault_key(root),
    });
    options.read_only = true;
    turso_connect_factory()
        .connect(db_path, options)
        .await
        .map_err(|error| format!("read-only vault open: {error}"))
}

/// A read-only open of the same file with **no** key material. Non-vacuity only.
async fn open_vault_keyless(root: &Path) -> Result<DbHandle, String> {
    let config = vault_config(root);
    let mut options = ConnectOptions::for_config(&config, Platform::current());
    options.encryption = None;
    options.read_only = true;
    turso_connect_factory()
        .connect(vault_db_path(root), options)
        .await
        .map_err(|error| format!("keyless vault open: {error}"))
}

async fn content_hashes(db: &dyn Db) -> Result<Vec<String>, String> {
    let rows = db
        .query(
            "SELECT content_hash FROM vault_entries ORDER BY content_hash",
            Vec::new(),
        )
        .await
        .map_err(|error| error.to_string())?;
    Ok(rows
        .iter()
        .filter_map(|row| row.text("content_hash"))
        .collect())
}

async fn integrity(db: &dyn Db) -> Result<String, String> {
    let rows = db
        .query("PRAGMA integrity_check", Vec::new())
        .await
        .map_err(|error| error.to_string())?;
    let Some(row) = rows.first() else {
        return Err("PRAGMA integrity_check returned no row".to_string());
    };
    let Some(column) = row.column_names().first().cloned() else {
        return Err("PRAGMA integrity_check returned a row with no columns".to_string());
    };
    row.text(&column)
        .ok_or_else(|| format!("PRAGMA integrity_check's first column is not TEXT: {row:?}"))
}

// ---------------------------------------------------------------------------
// The re-exec'd child
// ---------------------------------------------------------------------------

/// INERT unless `MEM53_VAULT_CHILD=1`, so a suite run can never touch it by
/// accident. Protocol (stdout, literal):
///
/// - `VAULT-CHILD-CELL mode=…`
/// - `VAULT-CHILD-OPEN-OK` / `VAULT-CHILD-OPEN-ERR <text>`
/// - `VAULT-CHILD-HASHES <csv>` / `VAULT-CHILD-READ-ERR <text>`
/// - `VAULT-CHILD-INTEGRITY <verdict>` / `VAULT-CHILD-INTEGRITY-ERR <text>`
/// - `rw` only: `VAULT-CHILD-PUT ok <hash>` / `VAULT-CHILD-PUT-ERR <text>`
/// - `VAULT-CHILD-DONE`
#[test]
#[ignore = "issue 53 re-exec'd vault measurement child; spawned with MEM53_VAULT_CHILD set"]
fn issue53_vault_child_entry() {
    if std::env::var(ENV_CHILD).as_deref() != Ok("1") {
        return;
    }
    let root = PathBuf::from(std::env::var(ENV_ROOT).expect("MEM53_VAULT_ROOT"));
    let mode = std::env::var(ENV_MODE).expect("MEM53_VAULT_MODE");
    println!("VAULT-CHILD-CELL mode={mode}");

    let runtime = mem_store::tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("child runtime");
    runtime.block_on(async move {
        if mode == "ro" {
            match open_vault_read_only(&root).await {
                Ok(db) => {
                    println!("VAULT-CHILD-OPEN-OK");
                    match content_hashes(db.as_ref()).await {
                        Ok(hashes) => println!("VAULT-CHILD-HASHES {}", hashes.join(",")),
                        Err(error) => println!("VAULT-CHILD-READ-ERR {error}"),
                    }
                    match integrity(db.as_ref()).await {
                        Ok(verdict) => println!("VAULT-CHILD-INTEGRITY {verdict}"),
                        Err(error) => println!("VAULT-CHILD-INTEGRITY-ERR {error}"),
                    }
                }
                Err(error) => println!("VAULT-CHILD-OPEN-ERR {error}"),
            }
        } else {
            let content = std::env::var(ENV_CONTENT).expect("MEM53_VAULT_CONTENT");
            let store = VaultStore::new(vault_config(&root));
            match store.ready(SCOPE).await {
                Ok(db) => {
                    println!("VAULT-CHILD-OPEN-OK");
                    match content_hashes(db.as_ref()).await {
                        Ok(hashes) => println!("VAULT-CHILD-HASHES {}", hashes.join(",")),
                        Err(error) => println!("VAULT-CHILD-READ-ERR {error}"),
                    }
                    match integrity(db.as_ref()).await {
                        Ok(verdict) => println!("VAULT-CHILD-INTEGRITY {verdict}"),
                        Err(error) => println!("VAULT-CHILD-INTEGRITY-ERR {error}"),
                    }
                    match store.put(SCOPE, entry(&content)).await {
                        Ok(hash) => println!("VAULT-CHILD-PUT ok {hash}"),
                        Err(error) => println!("VAULT-CHILD-PUT-ERR {error}"),
                    }
                }
                Err(error) => println!("VAULT-CHILD-OPEN-ERR {error}"),
            }
        }
        println!("VAULT-CHILD-DONE");
    });
}

// ---------------------------------------------------------------------------
// Child driver (the `open_guard.rs` / `turso_capabilities.rs` pattern)
// ---------------------------------------------------------------------------

struct ProbeChild {
    child: Option<std::process::Child>,
    lines: std::sync::mpsc::Receiver<String>,
}

impl ProbeChild {
    fn spawn(envs: &[(&str, String)]) -> Self {
        let mut command =
            std::process::Command::new(std::env::current_exe().expect("the running test binary"));
        command
            .arg("--exact")
            .arg("issue53_vault_child_entry")
            .arg("--ignored")
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env(ENV_CHILD, "1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit());
        for (name, value) in envs {
            command.env(name, value);
        }
        let mut child = command.spawn().expect("spawn the issue-53 vault child");
        let stdout = child.stdout.take().expect("piped child stdout");
        let (sender, lines) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        Self {
            child: Some(child),
            lines,
        }
    }

    fn next_line(&self, side: &str) -> Result<String, String> {
        match self.lines.recv_timeout(PROBE_TIMEOUT) {
            Ok(line) => {
                println!("[{side}] {line}");
                Ok(line)
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(format!(
                "the {side} child produced no line for {PROBE_TIMEOUT:?}: measured a BLOCK, not a verdict"
            )),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(format!(
                "the {side} child closed stdout without finishing its protocol: it exited early"
            )),
        }
    }

    fn finish(mut self, side: &str) -> Result<Vec<String>, String> {
        let mut collected = Vec::new();
        loop {
            let line = self.next_line(side)?;
            let done = line.ends_with("VAULT-CHILD-DONE");
            collected.push(line);
            if done {
                break;
            }
        }
        let mut child = self.child.take().expect("child handle");
        let status = child.wait().expect("reap the issue-53 vault child");
        if !status.success() {
            return Err(format!(
                "the {side} child must exit successfully; got {status:?}"
            ));
        }
        Ok(collected)
    }
}

impl Drop for ProbeChild {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

// ---------------------------------------------------------------------------
// The probe record
// ---------------------------------------------------------------------------

struct ProbeRecord {
    name: String,
    failure: Option<(String, String)>,
}

impl ProbeRecord {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            failure: None,
        }
    }

    fn ok(&mut self, line: impl Into<String>) {
        println!("[{}] {}", self.name, line.into());
    }

    fn fail(&mut self, side: &str, error: impl Into<String>) {
        let error = error.into();
        self.ok(format!("FAIL side={side} error={error}"));
        if self.failure.is_none() {
            self.failure = Some((side.to_string(), error));
        }
    }

    fn finish(self) -> String {
        let verdict = match &self.failure {
            None => format!("PROBE {} RESULT: PASS", self.name),
            Some((side, error)) => {
                format!("PROBE {} RESULT: FAIL side={side} error={error}", self.name)
            }
        };
        println!("{verdict}");
        if let Some(path) = std::env::var_os(ENV_REPORT)
            && let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
        {
            let _ = writeln!(file, "{verdict}");
        }
        verdict
    }
}

fn inventory(root: &Path) -> String {
    let dir = root.join("vault").join(SCOPE);
    let mut entries = Vec::new();
    if let Ok(read) = std::fs::read_dir(&dir) {
        for entry in read.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with("vault.db") || name.starts_with(".vault-encrypted") {
                let len = entry.metadata().map(|meta| meta.len()).unwrap_or(0);
                entries.push(format!("{name}={len}"));
            }
        }
    }
    entries.sort();
    entries.join(" ")
}

// ---------------------------------------------------------------------------
// G2
// ---------------------------------------------------------------------------

/// G2 — a fresh vault scope, encryption forced on, multiprocess WAL on: the parent
/// writes, a second process reads read-only and a third writes writable while the
/// parent's handle is live, then the parent writes, drops, reopens and asks the
/// engine for `integrity_check`.
#[mem_store::tokio::test(crate = "mem_store::tokio")]
#[ignore = "issue 53 measurement probe; run with --ignored --nocapture"]
async fn issue53_g2_vault_encrypted_multiprocess() {
    let root = scratch_root("g2");
    let mut record = ProbeRecord::new("G2");
    record.ok(format!("ROOT {}", root.display()));
    record.ok(format!("DB {}", vault_db_path(&root).display()));

    let store = VaultStore::new(vault_config(&root));
    let parent_hash = match store.put(SCOPE, entry("vault-entry-parent-1")).await {
        Ok(hash) => {
            record.ok(format!("PARENT-PUT-1 ok {hash}"));
            hash
        }
        Err(error) => {
            record.fail("parent-put-1", error.to_string());
            record.finish();
            return;
        }
    };
    let mut parent_hashes = vec![parent_hash.clone()];
    for index in 0..VAULT_FILLER_ENTRIES {
        let content = format!("vault-filler-{index:02}-{}", "x".repeat(VAULT_FILLER_BYTES));
        match store.put(SCOPE, entry(&content)).await {
            Ok(hash) => parent_hashes.push(hash),
            Err(error) => {
                record.fail("parent-filler", error.to_string());
                break;
            }
        }
    }
    record.ok(format!(
        "PARENT-FILLER-COUNT {} of {VAULT_FILLER_ENTRIES}",
        parent_hashes.len() - 1
    ));
    match integrity(store.ready(SCOPE).await.expect("parent handle").as_ref()).await {
        Ok(verdict) => record.ok(format!("PARENT-INTEGRITY {verdict}")),
        Err(error) => record.fail("parent-integrity", error),
    }
    record.ok(format!(
        "KEY-PATH {}",
        root.join("vault-keys")
            .join(format!("{SCOPE}.key"))
            .display()
    ));
    record.ok(format!("INVENTORY-WRITTEN {}", inventory(&root)));

    // -- The second process: read-only with the scope's own key.
    let child_ro = ProbeChild::spawn(&[
        (ENV_ROOT, root.display().to_string()),
        (ENV_MODE, "ro".to_string()),
    ]);
    match child_ro.finish("vault-child-ro") {
        Ok(lines) => {
            for line in &lines {
                if line.contains("OPEN-ERR")
                    || line.contains("READ-ERR")
                    || line.contains("INTEGRITY-ERR")
                {
                    record.fail("vault-child-ro", line.trim().to_string());
                }
            }
            let saw_parent = lines
                .iter()
                .any(|line| line.starts_with("VAULT-CHILD-HASHES") && line.contains(&parent_hash));
            if saw_parent {
                record.ok("VAULT-CHILD-RO saw the parent's entry");
            } else {
                record.fail(
                    "vault-child-ro",
                    "the read-only child did not see the parent's content hash",
                );
            }
        }
        Err(error) => record.fail("vault-child-ro", error),
    }

    // -- The third process: writable, through `VaultStore::ready`, committing while
    // the parent's handle is still live.
    let child_hash = {
        let child = ProbeChild::spawn(&[
            (ENV_ROOT, root.display().to_string()),
            (ENV_MODE, "rw".to_string()),
            (ENV_CONTENT, "vault-entry-child-rw".to_string()),
        ]);
        match child.finish("vault-child-rw") {
            Ok(lines) => {
                for line in &lines {
                    if line.contains("OPEN-ERR")
                        || line.contains("READ-ERR")
                        || line.contains("INTEGRITY-ERR")
                        || line.contains("PUT-ERR")
                    {
                        record.fail("vault-child-rw", line.trim().to_string());
                    }
                }
                lines
                    .iter()
                    .find_map(|line| line.strip_prefix("VAULT-CHILD-PUT ok "))
                    .map(|hash| hash.trim().to_string())
            }
            Err(error) => {
                record.fail("vault-child-rw", error);
                None
            }
        }
    };
    match &child_hash {
        Some(hash) => record.ok(format!("VAULT-CHILD-RW put ok {hash}")),
        None => record.fail("vault-child-rw", "the child's put produced no hash line"),
    }

    // -- The parent commits after the second process has been and gone.
    let after_hash = match store.put(SCOPE, entry("vault-entry-parent-after")).await {
        Ok(hash) => {
            record.ok(format!("PARENT-PUT-2 ok {hash}"));
            hash
        }
        Err(error) => {
            record.fail("parent-put-2", error.to_string());
            String::new()
        }
    };
    match integrity(store.ready(SCOPE).await.expect("parent handle").as_ref()).await {
        Ok(verdict) => record.ok(format!("PARENT-INTEGRITY-AFTER {verdict}")),
        Err(error) => record.fail("parent-integrity-after", error),
    }
    record.ok(format!("INVENTORY-POST-CHILD {}", inventory(&root)));
    drop(store);

    // -- Reopen: a fresh VaultStore reads every entry back by hash, and the engine
    // is asked for its verdict.
    let reopened = VaultStore::new(vault_config(&root));
    match reopened.ready(SCOPE).await {
        Ok(db) => {
            record.ok("REOPEN ok");
            match integrity(db.as_ref()).await {
                Ok(verdict) => record.ok(format!("REOPEN-INTEGRITY {verdict}")),
                Err(error) => record.fail("reopen-integrity", error),
            }
            // The three canonical entries (parent before, parent after, child) get a
            // `get`-by-hash read; the full set (filler included) is compared raw.
            let canonical: Vec<String> = [
                Some(parent_hash.clone()),
                Some(after_hash.clone()),
                child_hash.clone(),
            ]
            .into_iter()
            .flatten()
            .filter(|hash| !hash.is_empty())
            .collect();
            let mut expected = parent_hashes.clone();
            expected.extend(
                [after_hash.clone()]
                    .into_iter()
                    .filter(|hash| !hash.is_empty()),
            );
            expected.extend(
                child_hash
                    .clone()
                    .into_iter()
                    .filter(|hash| !hash.is_empty()),
            );
            expected.sort();
            match content_hashes(db.as_ref()).await {
                Ok(mut hashes) => {
                    hashes.sort();
                    if hashes == expected {
                        record.ok(format!("REOPEN-HASHES count={} exact match", hashes.len()));
                    } else {
                        record.fail(
                            "reopen-hashes",
                            format!(
                                "expected {} hashes, got {}; missing {:?}",
                                expected.len(),
                                hashes.len(),
                                expected
                                    .iter()
                                    .filter(|hash| !hashes.contains(hash))
                                    .collect::<Vec<_>>()
                            ),
                        );
                    }
                }
                Err(error) => record.fail("reopen-read", error),
            }
            for hash in &canonical {
                match reopened.get(SCOPE, hash).await {
                    Ok(Some(record_row)) => record.ok(format!(
                        "REOPEN-GET {hash} ok ({} bytes)",
                        record_row.content.len()
                    )),
                    Ok(None) => record.fail("reopen-get", format!("hash {hash} is absent")),
                    Err(error) => record.fail("reopen-get", error.to_string()),
                }
            }
        }
        Err(error) => record.fail("reopen-open", error.to_string()),
    }

    // -- Non-vacuity: the same file must refuse a keyless read.
    let keyless_refused = match open_vault_keyless(&root).await {
        Ok(db) => match content_hashes(db.as_ref()).await {
            Ok(hashes) => {
                record.ok(format!("KEYLESS-READ ok hashes={hashes:?}"));
                false
            }
            Err(error) => {
                record.ok(format!("KEYLESS-READ ERR {error}"));
                true
            }
        },
        Err(error) => {
            record.ok(format!("KEYLESS-OPEN ERR {error}"));
            true
        }
    };
    if !keyless_refused {
        record.fail(
            "non-vacuity",
            "a keyless read of the vault succeeded: this probe did not measure encryption",
        );
    }
    record.ok(format!("INVENTORY-FINAL {}", inventory(&root)));

    record.finish();
}
