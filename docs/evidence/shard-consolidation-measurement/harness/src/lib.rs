use std::path::Path;

#[derive(Debug, Clone, Copy)]
pub enum Case {
    Containment,
    RawCopy,
    ExactDuplicates,
    DivergentIds,
    CrossDb,
}

#[derive(Debug)]
pub struct Observation {
    pub case: Case,
    pub status: &'static str,
    pub schema: serde_json::Value,
    pub row_differences: Vec<String>,
    pub duplicate_occurrences: serde_json::Value,
    pub barrier_observations: serde_json::Value,
    pub duration_ms: u128,
    pub limitations: Vec<String>,
    pub artifact_paths: Vec<String>,
}

#[derive(Debug)]
pub enum MeasurementError {
    Containment(String),
    Engine(String),
}

pub const RUNS: &str =
    "/home/opencode/Code/Personal/Rust/opencode-agents-memory/.tmp/scratch/shard-measurement/runs";

pub fn run_case(case: Case, root: &Path) -> Result<Observation, MeasurementError> {
    use std::path::Component;
    let refuse = || MeasurementError::Containment(root.display().to_string());
    let runs = Path::new(RUNS);
    if !root.is_absolute()
        || root
            .components()
            .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
        || !root.starts_with(runs)
        || root == runs
        || root.exists()
        || root.symlink_metadata().is_ok()
    {
        return Err(refuse());
    }
    for ancestor in root.ancestors().skip(1) {
        if let Ok(metadata) = ancestor.symlink_metadata()
            && metadata.file_type().is_symlink()
        {
            return Err(refuse());
        }
    }
    let parent = root.parent().ok_or_else(refuse)?;
    let canonical_parent = parent.canonicalize().map_err(|_| refuse())?;
    let canonical_runs = runs.canonicalize().map_err(|_| refuse())?;
    if canonical_runs != runs || !canonical_parent.starts_with(&canonical_runs) {
        return Err(refuse());
    }
    std::fs::create_dir(root).map_err(|error| MeasurementError::Containment(error.to_string()))?;
    let (
        schema,
        limitations,
        artifact_paths,
        duration_ms,
        duplicate_occurrences,
        barrier_observations,
    ) = if matches!(case, Case::RawCopy) {
        let runtime =
            tokio::runtime::Runtime::new().map_err(|e| MeasurementError::Engine(e.to_string()))?;
        let result = runtime.block_on(raw_copy(root))?;
        let schema = result.get("schema_columns").cloned().unwrap_or(Json::Null);
        let limitations = limitations_from(&result);
        let duration_ms = u128::from(
            result
                .get("duration_ms")
                .and_then(Json::as_u64)
                .unwrap_or(0),
        );
        let artifact_paths = vec![root.join("raw-copy.json").display().to_string()];
        (
            schema,
            limitations,
            artifact_paths,
            duration_ms,
            Json::Null,
            Json::Null,
        )
    } else if matches!(case, Case::ExactDuplicates | Case::DivergentIds) {
        let runtime =
            tokio::runtime::Runtime::new().map_err(|e| MeasurementError::Engine(e.to_string()))?;
        let (dups, barrier, paths) =
            runtime.block_on(duplicate_case(root, matches!(case, Case::DivergentIds)))?;
        let source = if dups.is_null() { &barrier } else { &dups };
        let limitations = limitations_from(source);
        let duration_ms = u128::from(
            source
                .get("duration_ms")
                .and_then(Json::as_u64)
                .unwrap_or(0),
        );
        // duplicate_case already lists raw-copy.json first in `paths`.
        (Json::Null, limitations, paths, duration_ms, dups, barrier)
    } else if matches!(case, Case::CrossDb) {
        let runtime =
            tokio::runtime::Runtime::new().map_err(|e| MeasurementError::Engine(e.to_string()))?;
        let (barrier, paths) = runtime.block_on(cross_db_case(root))?;
        let limitations = limitations_from(&barrier);
        let duration_ms = u128::from(
            barrier
                .get("duration_ms")
                .and_then(Json::as_u64)
                .unwrap_or(0),
        );
        (
            Json::Null,
            limitations,
            paths,
            duration_ms,
            Json::Null,
            barrier,
        )
    } else if matches!(case, Case::Containment) {
        // Containment only proves the guard: no experiment body, zero-valued
        // observation fields by design.
        (
            Json::Null,
            Vec::new(),
            Vec::new(),
            0,
            Json::Null,
            Json::Null,
        )
    } else {
        return Err(MeasurementError::Engine("case not wired".into()));
    };
    Ok(Observation {
        case,
        status: "pass",
        schema,
        row_differences: Vec::new(),
        duplicate_occurrences,
        barrier_observations,
        duration_ms,
        limitations,
        artifact_paths,
    })
}

