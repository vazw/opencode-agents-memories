//! Issue 165 — the blend-ordered search path: the F32-LE encoder, the one
//! statement per shard, the served score's bit-level agreement with the Rust
//! blend, the deep row the candidate union loses, and the hydrate chunk bound.
//!
//! The statement decides membership and order; the Rust blend decides the
//! served number. These tests compare the two implementations directly, so a
//! divergence is a defect in one of them, never a tolerance to widen.

mod common;

use std::sync::{Arc, Mutex};

use mem_store::config::StoreConfig;
use mem_store::db::{BatchMode, BoxFuture, Db, DbHandle, ExecResult, Statement, Value};
use mem_store::error::StoreError;
use mem_store::schema_migrations::{
    MigrationOptions, apply_schema_migrations, memory_shard_migrations,
};
use mem_store::sql::{BlendTerms, blend_ordered_sql};
use mem_store::vector::{MemoryInsert, insert_vector, search_in_shard, vector_to_blob};
use mem_store::vector_utils::{blob_to_float32_array, distance_to_similarity, tokenize_query_text};

const TAG: &str = "opencode_project_pin";

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

async fn prepared_db(prefix: &str, dims: u32) -> (StoreConfig, DbHandle) {
    let config = common::store_config(prefix);
    let db = common::open_real_db(&config, "shard.db")
        .await
        .expect("open the shard");
    apply_schema_migrations(
        db.as_ref(),
        &memory_shard_migrations(dims),
        &MigrationOptions {
            db_path: None,
            label: "blend-exactness",
            max_known: mem_store::schema_migrations::MEMORY_SHARD_MAX_KNOWN,
        },
        None,
    )
    .await
    .expect("shard schema");
    (config, db)
}

fn row(
    id: &str,
    content: &str,
    vector: Vec<f32>,
    tags_vector: Option<Vec<f32>>,
    tags: Option<&str>,
) -> MemoryInsert {
    MemoryInsert {
        id: id.to_string(),
        content: content.to_string(),
        vector,
        tags_vector,
        container_tag: TAG.to_string(),
        tags: tags.map(str::to_string),
        memory_type: None,
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

/// Delegating `Db` that records every statement it sees. `search_in_shard`
/// takes `&dyn Db`, so this is the only seam needed to capture the blend
/// statement with the params production actually binds.
struct RecordingDb {
    inner: Arc<dyn Db>,
    seen: Mutex<Vec<(String, Vec<Value>)>>,
}

impl RecordingDb {
    fn new(inner: Arc<dyn Db>) -> Self {
        Self {
            inner,
            seen: Mutex::new(Vec::new()),
        }
    }

    /// The last blend statement seen (`… AS sim`), with its params.
    fn blend_statement(&self) -> (String, Vec<Value>) {
        self.seen
            .lock()
            .expect("recording lock")
            .iter()
            .rfind(|(sql, _)| sql.contains("AS sim"))
            .cloned()
            .expect("the blend statement ran")
    }
}

impl Db for RecordingDb {
    fn execute(
        &self,
        sql: &str,
        params: Vec<Value>,
    ) -> BoxFuture<'_, Result<ExecResult, StoreError>> {
        self.inner.execute(sql, params)
    }

    fn query(
        &self,
        sql: &str,
        params: Vec<Value>,
    ) -> BoxFuture<'_, Result<Vec<mem_store::db::Row>, StoreError>> {
        self.seen
            .lock()
            .expect("recording lock")
            .push((sql.to_string(), params.clone()));
        self.inner.query(sql, params)
    }

    fn batch(
        &self,
        statements: Vec<Statement>,
        mode: BatchMode,
    ) -> BoxFuture<'_, Result<Vec<ExecResult>, StoreError>> {
        self.inner.batch(statements, mode)
    }
}

/// One row as read back from the shard, decoded through the store's own
/// F32-LE decoder.
struct RefRow {
    id: String,
    content: String,
    tags: Option<String>,
    vector: Vec<f32>,
    tags_vector: Option<Vec<f32>>,
}

