use shard_measurement::{
    Case, ConnectionManager, EngineLockOutcome, ExclusiveRoot, MeasurementError,
    OPERATION_LOCK_FILE, Platform, ProcessProbe, RUNS, Scope, ShardManager, StoreConfig,
    StoreError, SystemProcessProbe, Value, acquire_engine_lock, acquire_turso_operation_lock,
    assert_no_turso_migration_in_progress, open, read_live_lock, run_case, turso_connect_factory,
};
use std::path::{Path, PathBuf};

#[test]
fn test_refuses_outside_or_symlink_root() {
    let runs = Path::new(RUNS);
    std::fs::create_dir_all(runs).unwrap();
    let fixture = runs.join(format!("containment-{}", std::process::id()));
    let outside = runs
        .parent()
        .unwrap()
        .join(format!("runs-evil-{}", std::process::id()));
    std::fs::create_dir(&outside).unwrap();
    let sentinel = outside.join("sentinel");
    std::fs::write(&sentinel, b"unchanged").unwrap();
    assert!(
        run_case(Case::Containment, &outside.join("escape")).is_err(),
        "sibling prefix must refuse"
    );
    assert!(
        run_case(Case::Containment, &runs.join("../outside")).is_err(),
        "traversal must refuse"
    );
    let link = runs.join(format!("symlink-{}", std::process::id()));
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    assert!(
        run_case(Case::Containment, &link.join("escape")).is_err(),
        "symlink ancestor must refuse"
    );
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"unchanged");
    assert!(!outside.join("escape").exists());
    assert_eq!(
        run_case(Case::Containment, &fixture).unwrap().status,
        "pass"
    );
    assert!(fixture.is_dir());
    std::fs::write(fixture.join("used"), b"retained").unwrap();
    assert!(
        run_case(Case::Containment, &fixture).is_err(),
        "second call must refuse"
    );
    assert_eq!(std::fs::read(fixture.join("used")).unwrap(), b"retained");
    let empty = runs.join(format!("empty-existing-{}", std::process::id()));
    std::fs::create_dir(&empty).unwrap();
    let err = run_case(Case::Containment, &empty).unwrap_err();
    let MeasurementError::Containment(message) = err else {
        panic!("existing empty root must be refused as containment");
    };
    assert_eq!(
        message,
        empty.display().to_string(),
        "existing empty root must be refused by the existing-root guard, not by create_dir"
    );
    assert!(empty.is_dir() && std::fs::read_dir(&empty).unwrap().next().is_none());
}

#[test]
fn test_raw_blob_and_all_columns_roundtrip() {
    let root = Path::new(RUNS).join(format!("raw-copy-{}", std::process::id()));
    let observation = run_case(Case::RawCopy, &root).unwrap();
    assert_eq!(observation.status, "pass");
    assert!(observation.row_differences.is_empty());
    assert!(
        root.join("raw-copy.json").is_file(),
        "missing typed/blob/cosine oracle evidence"
    );
    let result: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("raw-copy.json")).unwrap()).unwrap();
    assert_eq!(
        observation.schema, result["schema_columns"],
        "Observation.schema must be populated from the measured artifact, not zero-valued"
    );
    assert!(
        observation.duration_ms > 0,
        "Observation.duration_ms must carry the measured duration"
    );
    assert!(
        !observation.artifact_paths.is_empty()
            && observation
                .artifact_paths
                .iter()
                .all(|p| Path::new(p).is_file()),
        "Observation.artifact_paths must reference existing artifacts"
    );
    assert!(!observation.limitations.is_empty());
    assert_eq!(result["oracle_columns"], result["schema_columns"]);
    assert_eq!(result["before"]["row_count"], 3);
    assert_eq!(result["after"]["row_count"], 3);
    assert_eq!(
        result["before"]["typed_rows"],
        result["after"]["typed_rows"]
    );
    assert_eq!(result["negative_controls_rejected"], 4);
    let before = result["before"]["distances"].as_array().unwrap();
    let after = result["after"]["distances"].as_array().unwrap();
    for rows in [before, after] {
        let values: Vec<Vec<f64>> = rows
            .iter()
            .map(|r| {
                r.as_array()
                    .unwrap()
                    .iter()
                    .map(|x| x.as_f64().unwrap())
                    .collect()
            })
            .collect();
        assert!(values[0][0] < values[1][0], "q1 must rank v1 first");
        assert!(values[1][1] < values[0][1], "q2 must rank v2 first");
        for (i, v) in [[2.0_f64, 1.0, 0.0], [1.0, 3.0, 0.0]].iter().enumerate() {
            for (j, q) in [[3.0_f64, 0.0, 1.0], [0.0, 2.0, 1.0]].iter().enumerate() {
                let norm = |a: &[f64; 3]| a.iter().map(|x| x * x).sum::<f64>().sqrt();
                let expected =
                    1.0 - v.iter().zip(q).map(|(x, y)| x * y).sum::<f64>() / (norm(v) * norm(q));
                assert!((values[i][j] - expected).abs() <= 1e-6);
            }
        }
    }
}

#[test]
fn test_identical_ids_collapse_only_after_association_normalization() {
    use mem_store::db::Value;
    use shard_measurement::inspect;
    let columns: Vec<String> = [
        "id",
        "content",
        "container_tag",
        "project_path",
        "project_name",
        "display_name",
        "git_repo_url",
        "is_pinned",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let row = |id: &str, source: &str| {
        vec![
            Value::Text(id.into()),
            Value::Text("<private>historical /home/vaz/repo</private>".into()),
            Value::Text(source.into()),
            Value::Text(format!("/home/vaz/{source}")),
            Value::Text(source.into()),
            Value::Text(source.into()),
            Value::Text(source.into()),
            Value::Integer(1),
        ]
    };
    let inputs = vec![
        ("target".into(), row("shared", "target")),
        ("source1".into(), row("shared", "old1")),
        ("source2".into(), row("shared", "old2")),
        ("source1".into(), row("source-only", "old1")),
        ("target".into(), row("target-only", "target")),
    ];
    let plan = inspect(&columns, &inputs).unwrap();
    assert_eq!(
        plan.rows.len(),
        3,
        "collapse all three occurrences only after association mapping"
    );
    assert_eq!(plan.occurrences["shared"].len(), 3);
    assert_eq!(plan.occurrences.values().map(Vec::len).sum::<usize>(), 5);
    assert!(
        plan.rows
            .iter()
            .all(|r| r[1] == Value::Text("<private>historical /home/vaz/repo</private>".into()))
    );
    let mut reversed = inputs.clone();
    reversed.reverse();
    assert_eq!(inspect(&columns, &reversed).unwrap(), plan);
    assert!(inspect(&columns, &[]).unwrap().rows.is_empty());
    assert_eq!(inspect(&columns, &inputs[..1]).unwrap().rows.len(), 1);
    assert_eq!(inspect(&columns, &inputs[..3]).unwrap().rows.len(), 1);
    let (actual_columns, actual_rows, _) = actual_fixture("duplicates");
    let mut old1 = actual_rows[0].clone();
    let mut old2 = old1.clone();
    for (i, c) in actual_columns.iter().enumerate() {
        if [
            "container_tag",
            "project_path",
            "project_name",
            "display_name",
            "git_repo_url",
        ]
        .contains(&c.as_str())
        {
            old1[i] = Value::Text(format!("old1-{c}"));
            old2[i] = Value::Text(format!("old2-{c}"));
        }
    }
    let actual_inputs = vec![
        ("target".into(), actual_rows[0].clone()),
        ("source1".into(), old1),
        ("source2".into(), old2),
        ("source1".into(), actual_rows[1].clone()),
        ("target".into(), actual_rows[2].clone()),
    ];
    let actual_plan = inspect(&actual_columns, &actual_inputs).unwrap();
    assert_eq!(actual_plan.rows.len(), 3);
    assert_eq!(
        actual_plan
            .occurrences
            .values()
            .map(Vec::len)
            .sum::<usize>(),
        5
    );
    let mut reverse = actual_inputs.clone();
    reverse.reverse();
    assert_eq!(inspect(&actual_columns, &reverse).unwrap(), actual_plan);
}

fn actual_fixture(
    label: &str,
) -> (
    Vec<String>,
    Vec<Vec<mem_store::db::Value>>,
    std::path::PathBuf,
) {
    use mem_store::db::Value;
    let root = Path::new(RUNS).join(format!("{label}-{}", std::process::id()));
    run_case(Case::RawCopy, &root).unwrap();
    let result: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("raw-copy.json")).unwrap()).unwrap();
    let cols = result["schema_columns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_string())
        .collect();
    let rows = result["before"]["typed_rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            r.as_array()
                .unwrap()
                .iter()
                .map(|v| {
                    if v.get("null").is_some() {
                        Value::Null
                    } else if let Some(n) = v.get("integer") {
                        Value::Integer(n.as_i64().unwrap())
                    } else if let Some(n) = v.get("real_bits") {
                        Value::Real(f64::from_bits(n.as_u64().unwrap()))
                    } else if let Some(n) = v.get("text") {
                        Value::Text(n.as_str().unwrap().into())
                    } else {
                        Value::Blob(
                            v["blob"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .map(|n| n.as_u64().unwrap() as u8)
                                .collect(),
                        )
                    }
                })
                .collect()
        })
        .collect();
    (cols, rows, root)
}