fn limitations_from(source: &Json) -> Vec<String> {
    source
        .get("limitations")
        .and_then(Json::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Json::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

pub use mem_core::scope::Scope;
pub use mem_store::config::StoreConfig;
pub use mem_store::connection::ConnectionManager;
pub use mem_store::db::turso_connect_factory;
pub use mem_store::db::{ConnectFactory, ConnectOptions, DbHandle, Value};
pub use mem_store::delete_guard::ExclusiveRoot;
pub use mem_store::engine_lock::{EngineLockOutcome, acquire_engine_lock};
pub use mem_store::error::StoreError;
pub use mem_store::operation_lock::{
    OPERATION_LOCK_FILE, acquire_turso_operation_lock, assert_no_turso_migration_in_progress,
    read_live_lock,
};
pub use mem_store::platform::{Platform, ProcessProbe, SystemProcessProbe};
use mem_store::schema_migrations::{
    MEMORY_SHARD_MAX_KNOWN, MigrationOptions, apply_schema_migrations, memory_shard_migrations,
};
pub use mem_store::shard_manager::ShardManager;
use serde_json::{Value as Json, json};

type MResult<T> = std::result::Result<T, MeasurementError>;
fn engine(e: impl std::fmt::Display) -> MeasurementError {
    MeasurementError::Engine(e.to_string())
}

pub async fn open(path: &Path) -> MResult<DbHandle> {
    let config = mem_store::config::StoreConfig {
        storage_path: path.parent().ok_or_else(|| engine("parent"))?.to_path_buf(),
        embedding_dimensions: 3,
        embedding_model: "synthetic".into(),
        max_vectors_per_shard: 99,
        container_tag_prefix: "synthetic".into(),
        database_encryption_enabled: false,
        database_encryption_cipher: "aes256gcm".into(),
        database_encryption_key: None,
        allow_unguarded_open: false,
    };
    mem_store::db::turso_connect_factory()
        .connect(
            path.to_path_buf(),
            ConnectOptions::for_config(&config, mem_store::platform::Platform::current()),
        )
        .await
        .map_err(engine)
}
async fn init(path: &Path) -> MResult<DbHandle> {
    let db = open(path).await?;
    apply_schema_migrations(
        db.as_ref(),
        &memory_shard_migrations(3),
        &MigrationOptions {
            db_path: None,
            label: "fixture",
            max_known: MEMORY_SHARD_MAX_KNOWN,
        },
        None,
    )
    .await
    .map_err(engine)?;
    Ok(db)
}
fn typed(v: &Value) -> Json {
    match v {
        Value::Null => json!({"null":true}),
        Value::Integer(x) => json!({"integer":x}),
        Value::Real(x) => json!({"real_bits":x.to_bits()}),
        Value::Text(x) => json!({"text":x}),
        Value::Blob(x) => json!({"blob":x,"len":x.len()}),
    }
}
async fn columns(db: &DbHandle) -> MResult<Vec<String>> {
    db.query("PRAGMA table_info(memories)", vec![])
        .await
        .map_err(engine)?
        .iter()
        .map(|r| r.text("name").ok_or_else(|| engine("column name")))
        .collect()
}
async fn rows(db: &DbHandle) -> MResult<Vec<Vec<Value>>> {
    db.query("SELECT * FROM memories ORDER BY id", vec![])
        .await
        .map_err(engine)?
        .iter()
        .map(|r| {
            r.column_names()
                .iter()
                .map(|c| r.get(c).cloned().ok_or_else(|| engine("missing value")))
                .collect()
        })
        .collect()
}
const ASSOCIATIONS: [&str; 5] = [
    "container_tag",
    "project_path",
    "project_name",
    "display_name",
    "git_repo_url",
];
fn normalize(columns: &[String], row: &mut [Value]) {
    for (c, v) in columns.iter().zip(row) {
        if ASSOCIATIONS.contains(&c.as_str()) {
            *v = Value::Text(format!("target-{c}"));
        }
    }
}
fn close_enough(a: f64, b: f64) -> bool {
    a.is_finite() && b.is_finite() && (a - b).abs() <= 1e-6
}
async fn distances(db: &DbHandle) -> MResult<Vec<Vec<f64>>> {
    let mut result = Vec::new();
    for id in ["v1", "v2"] {
        let mut ds = Vec::new();
        for q in ["[3,0,1]", "[0,2,1]"] {
            let r = db
                .query(
                    "SELECT vector_distance_cos(vector,vector32(?)) AS d FROM memories WHERE id=?",
                    vec![Value::Text(q.into()), Value::Text(id.into())],
                )
                .await
                .map_err(engine)?;
            ds.push(
                r.first()
                    .and_then(|r| r.f64("d"))
                    .ok_or_else(|| engine("missing cosine"))?,
            );
        }
        result.push(ds);
    }
    Ok(result)
}
fn verify_distances(d: &[Vec<f64>]) -> bool {
    let vs = [[2.0_f64, 1.0, 0.0], [1.0, 3.0, 0.0]];
    let qs = [[3.0_f64, 0.0, 1.0], [0.0, 2.0, 1.0]];
    if d.len() != 2 || d.iter().any(|r| r.len() != 2) {
        return false;
    }
    for (i, v) in vs.iter().enumerate() {
        for (j, q) in qs.iter().enumerate() {
            let norm = |a: &[f64; 3]| a.iter().map(|x| x * x).sum::<f64>().sqrt();
            let expected =
                1.0 - v.iter().zip(q).map(|(a, b)| a * b).sum::<f64>() / (norm(v) * norm(q));
            if !close_enough(d[i][j], expected) {
                return false;
            }
        }
    }
    d[0][0] < d[1][0] && d[1][1] < d[0][1] && d[0][0] != d[0][1] && d[1][0] != d[1][1]
}
async fn raw_copy(root: &Path) -> MResult<Json> {
    let start = std::time::Instant::now();
    let mut names = None;
    let fixture_specs = [
        ("source1", "v1", "[2,1,0]", Some("[1,3,0]")),
        ("source2", "v2", "[1,3,0]", Some("[2,1,0]")),
        ("target", "null-tags", "[2,1,0]", None),
    ];
    for (origin, id, vector, tags) in fixture_specs {
        let db = init(&root.join(format!("{origin}.db"))).await?;
        let cols = columns(&db).await?;
        let known = [
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
        if cols.iter().map(String::as_str).collect::<Vec<_>>() != known {
            return Err(engine("uncovered schema column"));
        }
        let mut values = Vec::new();
        let mut exprs = Vec::new();
        for c in &cols {
            let value = match c.as_str() {
                "id" => Value::Text(id.into()),
                "vector" => Value::Text(vector.into()),
                "tags_vector" => tags.map(|s| Value::Text(s.into())).unwrap_or(Value::Null),
                "content" => Value::Text(
                    if id == "v2" {
                        "<private>only /home/vaz/old</private>"
                    } else {
                        "mixed <private>secret</private> /home/vaz/old"
                    }
                    .into(),
                ),
                "metadata" => Value::Text(
                    "{\"historical\":\"/home/vaz/old\",\"sessionID\":\"old-session\"}".into(),
                ),
                "created_at" => Value::Integer(111),
                "updated_at" => Value::Integer(222),
                "source_prompt_created_at" => Value::Integer(333),
                "is_pinned" => Value::Integer(1),
                _ => Value::Text(format!("{origin}-{c}-sentinel")),
            };
            exprs.push(if c == "vector" || (c == "tags_vector" && tags.is_some()) {
                "vector32(?)"
            } else {
                "?"
            });
            values.push(value);
        }
        db.execute(
            &format!(
                "INSERT INTO memories ({}) VALUES ({})",
                cols.join(","),
                exprs.join(",")
            ),
            values,
        )
        .await
        .map_err(engine)?;
        names = Some(cols);
        let inventory = db
            .query(
                "SELECT type,name,tbl_name,sql FROM sqlite_master ORDER BY type,name",
                vec![],
            )
            .await
            .map_err(engine)?;
        let inv: Vec<_> = inventory
            .iter()
            .map(|r| {
                r.column_names()
                    .iter()
                    .map(|c| (c.clone(), typed(r.get(c).unwrap_or(&Value::Null))))
                    .collect::<serde_json::Map<_, _>>()
            })
            .collect();
        std::fs::write(
            root.join(format!("{origin}-schema.json")),
            serde_json::to_vec_pretty(&inv).map_err(engine)?,
        )
        .map_err(engine)?;
        drop(db);
    }
    let cols = names.ok_or_else(|| engine("schema absent"))?;
    // All baseline handles are closed before any file cloning.
    let work = root.join("work");
    std::fs::create_dir(&work).map_err(engine)?;
    for entry in std::fs::read_dir(root).map_err(engine)? {
        let entry = entry.map_err(engine)?;
        if entry.file_type().map_err(engine)?.is_file()
            && entry.file_name().to_string_lossy().contains(".db")
        {
            std::fs::copy(entry.path(), work.join(entry.file_name())).map_err(engine)?;
        }
    }
    let mut expected = Vec::new();
    let mut source_distances = vec![vec![], vec![]];
    for origin in ["source1", "source2", "target"] {
        let db = open(&work.join(format!("{origin}.db"))).await?;
        let mut data = rows(&db).await?;
        if origin != "target" {
            // Each source holds one discriminating vector; both queries must execute.
            let i = if origin == "source1" { 0 } else { 1 };
            for q in ["[3,0,1]", "[0,2,1]"] {
                let r = db
                    .query(
                        "SELECT vector_distance_cos(vector,vector32(?)) AS d FROM memories",
                        vec![Value::Text(q.into())],
                    )
                    .await
                    .map_err(engine)?;
                source_distances[i].push(r[0].f64("d").ok_or_else(|| engine("cosine"))?);
            }
        }
        for r in &mut data {
            normalize(&cols, r)
        }
        expected.extend(data);
        drop(db);
    }
    if !verify_distances(&source_distances) {
        return Err(engine("baseline cosine rejected"));
    }
    let target = open(&work.join("target.db")).await?;
    target
        .execute("DELETE FROM memories", vec![])
        .await
        .map_err(engine)?;
    for row in &expected {
        target
            .execute(
                &format!(
                    "INSERT INTO memories ({}) VALUES ({})",
                    cols.join(","),
                    vec!["?"; cols.len()].join(",")
                ),
                row.clone(),
            )
            .await
            .map_err(engine)?;
    }
    let mut actual = rows(&target).await?;
    expected.sort_by_key(|r| format!("{:?}", r[0]));
    actual.sort_by_key(|r| format!("{:?}", r[0]));
    if expected != actual {
        return Err(engine("typed row mismatch"));
    }
    let after = distances(&target).await?;
    if !verify_distances(&after) {
        return Err(engine("copied cosine rejected"));
    }
    let mut changed = actual.clone();
    let vector_index = cols
        .iter()
        .position(|c| c == "vector")
        .ok_or_else(|| engine("vector missing"))?;
    if let Value::Blob(bytes) = &mut changed[0][vector_index] {
        bytes[0] ^= 1;
    } else {
        return Err(engine("vector not raw blob"));
    }
    if changed == actual
        || verify_distances(&[vec![0.5, 0.5], vec![0.5, 0.5]])
        || verify_distances(&[vec![after[0][0]; 2], vec![after[1][0]; 2]])
        || close_enough(0.5, 0.5 + 2e-6)
    {
        return Err(engine("negative control accepted"));
    }
    drop(target);
    let typed_rows: Vec<_> = actual
        .iter()
        .map(|r| r.iter().map(typed).collect::<Vec<_>>())
        .collect();
    let result = json!({"schema_columns":cols,"oracle_columns":cols,"before":{"row_count":expected.len(),"typed_rows":typed_rows,"distances":source_distances},"after":{"row_count":actual.len(),"typed_rows":typed_rows,"distances":after},"negative_controls_rejected":4,"duration_ms":start.elapsed().as_millis(),"limitations":["synthetic rows only; no power-loss proof"]});
    std::fs::write(
        root.join("raw-copy.json"),
        serde_json::to_vec_pretty(&result).map_err(engine)?,
    )
    .map_err(engine)?;
    Ok(result)
}

#[derive(Debug, Clone, PartialEq)]
pub struct MergePlan {
    pub rows: Vec<Vec<Value>>,
    pub occurrences: std::collections::BTreeMap<String, Vec<String>>,
}
#[derive(Debug, PartialEq)]
pub struct Conflict {
    pub id: String,
    pub column: String,
}
pub fn inspect(
    columns: &[String],
    inputs: &[(String, Vec<Value>)],
) -> std::result::Result<MergePlan, Conflict> {
    let mut seen: std::collections::BTreeMap<String, Vec<Value>> = Default::default();
    let mut occurrences: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    let id_index = columns
        .iter()
        .position(|c| c == "id")
        .ok_or_else(|| Conflict {
            id: String::new(),
            column: "missing id".into(),
        })?;
    for (source, input) in inputs {
        if input.len() != columns.len() {
            return Err(Conflict {
                id: String::new(),
                column: "shape".into(),
            });
        }
        let id = match &input[id_index] {
            Value::Text(id) => id.clone(),
            _ => {
                return Err(Conflict {
                    id: String::new(),
                    column: "id type".into(),
                });
            }
        };
        let mut row = input.clone();
        normalize(columns, &mut row);
        // First complete preflight, then callers may copy the approved rows.
        if let Some(prior) = seen.get(&id) {
            for (i, (a, b)) in prior.iter().zip(&row).enumerate() {
                let equal = match (a, b) {
                    (Value::Real(a), Value::Real(b)) => a.to_bits() == b.to_bits(),
                    _ => a == b,
                };
                if !equal {
                    return Err(Conflict {
                        id: id.clone(),
                        column: columns[i].clone(),
                    });
                }
            }
        } else {
            seen.insert(id.clone(), row);
        }
        occurrences.entry(id).or_default().push(source.clone());
    }
    for sources in occurrences.values_mut() {
        sources.sort();
    }
    Ok(MergePlan {
        rows: seen.into_values().collect(),
        occurrences,
    })
}

pub fn remap_identity(
    table: &str,
    column: &str,
    value: &Value,
) -> std::result::Result<Value, MeasurementError> {
    let path_binding = matches!(
        (table, column),
        ("user_prompts", "project_path")
            | (
                "agent_sessions"
                    | "episodic_milestones"
                    | "episodic_raw_context"
                    | "episodic_segments"
                    | "capture_inbox",
                "project_id"
            )
    );
    let tag_binding = table == "skills" && column == "container_tag";
    if !path_binding && !tag_binding {
        return Err(engine("unsupported table/identity binding"));
    }
    match value {
        Value::Null => Ok(Value::Null),
        Value::Text(v) if path_binding => match v.as_str() {
            "/home/vaz/synthetic-a" | "/home/vaz/synthetic-b" => {
                Ok(Value::Text("/home/opencode/synthetic-missing-repo".into()))
            }
            "/home/vaz/Code/Personal/JS/opencode" => {
                Ok(Value::Text("/home/opencode/Code/opencode".into()))
            }
            v if v.starts_with('/') => Ok(value.clone()),
            _ => Err(engine("unsupported project encoding")),
        },
        Value::Text(v) if tag_binding => Ok(match v.as_str() {
            "synthetic_project_old_a" | "synthetic_project_old_b" => {
                Value::Text("synthetic_project_target".into())
            }
            _ => value.clone(),
        }),
        _ => Err(engine("unsupported identity type")),
    }
}

/// Auxiliary preflight: all rows and unique keys are checked before any writes.
pub fn inspect_auxiliary(
    table: &str,
    cols: &[String],
    rows: &[Vec<Value>],
) -> MResult<Vec<Vec<Value>>> {
    let binding = match table {
        "skills" => "container_tag",
        "user_prompts" => "project_path",
        "agent_sessions"
        | "episodic_milestones"
        | "episodic_raw_context"
        | "episodic_segments"
        | "capture_inbox" => "project_id",
        _ => return Err(engine("unknown auxiliary schema")),
    };
    let idx = cols
        .iter()
        .position(|c| c == binding)
        .ok_or_else(|| engine("identity absent"))?;
    let mut output: Vec<Vec<Value>> = Vec::new();
    for input in rows {
        if input.len() != cols.len() {
            return Err(engine("auxiliary shape"));
        }
        let mut row = input.clone();
        row[idx] = remap_identity(table, binding, &row[idx])?;
        let key: Vec<&str> = match table {
            "episodic_raw_context" => vec!["project_id", "session_id", "agent_id"],
            "episodic_segments" => vec!["project_id", "session_id", "agent_id", "segment_index"],
            "capture_inbox" => vec!["capture_id"],
            _ => vec!["id"],
        };
        let keys_equal = |a: &[Value], b: &[Value], ks: &[&str]| -> bool {
            ks.iter().all(|k| {
                cols.iter()
                    .position(|c| c == k)
                    .is_some_and(|i| a[i] == b[i])
            })
        };
        for prior in &output {
            if (keys_equal(prior, &row, &key)
                || (table == "agent_sessions"
                    && keys_equal(prior, &row, &["project_id", "session_id", "agent_id"])))
                && prior != &row
            {
                return Err(engine("divergent auxiliary unique key"));
            }
        }
        if !output.contains(&row) {
            output.push(row)
        }
    }
    Ok(output)
}

pub async fn auxiliary_fixture(root: &Path) -> MResult<Json> {
    // Require existing fresh harness-owned case directory. No external root fallback.
    if !root.starts_with(Path::new(RUNS)) || root.canonicalize().map_err(engine)? != root {
        return Err(engine("aux root containment"));
    }
    let episodic = open(&root.join("episodic.db")).await?;
    mem_store::schema_migrations::apply_episodic_schema(
        episodic.as_ref(),
        3,
        &MigrationOptions {
            db_path: None,
            label: "synthetic episodic",
            max_known: mem_store::schema_migrations::EPISODIC_MAX_KNOWN,
        },
        None,
    )
    .await
    .map_err(engine)?;
    let prompts = open(&root.join("prompts.db")).await?;
    mem_store::schema_migrations::apply_user_prompts_schema(
        prompts.as_ref(),
        &MigrationOptions {
            db_path: None,
            label: "synthetic prompts",
            max_known: mem_store::schema_migrations::USER_PROMPTS_MAX_KNOWN,
        },
        None,
    )
    .await
    .map_err(engine)?;
    let skills = open(&root.join("skills.db")).await?;
    // Verbatim DDL from crates/mem-skills/src/schema.rs:32-44. Coverage is SQL, not manager API.
    skills.execute("CREATE TABLE IF NOT EXISTS skills (id TEXT PRIMARY KEY, name TEXT NOT NULL, description TEXT NOT NULL DEFAULT '', content TEXT NOT NULL, autoinvoke INTEGER NOT NULL DEFAULT 1, enabled INTEGER NOT NULL DEFAULT 1, container_tag TEXT, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL)",vec![]).await.map_err(engine)?;
    let registry = open(&root.join("metadata.db")).await?;
    apply_schema_migrations(
        registry.as_ref(),
        &mem_store::schema_migrations::metadata_db_migrations(),
        &MigrationOptions {
            db_path: None,
            label: "synthetic registry",
            max_known: mem_store::schema_migrations::METADATA_MAX_KNOWN,
        },
        None,
    )
    .await
    .map_err(engine)?;
    registry.execute("INSERT INTO shards(scope,scope_hash,shard_index,db_path,vector_count,is_active,created_at) VALUES ('project','old-a',0,'projects/old-a.db',0,1,17)",vec![]).await.map_err(engine)?;
    let mut manifests = serde_json::Map::new();
    for (table, db, binding) in [
        ("agent_sessions", &episodic, "project_id"),
        ("episodic_milestones", &episodic, "project_id"),
        ("episodic_raw_context", &episodic, "project_id"),
        ("episodic_segments", &episodic, "project_id"),
        ("capture_inbox", &episodic, "project_id"),
        ("user_prompts", &prompts, "project_path"),
        ("skills", &skills, "container_tag"),
    ] {
        let info = db
            .query(&format!("PRAGMA table_info({table})"), vec![])
            .await
            .map_err(engine)?;
        let cols: Vec<_> = info
            .iter()
            .map(|r| r.text("name").ok_or_else(|| engine("aux col")))
            .collect::<MResult<_>>()?;
        let mut original = Vec::new();
        for (n, identity) in [
            "/home/vaz/synthetic-a",
            "/home/vaz/synthetic-b",
            "/home/vaz/Code/Personal/JS/opencode",
            "/unrelated/sibling",
        ]
        .iter()
        .enumerate()
        {
            let mut row = Vec::new();
            let mut exprs = Vec::new();
            for (i, c) in cols.iter().enumerate() {
                let typ = info[i].text("type").unwrap_or_default();
                let v = if c == binding {
                    if table == "skills" {
                        if n == 3 {
                            Value::Null
                        } else {
                            Value::Text(format!(
                                "synthetic_project_old_{}",
                                if n == 0 { "a" } else { "b" }
                            ))
                        }
                    } else {
                        Value::Text(identity.to_string())
                    }
                } else if c == "embedding" {
                    Value::Text("[2,1,0]".into())
                } else if c == "parent_session_id" || c == "deleted_at" || c == "not_before" {
                    Value::Null
                } else if typ.contains("INT") || typ.contains("BOOL") {
                    Value::Integer((n * 100 + i + 1) as i64)
                } else {
                    Value::Text(format!(
                        "{table}-{c}-{n} /home/vaz/historical <private>verbatim</private>"
                    ))
                };
                exprs.push(if c == "embedding" { "vector32(?)" } else { "?" });
                row.push(v);
            }
            db.execute(
                &format!(
                    "INSERT INTO {table} ({}) VALUES ({})",
                    cols.join(","),
                    exprs.join(",")
                ),
                row,
            )
            .await
            .map_err(engine)?;
        }
        let queried = db
            .query(&format!("SELECT * FROM {table}"), vec![])
            .await
            .map_err(engine)?;
        for r in queried {
            original.push(
                cols.iter()
                    .map(|c| r.get(c).cloned().ok_or_else(|| engine("aux missing")))
                    .collect::<MResult<Vec<_>>>()?,
            );
        }
        let mapped = inspect_auxiliary(table, &cols, &original)?;
        let binding_index = cols
            .iter()
            .position(|c| c == binding)
            .ok_or_else(|| engine("binding"))?;
        for (a, b) in original.iter().zip(&mapped) {
            for i in 0..cols.len() {
                if i != binding_index && a[i] != b[i] {
                    return Err(engine("payload changed"));
                }
            }
        }
        // Deliberately diverge one historical field after remapping the same key.
        let mut collision = mapped[0].clone();
        let payload = cols
            .iter()
            .position(|c| {
                matches!(
                    c.as_str(),
                    "summary" | "raw_context" | "raw_content" | "payload" | "content"
                )
            })
            .ok_or_else(|| engine("payload col"))?;
        collision[payload] = Value::Text("divergent historical payload".into());
        let mut input = mapped.clone();
        input.push(collision);
        if inspect_auxiliary(table, &cols, &input).is_ok() {
            return Err(engine("aux collision accepted"));
        }
        db.execute(&format!("DELETE FROM {table}"), vec![])
            .await
            .map_err(engine)?;
        for row in &mapped {
            db.execute(
                &format!(
                    "INSERT INTO {table} ({}) VALUES ({})",
                    cols.join(","),
                    vec!["?"; cols.len()].join(",")
                ),
                row.clone(),
            )
            .await
            .map_err(engine)?;
        }
        manifests.insert(table.into(),json!({"columns":cols,"mapped_rows":mapped.iter().map(|r|r.iter().map(typed).collect::<Vec<_>>()).collect::<Vec<_>>(),"payload_preserved":true,"collision_refused":true}));
    }
    let prior = registry
        .query("SELECT * FROM shards", vec![])
        .await
        .map_err(engine)?;
    registry.execute("UPDATE shards SET scope_hash='explicit-git-common-target',db_path='projects/explicit-target.db' WHERE scope='project' AND scope_hash='old-a'",vec![]).await.map_err(engine)?;
    let after = registry
        .query("SELECT * FROM shards", vec![])
        .await
        .map_err(engine)?;
    for c in prior[0].column_names() {
        if c != "scope_hash" && c != "db_path" && prior[0].get(c) != after[0].get(c) {
            return Err(engine("registry payload changed"));
        }
    }
    manifests.insert("shards".into(),json!({"payload_preserved":true,"scope_hash":"explicit-git-common-target","db_path":"projects/explicit-target.db"}));
    drop(registry);
    drop(skills);
    drop(prompts);
    drop(episodic);
    let result = json!({"tables":manifests,"limitations":["skills source DDL not manager API","ai-sessions.db unknown; production blocked"]});
    std::fs::write(
        root.join("auxiliary.json"),
        serde_json::to_vec_pretty(&result).map_err(engine)?,
    )
    .map_err(engine)?;
    Ok(result)
}

pub fn refuse_unsupported_auxiliary(root: &Path) -> MResult<()> {
    for name in ["facts.db", "vault", "vault-keys", "ai-sessions.db"] {
        if root.join(name).symlink_metadata().is_ok() {
            return Err(engine(format!("unsupported auxiliary presence: {name}")));
        }
    }
    Ok(())
}

/// Prepare closed synthetic data only; syseng must snapshot the entire root before starting child.
pub fn prepare_child_fixture(root: &Path) -> MResult<()> {
    run_case(Case::Containment, root)?;
    let runtime = tokio::runtime::Runtime::new().map_err(engine)?;
    runtime.block_on(async {
        let db = init(&root.join("shard.db")).await?;
        drop(db);
        for name in ["registry.db", "aux-one.db", "aux-two.db"] {
            let db = open(&root.join(name)).await?;
            db.execute(
                "CREATE TABLE fixture (id INTEGER PRIMARY KEY,payload TEXT NOT NULL)",
                vec![],
            )
            .await
            .map_err(engine)?;
            db.execute("INSERT INTO fixture VALUES (1,'baseline')", vec![])
                .await
                .map_err(engine)?;
            drop(db);
        }
        Ok::<_, MeasurementError>(())
    })?;
    std::fs::write(
        root.join("owned-child-fixture"),
        b"synthetic-shard-measurement-v1\n",
    )
    .map_err(engine)?;
    Ok(())
}

pub async fn owned_child(root: &Path, phase: &str) -> MResult<()> {
    use std::io::{BufRead, Write};
    if !matches!(phase, "shard-before-registry" | "aux-one-before-two") {
        return Err(engine("unknown phase"));
    }
    if !root.is_absolute()
        || !root.starts_with(Path::new(RUNS))
        || root == Path::new(RUNS)
        || root.canonicalize().map_err(engine)? != root
    {
        return Err(engine("child containment"));
    }
    for ancestor in root.ancestors() {
        if ancestor
            .symlink_metadata()
            .map_err(engine)?
            .file_type()
            .is_symlink()
        {
            return Err(engine("symlink root"));
        }
    }
    if std::fs::read(root.join("owned-child-fixture")).map_err(engine)?
        != b"synthetic-shard-measurement-v1\n"
    {
        return Err(engine("missing fixture marker"));
    }
    refuse_unsupported_auxiliary(root)?;
    let lock = match mem_store::engine_lock::acquire_engine_lock(
        root,
        std::sync::Arc::new(mem_store::platform::SystemProcessProbe),
    )
    .map_err(engine)?
    {
        mem_store::engine_lock::EngineLockOutcome::Acquired(lock) => lock,
        _ => return Err(engine("fixture already owned")),
    };
    let shard = open(&root.join("shard.db")).await?;
    shard
        .execute(
            "INSERT INTO shard_metadata(key,value) VALUES ('child-stage','committed')",
            vec![],
        )
        .await
        .map_err(engine)?;
    let mut handles = vec![shard];
    if phase == "aux-one-before-two" {
        let registry = open(&root.join("registry.db")).await?;
        registry
            .execute(
                "UPDATE fixture SET payload='registry committed' WHERE id=1",
                vec![],
            )
            .await
            .map_err(engine)?;
        handles.push(registry);
        let aux = open(&root.join("aux-one.db")).await?;
        aux.execute(
            "UPDATE fixture SET payload='aux one committed' WHERE id=1",
            vec![],
        )
        .await
        .map_err(engine)?;
        handles.push(aux);
    }
    println!("SHARD-MEASUREMENT-PHASE {phase}");
    std::io::stdout().flush().map_err(engine)?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let result = std::io::stdin().lock().read_line(&mut line).map(|_| line);
        let _ = tx.send(result);
    });
    // A bounded explicit handshake, never sleep-based timing. Syseng may kill this owned child while held here.
    match rx.recv_timeout(std::time::Duration::from_secs(30)) {
        Ok(Ok(line)) if line.trim() == "RELEASE" => {}
        _ => {
            return Err(engine(
                "child handshake timeout/EOF/invalid release; inconclusive",
            ));
        }
    }
    drop(handles);
    drop(lock);
    Ok(())
}

/// Typed-artifact decoder: the only inverse of `typed`.
fn untyped(v: &Json) -> MResult<Value> {
    if v.get("null").is_some() {
        Ok(Value::Null)
    } else if let Some(n) = v.get("integer") {
        Ok(Value::Integer(
            n.as_i64().ok_or_else(|| engine("typed int"))?,
        ))
    } else if let Some(n) = v.get("real_bits") {
        Ok(Value::Real(f64::from_bits(
            n.as_u64().ok_or_else(|| engine("typed bits"))?,
        )))
    } else if let Some(n) = v.get("text") {
        Ok(Value::Text(
            n.as_str().ok_or_else(|| engine("typed text"))?.to_string(),
        ))
    } else if let Some(b) = v.get("blob") {
        Ok(Value::Blob(
            b.as_array()
                .ok_or_else(|| engine("typed blob"))?
                .iter()
                .map(|n| {
                    n.as_u64()
                        .map(|x| x as u8)
                        .ok_or_else(|| engine("typed byte"))
                })
                .collect::<MResult<Vec<u8>>>()?,
        ))
    } else {
        Err(engine("typed cell"))
    }
}

/// Dedicated duplicate experiments routed through `run_case` (debug-1 item 3):
/// the same raw fixture rows feed `inspect`; outcomes persist as artifacts and
/// come back inside the `Observation`, never as self-reported flags alone.
async fn duplicate_case(root: &Path, diverge: bool) -> MResult<(Json, Json, Vec<String>)> {
    let start = std::time::Instant::now();
    let raw = raw_copy(root).await?;
    let cols: Vec<String> = raw["schema_columns"]
        .as_array()
        .ok_or_else(|| engine("schema columns"))?
        .iter()
        .map(|c| c.as_str().map(String::from).ok_or_else(|| engine("column")))
        .collect::<MResult<_>>()?;
    let rows: Vec<Vec<Value>> = raw["before"]["typed_rows"]
        .as_array()
        .ok_or_else(|| engine("typed rows"))?
        .iter()
        .map(|r| {
            r.as_array()
                .ok_or_else(|| engine("typed row"))?
                .iter()
                .map(untyped)
                .collect()
        })
        .collect::<MResult<_>>()?;
    let id_index = cols
        .iter()
        .position(|c| c == "id")
        .ok_or_else(|| engine("id column"))?;
    let text_id = |r: &[Value]| match &r[id_index] {
        Value::Text(t) => Ok(t.clone()),
        _ => Err(engine("id type")),
    };
    if text_id(&rows[0])? != "null-tags" || text_id(&rows[1])? != "v1" || text_id(&rows[2])? != "v2"
    {
        return Err(engine("fixture id order"));
    }
    let (null_tags, v1, v2) = (&rows[0], &rows[1], &rows[2]);
    let assoc = |row: &[Value], tag: &str| {
        let mut r = row.to_vec();
        for (i, c) in cols.iter().enumerate() {
            if ASSOCIATIONS.contains(&c.as_str()) {
                r[i] = Value::Text(format!("{tag}-{c}"));
            }
        }
        r
    };
    let mut old1 = assoc(v1, "old1");
    if diverge {
        let content_index = cols
            .iter()
            .position(|c| c == "content")
            .ok_or_else(|| engine("content column"))?;
        old1[content_index] = Value::Text("divergent historical payload".into());
    }
    let inputs = vec![
        ("target".to_string(), v1.clone()),
        ("source1".to_string(), old1),
        ("source2".to_string(), assoc(v1, "old2")),
        ("source1".to_string(), v2.clone()),
        ("target".to_string(), null_tags.clone()),
    ];
    let mut artifacts = vec![root.join("raw-copy.json").display().to_string()];
    if diverge {
        let conflict = inspect(&cols, &inputs)
            .err()
            .ok_or_else(|| engine("divergent ids accepted"))?;
        if conflict.id != "v1" || conflict.column != "content" {
            return Err(engine("unexpected conflict target"));
        }
        let result = json!({"refused":true,"conflict":{"id":conflict.id,"column":conflict.column},"occurrences":5,"duration_ms":start.elapsed().as_millis(),"limitations":["synthetic preflight only; no production write path"]});
        std::fs::write(
            root.join("divergent.json"),
            serde_json::to_vec_pretty(&result).map_err(engine)?,
        )
        .map_err(engine)?;
        artifacts.push(root.join("divergent.json").display().to_string());
        Ok((Json::Null, result, artifacts))
    } else {
        let plan = inspect(&cols, &inputs)
            .map_err(|c| engine(format!("unexpected plan conflict {}:{}", c.id, c.column)))?;
        if plan.rows.len() != 3 || plan.occurrences.values().map(Vec::len).sum::<usize>() != 5 {
            return Err(engine("duplicate plan shape"));
        }
        let plan_rows: Vec<_> = plan
            .rows
            .iter()
            .map(|r| r.iter().map(typed).collect::<Vec<_>>())
            .collect();
        let result = json!({"plan_rows":plan_rows,"occurrences":serde_json::to_value(&plan.occurrences).map_err(engine)?,"duration_ms":start.elapsed().as_millis(),"limitations":["synthetic preflight only; no production write path"]});
        std::fs::write(
            root.join("duplicates.json"),
            serde_json::to_vec_pretty(&result).map_err(engine)?,
        )
        .map_err(engine)?;
        artifacts.push(root.join("duplicates.json").display().to_string());
        Ok((result, Json::Null, artifacts))
    }
}

/// System `sha256sum` over every file under `dir` (sorted, recursive). The
/// refusal evidence is produced by the system tool, not a new crate.
fn closed_root_manifest(dir: &Path) -> MResult<Json> {
    fn walk(dir: &Path, out: &mut Vec<Json>) -> MResult<()> {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .map_err(engine)?
            .collect::<std::result::Result<_, _>>()
            .map_err(engine)?;
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out)?;
            } else {
                let hashed = std::process::Command::new("sha256sum")
                    .arg(&path)
                    .output()
                    .map_err(engine)?;
                if !hashed.status.success() {
                    return Err(engine("sha256sum failed"));
                }
                let text = String::from_utf8(hashed.stdout).map_err(engine)?;
                let hash = text
                    .split_whitespace()
                    .next()
                    .ok_or_else(|| engine("sha256sum output"))?;
                out.push(json!({
                    "path": path.display().to_string(),
                    "sha256": hash,
                    "len": path.metadata().map_err(engine)?.len(),
                }));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, &mut out)?;
    Ok(Json::Array(out))
}