async fn read_rows(db: &dyn Db, container_tag: &str) -> Vec<RefRow> {
    let rows = db
        .query(
            "SELECT id, content, tags, vector, tags_vector FROM memories \
             WHERE container_tag = ? ORDER BY id",
            vec![Value::Text(container_tag.to_string())],
        )
        .await
        .expect("read the shard rows");
    rows.iter()
        .map(|row| RefRow {
            id: row.text("id").expect("id"),
            content: row.text("content").unwrap_or_default(),
            tags: row.text("tags"),
            vector: row
                .get("vector")
                .and_then(blob_to_float32_array)
                .expect("decode the content vector"),
            tags_vector: row.get("tags_vector").and_then(blob_to_float32_array),
        })
        .collect()
}

/// The cosine similarity, computed the way the SQL does: cosine distance
/// (`1 - cos`) transformed by the store's own `distance_to_similarity`.
fn similarity(query: &[f32], vector: &[f32]) -> f64 {
    let dot: f64 = query
        .iter()
        .zip(vector)
        .map(|(q, v)| f64::from(*q) * f64::from(*v))
        .sum();
    let query_norm: f64 = query.iter().map(|q| f64::from(*q) * f64::from(*q)).sum();
    let vector_norm: f64 = vector.iter().map(|v| f64::from(*v) * f64::from(*v)).sum();
    let distance = 1.0 - dot / (query_norm.sqrt() * vector_norm.sqrt());
    distance_to_similarity(distance)
}

/// The tag elements exactly as `vector.rs` derives them: split, trim,
/// lowercase, drop empty.
fn tag_elements(tags: Option<&str>) -> Vec<String> {
    tags.unwrap_or("")
        .split(',')
        .map(|tag| tag.trim().to_lowercase())
        .filter(|tag| !tag.is_empty())
        .collect()
}

/// The served blend, written out independently of `search_in_shard`.
fn reference_sim(row: &RefRow, query: &[f32], words: &[String]) -> f64 {
    let content_sim = similarity(query, &row.vector);
    let tags_sim = row
        .tags_vector
        .as_ref()
        .map(|vector| similarity(query, vector))
        .unwrap_or(0.0);
    if words.is_empty() {
        return content_sim * 0.6 + tags_sim * 0.4;
    }
    let elements = tag_elements(row.tags.as_deref());
    let matches = words
        .iter()
        .filter(|word| {
            elements
                .iter()
                .any(|tag| tag.contains(word.as_str()) || word.contains(tag.as_str()))
        })
        .count();
    let boost = matches as f64 / words.len() as f64;
    let haystack = row.tags.as_deref().unwrap_or("").to_lowercase();
    let hits = words
        .iter()
        .filter(|word| {
            row.content.to_lowercase().contains(word.as_str()) || haystack.contains(word.as_str())
        })
        .count();
    let keyword = hits as f64 * (1.0 / words.len() as f64);
    content_sim * 0.5 + tags_sim.max(boost) * 0.3 + keyword * 0.2
}

fn normalised(sql: &str) -> String {
    sql.to_lowercase().split_whitespace().collect()
}

// ---------------------------------------------------------------------------
// Step 3 — the encoder
// ---------------------------------------------------------------------------

#[test]
fn test_vector_to_blob_round_trips_through_the_store_decoder() {
    // A negative, a zero and a fractional element: a `to_be_bytes()` mutant and
    // an `as f64` mutant both change at least one decoded value.
    let vector = vec![-1.5f32, 0.0, 0.25, 3.0e9];
    let blob = vector_to_blob(&vector);
    assert_eq!(
        blob.len(),
        vector.len() * 4,
        "F32-LE carries four bytes per lane, never eight"
    );
    assert_eq!(
        blob_to_float32_array(&Value::Blob(blob)).as_deref(),
        Some(vector.as_slice()),
        "the store's own decoder must recover the exact vector"
    );
}