#[test]
fn test_any_nonassociation_difference_aborts_without_writes() {
    use mem_store::db::Value;
    let (columns, rows, root) = actual_fixture("divergent");
    let hashes = || {
        let mut files: Vec<_> = std::fs::read_dir(root.join("work"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        files.sort();
        files
            .into_iter()
            .map(|p| {
                let out = std::process::Command::new("sha256sum")
                    .arg(&p)
                    .output()
                    .unwrap();
                assert!(out.status.success());
                out.stdout
            })
            .collect::<Vec<_>>()
    };
    let before = hashes();
    let id = columns.iter().position(|c| c == "id").unwrap();
    let mut examined = 0;
    for (index, column) in columns.iter().enumerate() {
        if [
            "id",
            "container_tag",
            "project_path",
            "project_name",
            "display_name",
            "git_repo_url",
        ]
        .contains(&column.as_str())
        {
            continue;
        }
        let mut divergent = rows[1].clone();
        divergent[index] = match &divergent[index] {
            Value::Null => Value::Blob(vec![1, 2, 3]),
            Value::Integer(x) => Value::Integer(x + 1),
            Value::Real(x) => Value::Real(x + 1.0),
            Value::Text(x) => Value::Text(format!("{x}-different")),
            Value::Blob(x) => {
                let mut x = x.clone();
                x[0] ^= 1;
                Value::Blob(x)
            }
        };
        let mut good = rows[2].clone();
        good[id] = Value::Text("a-copyable-first".into());
        let inputs = vec![
            ("target".into(), rows[1].clone()),
            ("source-good".into(), good),
            ("source-conflict-last".into(), divergent),
        ];
        assert!(
            shard_measurement::inspect(&columns, &inputs).is_err(),
            "{column} mismatch must abort all preflight"
        );
        assert_eq!(
            hashes(),
            before,
            "no closed work file may change on {column} conflict"
        );
        examined += 1;
    }
    assert_eq!(examined, columns.len() - 6);
    let tag = columns.iter().position(|c| c == "tags_vector").unwrap();
    let mut null_row = rows[1].clone();
    null_row[tag] = Value::Null;
    assert!(
        shard_measurement::inspect(
            &columns,
            &[("one".into(), rows[1].clone()), ("null".into(), null_row)]
        )
        .is_err()
    );
    assert_eq!(hashes(), before);
}

#[test]
fn test_cross_db_mapping_preserves_payload_and_rejects_unknowns() {
    use mem_store::db::Value;
    use shard_measurement::remap_identity;
    for table in [
        "agent_sessions",
        "episodic_milestones",
        "episodic_raw_context",
        "episodic_segments",
        "capture_inbox",
    ] {
        for source in ["/home/vaz/synthetic-a", "/home/vaz/synthetic-b"] {
            assert_eq!(
                remap_identity(table, "project_id", &Value::Text(source.into())).unwrap(),
                Value::Text("/home/opencode/synthetic-missing-repo".into())
            );
        }
        assert_eq!(
            remap_identity(
                table,
                "project_id",
                &Value::Text("/home/vaz/Code/Personal/JS/opencode".into())
            )
            .unwrap(),
            Value::Text("/home/opencode/Code/opencode".into())
        );
        assert!(
            remap_identity(
                table,
                "project_id",
                &Value::Text("unsupported:encoded-hash".into())
            )
            .is_err()
        );
    }
    assert_eq!(
        remap_identity(
            "user_prompts",
            "project_path",
            &Value::Text("/home/vaz/synthetic-a".into())
        )
        .unwrap(),
        Value::Text("/home/opencode/synthetic-missing-repo".into())
    );
    assert_eq!(
        remap_identity("skills", "container_tag", &Value::Null).unwrap(),
        Value::Null
    );
    assert_eq!(
        remap_identity(
            "skills",
            "container_tag",
            &Value::Text("synthetic_project_old_a".into())
        )
        .unwrap(),
        Value::Text("synthetic_project_target".into())
    );
    assert!(
        remap_identity(
            "unknown_table",
            "project_id",
            &Value::Text("/home/vaz/synthetic-a".into())
        )
        .is_err()
    );
    assert!(
        remap_identity(
            "facts",
            "project_id",
            &Value::Text("/home/vaz/synthetic-a".into())
        )
        .is_err()
    );
    assert!(
        remap_identity(
            "vault_entries",
            "content_hash",
            &Value::Text("opaque".into())
        )
        .is_err()
    );
    assert!(!Path::new("/home/opencode/synthetic-missing-repo").exists());
    let root = Path::new(RUNS).join(format!("auxiliary-{}", std::process::id()));
    run_case(Case::Containment, &root).unwrap();
    let result = tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(shard_measurement::auxiliary_fixture(&root))
        .unwrap();
    assert_eq!(result["tables"].as_object().unwrap().len(), 8);
    for name in ["facts.db", "vault", "vault-keys", "ai-sessions.db"] {
        let child = root.join(format!("guard-{name}"));
        std::fs::create_dir(&child).unwrap();
        std::fs::write(child.join(name), b"synthetic unsupported sentinel").unwrap();
        let before = std::fs::read(child.join(name)).unwrap();
        assert!(shard_measurement::refuse_unsupported_auxiliary(&child).is_err());
        assert_eq!(std::fs::read(child.join(name)).unwrap(), before);
    }
    assert_eq!(
        remap_identity(
            "user_prompts",
            "project_path",
            &Value::Text("/unrelated/sibling".into())
        )
        .unwrap(),
        Value::Text("/unrelated/sibling".into())
    );

    for table in [
        "agent_sessions",
        "episodic_milestones",
        "episodic_raw_context",
        "episodic_segments",
        "capture_inbox",
        "user_prompts",
        "skills",
    ] {
        assert_eq!(result["tables"][table]["payload_preserved"], true);
        assert_eq!(result["tables"][table]["collision_refused"], true);
    }
}

#[test]
fn test_owned_child_refuses_outside_root() {
    let result = std::process::Command::new(env!("CARGO_BIN_EXE_shard-measurement"))
        .args(["child", "/outside-owned-fixture", "shard-before-registry"])
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("Engine"));
    assert!(!String::from_utf8_lossy(&result.stdout).contains("SHARD-MEASUREMENT-PHASE"));
}

#[test]
fn test_dedicated_case_experiments_persist_observations() {
    let stamp = std::process::id();
    let dup_root = Path::new(RUNS).join(format!("exact-dups-{stamp}"));
    let dup = run_case(Case::ExactDuplicates, &dup_root).unwrap();
    assert_eq!(dup.status, "pass");
    assert!(dup.duration_ms > 0);
    assert!(dup.artifact_paths.iter().all(|p| Path::new(p).is_file()));
    let persisted: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dup_root.join("duplicates.json")).unwrap()).unwrap();
    assert_eq!(
        dup.duplicate_occurrences, persisted,
        "Observation must carry the persisted duplicate manifest, not a self-reported flag"
    );
    assert_eq!(persisted["plan_rows"].as_array().unwrap().len(), 3);
    let occurrences = persisted["occurrences"].as_object().unwrap();
    assert_eq!(occurrences.len(), 3);
    assert_eq!(occurrences["v1"].as_array().unwrap().len(), 3);
    assert_eq!(
        occurrences
            .values()
            .map(|v| v.as_array().unwrap().len())
            .sum::<usize>(),
        5
    );

    let div_root = Path::new(RUNS).join(format!("divergent-ids-{stamp}"));
    let div = run_case(Case::DivergentIds, &div_root).unwrap();
    assert_eq!(div.status, "pass");
    let refusal: serde_json::Value =
        serde_json::from_slice(&std::fs::read(div_root.join("divergent.json")).unwrap()).unwrap();
    assert_eq!(
        div.barrier_observations, refusal,
        "Observation must carry the persisted refusal manifest"
    );
    assert_eq!(refusal["refused"], true);
    assert_eq!(refusal["conflict"]["id"], "v1");
    assert_eq!(refusal["conflict"]["column"], "content");
    assert!(div.duplicate_occurrences.is_null());
}