/// Reopen every persisted auxiliary DB and compare each table's rows, typed
/// cell by typed cell, against the mapped manifest; then assert the binding
/// predicates on the remapped identity columns. This is the persisted-state
/// oracle: it reads what is on disk, never what the writer reported.
async fn verify_persisted(root: &Path, aux: &Json) -> MResult<()> {
    let db_for = |table: &str| match table {
        "user_prompts" => "prompts.db",
        "skills" => "skills.db",
        "shards" => "metadata.db",
        _ => "episodic.db",
    };
    let tables = aux
        .get("tables")
        .and_then(Json::as_object)
        .ok_or_else(|| engine("aux tables"))?;
    for (table, manifest) in tables {
        let db = open(&root.join(db_for(table))).await?;
        let info = db
            .query(&format!("PRAGMA table_info({table})"), vec![])
            .await
            .map_err(engine)?;
        let cols: Vec<String> = info
            .iter()
            .map(|r| r.text("name").ok_or_else(|| engine("aux col")))
            .collect::<MResult<_>>()?;
        let queried = db
            .query(&format!("SELECT * FROM {table}"), vec![])
            .await
            .map_err(engine)?;
        let actual: Vec<Vec<Value>> = queried
            .iter()
            .map(|r| {
                cols.iter()
                    .map(|c| r.get(c).cloned().ok_or_else(|| engine("persisted cell")))
                    .collect()
            })
            .collect::<MResult<_>>()?;
        // `shards` is a scope manifest, not a row remap: the predicates below
        // are its oracle. Every other table compares typed rows byte-for-byte.
        let expected: Vec<Vec<Value>> = if table == "shards" {
            Vec::new()
        } else {
            manifest["mapped_rows"]
                .as_array()
                .ok_or_else(|| engine("mapped rows"))?
                .iter()
                .map(|r| {
                    r.as_array()
                        .ok_or_else(|| engine("mapped row"))?
                        .iter()
                        .map(untyped)
                        .collect()
                })
                .collect::<MResult<_>>()?
        };
        let serialize = |rows: &[Vec<Value>]| -> MResult<Vec<String>> {
            let mut keys: Vec<String> = rows
                .iter()
                .map(|r| {
                    serde_json::to_string(&r.iter().map(typed).collect::<Vec<_>>()).map_err(engine)
                })
                .collect::<MResult<_>>()?;
            keys.sort();
            Ok(keys)
        };
        if table != "shards" && serialize(&actual)? != serialize(&expected)? {
            return Err(engine(format!("persisted mismatch {table}")));
        }
        let binding = match table.as_str() {
            "skills" => "container_tag",
            "user_prompts" => "project_path",
            "shards" => "scope_hash",
            _ => "project_id",
        };
        let index = cols
            .iter()
            .position(|c| c == binding)
            .ok_or_else(|| engine("binding absent"))?;
        for row in &actual {
            let acceptable = match (table.as_str(), &row[index]) {
                ("skills", Value::Null) => true,
                ("skills", Value::Text(tag)) => tag == "synthetic_project_target",
                ("shards", Value::Text(h)) => h == "explicit-git-common-target",
                (_, Value::Text(path)) => !path.starts_with("/home/vaz/"),
                _ => false,
            };
            if !acceptable {
                return Err(engine(format!("persisted identity residual {table}")));
            }
        }
        if table == "shards" {
            let db_path_index = cols
                .iter()
                .position(|c| c == "db_path")
                .ok_or_else(|| engine("db_path absent"))?;
            if actual
                .iter()
                .any(|r| r[db_path_index] != Value::Text("projects/explicit-target.db".into()))
            {
                return Err(engine("persisted shard db_path unmapped"));
            }
        }
    }
    Ok(())
}