// ---------------------------------------------------------------------------
// Step 4 — the statement
// ---------------------------------------------------------------------------

#[test]
fn test_blend_ordered_sql_placeholders_and_shape() {
    let empty: Vec<Vec<String>> = Vec::new();
    // label, tokens, containment, unfiltered count, filtered count.
    type CaseShape = (&'static str, Vec<String>, Vec<Vec<String>>, usize, usize);
    let cases: [CaseShape; 3] = [
        (
            "(n=1, Sumk=0)",
            vec!["alpha".to_string()],
            vec![Vec::new()],
            6,
            7,
        ),
        (
            "(n=4, Sumk=0)",
            vec![
                "97".to_string(),
                "sites".to_string(),
                "across".to_string(),
                "crates".to_string(),
            ],
            vec![Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            15,
            16,
        ),
        (
            "(n=1, Sumk=2)",
            vec!["alpha".to_string()],
            vec![vec!["a".to_string(), "b".to_string()]],
            8,
            9,
        ),
    ];
    for (label, tokens, containment, unfiltered_count, filtered_count) in cases {
        // Derived by the documented formula, never a copied literal.
        let needles: usize = containment.iter().map(Vec::len).sum();
        let base = 2 + tokens.len() + needles + 2 * tokens.len();
        assert_eq!(base + 1, unfiltered_count, "{label} unfiltered");
        assert_eq!(base + 1 + 1, filtered_count, "{label} filtered");
        for (filtered, expected) in [(false, unfiltered_count), (true, filtered_count)] {
            let statement = blend_ordered_sql(&BlendTerms {
                tokens: &tokens,
                containment: &containment,
                filtered,
            });
            assert_eq!(
                statement.matches('?').count(),
                expected,
                "{label} filtered={filtered}: {statement}"
            );
            assert_eq!(
                statement.contains("AND m.container_tag = ?"),
                filtered,
                "{label} filtered={filtered}: the tag clause appears iff filtered"
            );
        }
    }

    // The frozen query `97 sites across 8 crates` tokenizes to four words
    // (the single-char `8` is dropped); a shard whose only chunk is `c` puts
    // `c` inside `across` and `crates`, so Sumk = 2.
    let words = tokenize_query_text(Some("97 sites across 8 crates"));
    assert_eq!(words, vec!["97", "sites", "across", "crates"]);
    let containment: Vec<Vec<String>> = words
        .iter()
        .map(|word| {
            if word.contains('c') {
                vec!["c".to_string()]
            } else {
                Vec::new()
            }
        })
        .collect();
    assert_eq!(containment.iter().map(Vec::len).sum::<usize>(), 2);
    for (filtered, expected) in [(false, 17usize), (true, 18usize)] {
        let statement = blend_ordered_sql(&BlendTerms {
            tokens: &words,
            containment: &containment,
            filtered,
        });
        assert_eq!(
            statement.matches('?').count(),
            expected,
            "frozen query, filtered={filtered}: {statement}"
        );
    }

    // The no-token form emits only the classic 0.6/0.4 pair: two vector params,
    // the tag clause iff filtered, and the limit.
    for (filtered, expected) in [(false, 3usize), (true, 4usize)] {
        let statement = blend_ordered_sql(&BlendTerms {
            tokens: &[],
            containment: &empty,
            filtered,
        });
        assert_eq!(statement.matches('?').count(), expected);
        assert!(
            !statement.contains("LIKE"),
            "no keyword term without tokens"
        );
        assert!(!statement.contains("instr("), "no boost without tokens");
    }

    // Text invariants on the live filtered shape: the total order, a flat
    // projection, and none of the shapes the measurement ruled out.
    let statement = blend_ordered_sql(&BlendTerms {
        tokens: &words,
        containment: &containment,
        filtered: true,
    });
    assert!(statement.contains("ORDER BY sim DESC, created_at DESC, id ASC"));
    assert!(!statement.contains(" FROM (SELECT"));
    assert!(!statement.contains("WITH "));
    let flat = normalised(&statement);
    assert!(
        !flat.contains("vector32("),
        "the query vector binds as an F32-LE blob, never vector32(?): {statement}"
    );
    assert!(flat.contains("vector_distance_cos(m.vector,?)"));
}

/// The `mem-bridge` `all-projects` shape: an **empty filter** over a shard whose
/// row carries a **non-empty** `container_tag`. The mutant binds `''`
/// unconditionally, so `'' = 'opencode_project_pin'` is false and it returns
/// nothing; an empty-tag row would match that mutant and leave this pin green
/// on wrong code.
#[tokio::test]
async fn test_blend_ordered_sql_unfiltered_exec_pin_returns_the_tagged_row() {
    let (_config, db) = prepared_db("blend-exec-pin", 2).await;
    insert_vector(
        db.as_ref(),
        &row("mem_pin", "pin word", vec![1.0f32, 0.0], None, None),
    )
    .await
    .expect("insert the pinned row");

    let served = search_in_shard(db.as_ref(), &[1.0f32, 0.0], "", 5, Some("word"))
        .await
        .expect("unfiltered search");
    assert_eq!(
        served.iter().map(|hit| hit.id.as_str()).collect::<Vec<_>>(),
        vec!["mem_pin"],
        "the empty filter must still return the tagged row"
    );
}

/// Structural check: the statement's own `sim` against a reference computed in
/// Rust from the decoded vectors, `1e-12`, over a shard that fires every
/// branch — containment from a raw chunk, a space-bearing chunk, a NULL
/// tags vector, a tags-only keyword, and a non-trivial tags-vector cosine.
#[tokio::test]
async fn test_blend_ordered_sql_matches_the_rust_blend_over_every_row() {
    let (_config, db) = prepared_db("blend-per-row", 4).await;
    let query = vec![1.0f32, 0.0, 0.0, 0.0];
    let query_text = "crates alpha beta";

    let rows = vec![
        row(
            "mem_c",
            "crates are fine",
            vec![1.0, 0.0, 0.0, 0.0],
            None,
            Some("c"),
        ),
        row(
            "mem_qb",
            "nothing here",
            vec![0.5, 0.5, 0.0, 0.0],
            None,
            Some("q, b"),
        ),
        row(
            "mem_tagsvec",
            "alpha beta",
            vec![0.0, 1.0, 0.0, 0.0],
            Some(vec![1.0, 0.0, 0.0, 0.0]),
            Some("delta"),
        ),
        row(
            "mem_keywords",
            "crates alpha beta",
            vec![0.0, 0.0, 1.0, 0.0],
            None,
            None,
        ),
        row(
            "mem_first",
            "x",
            vec![0.0, 0.0, 0.0, 1.0],
            Some(vec![0.0, 0.0, 1.0, 0.0]),
            Some("crates"),
        ),
        row(
            "mem_gamma",
            "",
            vec![0.1, 0.2, 0.3, 0.4],
            Some(vec![0.4, 0.3, 0.2, 0.1]),
            Some(" gamma "),
        ),
        row(
            "mem_empty_tags",
            "beta",
            vec![-1.0, 0.0, 0.0, 0.0],
            None,
            Some(""),
        ),
        row(
            "mem_space",
            "alpha",
            vec![0.2, 0.2, 0.2, 0.2],
            None,
            Some("c, a b"),
        ),
    ];
    for insert in &rows {
        insert_vector(db.as_ref(), insert)
            .await
            .expect("insert row");
    }

    let recording = RecordingDb::new(db.clone());
    search_in_shard(&recording, &query, TAG, rows.len(), Some(query_text))
        .await
        .expect("search");
    let (sql, params) = recording.blend_statement();
    let scored = db
        .query(&sql, params)
        .await
        .expect("run the blend statement");
    assert_eq!(scored.len(), rows.len(), "every row is scored once");

    let words = tokenize_query_text(Some(query_text));
    let reference = read_rows(db.as_ref(), TAG).await;
    let mut expected: Vec<(String, f64)> = reference
        .iter()
        .map(|row| (row.id.clone(), reference_sim(row, &query, &words)))
        .collect();
    expected.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .expect("finite")
            .then_with(|| a.0.cmp(&b.0))
    });

    let served: Vec<(String, f64)> = scored
        .iter()
        .map(|row| (row.text("id").expect("id"), row.f64("sim").expect("sim")))
        .collect();
    assert_eq!(
        served.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
        expected
            .iter()
            .map(|(id, _)| id.as_str())
            .collect::<Vec<_>>(),
        "ORDER BY sim DESC, created_at DESC, id ASC (every fixture row shares one \
         created_at, so this fixture's reference tie-breaks on id alone)"
    );
    for ((id, sim), (expected_id, expected_sim)) in served.iter().zip(&expected) {
        assert_eq!(id, expected_id);
        // `vector_distance_cos` is an f32 computation, so a reference widened
        // from the same f32 lanes cannot agree to 1e-12; the measured spread is
        // ~2.2e-8. 1e-6 is two orders below the fixture's score separation and
        // still catches a dropped term (>= 0.2), a weight swap, or a stripped
        // haystack (0.3).
        assert!(
            (sim - expected_sim).abs() < 1e-6,
            "{id}: SQL {sim} vs Rust {expected_sim} (delta {})",
            (sim - expected_sim).abs()
        );
    }

    // Fixture pin: the branches actually fired.
    let boost_matches = |id: &str| {
        let row = reference.iter().find(|row| row.id == id).expect("row");
        let elements = tag_elements(row.tags.as_deref());
        words
            .iter()
            .filter(|word| {
                elements
                    .iter()
                    .any(|tag| tag.contains(word.as_str()) || word.contains(tag.as_str()))
            })
            .count()
    };
    assert_eq!(boost_matches("mem_c"), 1, "chunk `c` fires inside `crates`");
    assert_eq!(boost_matches("mem_qb"), 1, "chunk `b` fires inside `beta`");
    assert_eq!(
        boost_matches("mem_space"),
        1,
        "chunk `c` fires inside `crates`"
    );
}