#[test]
fn test_cross_db_run_case_reopens_and_verifies_persisted_state() {
    let root = Path::new(RUNS).join(format!("cross-db-run-{}", std::process::id()));
    let obs = run_case(Case::CrossDb, &root).unwrap();
    assert_eq!(obs.status, "pass");
    assert!(obs.duration_ms > 0);
    assert!(
        obs.artifact_paths.iter().all(|p| Path::new(p).is_file()),
        "every advertised artifact must exist"
    );
    assert_eq!(
        obs.barrier_observations["persisted_verified"], true,
        "reopened persisted state must be verified against the mapped manifest, not self-reported"
    );
    assert_eq!(obs.barrier_observations["tables"], 8);
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(root.join("auxiliary-refusal-manifest.json")).unwrap(),
    )
    .unwrap();
    for case in ["facts.db", "vault", "vault-keys", "ai-sessions.db"] {
        assert_eq!(
            manifest["cases"][case]["refused"], true,
            "{case} must refuse"
        );
        assert!(
            !manifest["cases"][case]["manifest"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
    assert_eq!(manifest["cases"]["unknown_encoding"]["refused"], true);
    assert_eq!(manifest["cases"]["divergent_aux_key"]["refused"], true);
    let entry = manifest["root_manifest"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["path"].as_str().unwrap().ends_with("auxiliary.json"))
        .unwrap()
        .clone();
    let out = std::process::Command::new("sha256sum")
        .arg(root.join("auxiliary.json"))
        .output()
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap(),
        entry["sha256"].as_str().unwrap(),
        "retained manifest hash must match an independent sha256sum recomputation"
    );
    let sentinel = root.join("refusal-guards/facts.db/facts.db");
    assert_eq!(
        std::fs::read(&sentinel).unwrap(),
        b"synthetic unsupported sentinel",
        "refusal guards must not be modified by the refusal checks"
    );
}

/// Ownership locks are custody, not restorable data (blueprint step 8).
const S08_LOCK_FILES: [&str; 4] = [
    ".engine.lock",
    ".turso-operation.lock",
    ".turso-migrate.lock",
    "status",
];

fn s08_sha256(path: &Path) -> String {
    let out = std::process::Command::new("sha256sum")
        .arg(path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "sha256sum failed for {}",
        path.display()
    );
    String::from_utf8(out.stdout)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string()
}

fn s08_walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            s08_walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// Baseline snapshot of the CLOSED root: every non-ownership file with its
/// bytes' SHA-256 and length; ownership locks are reported separately as
/// excluded-from-restoration custody artifacts.
fn s08_manifest_excluding_locks(root: &Path) -> (Vec<(PathBuf, String, u64)>, Vec<PathBuf>) {
    let mut all = Vec::new();
    s08_walk(root, &mut all);
    let mut baseline = Vec::new();
    let mut excluded = Vec::new();
    for path in all {
        let name = path.file_name().unwrap().to_str().unwrap().to_string();
        if S08_LOCK_FILES.contains(&name.as_str()) {
            excluded.push(path);
        } else {
            baseline.push((
                path.clone(),
                s08_sha256(&path),
                path.metadata().unwrap().len(),
            ));
        }
    }
    (baseline, excluded)
}

/// Every file currently under the root, hashes recomputed (no cache), sorted.
fn s08_full_hash_set(root: &Path) -> Vec<(PathBuf, String)> {
    let mut paths = Vec::new();
    s08_walk(root, &mut paths);
    paths
        .into_iter()
        .map(|path| {
            let hash = s08_sha256(&path);
            (path, hash)
        })
        .collect()
}

/// Restore is a pure byte-level action over an immutable in-memory snapshot:
/// rewrite every baseline tuple, then remove every file the baseline does not
/// know — including any new ownership lock, so a dead PID lock is never
/// restored as active custody.
fn s08_restore(
    root: &Path,
    baseline: &[(PathBuf, String, u64)],
    bytes: &std::collections::HashMap<PathBuf, Vec<u8>>,
) {
    // ponytail: byte-level restore over the immutable snapshot; upgrade path
    // would stream large tuples instead of holding them in memory (fixture
    // scale is <100 KB here).
    for (path, _, _) in baseline {
        std::fs::write(path, &bytes[path]).unwrap();
    }
    let mut current = Vec::new();
    s08_walk(root, &mut current);
    let known: std::collections::HashSet<&PathBuf> =
        baseline.iter().map(|(path, _, _)| path).collect();
    for path in current {
        if !known.contains(&path) {
            std::fs::remove_file(&path).unwrap();
        }
    }
}

fn s08_typed(value: &Value) -> serde_json::Value {
    match value {
        Value::Null => serde_json::json!({ "null": true }),
        Value::Integer(x) => serde_json::json!({ "integer": x }),
        Value::Real(x) => serde_json::json!({ "real_bits": x.to_bits() }),
        Value::Text(x) => serde_json::json!({ "text": x }),
        Value::Blob(x) => serde_json::json!({ "blob": x, "len": x.len() }),
    }
}

/// Typed logical oracle: reopen every fixture DB via Turso, verify integrity,
/// and serialize every table's typed rows as sorted sets.
async fn s08_typed_oracle(root: &Path) -> serde_json::Value {
    let mut oracle = serde_json::Map::new();
    for name in ["shard.db", "registry.db", "aux-one.db", "aux-two.db"] {
        let db = open(&root.join(name)).await.unwrap();
        let integrity = db
            .query("PRAGMA integrity_check", vec![])
            .await
            .unwrap()
            .first()
            .and_then(|row| row.get("integrity_check").cloned())
            .unwrap();
        let Value::Text(status) = integrity else {
            panic!("integrity_check must return text for {name}");
        };
        assert_eq!(status, "ok", "integrity_check failed for {name}");
        let mut tables = Vec::new();
        for row in db
            .query(
                "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
                vec![],
            )
            .await
            .unwrap()
        {
            let Value::Text(table) = row.get("name").cloned().unwrap() else {
                panic!("table name must be text in {name}");
            };
            tables.push(table);
        }
        let mut per_table = serde_json::Map::new();
        for table in tables {
            let rows = db
                .query(&format!("SELECT * FROM {table}"), vec![])
                .await
                .unwrap();
            let mut serialized: Vec<String> = rows
                .iter()
                .map(|row| {
                    let cells: Vec<serde_json::Value> = row
                        .column_names()
                        .iter()
                        .map(|column| s08_typed(&row.get(column).cloned().unwrap()))
                        .collect();
                    serde_json::to_string(&cells).unwrap()
                })
                .collect();
            serialized.sort();
            per_table.insert(table, serde_json::json!(serialized));
        }
        oracle.insert(name.to_string(), serde_json::Value::Object(per_table));
    }
    serde_json::Value::Object(oracle)
}

fn s08_run_phase(phase: &str) -> serde_json::Value {
    let root = Path::new(RUNS).join(format!(
        "s08-{}-{}",
        phase,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));

    // 1. Fresh synthetic case root, created and CLOSED by the prepare CLI.
    let prepare = std::process::Command::new(env!("CARGO_BIN_EXE_shard-measurement"))
        .args(["prepare", root.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        prepare.status.success(),
        "prepare failed: {}",
        String::from_utf8_lossy(&prepare.stderr)
    );

    let runtime = tokio::runtime::Runtime::new().unwrap();

    // 2. Legitimate EngineLock custody in this parent process.
    let started = std::time::Instant::now();
    let custody = match acquire_engine_lock(&root, std::sync::Arc::new(SystemProcessProbe)).unwrap()
    {
        EngineLockOutcome::Acquired(lock) => lock,
        EngineLockOutcome::Lost { holder_pid } => {
            panic!("parent custody refused by live holder pid {holder_pid}")
        }
    };
    let custody_ms = started.elapsed().as_millis() as u64;

    // 3. Baseline typed oracle read under custody; handles dropped, root closed.
    let started = std::time::Instant::now();
    let baseline_oracle = runtime.block_on(s08_typed_oracle(&root));
    let oracle_ms = started.elapsed().as_millis() as u64;
    let fixture_rows = baseline_oracle["registry.db"]["fixture"]
        .as_array()
        .unwrap();
    assert_eq!(fixture_rows.len(), 1, "baseline fixture row must be read");
    assert!(
        fixture_rows[0].as_str().unwrap().contains("baseline"),
        "baseline fixture value must be the sentinel, not an empty capture"
    );
    assert_eq!(
        baseline_oracle["shard.db"]["memories"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert!(
        baseline_oracle["shard.db"].get("shard_metadata").is_some(),
        "child-stage table must exist before the interruption"
    );

    // 4. Snapshot the entire CLOSED root before any checkpoint/open of the
    // interrupted run; bytes and manifest are immutable from here on.
    let started = std::time::Instant::now();
    let (baseline, excluded_locks) = s08_manifest_excluding_locks(&root);
    let mut snapshot_bytes = std::collections::HashMap::new();
    for (path, _, _) in &baseline {
        snapshot_bytes.insert(path.clone(), std::fs::read(path).unwrap());
    }
    let snapshot_ms = started.elapsed().as_millis() as u64;
    let baseline_manifest: Vec<serde_json::Value> = baseline
        .iter()
        .map(|(path, hash, len)| {
            serde_json::json!({
                "path": path.display().to_string(),
                "sha256": hash,
                "len": len,
            })
        })
        .collect();
    assert!(
        baseline_manifest
            .iter()
            .any(|entry| entry["path"].as_str().unwrap().ends_with("shard.db-wal")),
        "baseline must contain the committed WAL tuple"
    );

    // 5. Release parent custody: the fixture root is unowned between runs.
    assert!(
        root.join(".engine.lock").exists(),
        "custody lock file must exist while held"
    );
    drop(custody);
    assert!(
        !root.join(".engine.lock").exists(),
        "release must unlink the custody lock"
    );

    // 6. Spawn the owned child and wait for its flushed phase marker under a
    // bounded (30 s) explicit handshake; never sleep-based scheduling.
    let started = std::time::Instant::now();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_shard-measurement"))
        .args(["child", root.to_str().unwrap(), phase])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (marker_tx, marker_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let mut reader = std::io::BufReader::new(stdout);
        let _ = std::io::BufRead::read_line(&mut reader, &mut line);
        let _ = marker_tx.send(line);
    });
    let marker = marker_rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .unwrap_or_else(|_| {
            let _ = child.kill();
            let _ = child.wait();
            panic!("phase marker wait timed out after 30s; INCONCLUSIVE, child killed");
        });
    let marker_ms = started.elapsed().as_millis() as u64;
    assert_eq!(
        marker.trim(),
        format!("SHARD-MEASUREMENT-PHASE {phase}"),
        "flushed marker must name this phase"
    );
    assert!(marker_ms < 30_000, "child exceeded its bounded lifetime");

    // 7. Interrupt ONLY the spawned child, immediately after the marker, while
    // it blocks on the explicit stdin handshake. No RELEASE is ever sent here.
    let started = std::time::Instant::now();
    child.kill().expect("owned child kill");
    let status = child.wait().expect("owned child reap");
    let kill_ms = started.elapsed().as_millis() as u64;
    assert!(!status.success(), "interrupted child must not exit cleanly");

    // 8. The interruption delta: which tuples changed, appeared, vanished.
    let mut changed = Vec::new();
    let mut appeared = Vec::new();
    let mut missing = Vec::new();
    let baseline_hashes: std::collections::HashMap<PathBuf, String> = baseline
        .iter()
        .map(|(path, hash, _)| (path.clone(), hash.clone()))
        .collect();
    for path in &s08_full_hash_set(&root) {
        match baseline_hashes.get(&path.0) {
            Some(hash) if *hash != path.1 => changed.push(path.0.display().to_string()),
            Some(_) => {}
            None => appeared.push(path.0.display().to_string()),
        }
    }
    for (path, _, _) in &baseline {
        if !path.exists() {
            missing.push(path.display().to_string());
        }
    }
    let touched = |suffix: &str| changed.iter().any(|path| path.ends_with(suffix));
    assert!(
        appeared.iter().any(|path| path.ends_with(".engine.lock")),
        "interrupted child must have held custody when killed"
    );
    assert!(
        missing.is_empty(),
        "the child never deletes baseline tuples"
    );
    match phase {
        "shard-before-registry" => {
            assert!(
                touched("shard.db-wal"),
                "committed shard-stage WAL write must be left mid-flight"
            );
            for untouched in [
                "registry.db",
                "registry.db-wal",
                "aux-one.db",
                "aux-one.db-wal",
                "aux-two.db",
                "aux-two.db-wal",
            ] {
                assert!(
                    !touched(untouched),
                    "phase (a) interrupts before registry: {untouched} must be untouched"
                );
            }
        }
        "aux-one-before-two" => {
            for committed in ["shard.db-wal", "registry.db-wal", "aux-one.db-wal"] {
                assert!(
                    touched(committed),
                    "phase (b) commits {committed} before interruption"
                );
            }
            for untouched in ["aux-two.db", "aux-two.db-wal"] {
                assert!(
                    !touched(untouched),
                    "phase (b) interrupts before the second auxiliary: {untouched} must be untouched"
                );
            }
        }
        other => panic!("unexpected phase {other}"),
    }

    // 9. Restore #1: every baseline tuple back, every new tuple removed.
    let started = std::time::Instant::now();
    s08_restore(&root, &baseline, &snapshot_bytes);
    let restore1_ms = started.elapsed().as_millis() as u64;
    let restored = s08_full_hash_set(&root);
    let baseline_set: std::collections::HashSet<PathBuf> =
        baseline.iter().map(|(path, _, _)| path.clone()).collect();
    let restored_set: std::collections::HashSet<PathBuf> =
        restored.iter().map(|(path, _)| path.clone()).collect();
    let restore1_set_match = restored_set == baseline_set;
    let restore1_hash_match = restored
        .iter()
        .all(|(path, hash)| baseline_hashes.get(path) == Some(hash));
    assert!(
        restore1_set_match,
        "restore #1 must leave exactly the baseline file set (zero extra tuple files)"
    );
    assert!(
        restore1_hash_match,
        "restored bytes must equal the immutable baseline hashes before any reopen"
    );
    for absent in &excluded_locks {
        assert!(
            !absent.exists(),
            "ownership lock must not be restored: {}",
            absent.display()
        );
    }

    // 10. Restore #2 before reopen: byte-idempotence under no stale ownership.
    let started = std::time::Instant::now();
    s08_restore(&root, &baseline, &snapshot_bytes);
    let restore2_ms = started.elapsed().as_millis() as u64;
    let idempotent = s08_full_hash_set(&root) == restored;
    assert!(
        idempotent,
        "repeated restore must be byte-idempotent before reopen"
    );

    // 11. Reacquire fresh legitimate custody, never the killed child's lock.
    let started = std::time::Instant::now();
    let reacquired =
        match acquire_engine_lock(&root, std::sync::Arc::new(SystemProcessProbe)).unwrap() {
            EngineLockOutcome::Acquired(lock) => lock,
            EngineLockOutcome::Lost { holder_pid } => {
                panic!("reacquire refused by live holder pid {holder_pid}")
            }
        };
    let reacquire_ms = started.elapsed().as_millis() as u64;
    assert!(
        root.join(".engine.lock").exists(),
        "reacquire must create fresh lock ownership"
    );

    // 12. Reopen via Turso: integrity plus the full typed logical baseline.
    let started = std::time::Instant::now();
    let recovered_oracle = runtime.block_on(s08_typed_oracle(&root));
    let reopen_ms = started.elapsed().as_millis() as u64;
    assert_eq!(
        recovered_oracle, baseline_oracle,
        "recovered typed rows must equal the baseline typed oracle"
    );
    let shard_metadata =
        serde_json::to_string(&recovered_oracle["shard.db"]["shard_metadata"]).unwrap();
    assert!(
        !shard_metadata.contains("child-stage"),
        "the committed child write must not survive the restore"
    );
    assert!(
        recovered_oracle["registry.db"]["fixture"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| !row.as_str().unwrap().contains("registry committed")),
        "phase (b) registry commit must not survive the restore"
    );
    drop(reacquired);

    // 13. Persisted report; read back so the evidence is the artifact, and
    // cross-check one baseline hash by independent recomputation.
    let sample = baseline
        .iter()
        .find(|(path, _, _)| path.ends_with("registry.db"))
        .unwrap();
    assert_eq!(
        s08_sha256(&sample.0),
        sample.1,
        "baseline hash must match an independent sha256sum recomputation"
    );
    let report = serde_json::json!({
        "phase": phase,
        "root": root.display().to_string(),
        "baseline": {
            "manifest": baseline_manifest,
            "excluded_locks": excluded_locks
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>(),
            "typed_oracle": baseline_oracle,
        },
        "custody_ms": custody_ms,
        "oracle_ms": oracle_ms,
        "snapshot_ms": snapshot_ms,
        "marker": marker.trim(),
        "marker_ms": marker_ms,
        "kill_ms": kill_ms,
        "interrupt_delta": {
            "changed": changed,
            "appeared": appeared,
            "missing": missing,
        },
        "restore1_ms": restore1_ms,
        "restore1_set_match": restore1_set_match,
        "restore1_hash_match": restore1_hash_match,
        "restore2_ms": restore2_ms,
        "restore2_idempotent": idempotent,
        "reacquire_ms": reacquire_ms,
        "reopen_verify_ms": reopen_ms,
        "recovered_typed_oracle": recovered_oracle,
        "limitations": [
            "process-kill recovery only; no power-loss or filesystem atomicity claim",
            "byte equality after Turso reopen is not required: reopening may rewrite sidecars",
            "auxiliary coverage limited to synthetic registry/aux-one/aux-two fixtures",
        ],
    });
    let report_path = root.join("restore-report.json");
    std::fs::write(&report_path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    let persisted: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();
    assert_eq!(persisted["restore1_set_match"], serde_json::json!(true));
    assert_eq!(persisted["restore1_hash_match"], serde_json::json!(true));
    assert_eq!(persisted["restore2_idempotent"], serde_json::json!(true));
    assert!(
        !persisted["interrupt_delta"]["changed"]
            .as_array()
            .unwrap()
            .is_empty(),
        "persisted report must carry the interruption delta"
    );
    persisted
}

#[test]
fn test_interrupted_cross_db_restore_matches_baseline_and_is_idempotent() {
    for phase in ["shard-before-registry", "aux-one-before-two"] {
        let report = s08_run_phase(phase);
        assert_eq!(report["phase"], serde_json::json!(phase));
        assert!(
            report["baseline"]["manifest"].as_array().unwrap().len() >= 4,
            "baseline manifest must cover every DB tuple"
        );
        assert!(
            !report["interrupt_delta"]["appeared"]
                .as_array()
                .unwrap()
                .is_empty(),
            "each phase must leave a new tuple behind (the child's custody lock)"
        );
    }
}

/// `futures::future::BoxFuture` under its structural definition: aliases are
/// transparent, so this local alias names the same type the store API takes.
type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// Bounded scope-barrier window. The negative poll and the positive control share
/// it: a degenerate window must be visible to both, not only to the negative one.
const S09_SCOPE_POLL: std::time::Duration = std::time::Duration::from_secs(1);

fn s09_signal_op(
    done: tokio::sync::oneshot::Sender<&'static str>,
) -> BoxFuture<'static, Result<(), StoreError>> {
    Box::pin(async move {
        let _ = done.send("done");
        Ok(())
    })
}

fn s09_latched_op(
    admitted: tokio::sync::oneshot::Sender<()>,
    release: tokio::sync::oneshot::Receiver<()>,
) -> BoxFuture<'static, Result<(), StoreError>> {
    Box::pin(async move {
        let _ = admitted.send(());
        let _ = release.await;
        Ok(())
    })
}

fn s09_config() -> StoreConfig {
    StoreConfig {
        storage_path: PathBuf::from("/unused-s09"),
        embedding_dimensions: 3,
        embedding_model: "synthetic".into(),
        max_vectors_per_shard: 99,
        container_tag_prefix: "synthetic".into(),
        database_encryption_enabled: false,
        database_encryption_cipher: "aes256gcm".into(),
        database_encryption_key: None,
        allow_unguarded_open: false,
    }
}

fn s09_manager(root: &Path) -> ShardManager {
    let config = s09_config();
    let connections = std::sync::Arc::new(ConnectionManager::new(
        root.to_path_buf(),
        config.clone(),
        Platform::current(),
        std::sync::Arc::new(turso_connect_factory()),
    ));
    ShardManager::new(
        root.to_path_buf(),
        config,
        connections,
        std::sync::Arc::new(SystemProcessProbe),
        None,
    )
}

/// Real auxiliary read/write through the harness's Turso interface. Returns
/// the read-back payload.
async fn s09_aux_write_async(root: &Path, payload: &str) -> String {
    let db = open(&root.join("aux-one.db")).await.unwrap();
    db.execute(
        "CREATE TABLE IF NOT EXISTS barrier_probe(id INTEGER PRIMARY KEY, payload TEXT)",
        vec![],
    )
    .await
    .unwrap();
    db.execute(
        "INSERT OR REPLACE INTO barrier_probe(id,payload) VALUES (1, ?1)",
        vec![Value::Text(payload.to_string())],
    )
    .await
    .unwrap();
    let rows = db
        .query("SELECT payload FROM barrier_probe WHERE id=1", vec![])
        .await
        .unwrap();
    let read_back = rows
        .first()
        .and_then(|row| row.get("payload").cloned())
        .unwrap();
    let Value::Text(read_back) = read_back else {
        panic!("auxiliary probe payload must be text");
    };
    assert_eq!(read_back, payload, "auxiliary write must read back");
    read_back
}

/// Synchronous wrapper measuring the whole read/write cycle in microseconds.
fn s09_aux_write(runtime: &tokio::runtime::Runtime, root: &Path, payload: &str) -> (String, u128) {
    let started = std::time::Instant::now();
    let read_back = runtime.block_on(s09_aux_write_async(root, payload));
    (read_back, started.elapsed().as_micros())
}

/// The observation ledger only ever carries measured statuses; an unknown path
/// classifies as `unknown`, never as `blocked` (negative-oracle rule).
fn s09_classify(paths: &serde_json::Value, name: &str) -> &'static str {
    paths
        .get(name)
        .and_then(|entry| entry.get("status"))
        .and_then(|status| status.as_str())
        .map(|status| match status {
            "blocked" => "blocked",
            "completed" => "completed",
            "refused" => "refused",
            "not_exercised" => "not_exercised",
            _ => "unknown",
        })
        .unwrap_or("unknown")
}

#[test]
fn test_existing_barriers_report_observed_coverage() {
    let root = Path::new(RUNS).join(format!(
        "s09-admission-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let probe = std::sync::Arc::new(SystemProcessProbe);
    // Measured statuses land here through s09_record; the RED revision forced
    // every status to the hardcoded "blocked" and failed the completed/refused
    // expectations below.
    let mut paths = serde_json::Map::new();
    let mut timings_us = serde_json::Map::new();
    let mut record = |name: &str, status: &'static str, detail: String| {
        paths.insert(
            name.to_string(),
            serde_json::json!({ "status": status, "detail": detail }),
        );
    };

    // ---- operation lock: held vs refused vs released ----
    let held = acquire_turso_operation_lock(&root, probe.clone(), "synthetic-measurement").unwrap();
    let live = read_live_lock(&root, OPERATION_LOCK_FILE, probe.as_ref());
    let live_status = if live.as_ref().is_some_and(|s| s.pid == std::process::id()) {
        "completed"
    } else {
        "blocked"
    };
    record(
        "operation-lock/live-lock-read-held",
        live_status,
        format!(
            "read_live_lock while held: {:?}",
            live.as_ref().map(|s| s.pid)
        ),
    );
    assert_eq!(live_status, "completed", "held lock must read back live");

    let second = acquire_turso_operation_lock(&root, probe.clone(), "synthetic-second");
    let second_status = match &second {
        Err(StoreError::MigrationLocked { pid, .. }) if *pid == std::process::id() => "refused",
        _ => "completed",
    };
    record(
        "operation-lock/second-acquire-while-held",
        second_status,
        format!(
            "second acquire outcome: {:?}",
            second.as_ref().err().map(|e| e.to_string())
        ),
    );
    assert_eq!(
        second_status, "refused",
        "same-root operation lock must refuse a live second holder with MigrationLocked"
    );

    let migration_assert = assert_no_turso_migration_in_progress(&root, probe.as_ref());
    let assert_status = if migration_assert.is_err() {
        "refused"
    } else {
        "completed"
    };
    record(
        "operation-lock/migration-assert-held",
        assert_status,
        format!(
            "migration assert while held: {:?}",
            migration_assert.err().map(|e| e.to_string())
        ),
    );
    assert_eq!(
        assert_status, "refused",
        "migration assert must refuse while the operation lock is live"
    );

    let aux_op = s09_aux_write(&runtime, &root, "operation-lock-held");
    timings_us.insert(
        "aux_write_while_operation_held_us".into(),
        (aux_op.1 as u64).into(),
    );
    record(
        "auxiliary/read-write-while-operation-held",
        "completed",
        format!("read-back {}", aux_op.0),
    );

    drop(held);
    assert!(
        !root.join(OPERATION_LOCK_FILE).exists(),
        "operation lock release must unlink the file"
    );
    let released = acquire_turso_operation_lock(&root, probe.clone(), "synthetic-released");
    let released_status = if released.is_ok() {
        "completed"
    } else {
        "blocked"
    };
    record(
        "operation-lock/acquire-after-release",
        released_status,
        format!(
            "acquire after release: {:?}",
            released.as_ref().err().map(|e| e.to_string())
        ),
    );
    assert_eq!(
        released_status, "completed",
        "released operation lock must allow acquisition"
    );
    let migration_released = {
        drop(released);
        assert_no_turso_migration_in_progress(&root, probe.as_ref())
    };
    let migration_released_status = if migration_released.is_ok() {
        "completed"
    } else {
        "blocked"
    };
    record(
        "operation-lock/migration-assert-released",
        migration_released_status,
        "migration assert after release".to_string(),
    );
    assert_eq!(migration_released_status, "completed");

    // ---- stale-handle coverage: dead-PID operation lock is reclaimed ----
    let mut stale_child = std::process::Command::new(env!("CARGO_BIN_EXE_shard-measurement"))
        .args(["child", "/outside-owned-fixture", "bogus-phase"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let dead_pid = stale_child.id();
    let stale_exit = stale_child.wait().unwrap();
    assert!(!stale_exit.success());
    assert!(
        !probe.is_alive(dead_pid),
        "stale fixture requires a reaped pid; pid {dead_pid} is alive"
    );
    std::fs::write(
        root.join(OPERATION_LOCK_FILE),
        format!(
            "{{\"pid\":{dead_pid},\"timestamp\":\"1970-01-01T00:00:00.000Z\",\"operation\":\"synthetic-stale\"}}"
        ),
    )
    .unwrap();
    assert!(root.join(OPERATION_LOCK_FILE).exists());
    // Measured mechanism (operation_lock.rs): acquire runs the migration
    // assert first, and read_live_lock unlinks a dead-owner lock as a side
    // effect — so a stale lock is reclaimed transparently before create_new.
    let stale_read = read_live_lock(&root, OPERATION_LOCK_FILE, probe.as_ref());
    let stale_reclaimed = stale_read.is_none() && !root.join(OPERATION_LOCK_FILE).exists();
    record(
        "operation-lock/stale-dead-pid-reclaim",
        if stale_reclaimed {
            "completed"
        } else {
            "blocked"
        },
        format!(
            "read_live_lock over a dead-pid lock: {:?}; lock file unlinked by the stale read: {stale_reclaimed}",
            stale_read.map(|s| s.pid)
        ),
    );
    assert!(
        stale_reclaimed,
        "reading the stale lock must unlink the dead-owner file (reclaim)"
    );
    let stale_retry = acquire_turso_operation_lock(&root, probe.clone(), "synthetic-stale-retry");
    let stale_retry_status = if stale_retry.is_ok() {
        "completed"
    } else {
        "blocked"
    };
    record(
        "operation-lock/stale-dead-pid-acquire-after-reclaim",
        stale_retry_status,
        format!(
            "acquire after reclaim: {:?}",
            stale_retry.as_ref().err().map(|e| e.to_string())
        ),
    );
    assert_eq!(
        stale_retry_status, "completed",
        "after the stale lock is reclaimed the acquire must complete"
    );
    drop(stale_retry);

    // ---- delete guard registry: engine custody presence is observable ----
    let custody = match acquire_engine_lock(&root, probe.clone()).unwrap() {
        EngineLockOutcome::Acquired(lock) => lock,
        EngineLockOutcome::Lost { holder_pid } => {
            panic!("custody refused by live holder pid {holder_pid}")
        }
    };
    let registry_while = ExclusiveRoot::current(&root);
    let registry_while_status = if registry_while.is_some() {
        "completed"
    } else {
        "blocked"
    };
    record(
        "delete-guard/registry-while-custody",
        registry_while_status,
        format!(
            "ExclusiveRoot::current while custody held: {}",
            registry_while.is_some()
        ),
    );
    assert_eq!(registry_while_status, "completed");
    // Measured handle-lifecycle fact: the ExclusiveRoot proof holds a strong
    // Arc<EngineLock>, so custody stays registered until BOTH are dropped.
    drop(registry_while);
    drop(custody);
    let registry_after = ExclusiveRoot::current(&root);
    let registry_after_status = if registry_after.is_none() {
        "completed"
    } else {
        "blocked"
    };
    record(
        "delete-guard/registry-after-release",
        registry_after_status,
        format!(
            "ExclusiveRoot::current after release: {}",
            registry_after.is_some()
        ),
    );
    assert_eq!(registry_after_status, "completed");

    // ---- scope locks: latch one admitted closure, contend same/different ----
    let scope = runtime.block_on(async {
        let manager = std::sync::Arc::new(s09_manager(&root));
        let (admitted_tx, admitted_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let (holder_tx, holder_rx) = tokio::sync::oneshot::channel::<Result<(), StoreError>>();
        let (contender_tx, mut contender_rx) = tokio::sync::oneshot::channel::<&'static str>();
        let (other_tx, other_rx) = tokio::sync::oneshot::channel::<&'static str>();

        let holder_manager = manager.clone();
        tokio::spawn(async move {
            let result = holder_manager
                .with_scope_write_lock(Scope::User, "scope-a", move || {
                    s09_latched_op(admitted_tx, release_rx)
                })
                .await;
            let _ = holder_tx.send(result);
        });
        let latch = tokio::time::timeout(std::time::Duration::from_secs(10), admitted_rx).await;
        assert!(
            latch.is_ok(),
            "holder must be admitted into the scope lock within the bounded wait"
        );
        let latch_us = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_micros();

        let contender_manager = manager.clone();
        tokio::spawn(async move {
            let _ = contender_manager
                .with_scope_write_lock(Scope::User, "scope-a", move || s09_signal_op(contender_tx))
                .await;
        });
        let other_manager = manager.clone();
        tokio::spawn(async move {
            let _ = other_manager
                .with_scope_write_lock(Scope::Project, "scope-b", move || s09_signal_op(other_tx))
                .await;
        });

        let other = tokio::time::timeout(std::time::Duration::from_secs(10), other_rx).await;
        let other_completed_while_held = other.is_ok();
        let poll_started = std::time::Instant::now();
        let poll = tokio::time::timeout(S09_SCOPE_POLL, &mut contender_rx).await;
        let poll_elapsed = poll_started.elapsed();
        assert!(
            poll.is_err(),
            "same-scope contender must not complete while the holder is latched"
        );
        // Structural: the contender is still queued with a live sender. `Closed`
        // would mean the spawned op died without signalling; `Ok` would mean it ran.
        assert!(
            matches!(
                contender_rx.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ),
            "same-scope contender must still be pending with a live sender while the holder is latched"
        );
        // The wait itself: a 1s monotone timer cannot fire early, so a LOWER bound is
        // load-safe (an upper bound would be flaky). This is what kills the degenerate poll.
        assert!(
            poll_elapsed >= S09_SCOPE_POLL / 2,
            "negative poll returned after {poll_elapsed:?}; a degenerate window proves nothing"
        );

        let aux_started = std::time::Instant::now();
        let aux_scope_read = s09_aux_write_async(&root, "scope-lock-held").await;
        assert_eq!(aux_scope_read, "scope-lock-held");
        let aux_scope_us = aux_started.elapsed().as_micros();

        let _ = release_tx.send(());
        let holder_done = tokio::time::timeout(std::time::Duration::from_secs(10), holder_rx).await;
        assert!(
            holder_done.is_ok() && holder_done.unwrap().is_ok(),
            "holder must complete after release"
        );
        let drained = tokio::time::timeout(std::time::Duration::from_secs(10), contender_rx).await;
        let drained_status = if drained.is_ok() {
            "completed"
        } else {
            "blocked"
        };

        // Positive control: the same key `user:scope-a` must admit a completion in this
        // harness, so `blocked` is a measured contrast and not an inert path.
        let control_started = std::time::Instant::now();
        let (control_tx, control_rx) = tokio::sync::oneshot::channel::<&'static str>();
        let control_manager = manager.clone();
        let control = tokio::time::timeout(
            S09_SCOPE_POLL,
            control_manager.with_scope_write_lock(Scope::User, "scope-a", move || {
                s09_signal_op(control_tx)
            }),
        )
        .await;
        let control_us = control_started.elapsed().as_micros();
        assert!(
            control.is_ok() && control.unwrap().is_ok(),
            "uncontended same-scope control must complete inside the poll window"
        );
        assert!(
            tokio::time::timeout(S09_SCOPE_POLL, control_rx).await.is_ok(),
            "uncontended same-scope control must deliver its completion signal"
        );

        serde_json::json!({
            "latch_epoch_us": latch_us,
            "other_scope_completed_while_held": other_completed_while_held,
            "scope_same_scope_negative_poll_us": poll_elapsed.as_micros() as u64,
            "scope_positive_control_us": control_us as u64,
            "aux_write_while_scope_held_us": aux_scope_us,
            "drained_status": drained_status,
        })
    });
    let other_scope_completed = scope["other_scope_completed_while_held"].as_bool().unwrap();
    let drained_completed = scope["drained_status"].as_str() == Some("completed");
    timings_us.extend(
        scope
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone())),
    );
    record(
        "scope-lock/holder-admitted-latch",
        "completed",
        "holder entered the per-(scope,hash) async mutex and latched".to_string(),
    );
    record(
        "scope-lock/same-scope-while-held",
        "blocked",
        "contender on scope user:scope-a did not complete within the bounded 1s negative poll while the holder was latched".to_string(),
    );
    record(
        "scope-lock/different-scope-while-held",
        if other_scope_completed {
            "completed"
        } else {
            "blocked"
        },
        "distinct scope project:scope-b completed while the holder was latched".to_string(),
    );
    assert!(
        other_scope_completed,
        "distinct scope must run unblocked (per-scope, not root-wide, barrier)"
    );
    record(
        "scope-lock/drain-after-release",
        if drained_completed {
            "completed"
        } else {
            "blocked"
        },
        "queued same-scope contender completed only after the holder released (in-flight drain)"
            .to_string(),
    );
    assert!(
        drained_completed,
        "admitted contender must drain after release"
    );

    let aux_after = s09_aux_write(&runtime, &root, "all-released");
    timings_us.insert(
        "aux_write_after_release_us".into(),
        (aux_after.1 as u64).into(),
    );
    record(
        "auxiliary/read-write-after-release",
        "completed",
        format!("read-back {}", aux_after.0),
    );

    // ---- truthful coverage ledger: unexercised traced paths, never "blocked" ----
    let not_exercised = serde_json::json!([
        {"path": "skills-manager/list-get-create", "reason": "source-map: no shared scope/operation barrier at those methods; manager crate not in this immutable dependency manifest"},
        {"path": "mem-episodic/inbox-store-direct-sql", "reason": "source-map: direct ready/execute traced, no shared barrier; crate not in dependency manifest"},
        {"path": "mem-bridge/capture-promotion-real-callers", "reason": "source-map: same-manager scope lock traced READ-only; crate not in dependency manifest"},
        {"path": "connection/close-drain-stale-cloned-handle", "reason": "source-map: manager close removes ownership but cannot revoke cloned Arc handles; no public close seam reachable from this manifest"},
        {"path": "power-loss-durability", "reason": "process-kill harness cannot establish power-loss semantics"},
    ]);

    let report = serde_json::json!({
        "case_root": root.display().to_string(),
        "paths": paths,
        "not_exercised": not_exercised,
        "unknown_path_behavior": "a path absent from the ledger classifies as unknown, never as blocked",
        "timings_us": timings_us,
        "limitations": [
            "observations cover only the public operation/scope/delete-guard seams reachable from this dependency manifest",
            "unexercised traced admission paths keep draft-B coverage inconclusive; Busy counts alone prove nothing",
            "the scope barrier serializes opted-in callers; an unbarred writer would bypass it by construction",
        ],
    });
    let report_path = root.join("admission-observations.json");
    std::fs::write(&report_path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    let persisted: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&report_path).unwrap()).unwrap();

    // Truthfulness gates over the PERSISTED artifact.
    let persisted_paths = &persisted["paths"];
    assert_eq!(
        s09_classify(persisted_paths, "operation-lock/second-acquire-while-held"),
        "refused",
        "hardcoded all-blocked/all-completed ledgers cannot pass"
    );
    assert_eq!(
        s09_classify(persisted_paths, "operation-lock/acquire-after-release"),
        "completed"
    );
    assert_eq!(
        s09_classify(persisted_paths, "scope-lock/same-scope-while-held"),
        "blocked"
    );
    assert_eq!(
        s09_classify(persisted_paths, "scope-lock/different-scope-while-held"),
        "completed"
    );
    assert_eq!(
        s09_classify(persisted_paths, "auxiliary/read-write-while-operation-held"),
        "completed"
    );
    assert_eq!(
        s09_classify(persisted_paths, "scope-lock/drain-after-release"),
        "completed"
    );
    // The persisted timing must itself carry the measured wait, so a degenerate poll
    // cannot leave a vacuous `blocked` row in the evidence file.
    let poll_us = persisted["timings_us"]["scope_same_scope_negative_poll_us"]
        .as_u64()
        .unwrap();
    assert!(
        poll_us >= 500_000,
        "the persisted negative poll must show the wait on the live contender, got {poll_us}us"
    );
    assert!(
        persisted["timings_us"]["scope_positive_control_us"]
            .as_u64()
            .is_some(),
        "the persisted positive control must record its measured duration"
    );
    assert_eq!(
        s09_classify(
            persisted_paths,
            "operation-lock/stale-dead-pid-acquire-after-reclaim"
        ),
        "completed",
        "stale-handle coverage must be an exercised completion, not an unexercised label"
    );
    for status in ["blocked", "completed", "refused"] {
        assert!(
            persisted_paths
                .as_object()
                .unwrap()
                .values()
                .any(|entry| entry["status"] == serde_json::json!(status)),
            "the ledger must contain a measured {status} outcome"
        );
    }
    assert!(
        persisted["not_exercised"].as_array().unwrap().len() >= 3,
        "unexercised traced paths must be honestly listed"
    );
    // Negative-oracle rule: missing paths stay unknown; unexercised entries are
    // never mislabeled as blocked.
    assert_eq!(
        s09_classify(persisted_paths, "nonexistent/fabricated-path"),
        "unknown"
    );
    for entry in persisted["not_exercised"].as_array().unwrap() {
        assert_eq!(
            s09_classify(persisted_paths, entry["path"].as_str().unwrap()),
            "unknown",
            "not_exercised entries must not appear in the measured map"
        );
    }
}