/// `Case::CrossDb` (debug-1 item 4): run the auxiliary remap, reopen and
/// verify the persisted state, exercise the refusal guards under a manifest
/// produced by the system `sha256sum`, and persist everything.
async fn cross_db_case(root: &Path) -> MResult<(Json, Vec<String>)> {
    let start = std::time::Instant::now();
    let aux = auxiliary_fixture(root).await?;
    verify_persisted(root, &aux).await?;
    let mut cases = serde_json::Map::new();
    let guards = root.join("refusal-guards");
    for name in ["facts.db", "vault", "vault-keys", "ai-sessions.db"] {
        let guard = guards.join(name);
        std::fs::create_dir_all(&guard).map_err(engine)?;
        std::fs::write(guard.join(name), b"synthetic unsupported sentinel").map_err(engine)?;
        let refused = refuse_unsupported_auxiliary(&guard).is_err();
        if !refused {
            return Err(engine(format!("refusal guard accepted {name}")));
        }
        let manifest = closed_root_manifest(&guard)?;
        cases.insert(name.into(), json!({"refused":true,"manifest":manifest}));
    }
    let unknown = remap_identity(
        "agent_sessions",
        "project_id",
        &Value::Text("unsupported:encoded-hash".into()),
    )
    .is_err();
    if !unknown {
        return Err(engine("unknown encoding accepted"));
    }
    cases.insert("unknown_encoding".into(), json!({"refused":true}));
    let cols: Vec<String> = aux["tables"]["agent_sessions"]["columns"]
        .as_array()
        .ok_or_else(|| engine("aux columns"))?
        .iter()
        .map(|c| c.as_str().map(String::from).ok_or_else(|| engine("column")))
        .collect::<MResult<_>>()?;
    let mut divergent: Vec<Vec<Value>> = aux["tables"]["agent_sessions"]["mapped_rows"]
        .as_array()
        .ok_or_else(|| engine("mapped rows"))?
        .iter()
        .map(|r| {
            r.as_array()
                .ok_or_else(|| engine("mapped row"))?
                .iter()
                .map(untyped)
                .collect()
        })
        .collect::<MResult<_>>()?;
    let payload_index = cols
        .iter()
        .position(|c| c == "summary")
        .ok_or_else(|| engine("summary absent"))?;
    // Same unique key, diverged payload: exactly the collision class that
    // inspect_auxiliary must refuse.
    let mut collision = divergent[0].clone();
    collision[payload_index] = Value::Text("divergent historical payload".into());
    divergent.push(collision);
    let diverged = inspect_auxiliary("agent_sessions", &cols, &divergent).is_err();
    if !diverged {
        return Err(engine("divergent auxiliary key accepted"));
    }
    cases.insert("divergent_aux_key".into(), json!({"refused":true}));
    let root_manifest = closed_root_manifest(root)?;
    let result = json!({
        "persisted_verified": true,
        "tables": aux["tables"].as_object().map(|t| t.len()).unwrap_or(0),
        "cases": cases,
        "root_manifest": root_manifest,
        "duration_ms": start.elapsed().as_millis(),
        "limitations": ["synthetic refusals only; closed-root manifest is system-sha256sum evidence"],
    });
    std::fs::write(
        root.join("auxiliary-refusal-manifest.json"),
        serde_json::to_vec_pretty(&result).map_err(engine)?,
    )
    .map_err(engine)?;
    Ok((
        result,
        vec![
            root.join("auxiliary.json").display().to_string(),
            root.join("auxiliary-refusal-manifest.json")
                .display()
                .to_string(),
        ],
    ))
}