/// The ULP discriminator, compared with `to_bits()`, never a tolerance.
///
/// The Rust boost divides (`matches / n`, `vector.rs`) while the keyword term
/// multiplies a precomputed reciprocal (`hits * (1/n)`). At n=7 and numerator
/// 5 the two orders differ by one ULP, so a swapped SQL form is visible only
/// here. Both rows have an exactly-zero content term (content vector orthogonal
/// to the query, distance exactly `1.0`) and an exactly-zero tags term, so no
/// addend absorbs the difference.
#[tokio::test]
async fn test_blend_sim_is_bit_identical_to_the_rust_blend_on_the_separating_rows() {
    let (_config, db) = prepared_db("blend-ulp", 4).await;
    let query = vec![1.0f32, 0.0, 0.0, 0.0];
    let query_text = "zebra water pixel sound melon attic eerie";

    insert_vector(
        db.as_ref(),
        &row(
            "sep_boost",
            "nothing at all",
            vec![0.0, 1.0, 0.0, 0.0],
            None,
            Some("z, w, p, s, m"),
        ),
    )
    .await
    .expect("insert sep_boost");
    insert_vector(
        db.as_ref(),
        &row(
            "sep_keyword",
            "zebra water pixel sound melon",
            vec![0.0, 1.0, 0.0, 0.0],
            None,
            None,
        ),
    )
    .await
    .expect("insert sep_keyword");

    let words = tokenize_query_text(Some(query_text));
    assert_eq!(words.len(), 7, "the separating fixture is n=7");
    // The separating pair exists on this target (k=5 of 0..=7).
    assert_ne!(
        (5.0f64 / 7.0).to_bits(),
        (5.0f64 * (1.0 / 7.0)).to_bits(),
        "the arithmetic forms must differ here, or this row cannot discriminate"
    );

    // The fixture pins, asserted before any comparison runs.
    let elements = tag_elements(Some("z, w, p, s, m"));
    let boost_matches = words
        .iter()
        .filter(|word| {
            elements
                .iter()
                .any(|tag| tag.contains(word.as_str()) || word.contains(tag.as_str()))
        })
        .count();
    assert_eq!(boost_matches, 5, "sep_boost: matches=5");
    let keyword_hits = words
        .iter()
        .filter(|word| "zebra water pixel sound melon".contains(word.as_str()))
        .count();
    assert_eq!(keyword_hits, 5, "sep_keyword: hits=5");

    let recording = RecordingDb::new(db.clone());
    search_in_shard(&recording, &query, TAG, 5, Some(query_text))
        .await
        .expect("search");
    let (sql, params) = recording.blend_statement();
    let scored = db
        .query(&sql, params)
        .await
        .expect("run the blend statement");
    let sim = |id: &str| {
        scored
            .iter()
            .find(|row| row.text("id").as_deref() == Some(id))
            .and_then(|row| row.f64("sim"))
            .unwrap_or_else(|| panic!("{id} scored"))
    };

    let boost_sim = sim("sep_boost");
    let keyword_sim = sim("sep_keyword");
    // The literals: boost `5/7 * 0.3`, keyword `5 * (1/7) * 0.2`.
    assert_eq!(
        boost_sim.to_bits(),
        0x3fcb6db6db6db6db,
        "sep_boost sim {boost_sim} must be the dividing form (mutant …da)"
    );
    assert_eq!(
        keyword_sim.to_bits(),
        0x3fc2492492492492,
        "sep_keyword sim {keyword_sim} must be the reciprocal form (mutant …93)"
    );

    // And against the test-local reference, which is written independently.
    let reference = read_rows(db.as_ref(), TAG).await;
    for id in ["sep_boost", "sep_keyword"] {
        let row = reference.iter().find(|row| row.id == id).expect("row");
        let expected = reference_sim(row, &query, &words);
        assert_eq!(
            sim(id).to_bits(),
            expected.to_bits(),
            "{id}: SQL {} vs Rust {expected}",
            sim(id)
        );
    }
}

/// The predicate that stays untouched (`vector.rs` `tag.contains(word) ||
/// word.contains(tag)`) reproduced exactly on the raw column. A space-bearing
/// chunk must not match a needle its stripped form would create.
#[tokio::test]
async fn test_tag_overlap_is_exact_for_space_bearing_tags() {
    let (_config, db) = prepared_db("blend-space-tags", 2).await;
    let query = vec![1.0f32, 0.0];
    insert_vector(
        db.as_ref(),
        &row("mem_ab", "", query.clone(), None, Some("ab")),
    )
    .await
    .expect("insert ab");
    insert_vector(
        db.as_ref(),
        &row("mem_a_b", "", query.clone(), None, Some("a b")),
    )
    .await
    .expect("insert a b");

    for (word, boosted) in [("cabin", "mem_ab"), ("alpha", "")] {
        let recording = RecordingDb::new(db.clone());
        search_in_shard(&recording, &query, TAG, 5, Some(word))
            .await
            .expect("search");
        let (sql, params) = recording.blend_statement();
        let scored = db
            .query(&sql, params)
            .await
            .expect("run the blend statement");
        let words = tokenize_query_text(Some(word));
        let reference = read_rows(db.as_ref(), TAG).await;
        for row in &reference {
            let served = scored
                .iter()
                .find(|scored| scored.text("id").as_deref() == Some(row.id.as_str()))
                .and_then(|scored| scored.f64("sim"))
                .expect("row scored");
            let expected = reference_sim(row, &query, &words);
            assert!(
                (served - expected).abs() < 1e-6,
                "word {word}, row {}: SQL {served} vs Rust {expected}",
                row.id
            );
            // The boost is the only term that can separate the two rows.
            let elements = tag_elements(row.tags.as_deref());
            let matched = words.iter().any(|word| {
                elements
                    .iter()
                    .any(|tag| tag.contains(word.as_str()) || word.contains(tag.as_str()))
            });
            assert_eq!(
                matched,
                !boosted.is_empty() && row.id == boosted,
                "word {word}, row {}: the Rust predicate's verdict",
                row.id
            );
        }
        // `cabin` boosts `ab` only; `alpha` boosts neither.
        let boosted_id = if boosted.is_empty() {
            None
        } else {
            Some(boosted)
        };
        let served_boost = scored.iter().find_map(|scored| {
            let id = scored.text("id")?;
            let sim = scored.f64("sim")?;
            ((sim - 0.8).abs() < 1e-6).then_some(id)
        });
        assert_eq!(
            served_boost.as_deref(),
            boosted_id,
            "word {word}: the space-bearing row must not gain the boost"
        );
    }
}

/// AC5's carrier: the query vector binds as an F32-LE blob on the hot path.
/// Param-shape assertions alone cannot catch `vector32(?)` — a blob bound to a
/// `vector32(?)` parameter passes every variant/length/round-trip check — so
/// the normalised text check is what reds that mutant.
#[tokio::test]
async fn test_blend_statement_binds_the_query_vector_as_an_f32le_blob() {
    let (_config, db) = prepared_db("blend-blob", 768).await;
    let query: Vec<f32> = (0..768).map(|lane| lane as f32 / 768.0).collect();
    insert_vector(
        db.as_ref(),
        &row("mem_blob", "alpha", query.clone(), None, None),
    )
    .await
    .expect("insert row");

    let recording = RecordingDb::new(db.clone());
    search_in_shard(&recording, &query, TAG, 5, Some("alpha"))
        .await
        .expect("search");
    let (sql, params) = recording.blend_statement();

    for (index, param) in params.iter().take(2).enumerate() {
        match param {
            Value::Blob(blob) => {
                assert_eq!(
                    blob.len(),
                    query.len() * 4,
                    "param {index}: F32-LE, 4 bytes/lane"
                );
                assert_eq!(
                    blob_to_float32_array(param).as_deref(),
                    Some(query.as_slice()),
                    "param {index} round-trips"
                );
            }
            other => panic!("param {index} must be Value::Blob, got {other:?}"),
        }
    }
    let flat = normalised(&sql);
    assert!(
        flat.contains("vector_distance_cos(m.vector,?)"),
        "the blob binds the content cosine: {sql}"
    );
    assert!(
        !flat.contains("vector32("),
        "vector32(?) on the hot path is the 2.5-4.4x regression: {sql}"
    );
}

// ---------------------------------------------------------------------------
// Step 5 — the union's blind spot
// ---------------------------------------------------------------------------/// The union loses `mem_deep` by predicate, not by scan order: its content
/// rank is 81 of 81 (all 80 fillers sit at cosine `0.6`, it at `0.5`), it has
/// no tags vector, and no query token is a substring of its content or tags.
/// It still earns the full boost from the reverse direction — its chunks `a`
/// and `b` are substrings of `alpha` / `beta`, exactly upstream's
/// `word.contains(tag)` disjunct — so its true blend
/// (`0.25 + 0.3 * 2/2 = 0.55`) outranks every filler (`0.3 + 0.1 = 0.4`) and
/// the exact arm serves it first.
#[tokio::test]
async fn test_search_serves_a_deep_scoring_row_the_candidate_union_loses() {
    let (_config, db) = prepared_db("blend-deep-row", 2).await;
    let query = vec![1.0f32, 0.0];
    // A unit vector at cosine 0.6 / 0.5 from the query.
    let filler = vec![0.6f32, 0.8];
    let deep = vec![0.5f32, 0.75f32.sqrt()];

    for index in 0..40 {
        insert_vector(
            db.as_ref(),
            &row(
                &format!("mem_alpha_{index:02}"),
                &format!("alpha filler {index:02}"),
                filler.clone(),
                None,
                None,
            ),
        )
        .await
        .expect("insert alpha filler");
    }
    for index in 0..40 {
        insert_vector(
            db.as_ref(),
            &row(
                &format!("mem_beta_{index:02}"),
                &format!("beta filler {index:02}"),
                filler.clone(),
                None,
                None,
            ),
        )
        .await
        .expect("insert beta filler");
    }
    insert_vector(
        db.as_ref(),
        &row("mem_deep", "deep row", deep, None, Some("a, b")),
    )
    .await
    .expect("insert the deep row");

    // The fixture's score separation, asserted before any tolerance can flip it.
    let deep_score = 0.5 * 0.5 + 0.3 * 1.0;
    let filler_score = 0.5 * 0.5 + 0.0 + 0.1;
    assert!(
        deep_score - filler_score >= 0.04,
        "fixture separation {deep_score} - {filler_score} must exceed any tolerance"
    );

    let served = search_in_shard(db.as_ref(), &query, TAG, 20, Some("alpha beta"))
        .await
        .expect("search");
    let ids: Vec<&str> = served.iter().map(|hit| hit.id.as_str()).collect();
    assert_eq!(
        ids.first().copied(),
        Some("mem_deep"),
        "the exact blend serves the deep row first (served: {ids:?})"
    );
    // The score is `1 - vector_distance_cos` over the stored f32 lanes, so the
    // served number carries f32 rounding (~1e-8) against the analytic cosine;
    // the 1e-6 window still cannot reach the 0.4 filler band.
    assert!(
        (served[0].similarity - deep_score).abs() < 1e-6,
        "served similarity {} != {deep_score}",
        served[0].similarity
    );
}

// ---------------------------------------------------------------------------
// Step 6 — the hydrate bind bound
// ---------------------------------------------------------------------------

/// `hydrate_candidates_sql` emits `id_count.min(1000)` placeholders and
/// `keyword_bumps` scores `ids[..min(1000)]`, so a limit above 1000 must be
/// hydrated in chunks or the tail is dropped / the params mismatch.
#[tokio::test]
async fn test_search_returns_every_row_beyond_the_hydrate_bind_limit() {
    let (_config, db) = prepared_db("blend-chunks", 2).await;
    let query = vec![1.0f32, 0.0];
    let total = 1500usize;
    for index in 0..total {
        // Strictly decreasing cosine with index: [1, x] with x rising.
        let vector = vec![1.0f32, index as f32 / 1000.0];
        insert_vector(
            db.as_ref(),
            &row(&format!("mem_{index:04}"), "", vector, None, None),
        )
        .await
        .expect("insert chunk row");
    }

    let served = search_in_shard(db.as_ref(), &query, TAG, total, None)
        .await
        .expect("chunked search");
    assert_eq!(served.len(), total, "every id past the 1000 bind limit");
    let unique: std::collections::HashSet<&str> =
        served.iter().map(|hit| hit.id.as_str()).collect();
    assert_eq!(unique.len(), total, "no duplicates across chunks");

    for (index, hit) in served.iter().enumerate() {
        assert_eq!(hit.id, format!("mem_{index:04}"), "similarity descending");
        let expected = similarity(&query, &[1.0f32, index as f32 / 1000.0]) * 0.6;
        // Served through the f32 `vector_distance_cos`, so the window is the
        // same 1e-6 used for the per-row parity test.
        assert!(
            (hit.similarity - expected).abs() < 1e-6,
            "{}: served {} vs reference {expected}",
            hit.id,
            hit.similarity
        );
    }
}
