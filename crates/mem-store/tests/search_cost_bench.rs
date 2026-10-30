//! W8 — brute-force search-cost benchmark harness (issue 43 §43.10).
//!
//! Test-only: the fast tests below pin the harness's pure helpers; the four
//! `#[ignore]`d `#[tokio::test]` entries are the measurement path driven by
//! `scripts/w8-search-cost-bench.sh` (one process per cold sample). No
//! production code is exercised for measurement — the narrow arm is a
//! harness-local twin of `vector::search_in_shard` over a companion table.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

use mem_core::scope::Scope;
use mem_store::config::StoreConfig;
use mem_store::connection::ConnectionManager;
use mem_store::db::{BatchMode, ConnectFactory, ConnectOptions, Db, DbHandle, Statement, Value};
use mem_store::error::StoreError;
use mem_store::platform::Platform;
use mem_store::schema_migrations::{
    MEMORY_SHARD_MAX_KNOWN, MigrationOptions, apply_schema_migrations, memory_shard_migrations,
};
use mem_store::shard_facts;
use mem_store::shard_manager::ShardManager;
use mem_store::vector::{self, MemoryInsert, SearchResult};

/// One measured sample (cold process, warm in-process, or diagnostic).
#[derive(Debug, Clone, Copy, Default)]
struct Sample {
    elapsed_us: f64,
    read_bytes: u64,
    scan_us: f64,
    hydrate_us: f64,
    search_us: f64,
}

/// Nearest-rank percentile aggregate over one sample series.
#[derive(Debug, Clone, PartialEq)]
struct Summary {
    n: usize,
    min: f64,
    p50: f64,
    p95: f64,
    max: f64,
    mean: f64,
}

// ---------------------------------------------------------------------------
// Step 1 — percentile + summary invariants
// ---------------------------------------------------------------------------

#[test]
fn test_percentile_nearest_rank_uses_ceil_not_floor() {
    let eleven: Vec<f64> = (1..=11).map(f64::from).collect();
    assert_eq!(percentile_nearest_rank(&eleven, 0.95), Some(11.0));
    assert_eq!(percentile_nearest_rank(&eleven, 0.50), Some(6.0));

    let twenty: Vec<f64> = (1..=20).map(f64::from).collect();
    assert_eq!(percentile_nearest_rank(&twenty, 0.95), Some(19.0));
    assert_eq!(percentile_nearest_rank(&twenty, 0.50), Some(10.0));
}

#[test]
fn test_summary_keeps_max_and_counts_every_sample() {
    let samples: Vec<Sample> = (1..=20)
        .map(|value| Sample {
            elapsed_us: f64::from(value),
            ..Sample::default()
        })
        .collect();
    let summary = summarize(&samples);
    assert_eq!(summary.n, 20);
    assert_eq!(summary.min, 1.0);
    assert_eq!(summary.max, 20.0);
    assert_eq!(summary.p50, 10.0);
    assert_eq!(summary.p95, 19.0);
    assert!((summary.mean - 10.5).abs() < 1e-12, "{summary:?}");
}

// ---------------------------------------------------------------------------
// Step 2 — deterministic generator + the measured length fixture
// ---------------------------------------------------------------------------

const HISTOGRAM_TSV: &str = include_str!("fixtures/w8_content_length_histogram.tsv");

const QUERY_TOKENS: [&str; 3] = ["database", "hardening", "benchmark"];
const QUERY_TEXT: &str = "database hardening benchmark";

/// 256-word filler vocabulary — content bytes are synthetic ASCII only.
const VOCABULARY: [&str; 256] = [
    "the",
    "of",
    "and",
    "to",
    "in",
    "is",
    "you",
    "that",
    "it",
    "he",
    "was",
    "for",
    "on",
    "are",
    "as",
    "with",
    "his",
    "they",
    "i",
    "at",
    "be",
    "this",
    "have",
    "from",
    "or",
    "one",
    "had",
    "by",
    "word",
    "but",
    "not",
    "what",
    "all",
    "were",
    "we",
    "when",
    "your",
    "can",
    "said",
    "there",
    "use",
    "an",
    "each",
    "which",
    "she",
    "do",
    "how",
    "their",
    "if",
    "will",
    "up",
    "other",
    "about",
    "out",
    "many",
    "then",
    "them",
    "these",
    "so",
    "some",
    "her",
    "would",
    "make",
    "like",
    "him",
    "into",
    "time",
    "has",
    "look",
    "two",
    "more",
    "write",
    "go",
    "see",
    "number",
    "no",
    "way",
    "could",
    "people",
    "my",
    "than",
    "first",
    "water",
    "been",
    "call",
    "who",
    "oil",
    "its",
    "now",
    "find",
    "long",
    "down",
    "day",
    "did",
    "get",
    "come",
    "made",
    "may",
    "part",
    "over",
    "new",
    "sound",
    "take",
    "only",
    "little",
    "work",
    "know",
    "place",
    "year",
    "live",
    "me",
    "back",
    "give",
    "most",
    "very",
    "after",
    "thing",
    "our",
    "just",
    "name",
    "good",
    "sentence",
    "man",
    "think",
    "say",
    "great",
    "where",
    "help",
    "through",
    "much",
    "before",
    "line",
    "right",
    "too",
    "mean",
    "old",
    "any",
    "same",
    "tell",
    "boy",
    "follow",
    "came",
    "want",
    "show",
    "also",
    "around",
    "form",
    "three",
    "small",
    "set",
    "put",
    "end",
    "does",
    "another",
    "well",
    "large",
    "must",
    "big",
    "even",
    "such",
    "because",
    "turn",
    "here",
    "why",
    "ask",
    "went",
    "men",
    "read",
    "need",
    "land",
    "different",
    "home",
    "us",
    "move",
    "try",
    "kind",
    "hand",
    "picture",
    "again",
    "change",
    "off",
    "play",
    "spell",
    "air",
    "away",
    "animal",
    "house",
    "point",
    "page",
    "letter",
    "mother",
    "answer",
    "found",
    "study",
    "still",
    "learn",
    "should",
    "america",
    "world",
    "high",
    "every",
    "near",
    "add",
    "food",
    "between",
    "own",
    "below",
    "country",
    "plant",
    "last",
    "school",
    "father",
    "keep",
    "tree",
    "never",
    "start",
    "city",
    "earth",
    "eye",
    "light",
    "thought",
    "head",
    "under",
    "story",
    "saw",
    "left",
    "few",
    "while",
    "along",
    "might",
    "close",
    "something",
    "seem",
    "next",
    "hard",
    "open",
    "example",
    "begin",
    "life",
    "always",
    "those",
    "both",
    "paper",
    "together",
    "got",
    "group",
    "often",
    "run",
    "important",
    "until",
    "children",
    "side",
    "feet",
    "car",
    "mile",
    "night",
];

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
const CONTENT_SALT: u64 = 0x9e37_79b9_7f4a_7c15;
const VECTOR_SALT: u64 = 0xc2b2_ae3d_27d4_eb4f;
const TAGS_VECTOR_SALT: u64 = 0x1656_67b1_9e37_79f9;
const QUERY_SALT: u64 = 0x2545_f491_4f6c_dd1d;

/// The measured content-length multiset (ascending), expanded once.
fn length_multiset() -> Vec<usize> {
    multiset_cached().to_vec()
}

fn multiset_cached() -> &'static [usize] {
    static MULTISET: std::sync::OnceLock<Vec<usize>> = std::sync::OnceLock::new();
    MULTISET.get_or_init(|| {
        let mut lengths = Vec::new();
        for line in HISTOGRAM_TSV.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let mut fields = line.split('\t');
            let length: usize = fields
                .next()
                .expect("length column")
                .parse()
                .expect("length");
            let count: usize = fields.next().expect("count column").parse().expect("count");
            for _ in 0..count {
                lengths.push(length);
            }
        }
        lengths.sort_unstable();
        lengths
    })
}

fn length_for(index: usize, multiset: &[usize]) -> usize {
    multiset[index % multiset.len()]
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layout {
    Inline,
    Narrow,
}

#[derive(Debug, Clone, Copy)]
struct Cell {
    name: &'static str,
    rows: u64,
    cap: u32,
    layout: Layout,
    multiplier: u64,
}

fn plan_rows(cell: &Cell, base_rows: u64) -> u64 {
    cell.multiplier * base_rows
}

fn cell_spec(
    name: &'static str,
    multiplier: u64,
    cap: u32,
    layout: Layout,
    base_rows: u64,
) -> Cell {
    let mut cell = Cell {
        name,
        rows: 0,
        cap,
        layout,
        multiplier,
    };
    cell.rows = plan_rows(&cell, base_rows);
    cell
}

fn shard_count(rows: u64, cap: u32) -> u32 {
    let cap = u64::from(cap.max(1));
    let count = rows.div_ceil(cap).max(1);
    u32::try_from(count).unwrap_or(u32::MAX)
}

fn cell_plan(base_rows: u64) -> [Cell; 9] {
    [
        cell_spec("x1-inline-cap50k", 1, 50_000, Layout::Inline, base_rows),
        cell_spec("x10-inline-cap50k", 10, 50_000, Layout::Inline, base_rows),
        cell_spec("x10-inline-cap12k5", 10, 12_500, Layout::Inline, base_rows),
        cell_spec("x10-narrow-cap50k", 10, 50_000, Layout::Narrow, base_rows),
        cell_spec("x10-narrow-cap12k5", 10, 12_500, Layout::Narrow, base_rows),
        cell_spec("x100-inline-cap50k", 100, 50_000, Layout::Inline, base_rows),
        cell_spec(
            "x100-inline-cap12k5",
            100,
            12_500,
            Layout::Inline,
            base_rows,
        ),
        cell_spec("x100-narrow-cap50k", 100, 50_000, Layout::Narrow, base_rows),
        cell_spec(
            "x100-narrow-cap12k5",
            100,
            12_500,
            Layout::Narrow,
            base_rows,
        ),
    ]
}

/// One generated memory row — the cell name never enters the data.
#[derive(Debug, Clone)]
struct RowPlan {
    id: String,
    content: String,
    vector: Vec<f32>,
    tags_vector: Option<Vec<f32>>,
    tags: Option<String>,
    metadata: Option<String>,
}

fn xorshift64(state: &mut u64) -> u64 {
    let mut value = *state;
    value ^= value >> 12;
    value ^= value << 25;
    value ^= value >> 27;
    *state = value;
    value.wrapping_mul(0x2545_f491_4f6c_dd1d)
}

fn row_state(row_seed: u64, index: usize, salt: u64) -> u64 {
    row_seed ^ (index as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ salt
}

fn generated_content(row_seed: u64, index: usize, length: usize) -> String {
    let mut state = row_state(row_seed, index, CONTENT_SALT);
    let _ = xorshift64(&mut state);
    let mut content = String::with_capacity(length);
    while content.len() < length {
        if !content.is_empty() {
            if content.len() + 1 >= length {
                break;
            }
            content.push(' ');
        }
        let roll = xorshift64(&mut state) % 100;
        let word: &str = if roll < 3 {
            QUERY_TOKENS[roll as usize]
        } else {
            VOCABULARY[(xorshift64(&mut state) % VOCABULARY.len() as u64) as usize]
        };
        let remaining = length - content.len();
        if word.len() <= remaining {
            content.push_str(word);
        } else {
            content.push_str(&word[..remaining]);
            break;
        }
    }
    while content.len() < length {
        content.push('x');
    }
    content
}

fn generated_vector(row_seed: u64, index: usize, salt: u64) -> Vec<f32> {
    let mut state = row_state(row_seed, index, salt);
    (0..768)
        .map(|_| {
            let bits = xorshift64(&mut state) >> 11;
            let unit = bits as f64 / (1_u64 << 53) as f64;
            (unit * 2.0 - 1.0) as f32
        })
        .collect()
}

fn generated_row(row_seed: u64, index: usize) -> RowPlan {
    let shape_cycle = index % 3_016;
    RowPlan {
        id: format!("w8-{index:07}"),
        content: generated_content(row_seed, index, length_for(index, multiset_cached())),
        vector: generated_vector(row_seed, index, VECTOR_SALT),
        tags_vector: if shape_cycle < 2_954 {
            Some(generated_vector(row_seed, index, TAGS_VECTOR_SALT))
        } else {
            None
        },
        tags: Some(format!("w8,tag{}", index % 7)),
        metadata: if shape_cycle < 1_402 {
            Some(format!("{{\"w8\":\"{}\"}}", "x".repeat(136)))
        } else {
            None
        },
    }
}

fn query_vector(query_seed: u64) -> [f32; 768] {
    let mut state = row_state(query_seed, 0, QUERY_SALT);
    let mut vector = [0.0_f32; 768];
    for value in &mut vector {
        let bits = xorshift64(&mut state) >> 11;
        let unit = bits as f64 / (1_u64 << 53) as f64;
        *value = (unit * 2.0 - 1.0) as f32;
    }
    vector
}

fn digest_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(FNV_PRIME);
    }
}

fn row_digest(plan: &RowPlan) -> u64 {
    let mut hash = FNV_OFFSET;
    digest_bytes(&mut hash, plan.id.as_bytes());
    digest_bytes(&mut hash, b"|");
    digest_bytes(&mut hash, &(plan.content.len() as u64).to_le_bytes());
    digest_bytes(&mut hash, b"|v");
    for value in &plan.vector {
        digest_bytes(&mut hash, &value.to_bits().to_le_bytes());
    }
    digest_bytes(&mut hash, b"|t");
    match &plan.tags_vector {
        Some(values) => {
            digest_bytes(&mut hash, b"1");
            for value in values {
                digest_bytes(&mut hash, &value.to_bits().to_le_bytes());
            }
        }
        None => digest_bytes(&mut hash, b"0"),
    }
    digest_bytes(&mut hash, b"|m");
    match &plan.metadata {
        Some(metadata) => digest_bytes(&mut hash, metadata.as_bytes()),
        None => digest_bytes(&mut hash, b"0"),
    }
    hash
}

// ---------------------------------------------------------------------------
// Step 2 tests
// ---------------------------------------------------------------------------

fn digest_series(row_seed: u64, count: usize) -> Vec<u64> {
    (0..count)
        .map(|index| row_digest(&generated_row(row_seed, index)))
        .collect()
}

#[test]
fn test_generator_is_deterministic() {
    let seed = 0x5eed_0000_0000_0043_u64;

    let first = digest_series(seed, 500);
    let second = digest_series(seed, 500);
    assert_eq!(first, second, "the same seed must rebuild the same rows");

    let other = digest_series(seed ^ 0xa5a5_a5a5_a5a5_a5a5, 500);
    assert_ne!(first, other, "a different row seed must change the data");

    let base = length_multiset().len();
    let ten = digest_series(seed, 10 * base);
    let mut hundred_prefix = Vec::with_capacity(10 * base);
    for index in 0..100 * base {
        let digest = row_digest(&generated_row(seed, index));
        if index < 10 * base {
            hundred_prefix.push(digest);
        }
    }
    assert_eq!(
        ten, hundred_prefix,
        "the 10x stream must be the strict prefix of the 100x stream"
    );

    let plan_time_rows: Vec<_> = (0..3_016).map(|index| generated_row(seed, index)).collect();
    assert_eq!(
        plan_time_rows
            .iter()
            .filter(|plan| plan.tags_vector.is_some())
            .count(),
        2_954,
        "plan-time tags-vector shape"
    );
    assert_eq!(
        plan_time_rows
            .iter()
            .filter(|plan| plan.metadata.is_some())
            .count(),
        1_402,
        "plan-time metadata shape"
    );

    for (index, plan) in plan_time_rows.iter().enumerate() {
        assert_eq!(plan.id, format!("w8-{index:07}"), "id is index-only");
        for marker in [
            "x1-", "x10-", "x100-", "cap50k", "cap12k5", "inline", "narrow",
        ] {
            assert!(!plan.id.contains(marker), "cell marker in id: {marker}");
            assert!(
                !plan.content.contains(marker),
                "cell marker in content: {marker}"
            );
        }
    }
}

#[test]
fn test_generated_lengths_match_the_measured_multiset() {
    let multiset = length_multiset();
    assert!(!multiset.is_empty(), "the fixture must not be empty");
    assert!(
        multiset.windows(2).all(|pair| pair[0] <= pair[1]),
        "the multiset is ascending"
    );

    let tsv_total: usize = HISTOGRAM_TSV
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let mut fields = line.split('\t');
            let _length = fields.next().expect("length field");
            fields
                .next()
                .expect("count field")
                .parse::<usize>()
                .expect("count parses")
        })
        .sum();
    assert_eq!(multiset.len(), tsv_total);

    let one_cycle: Vec<usize> = (0..multiset.len())
        .map(|index| length_for(index, &multiset))
        .collect();
    assert_eq!(one_cycle, multiset);

    let one_sum: usize = multiset.iter().sum();
    let ten_sum: usize = (0..10 * multiset.len())
        .map(|index| length_for(index, &multiset))
        .sum();
    assert_eq!(ten_sum, one_sum * 10, "10 cycles scale exactly");
    let hundred_sum: usize = (0..100 * multiset.len())
        .map(|index| length_for(index, &multiset))
        .sum();
    assert_eq!(hundred_sum, one_sum * 100, "100 cycles scale exactly");

    let ten_cycle: Vec<usize> = (0..10 * multiset.len())
        .map(|index| length_for(index, &multiset))
        .collect();
    assert_eq!(
        &ten_cycle[..multiset.len()],
        &multiset[..],
        "the second cycle starts the multiset again"
    );
    assert_eq!(
        &ten_cycle[9 * multiset.len()..],
        &multiset[..],
        "the tenth cycle ends the multiset"
    );
}

fn percentile_nearest_rank(sorted_asc: &[f64], p: f64) -> Option<f64> {
    if sorted_asc.is_empty() {
        return None;
    }
    let n = sorted_asc.len();
    let rank = (p * n as f64).ceil();
    let index = if rank <= 1.0 {
        0
    } else {
        (rank as usize - 1).min(n - 1)
    };
    Some(sorted_asc[index])
}

fn summarize(samples: &[Sample]) -> Summary {
    if samples.is_empty() {
        return Summary {
            n: 0,
            min: 0.0,
            p50: 0.0,
            p95: 0.0,
            max: 0.0,
            mean: 0.0,
        };
    }
    let mut values: Vec<f64> = samples.iter().map(|sample| sample.elapsed_us).collect();
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let n = values.len();
    Summary {
        n,
        min: values[0],
        p50: percentile_nearest_rank(&values, 0.50).unwrap_or(0.0),
        p95: percentile_nearest_rank(&values, 0.95).unwrap_or(0.0),
        max: values[n - 1],
        mean: values.iter().sum::<f64>() / n as f64,
    }
}

// ---------------------------------------------------------------------------
// Step 3 — cell plan and the cap partition boundary
// ---------------------------------------------------------------------------

#[test]
fn test_cell_plan_has_nine_cells_and_exact_row_products() {
    const NAMES: [&str; 9] = [
        "x1-inline-cap50k",
        "x10-inline-cap50k",
        "x10-inline-cap12k5",
        "x10-narrow-cap50k",
        "x10-narrow-cap12k5",
        "x100-inline-cap50k",
        "x100-inline-cap12k5",
        "x100-narrow-cap50k",
        "x100-narrow-cap12k5",
    ];
    for base in [3_016_u64, 1_000_u64] {
        let cells = cell_plan(base);
        let names: Vec<&str> = cells.iter().map(|cell| cell.name).collect();
        assert_eq!(names, NAMES, "the nine named cells, in order");
        for cell in &cells {
            let multiplier = if cell.name.starts_with("x100-") {
                100
            } else if cell.name.starts_with("x10-") {
                10
            } else {
                1
            };
            assert_eq!(
                cell.rows,
                multiplier * base,
                "{} rows must be multiplier x base at base {base}",
                cell.name
            );
            assert_eq!(plan_rows(cell, base), cell.rows);
            if cell.name.contains("cap50k") {
                assert_eq!(cell.cap, 50_000, "{}", cell.name);
            } else {
                assert_eq!(cell.cap, 12_500, "{}", cell.name);
            }
            if cell.name.contains("inline") {
                assert_eq!(cell.layout, Layout::Inline, "{}", cell.name);
            } else {
                assert_eq!(cell.layout, Layout::Narrow, "{}", cell.name);
            }
            println!(
                "{:<22} rows={:<7} cap={:<6} layout={:?}",
                cell.name, cell.rows, cell.cap, cell.layout
            );
        }
    }
}

#[test]
fn test_shard_count_rounds_up_and_never_zero() {
    assert_eq!(shard_count(25_000, 12_500), 2);
    assert_eq!(shard_count(25_001, 12_500), 3);
    assert_eq!(shard_count(301_600, 50_000), 7);
    assert_eq!(shard_count(301_600, 12_500), 25);
    assert_eq!(shard_count(0, 12_500), 1);
}

// ---------------------------------------------------------------------------
// Step 4 — the narrow twin: same ranking over the companion table
// ---------------------------------------------------------------------------

/// Harness-local failure type (the harness may fail loudly; production is
/// untouched).
#[derive(Debug)]
enum HarnessError {
    Env(String),
    ForbiddenRoot(PathBuf),
    ExistingRoot(PathBuf),
    Io(std::io::Error),
    Store(StoreError),
}

impl fmt::Display for HarnessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Env(message) => write!(formatter, "harness env: {message}"),
            Self::ForbiddenRoot(path) => {
                write!(formatter, "refusing forbidden root {}", path.display())
            }
            Self::ExistingRoot(path) => {
                write!(formatter, "refusing non-throwaway root {}", path.display())
            }
            Self::Io(error) => write!(formatter, "io: {error}"),
            Self::Store(error) => write!(formatter, "store: {error}"),
        }
    }
}

impl std::error::Error for HarnessError {}

impl From<std::io::Error> for HarnessError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<StoreError> for HarnessError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

/// Facts about a seeded root (one line in the cell JSONL).
#[derive(Debug, Clone)]
struct RootFacts {
    root: PathBuf,
    rows: u64,
    shards: u64,
    payload_bytes: u64,
    shard_bytes: u64,
}

/// The narrow twin's split timing.
#[derive(Debug, Clone, Copy, Default)]
struct Split {
    scan_us: f64,
    hydrate_us: f64,
}

#[tokio::test]
async fn test_narrow_schema_and_indexes_match_production() {
    let root = temp_root("w8-s4-schema");
    let config = store_config_for(&root, 50_000);
    let production = open_db_at(&config, &root.join("production.db"))
        .await
        .expect("production db");
    apply_schema_migrations(
        production.as_ref(),
        &memory_shard_migrations(768),
        &MigrationOptions {
            db_path: None,
            label: "w8-s4-production",
            max_known: MEMORY_SHARD_MAX_KNOWN,
        },
        None,
    )
    .await
    .expect("production schema");

    let narrow = open_db_at(&config, &root.join("narrow.db"))
        .await
        .expect("narrow db");
    exec_script(narrow.as_ref(), &narrow_ddl(768))
        .await
        .expect("narrow schema");

    let production_columns = table_columns(production.as_ref(), "memories").await;
    let narrow_columns = table_columns(narrow.as_ref(), "memories").await;
    let production_names: HashSet<String> = production_columns.keys().cloned().collect();
    let narrow_names: HashSet<String> = narrow_columns.keys().cloned().collect();

    let mut removed: Vec<String> = production_names
        .difference(&narrow_names)
        .cloned()
        .collect();
    removed.sort();
    assert_eq!(
        removed,
        vec!["tags_vector".to_string(), "vector".to_string()],
        "the narrow memories table drops exactly the two vector columns"
    );
    let mut added: Vec<String> = narrow_names
        .difference(&production_names)
        .cloned()
        .collect();
    added.sort();
    assert!(added.is_empty(), "narrow-only memories columns: {added:?}");
    for (name, production_column) in &production_columns {
        if let Some(narrow_column) = narrow_columns.get(name) {
            assert_eq!(
                narrow_column, production_column,
                "shared column {name} must keep type/notnull/default"
            );
        }
    }

    let production_indexes = created_index_names(production.as_ref(), "memories").await;
    let narrow_indexes = created_index_names(narrow.as_ref(), "memories").await;
    let expected_indexes: HashSet<String> = [
        "idx_container_tag",
        "idx_type",
        "idx_created_at",
        "idx_is_pinned",
        "idx_session_id",
    ]
    .iter()
    .map(|name| name.to_string())
    .collect();
    assert_eq!(production_indexes, expected_indexes);
    assert_eq!(narrow_indexes, expected_indexes);

    let vector_columns = table_columns(narrow.as_ref(), "memory_vectors").await;
    let vector_names: HashSet<String> = vector_columns.keys().cloned().collect();
    let expected_vector_names: HashSet<String> = ["id", "container_tag", "vector", "tags_vector"]
        .iter()
        .map(|name| name.to_string())
        .collect();
    assert_eq!(
        vector_names, expected_vector_names,
        "the companion table carries exactly the moved columns"
    );
    let vector_indexes = created_index_names(narrow.as_ref(), "memory_vectors").await;
    assert!(
        vector_indexes.contains("idx_memory_vectors_tag"),
        "the tag index must exist: {vector_indexes:?}"
    );
}

#[tokio::test]
async fn test_narrow_scan_matches_inline_top_k() {
    let cell = Cell {
        name: "parity",
        rows: 2_000,
        cap: 50_000,
        layout: Layout::Narrow,
        multiplier: 2,
    };
    let inline = create_inline_root(&cell, 2_000).await.expect("inline root");
    let narrow = create_narrow_root(&cell, 2_000).await.expect("narrow root");
    let qvec = query_vector(query_seed());
    rewrite_parity_fixture(&inline.root, Layout::Inline, &qvec)
        .await
        .expect("inline fixture");
    rewrite_parity_fixture(&narrow.root, Layout::Narrow, &qvec)
        .await
        .expect("narrow fixture");

    let inline_db =
        open_single_shard(&store_config_for(&inline.root, cell.cap), &inline.root).await;
    let narrow_db =
        open_single_shard(&store_config_for(&narrow.root, cell.cap), &narrow.root).await;

    let inline_results =
        vector::search_in_shard(inline_db.as_ref(), &qvec, TAG, 40, Some(QUERY_TEXT))
            .await
            .expect("inline search");
    let (narrow_results, _split) =
        narrow_search_in_shard(narrow_db.as_ref(), &qvec, TAG, 40, Some(QUERY_TEXT))
            .await
            .expect("narrow search");

    let inline_map = similarity_map(&inline_results);
    let narrow_map = similarity_map(&narrow_results);
    let union: HashSet<&String> = inline_map.keys().chain(narrow_map.keys()).collect();
    let mut max_delta = 0.0_f64;
    for id in union {
        let inline_similarity = inline_map
            .get(id)
            .unwrap_or_else(|| panic!("inline arm missing {id}"));
        let narrow_similarity = narrow_map
            .get(id)
            .unwrap_or_else(|| panic!("narrow arm missing {id}"));
        let delta = (inline_similarity - narrow_similarity).abs();
        max_delta = max_delta.max(delta);
        assert!(
            delta <= 1e-12,
            "pre-sort similarity diverged for {id}: {inline_similarity} vs {narrow_similarity}"
        );
    }
    let inline_ids: HashSet<&String> = inline_map.keys().collect();
    let narrow_ids: HashSet<&String> = narrow_map.keys().collect();
    assert_eq!(inline_ids, narrow_ids, "returned id sets must agree");

    let ordered_inline = merge_ordered_ids(&inline_results);
    let ordered_narrow = merge_ordered_ids(&narrow_results);
    assert_eq!(
        ordered_inline, ordered_narrow,
        "the same merge comparator must order both arms equally"
    );
    println!(
        "n={} pre_sort_sim_delta={max_delta:e} idset_equal=1 post_merge_ordered_equal=1",
        inline_map.len()
    );
}

// ---------------------------------------------------------------------------
// Step 4 GREEN — helpers, narrow schema, the narrow twin, root seeding
// ---------------------------------------------------------------------------

const DIMS: u32 = 768;
const SCOPE_HASH: &str = "a1b2c3d4e5f67890";
const TAG: &str = "opencode_project_a1b2c3d4e5f67890";
const BASE_TS_MS: i64 = 1_700_000_000_000;
const DEFAULT_ROW_SEED: u64 = 0x5eed_0000_0000_0043;
const DEFAULT_QUERY_SEED: u64 = 0x51ee_d000_0000_0027;

fn row_seed() -> u64 {
    std::env::var("W8_ROW_SEED")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_ROW_SEED)
}

fn query_seed() -> u64 {
    std::env::var("W8_QUERY_SEED")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_QUERY_SEED)
}

fn configured_root() -> PathBuf {
    std::env::var("HOME")
        .map(|home| Path::new(&home).join(".opencode-mem").join("data"))
        .unwrap_or_else(|_| PathBuf::from("/nonexistent/.opencode-mem/data"))
}

fn temp_path(prefix: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "mem-store-w8-{prefix}-{}-{counter}-{nanos}",
        std::process::id()
    ))
}

fn temp_root(prefix: &str) -> PathBuf {
    let root = temp_path(prefix);
    std::fs::create_dir_all(&root).expect("create harness temp root");
    root
}

fn store_config_for(root: &Path, cap: u32) -> StoreConfig {
    StoreConfig {
        storage_path: root.to_path_buf(),
        embedding_dimensions: DIMS,
        embedding_model: "w8-model".to_string(),
        max_vectors_per_shard: cap,
        container_tag_prefix: "opencode".to_string(),
        database_encryption_enabled: false,
        database_encryption_cipher: "aes256gcm".to_string(),
        database_encryption_key: None,
        allow_unguarded_open: false,
    }
}

async fn open_db_at(config: &StoreConfig, path: &Path) -> Result<DbHandle, StoreError> {
    mem_store::turso_connect_factory()
        .connect(
            path.to_path_buf(),
            ConnectOptions::for_config(config, Platform::current()),
        )
        .await
}

fn manager_for(root: &Path, cap: u32) -> (ShardManager, std::sync::Arc<ConnectionManager>) {
    let config = store_config_for(root, cap);
    let factory: std::sync::Arc<dyn ConnectFactory> =
        std::sync::Arc::new(mem_store::turso_connect_factory());
    let connections = std::sync::Arc::new(ConnectionManager::new(
        root.to_path_buf(),
        config.clone(),
        Platform::current(),
        factory,
    ));
    let manager = ShardManager::new(
        root.to_path_buf(),
        config,
        connections.clone(),
        std::sync::Arc::new(mem_store::platform::SystemProcessProbe),
        None,
    );
    (manager, connections)
}

async fn exec_script(db: &dyn Db, script: &str) -> Result<(), StoreError> {
    let statements: Vec<Statement> = script
        .split(';')
        .map(str::trim)
        .filter(|statement| !statement.is_empty())
        .map(|statement| Statement::new(statement, Vec::new()))
        .collect();
    db.batch(statements, BatchMode::Write).await?;
    Ok(())
}

async fn table_columns(db: &dyn Db, table: &str) -> HashMap<String, (String, i64, Option<String>)> {
    let rows = db
        .query(&format!("PRAGMA table_info({table})"), vec![])
        .await
        .expect("table_info");
    rows.into_iter()
        .map(|row| {
            let name = row.text("name").expect("column name");
            let shape = (
                row.text("type").unwrap_or_default(),
                row.i64("notnull").unwrap_or(0),
                row.text("dflt_value"),
            );
            (name, shape)
        })
        .collect()
}

async fn created_index_names(db: &dyn Db, table: &str) -> HashSet<String> {
    let rows = db
        .query(&format!("PRAGMA index_list({table})"), vec![])
        .await
        .expect("index_list");
    rows.into_iter()
        .filter(|row| row.text("origin").as_deref() == Some("c"))
        .filter_map(|row| row.text("name"))
        .collect()
}

/// The narrow layout: production columns minus both vector columns, the same
/// five `memories` indexes, and the vectors in a companion table.
fn narrow_ddl(dims: u32) -> String {
    format!(
        r#"
        CREATE TABLE IF NOT EXISTS memories (
          id TEXT PRIMARY KEY,
          content TEXT NOT NULL,
          container_tag TEXT NOT NULL,
          tags TEXT,
          type TEXT,
          created_at INTEGER NOT NULL,
          updated_at INTEGER NOT NULL,
          metadata TEXT,
          display_name TEXT,
          user_name TEXT,
          user_email TEXT,
          project_path TEXT,
          project_name TEXT,
          git_repo_url TEXT,
          is_pinned INTEGER DEFAULT 0,
          session_id TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_container_tag ON memories(container_tag);
        CREATE INDEX IF NOT EXISTS idx_type ON memories(type);
        CREATE INDEX IF NOT EXISTS idx_created_at ON memories(created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_is_pinned ON memories(is_pinned);
        CREATE INDEX IF NOT EXISTS idx_session_id ON memories(session_id);
        CREATE TABLE IF NOT EXISTS memory_vectors (
          id TEXT PRIMARY KEY,
          container_tag TEXT NOT NULL,
          vector F32_BLOB({dims}) NOT NULL,
          tags_vector F32_BLOB({dims})
        );
        CREATE INDEX IF NOT EXISTS idx_memory_vectors_tag ON memory_vectors(container_tag);
        {shard_metadata};
        {shard_stats};
        PRAGMA user_version = 3;
        "#,
        dims = dims,
        shard_metadata = mem_store::sql::shard_metadata_ddl().trim(),
        shard_stats = shard_facts::SHARD_STATS_DDL,
    )
}

const NARROW_CONTENT_SCAN_SQL: &str = r#"
    SELECT id, vector_distance_cos(vector, vector32(?)) AS dist
    FROM memory_vectors
    WHERE container_tag = ? AND vector IS NOT NULL
    ORDER BY dist ASC LIMIT ?
"#;

const NARROW_TAGS_SCAN_SQL: &str = r#"
    SELECT id, vector_distance_cos(tags_vector, vector32(?)) AS dist
    FROM memory_vectors
    WHERE container_tag = ? AND tags_vector IS NOT NULL
    ORDER BY dist ASC LIMIT ?
"#;

const NARROW_HYDRATE_SQL: &str = r#"
    SELECT m.id, m.content, m.tags, m.created_at, m.metadata, m.container_tag, m.display_name,
           m.user_name, m.user_email, m.project_path, m.project_name, m.git_repo_url, m.is_pinned,
           m.session_id,
           vector_distance_cos(v.vector, vector32(?)) AS content_dist,
           CASE WHEN v.tags_vector IS NOT NULL THEN vector_distance_cos(v.tags_vector, vector32(?))
                ELSE NULL END AS tags_dist
    FROM memories m JOIN memory_vectors v ON v.id = m.id
    WHERE m.id IN ({placeholders}) AND m.container_tag = ?
"#;

fn narrow_hydrate_sql(id_count: usize) -> String {
    let count = id_count.min(1000);
    let placeholders = vec!["?"; count].join(", ");
    NARROW_HYDRATE_SQL.replace("{placeholders}", &placeholders)
}

fn parse_metadata_column(value: Option<&Value>) -> Option<serde_json::Value> {
    let text = match value {
        Some(Value::Text(text)) if !text.is_empty() => text,
        _ => return None,
    };
    let parsed: serde_json::Value = serde_json::from_str(text).ok()?;
    parsed.is_object().then_some(parsed)
}

async fn narrow_scan(
    db: &dyn Db,
    sql: &str,
    query_json: &str,
    tag: &str,
    k: usize,
) -> Result<Vec<(String, f64)>, StoreError> {
    let rows = db
        .query(
            sql,
            vec![
                Value::Text(query_json.to_string()),
                Value::Text(tag.to_string()),
                Value::Integer(k as i64),
            ],
        )
        .await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            (
                row.text("id").unwrap_or_default(),
                mem_store::vector_utils::distance_to_similarity(row.f64("dist").unwrap_or(0.0)),
            )
        })
        .collect())
}

/// The harness-local twin of `vector::search_in_shard`: the same scanning,
/// candidate merge, ranking and blend; only the three SQL statements differ.
async fn narrow_search_in_shard(
    db: &dyn Db,
    query_vector: &[f32],
    container_tag: &str,
    limit: usize,
    query_text: Option<&str>,
) -> Result<(Vec<SearchResult>, Split), StoreError> {
    let query_json = vector::vector_to_text(query_vector)?;
    let k = limit.saturating_mul(2).max(32);

    let scan_started = std::time::Instant::now();
    let content_hits =
        narrow_scan(db, NARROW_CONTENT_SCAN_SQL, &query_json, container_tag, k).await?;
    let tags_hits = narrow_scan(db, NARROW_TAGS_SCAN_SQL, &query_json, container_tag, k).await?;
    let keyword = vector::keyword_scores(db, query_text, container_tag, k).await?;
    let scan_us = scan_started.elapsed().as_secs_f64() * 1_000_000.0;

    let mut seen = HashSet::new();
    let mut ids: Vec<String> = Vec::new();
    for id in content_hits
        .into_iter()
        .map(|(id, _)| id)
        .chain(tags_hits.into_iter().map(|(id, _)| id))
        .chain(keyword.iter().map(|(id, _)| id.clone()))
    {
        if seen.insert(id.clone()) {
            ids.push(id);
        }
    }
    if ids.is_empty() {
        return Ok((
            Vec::new(),
            Split {
                scan_us,
                hydrate_us: 0.0,
            },
        ));
    }
    ids.truncate(1000);

    let hydrate_started = std::time::Instant::now();
    let sql = narrow_hydrate_sql(ids.len());
    let mut params = vec![
        Value::Text(query_json.clone()),
        Value::Text(query_json.clone()),
    ];
    params.extend(ids.into_iter().map(Value::Text));
    params.push(Value::Text(container_tag.to_string()));
    let rows = db.query(&sql, params).await?;
    let hydrate_us = hydrate_started.elapsed().as_secs_f64() * 1_000_000.0;

    let query_words = mem_store::vector_utils::tokenize_query_text(query_text);
    let has_keyword = !query_words.is_empty();
    let mut results: Vec<SearchResult> = rows
        .into_iter()
        .map(|row| {
            let id = row.text("id").unwrap_or_default();
            let content_sim = mem_store::vector_utils::distance_to_similarity(
                row.f64("content_dist").unwrap_or(0.0),
            );
            let tags_sim = match row.f64("tags_dist") {
                Some(dist) => mem_store::vector_utils::distance_to_similarity(dist),
                None => 0.0,
            };
            let memory_tags_str = row.text("tags").unwrap_or_default();
            let memory_tags: Vec<String> = memory_tags_str
                .split(',')
                .map(|tag| tag.trim().to_lowercase())
                .filter(|tag| !tag.is_empty())
                .collect();
            let mut exact_match_boost = 0.0;
            if !query_words.is_empty() && !memory_tags.is_empty() {
                let matches = query_words
                    .iter()
                    .filter(|word| {
                        memory_tags
                            .iter()
                            .any(|tag| tag.contains(word.as_str()) || word.contains(tag.as_str()))
                    })
                    .count();
                exact_match_boost = matches as f64 / query_words.len().max(1) as f64;
            }
            let keyword_sim = keyword
                .iter()
                .find(|(scored_id, _)| *scored_id == id)
                .map(|(_, score)| *score)
                .unwrap_or(0.0);
            let final_tags_sim = tags_sim.max(exact_match_boost);
            let similarity = if has_keyword {
                content_sim * 0.5 + final_tags_sim * 0.3 + keyword_sim * 0.2
            } else {
                content_sim * 0.6 + final_tags_sim * 0.4
            };
            SearchResult {
                id,
                memory: row.text("content").unwrap_or_default(),
                similarity,
                created_at: row.i64("created_at").unwrap_or(0),
                tags: if memory_tags_str.is_empty() {
                    Vec::new()
                } else {
                    memory_tags_str.split(',').map(str::to_string).collect()
                },
                metadata: parse_metadata_column(row.get("metadata")),
                container_tag: row.text("container_tag").unwrap_or_default(),
                display_name: row.text("display_name").filter(|text| !text.is_empty()),
                user_name: row.text("user_name").filter(|text| !text.is_empty()),
                user_email: row.text("user_email").filter(|text| !text.is_empty()),
                project_path: row.text("project_path").filter(|text| !text.is_empty()),
                project_name: row.text("project_name").filter(|text| !text.is_empty()),
                git_repo_url: row.text("git_repo_url").filter(|text| !text.is_empty()),
                is_pinned: row.i64("is_pinned"),
            }
        })
        .collect();
    results.sort_by(|a, b| {
        b.similarity
            .partial_cmp(&a.similarity)
            .unwrap_or(Ordering::Equal)
    });
    results.truncate(limit);
    Ok((
        results,
        Split {
            scan_us,
            hydrate_us,
        },
    ))
}

fn memory_insert(plan: &RowPlan, index: usize, container_tag: &str) -> MemoryInsert {
    MemoryInsert {
        id: plan.id.clone(),
        content: plan.content.clone(),
        vector: plan.vector.clone(),
        tags_vector: plan.tags_vector.clone(),
        container_tag: container_tag.to_string(),
        tags: plan.tags.clone(),
        memory_type: Some("w8".to_string()),
        created_at: BASE_TS_MS + index as i64,
        updated_at: BASE_TS_MS + index as i64,
        metadata: plan.metadata.clone(),
        session_id: None,
        display_name: None,
        user_name: None,
        user_email: None,
        project_path: None,
        project_name: None,
        git_repo_url: None,
    }
}

fn payload_bytes(plan: &RowPlan) -> u64 {
    let mut bytes = (plan.content.len()
        + plan.tags.as_deref().map(str::len).unwrap_or(0)
        + plan.metadata.as_deref().map(str::len).unwrap_or(0)
        + plan.vector.len() * 4) as u64;
    if let Some(tags_vector) = &plan.tags_vector {
        bytes += (tags_vector.len() * 4) as u64;
    }
    bytes
}

fn value_or_null(value: &Option<String>) -> Value {
    value.clone().map(Value::Text).unwrap_or(Value::Null)
}

const NARROW_MEMORIES_INSERT: &str = r#"
    INSERT INTO memories (
      id, content, container_tag, tags, type, created_at, updated_at, metadata,
      session_id, display_name, user_name, user_email, project_path, project_name,
      git_repo_url, is_pinned
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
"#;

const NARROW_VECTORS_INSERT_WITH_TAGS: &str = r#"
    INSERT INTO memory_vectors (id, container_tag, vector, tags_vector)
    VALUES (?, ?, vector32(?), vector32(?))
"#;

const NARROW_VECTORS_INSERT_WITHOUT_TAGS: &str = r#"
    INSERT INTO memory_vectors (id, container_tag, vector)
    VALUES (?, ?, vector32(?))
"#;

async fn verify_registry(manager: &ShardManager, rows: u64, cap: u32) -> Result<(), HarnessError> {
    let shards = manager
        .get_all_shards(Scope::Project, Some(SCOPE_HASH))
        .await?;
    let expected = u64::from(shard_count(rows, cap));
    let sum: u64 = shards.iter().map(|shard| shard.vector_count).sum();
    let min = shards
        .iter()
        .map(|shard| shard.vector_count)
        .min()
        .unwrap_or(0);
    let actual = u64::try_from(shards.len()).unwrap_or(0);
    if actual != expected || sum != rows || min == 0 {
        return Err(HarnessError::Env(format!(
            "registry mismatch: shards={actual} (expected {expected}) sum={sum} (rows {rows}) min={min}"
        )));
    }
    Ok(())
}

async fn checkpoint_shards(manager: &ShardManager) -> Result<(), HarnessError> {
    let shards = manager
        .get_all_shards(Scope::Project, Some(SCOPE_HASH))
        .await?;
    for shard in shards {
        let db = manager.open_shard(&shard.db_path).await?;
        let _ = db.query("PRAGMA wal_checkpoint(TRUNCATE)", vec![]).await?;
    }
    Ok(())
}

async fn checkpoint_metadata(root: &Path, cap: u32) -> Result<(), HarnessError> {
    let config = store_config_for(root, cap);
    let db = open_db_at(&config, &root.join("metadata.db")).await?;
    let _ = db.query("PRAGMA wal_checkpoint(TRUNCATE)", vec![]).await?;
    drop(db);
    Ok(())
}

fn shard_file_bytes(root: &Path) -> Result<u64, HarnessError> {
    let projects = root.join("projects");
    let mut total = 0_u64;
    for entry in std::fs::read_dir(&projects)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) == Some("db") {
            total += entry.metadata()?.len();
        }
    }
    Ok(total)
}

fn measurement_root(label: &str) -> Result<PathBuf, HarnessError> {
    match std::env::var("W8_ROOT") {
        Ok(root) => {
            let path = PathBuf::from(root);
            if path == configured_root() {
                return Err(HarnessError::ForbiddenRoot(path));
            }
            Ok(path)
        }
        Err(_) => Ok(temp_root(label)),
    }
}

async fn seed_inline_root<F>(
    root: &Path,
    cell: &Cell,
    rows: u64,
    row_at: F,
) -> Result<RootFacts, HarnessError>
where
    F: Fn(usize) -> RowPlan,
{
    prepare_root(root, &configured_root())?;
    let (manager, connections) = manager_for(root, cell.cap);
    manager.create_shard(Scope::Project, SCOPE_HASH, 0).await?;
    let mut payload = 0_u64;
    let mut remaining = rows;
    let mut global = 0_usize;
    let mut expected_index = 0_u32;
    while remaining > 0 {
        let shard = manager.get_write_shard(Scope::Project, SCOPE_HASH).await?;
        if shard.shard_index != expected_index {
            return Err(HarnessError::Env(format!(
                "inline seed expected shard {expected_index}, resolved {}",
                shard.shard_index
            )));
        }
        let capacity = u64::from(cell.cap).saturating_sub(shard.vector_count);
        let take = capacity.min(remaining);
        if take == 0 {
            return Err(HarnessError::Env(
                "inline write shard is already full".into(),
            ));
        }
        let db = manager.open_shard(&shard.db_path).await?;
        let mut done = 0_u64;
        while done < take {
            let batch_rows = (take - done).min(250);
            let mut statements = Vec::with_capacity(batch_rows as usize);
            for offset in 0..batch_rows {
                let index = global + done as usize + offset as usize;
                let plan = row_at(index);
                payload += payload_bytes(&plan);
                let insert = memory_insert(&plan, index, TAG);
                statements.push(vector::insert_vector_statement(&insert)?);
            }
            db.batch(statements, BatchMode::Write).await?;
            done += batch_rows;
        }
        global += take as usize;
        remaining -= take;
        expected_index += 1;
    }

    if !rows.is_multiple_of(u64::from(cell.cap)) {
        // The final resolution syncs the PARTIAL last shard's own count (not
        // the scope total): `vector_count` is per shard. A full shard would
        // have rotated here, which is exactly why the `== 0` arm below skips
        // this call and adopts the last shard's committed count instead.
        let expected_last = rows % u64::from(cell.cap);
        let last = manager.get_write_shard(Scope::Project, SCOPE_HASH).await?;
        if last.vector_count != expected_last {
            return Err(HarnessError::Env(format!(
                "partial-shard sync read {} rows, expected {expected_last}",
                last.vector_count
            )));
        }
    } else {
        let last_index = shard_count(rows, cell.cap) - 1;
        let path = manager.get_shard_path(Scope::Project, SCOPE_HASH, last_index)?;
        let db = manager.open_shard(&path).await?;
        let count = shard_facts::read_count(db.as_ref()).await?;
        manager
            .register_existing_shard(Scope::Project, SCOPE_HASH, last_index, &path, count, true)
            .await?;
    }
    verify_registry(&manager, rows, cell.cap).await?;
    checkpoint_shards(&manager).await?;
    connections.close_all().await?;
    manager.reset()?;
    checkpoint_metadata(root, cell.cap).await?;
    let shard_bytes = shard_file_bytes(root)?;
    Ok(RootFacts {
        root: root.to_path_buf(),
        rows,
        shards: u64::from(shard_count(rows, cell.cap)),
        payload_bytes: payload,
        shard_bytes,
    })
}

async fn seed_narrow_root<F>(
    root: &Path,
    cell: &Cell,
    rows: u64,
    row_at: F,
) -> Result<RootFacts, HarnessError>
where
    F: Fn(usize) -> RowPlan,
{
    prepare_root(root, &configured_root())?;
    let (manager, connections) = manager_for(root, cell.cap);
    let _ = manager
        .get_all_shards(Scope::Project, Some(SCOPE_HASH))
        .await?;
    let shards = shard_count(rows, cell.cap);
    let mut payload = 0_u64;
    let mut remaining = rows;
    let mut global = 0_usize;
    for index in 0..shards {
        let count = remaining.min(u64::from(cell.cap));
        let path = manager.get_shard_path(Scope::Project, SCOPE_HASH, index)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let db = connections.get_connection(&path).await?;
        exec_script(db.as_ref(), &narrow_ddl(DIMS)).await?;
        let mut done = 0_u64;
        while done < count {
            let batch_rows = (count - done).min(250);
            let mut statements = Vec::with_capacity((batch_rows * 2) as usize);
            for offset in 0..batch_rows {
                let row_index = global + done as usize + offset as usize;
                let plan = row_at(row_index);
                payload += payload_bytes(&plan);
                let insert = memory_insert(&plan, row_index, TAG);
                statements.push(Statement::new(
                    NARROW_MEMORIES_INSERT,
                    vec![
                        Value::Text(insert.id.clone()),
                        Value::Text(insert.content.clone()),
                        Value::Text(insert.container_tag.clone()),
                        value_or_null(&insert.tags),
                        value_or_null(&insert.memory_type),
                        Value::Integer(insert.created_at),
                        Value::Integer(insert.updated_at),
                        value_or_null(&insert.metadata),
                        value_or_null(&insert.session_id),
                        value_or_null(&insert.display_name),
                        value_or_null(&insert.user_name),
                        value_or_null(&insert.user_email),
                        value_or_null(&insert.project_path),
                        value_or_null(&insert.project_name),
                        value_or_null(&insert.git_repo_url),
                        Value::Integer(0),
                    ],
                ));
                let vector_json = vector::vector_to_text(&insert.vector)?;
                match &insert.tags_vector {
                    Some(tags_vector) => statements.push(Statement::new(
                        NARROW_VECTORS_INSERT_WITH_TAGS,
                        vec![
                            Value::Text(insert.id.clone()),
                            Value::Text(insert.container_tag.clone()),
                            Value::Text(vector_json),
                            Value::Text(vector::vector_to_text(tags_vector)?),
                        ],
                    )),
                    None => statements.push(Statement::new(
                        NARROW_VECTORS_INSERT_WITHOUT_TAGS,
                        vec![
                            Value::Text(insert.id.clone()),
                            Value::Text(insert.container_tag.clone()),
                            Value::Text(vector_json),
                        ],
                    )),
                }
            }
            db.batch(statements, BatchMode::Write).await?;
            done += batch_rows;
        }
        let count_read = shard_facts::sync_count(db.as_ref()).await?;
        if count_read != count {
            return Err(HarnessError::Env(format!(
                "narrow shard {index} holds {count_read} rows, expected {count}"
            )));
        }
        let _ = db.query("PRAGMA wal_checkpoint(TRUNCATE)", vec![]).await?;
        manager
            .register_existing_shard(
                Scope::Project,
                SCOPE_HASH,
                index,
                &path,
                count_read,
                index + 1 == shards,
            )
            .await?;
        remaining -= count;
        global += count as usize;
    }
    verify_registry(&manager, rows, cell.cap).await?;
    connections.close_all().await?;
    manager.reset()?;
    checkpoint_metadata(root, cell.cap).await?;
    let shard_bytes = shard_file_bytes(root)?;
    Ok(RootFacts {
        root: root.to_path_buf(),
        rows,
        shards: u64::from(shards),
        payload_bytes: payload,
        shard_bytes,
    })
}

async fn create_inline_root(cell: &Cell, rows: u64) -> Result<RootFacts, HarnessError> {
    let root = measurement_root(cell.name)?;
    let seed = row_seed();
    seed_inline_root(&root, cell, rows, move |index| generated_row(seed, index)).await
}

async fn create_narrow_root(cell: &Cell, rows: u64) -> Result<RootFacts, HarnessError> {
    let root = measurement_root(cell.name)?;
    let seed = row_seed();
    seed_narrow_root(&root, cell, rows, move |index| generated_row(seed, index)).await
}

async fn open_single_shard(config: &StoreConfig, root: &Path) -> DbHandle {
    let projects = root.join("projects");
    let mut dbs: Vec<PathBuf> = std::fs::read_dir(&projects)
        .expect("projects dir")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("db"))
        .collect();
    dbs.sort();
    assert_eq!(
        dbs.len(),
        1,
        "expected exactly one shard in {}",
        root.display()
    );
    open_db_at(config, &dbs[0]).await.expect("open shard")
}

fn normalized(values: &[f32]) -> Vec<f64> {
    let norm = values
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>()
        .sqrt();
    values
        .iter()
        .map(|value| f64::from(*value) / norm)
        .collect()
}

fn with_cosine(unit: &[f64], cosine: f64) -> Vec<f32> {
    let mut orthogonal = vec![0.0_f64; unit.len()];
    orthogonal[0] = 1.0;
    let dot: f64 = unit
        .iter()
        .zip(&orthogonal)
        .map(|(left, right)| left * right)
        .sum();
    for (value, unit_value) in orthogonal.iter_mut().zip(unit) {
        *value -= dot * unit_value;
    }
    let norm = orthogonal
        .iter()
        .map(|value| value * value)
        .sum::<f64>()
        .sqrt();
    let sine = (1.0 - cosine * cosine).max(0.0).sqrt();
    unit.iter()
        .zip(&orthogonal)
        .map(|(unit_value, orthogonal_value)| {
            (cosine * unit_value + sine * orthogonal_value / norm) as f32
        })
        .collect()
}

/// One crafted parity-fixture row: index, vector, tags vector, content, tags.
type ParityRow<'a> = (usize, &'a Vec<f32>, Option<&'a Vec<f32>>, &'a str, &'a str);

async fn rewrite_parity_fixture(
    root: &Path,
    layout: Layout,
    qvec: &[f32],
) -> Result<(), HarnessError> {
    let config = store_config_for(root, 50_000);
    let db = open_single_shard(&config, root).await;
    let unit = normalized(qvec);
    let top_one = with_cosine(&unit, 0.9999);
    let top_two = with_cosine(&unit, 0.9998);
    let pair = with_cosine(&unit, 0.99);
    let fifth = with_cosine(&unit, 0.98);
    let rows: [ParityRow<'_>; 5] = [
        (
            0,
            &top_one,
            Some(&top_one),
            "database hardening benchmark top one",
            "w8,top",
        ),
        (
            1,
            &top_two,
            Some(&top_two),
            "database hardening benchmark top two",
            "w8,top",
        ),
        (
            2,
            &pair,
            Some(&pair),
            "database hardening benchmark duplicate pair",
            "w8,dup",
        ),
        (
            3,
            &pair,
            Some(&pair),
            "database hardening benchmark duplicate pair",
            "w8,dup",
        ),
        (
            4,
            &fifth,
            None,
            "database hardening benchmark null tags",
            "w8,nulltags",
        ),
    ];
    for (index, vector, tags_vector, content, tags) in rows {
        let id = format!("w8-{index:07}");
        let vector_json = vector::vector_to_text(vector)?;
        let tags_vector_json = match tags_vector {
            Some(values) => Some(vector::vector_to_text(values)?),
            None => None,
        };
        match layout {
            Layout::Inline => match tags_vector_json {
                Some(tags_vector_json) => {
                    db.execute(
                        "UPDATE memories SET content = ?, tags = ?, vector = vector32(?), tags_vector = vector32(?) WHERE id = ?",
                        vec![
                            Value::Text(content.to_string()),
                            Value::Text(tags.to_string()),
                            Value::Text(vector_json),
                            Value::Text(tags_vector_json),
                            Value::Text(id),
                        ],
                    )
                    .await?;
                }
                None => {
                    db.execute(
                        "UPDATE memories SET content = ?, tags = ?, vector = vector32(?), tags_vector = NULL WHERE id = ?",
                        vec![
                            Value::Text(content.to_string()),
                            Value::Text(tags.to_string()),
                            Value::Text(vector_json),
                            Value::Text(id),
                        ],
                    )
                    .await?;
                }
            },
            Layout::Narrow => {
                db.execute(
                    "UPDATE memories SET content = ?, tags = ? WHERE id = ?",
                    vec![
                        Value::Text(content.to_string()),
                        Value::Text(tags.to_string()),
                        Value::Text(id.clone()),
                    ],
                )
                .await?;
                match tags_vector_json {
                    Some(tags_vector_json) => {
                        db.execute(
                            "UPDATE memory_vectors SET vector = vector32(?), tags_vector = vector32(?) WHERE id = ?",
                            vec![
                                Value::Text(vector_json),
                                Value::Text(tags_vector_json),
                                Value::Text(id),
                            ],
                        )
                        .await?;
                    }
                    None => {
                        db.execute(
                            "UPDATE memory_vectors SET vector = vector32(?), tags_vector = NULL WHERE id = ?",
                            vec![Value::Text(vector_json), Value::Text(id)],
                        )
                        .await?;
                    }
                }
            }
        }
    }
    let _ = db.query("PRAGMA wal_checkpoint(TRUNCATE)", vec![]).await?;
    Ok(())
}

fn similarity_map(results: &[SearchResult]) -> HashMap<String, f64> {
    results
        .iter()
        .map(|result| (result.id.clone(), result.similarity))
        .collect()
}

fn merge_ordered_ids(results: &[SearchResult]) -> Vec<String> {
    let mut ordered = results.to_vec();
    ordered.sort_by(|a, b| {
        b.similarity
            .partial_cmp(&a.similarity)
            .unwrap_or(Ordering::Equal)
            .then_with(|| b.created_at.cmp(&a.created_at))
            .then_with(|| a.id.cmp(&b.id))
    });
    ordered.into_iter().map(|result| result.id).collect()
}

// ---------------------------------------------------------------------------
// Step 5 — the root guard and the cold-read invariant
// ---------------------------------------------------------------------------

/// Refuse a forbidden or non-throwaway root **before** any filesystem mutation.
fn prepare_root(path: &Path, forbidden: &Path) -> Result<(), HarnessError> {
    if path == forbidden {
        return Err(HarnessError::ForbiddenRoot(path.to_path_buf()));
    }
    if path.join("metadata.db").exists() {
        return Err(HarnessError::ExistingRoot(path.to_path_buf()));
    }
    std::fs::create_dir_all(path)?;
    Ok(())
}

/// A cold sample is honest only when at least half the payload was read from
/// storage (a warm sample masquerading as cold fails this).
fn cold_is_verified(read_bytes: u64, payload_bytes: u64) -> bool {
    read_bytes >= payload_bytes / 2
}

#[test]
fn test_prepare_root_refuses_the_forbidden_path() {
    let fresh = temp_path("w8-s5-fresh");
    assert!(prepare_root(&fresh, &fresh).is_err());
    assert!(
        !fresh.exists(),
        "a refusal must not create the directory (mutant: create_dir_all first)"
    );
    println!("prepare_root refused fresh_tmp == forbidden: {fresh:?}");

    let configured = configured_root();
    let metadata_path = configured.join("metadata.db");
    let before_metadata = std::fs::metadata(&metadata_path)
        .ok()
        .map(|metadata| (metadata.len(), metadata.modified().ok()));
    let before_entries = std::fs::read_dir(&configured)
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert!(
        prepare_root(&configured, &configured).is_err(),
        "the configured root must be refused"
    );
    let after_metadata = std::fs::metadata(&metadata_path)
        .ok()
        .map(|metadata| (metadata.len(), metadata.modified().ok()));
    let after_entries = std::fs::read_dir(&configured)
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(
        before_metadata, after_metadata,
        "configured metadata.db touched"
    );
    assert_eq!(
        before_entries, after_entries,
        "configured root entries touched"
    );
    println!("prepare_root refused the configured root: {configured:?}");

    let tmp = temp_root("w8-s5-ok");
    assert!(
        prepare_root(&tmp, &configured).is_ok(),
        "the forbidden argument is a guard, never read"
    );
    assert!(tmp.exists());

    let other = temp_root("w8-s5-other");
    assert!(prepare_root(&tmp, &other).is_ok());

    let preexisting = temp_root("w8-s5-existing");
    std::fs::write(preexisting.join("metadata.db"), b"not a real db").expect("dummy metadata.db");
    assert!(prepare_root(&preexisting, &configured).is_err());
    assert!(prepare_root(&preexisting, &other).is_err());

    assert!(
        prepare_root(&tmp, &configured).is_ok(),
        "a second call on a prepared directory is Ok"
    );
}

#[test]
fn test_cold_sample_without_reads_is_rejected() {
    assert!(!cold_is_verified(0, 1_000));
    assert!(cold_is_verified(600, 1_000));
    assert!(!cold_is_verified(499, 1_000));
    assert!(cold_is_verified(0, 0));
}

// ---------------------------------------------------------------------------
// Step 6 — the four ignored measurement entries
// ---------------------------------------------------------------------------

const EXACT_DIAGNOSTIC_REPS: usize = 5;
const NO_MATCH_DIAGNOSTIC_REPS: usize = 5;
const NO_MATCH_QUERY: &str = "zzqxjv nohitnonsense";

#[derive(Debug, Clone, Default)]
struct Timing {
    open_us: f64,
    facts_us: f64,
    scan_us: f64,
    hydrate_us: f64,
    search_us: f64,
    total_us: f64,
}

#[derive(Debug, Clone)]
struct SeedFacts {
    rows: u64,
    shards: u64,
    payload_bytes: u64,
    shard_bytes: u64,
    stamp: String,
}

fn env_string(name: &str) -> Result<String, HarnessError> {
    std::env::var(name).map_err(|_| HarnessError::Env(format!("{name} is not set")))
}

fn env_u64(name: &str) -> Result<u64, HarnessError> {
    env_string(name)?
        .parse()
        .map_err(|_| HarnessError::Env(format!("{name} must be an unsigned integer")))
}

fn out_dir() -> Result<PathBuf, HarnessError> {
    Ok(PathBuf::from(env_string("W8_OUT")?))
}

fn required_root() -> Result<PathBuf, HarnessError> {
    let path = PathBuf::from(env_string("W8_ROOT")?);
    if path == configured_root() {
        return Err(HarnessError::ForbiddenRoot(path));
    }
    Ok(path)
}

fn env_stamp() -> String {
    std::env::var("W8_STAMP").unwrap_or_else(|_| "unstamped".to_string())
}

fn cell_from_env() -> Result<Cell, HarnessError> {
    let name: &'static str = Box::leak(env_string("W8_CELL")?.into_boxed_str());
    let rows = env_u64("W8_ROWS")?;
    let cap = env_u64("W8_CAP")?;
    let cap =
        u32::try_from(cap).map_err(|_| HarnessError::Env("W8_CAP does not fit u32".to_string()))?;
    let layout = match env_string("W8_LAYOUT")?.as_str() {
        "inline" => Layout::Inline,
        "narrow" => Layout::Narrow,
        other => {
            return Err(HarnessError::Env(format!(
                "W8_LAYOUT must be inline or narrow, got {other}"
            )));
        }
    };
    Ok(Cell {
        name,
        rows,
        cap,
        layout,
        multiplier: 0,
    })
}

fn elapsed_us(started: std::time::Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1_000_000.0
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

async fn scope_wide_search(
    manager: &ShardManager,
    scope: Scope,
    hash: &str,
    tag: &str,
    qvec: &[f32],
    qtext: &str,
    layout: Layout,
) -> Result<Timing, StoreError> {
    let started = std::time::Instant::now();
    let mut timing = Timing::default();
    let shards = manager.get_all_shards(scope, Some(hash)).await?;
    let mut merged: Vec<SearchResult> = Vec::new();
    for shard in shards {
        let open_started = std::time::Instant::now();
        let db = manager.open_shard(&shard.db_path).await?;
        timing.open_us += elapsed_us(open_started);

        let facts_started = std::time::Instant::now();
        let facts =
            shard_facts::model_facts_cached(&db, manager.model_facts_cache(), &shard.db_path)
                .await?;
        let _authority = shard_facts::judge_model_facts(
            &facts,
            manager.embedding_identity(),
            &manager.config().embedding_model,
        );
        timing.facts_us += elapsed_us(facts_started);

        match layout {
            Layout::Inline => {
                let search_started = std::time::Instant::now();
                let results =
                    vector::search_in_shard(db.as_ref(), qvec, tag, 40, Some(qtext)).await?;
                timing.search_us += elapsed_us(search_started);
                merged.extend(results);
            }
            Layout::Narrow => {
                let (results, split) =
                    narrow_search_in_shard(db.as_ref(), qvec, tag, 40, Some(qtext)).await?;
                timing.scan_us += split.scan_us;
                timing.hydrate_us += split.hydrate_us;
                timing.search_us += split.scan_us + split.hydrate_us;
                merged.extend(results);
            }
        }
    }

    let mut seen = HashSet::new();
    merged.retain(|result| seen.insert(result.id.clone()));
    merged.sort_by(|a, b| {
        b.similarity
            .partial_cmp(&a.similarity)
            .unwrap_or(Ordering::Equal)
            .then_with(|| b.created_at.cmp(&a.created_at))
            .then_with(|| a.id.cmp(&b.id))
    });
    merged.truncate(20);

    timing.total_us = elapsed_us(started);
    Ok(timing)
}

async fn exact_only_scan(
    manager: &ShardManager,
    scope: Scope,
    hash: &str,
    tag: &str,
    qvec: &[f32],
    layout: Layout,
) -> Result<f64, StoreError> {
    let started = std::time::Instant::now();
    let query_json = vector::vector_to_text(qvec)?;
    let shards = manager.get_all_shards(scope, Some(hash)).await?;
    for shard in shards {
        let db = manager.open_shard(&shard.db_path).await?;
        match layout {
            Layout::Inline => {
                let _ = vector::exact_scan_kind(
                    db.as_ref(),
                    &query_json,
                    80,
                    tag,
                    vector::VectorColumn::Content,
                )
                .await?;
                let _ = vector::exact_scan_kind(
                    db.as_ref(),
                    &query_json,
                    80,
                    tag,
                    vector::VectorColumn::Tags,
                )
                .await?;
            }
            Layout::Narrow => {
                let _ =
                    narrow_scan(db.as_ref(), NARROW_CONTENT_SCAN_SQL, &query_json, tag, 80).await?;
                let _ =
                    narrow_scan(db.as_ref(), NARROW_TAGS_SCAN_SQL, &query_json, tag, 80).await?;
            }
        }
    }
    Ok(elapsed_us(started))
}

fn collect_files(root: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, files);
        } else {
            files.push(path);
        }
    }
}

/// `posix_fadvise(DONTNEED)` over every file in a closed-handle process.
fn fadvise_root(root: &Path) -> bool {
    let mut files = Vec::new();
    collect_files(root, &mut files);
    let mut advised = true;
    for path in files {
        match std::fs::File::open(&path) {
            Ok(file) => {
                use std::os::unix::io::AsRawFd;
                // SAFETY: the fd is valid for the life of `file`; the advice
                // only drops clean page-cache pages, it never changes data.
                let result = unsafe {
                    libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED)
                };
                if result != 0 {
                    eprintln!(
                        "W8 COLD_UNAVAILABLE posix_fadvise({}) rc={result}",
                        path.display()
                    );
                    advised = false;
                }
            }
            Err(error) => {
                eprintln!("W8 COLD_UNAVAILABLE open({}) {error}", path.display());
                advised = false;
            }
        }
    }
    advised
}

fn process_read_bytes() -> u64 {
    let Ok(text) = std::fs::read_to_string("/proc/self/io") else {
        return 0;
    };
    text.lines()
        .find_map(|line| line.strip_prefix("read_bytes:"))
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(0)
}

fn append_jsonl(path: &Path, line: &str) -> Result<(), HarnessError> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{line}")?;
    Ok(())
}

fn parse_json(line: &str) -> Result<serde_json::Value, HarnessError> {
    serde_json::from_str(line)
        .map_err(|error| HarnessError::Env(format!("invalid JSONL line: {error}")))
}

fn read_seed_facts(out: &Path, cell_name: &str) -> Result<SeedFacts, HarnessError> {
    let path = out.join(format!("{cell_name}.jsonl"));
    let text = std::fs::read_to_string(&path)?;
    for line in text.lines() {
        let value = parse_json(line)?;
        if value.get("kind").and_then(|kind| kind.as_str()) == Some("seed") {
            return Ok(SeedFacts {
                rows: value.get("rows").and_then(|v| v.as_u64()).unwrap_or(0),
                shards: value.get("shards").and_then(|v| v.as_u64()).unwrap_or(0),
                payload_bytes: value
                    .get("payload_bytes")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                shard_bytes: value
                    .get("shard_bytes")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0),
                stamp: value
                    .get("stamp")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unstamped")
                    .to_string(),
            });
        }
    }
    Err(HarnessError::Env(format!(
        "no seed line in {}",
        path.display()
    )))
}

fn sample_line(
    kind: &str,
    cell: &Cell,
    facts: &SeedFacts,
    timing: &Timing,
    read_bytes: u64,
    verified: Option<bool>,
) -> String {
    let verified_field = verified
        .map(|value| format!(r#","cold_verified":{value}"#))
        .unwrap_or_default();
    format!(
        r#"{{"kind":"{kind}","cell":"{}","elapsed_us":{:.3},"read_bytes":{},"rows":{},"shards":{},"shard_bytes":{},"open_us":{:.3},"facts_us":{:.3},"scan_us":{:.3},"hydrate_us":{:.3},"search_us":{:.3},"stamp":"{}"{verified_field}}}"#,
        cell.name,
        timing.total_us,
        read_bytes,
        facts.rows,
        facts.shards,
        facts.shard_bytes,
        timing.open_us,
        timing.facts_us,
        timing.scan_us,
        timing.hydrate_us,
        timing.search_us,
        env_stamp(),
    )
}

#[tokio::test]
#[ignore = "w8 measurement: seed a cell"]
async fn w8_seed_cell() {
    let cell = cell_from_env().expect("W8 cell env");
    let out = out_dir().expect("W8_OUT");
    std::fs::create_dir_all(&out).expect("W8_OUT dir");
    let _root = required_root().expect("W8_ROOT");
    let facts = match cell.layout {
        Layout::Inline => create_inline_root(&cell, cell.rows).await,
        Layout::Narrow => create_narrow_root(&cell, cell.rows).await,
    }
    .expect("seed cell root");
    let line = format!(
        r#"{{"kind":"seed","cell":"{}","layout":"{}","rows":{},"shards":{},"cap":{},"payload_bytes":{},"shard_bytes":{},"stamp":"{}"}}"#,
        cell.name,
        match cell.layout {
            Layout::Inline => "inline",
            Layout::Narrow => "narrow",
        },
        facts.rows,
        facts.shards,
        cell.cap,
        facts.payload_bytes,
        facts.shard_bytes,
        env_stamp(),
    );
    append_jsonl(&out.join(format!("{}.jsonl", cell.name)), &line).expect("seed jsonl");
    println!(
        "W8 SEED {} rows={} shards={} shard_bytes={} payload_bytes={}",
        cell.name, facts.rows, facts.shards, facts.shard_bytes, facts.payload_bytes
    );
}

#[tokio::test]
#[ignore = "w8 measurement: one cold sample"]
async fn w8_sample_cold() {
    let cell = cell_from_env().expect("W8 cell env");
    let out = out_dir().expect("W8_OUT");
    let facts = read_seed_facts(&out, cell.name).expect("seed facts");
    let root = required_root().expect("W8_ROOT");
    let (manager, connections) = manager_for(&root, cell.cap);
    let qvec = query_vector(query_seed());
    let advised = fadvise_root(&root);
    let read_before = process_read_bytes();
    let timing = scope_wide_search(
        &manager,
        Scope::Project,
        SCOPE_HASH,
        TAG,
        &qvec,
        QUERY_TEXT,
        cell.layout,
    )
    .await
    .expect("cold scope-wide search");
    let read_after = process_read_bytes();
    connections.close_all().await.expect("close cold handles");
    manager.reset().expect("reset cold manager");
    let read_bytes = read_after.saturating_sub(read_before);
    let verified = advised && cold_is_verified(read_bytes, facts.payload_bytes);
    let line = sample_line("cold", &cell, &facts, &timing, read_bytes, Some(verified));
    append_jsonl(&out.join(format!("{}.jsonl", cell.name)), &line).expect("cold jsonl");
    println!(
        "W8 COLD {} elapsed_us={:.3} read_bytes={} payload_bytes={} verified={}",
        cell.name, timing.total_us, read_bytes, facts.payload_bytes, verified
    );
}

#[tokio::test]
#[ignore = "w8 measurement: warm series"]
async fn w8_warm_series() {
    let cell = cell_from_env().expect("W8 cell env");
    let out = out_dir().expect("W8_OUT");
    let facts = read_seed_facts(&out, cell.name).expect("seed facts");
    let root = required_root().expect("W8_ROOT");
    let (manager, connections) = manager_for(&root, cell.cap);
    let qvec = query_vector(query_seed());
    let reps = env_u64("W8_REPS").unwrap_or(20) as usize;

    for _ in 0..3 {
        scope_wide_search(
            &manager,
            Scope::Project,
            SCOPE_HASH,
            TAG,
            &qvec,
            QUERY_TEXT,
            cell.layout,
        )
        .await
        .expect("warm-up");
    }
    for _ in 0..reps {
        let read_before = process_read_bytes();
        let timing = scope_wide_search(
            &manager,
            Scope::Project,
            SCOPE_HASH,
            TAG,
            &qvec,
            QUERY_TEXT,
            cell.layout,
        )
        .await
        .expect("warm sample");
        let read_bytes = process_read_bytes().saturating_sub(read_before);
        let line = sample_line("warm", &cell, &facts, &timing, read_bytes, None);
        append_jsonl(&out.join(format!("{}.jsonl", cell.name)), &line).expect("warm jsonl");
    }
    for _ in 0..EXACT_DIAGNOSTIC_REPS {
        let total_us = exact_only_scan(
            &manager,
            Scope::Project,
            SCOPE_HASH,
            TAG,
            &qvec,
            cell.layout,
        )
        .await
        .expect("exact-only diagnostic");
        let timing = Timing {
            total_us,
            ..Timing::default()
        };
        let line = sample_line("exact", &cell, &facts, &timing, 0, None);
        append_jsonl(&out.join(format!("{}.jsonl", cell.name)), &line).expect("exact jsonl");
    }
    for _ in 0..NO_MATCH_DIAGNOSTIC_REPS {
        let timing = scope_wide_search(
            &manager,
            Scope::Project,
            SCOPE_HASH,
            TAG,
            &qvec,
            NO_MATCH_QUERY,
            cell.layout,
        )
        .await
        .expect("no-match diagnostic");
        let line = sample_line("nomatch", &cell, &facts, &timing, 0, None);
        append_jsonl(&out.join(format!("{}.jsonl", cell.name)), &line).expect("nomatch jsonl");
    }
    connections.close_all().await.expect("close warm handles");
    manager.reset().expect("reset warm manager");
    println!("W8 WARM {} reps={reps}", cell.name);
}

#[tokio::test]
#[ignore = "w8 measurement: aggregate one cell"]
async fn w8_aggregate() {
    let cell = cell_from_env().expect("W8 cell env");
    let out = out_dir().expect("W8_OUT");
    let facts = read_seed_facts(&out, cell.name).expect("seed facts");
    let path = out.join(format!("{}.jsonl", cell.name));
    let text = std::fs::read_to_string(&path).expect("cell jsonl");
    let mut cold_samples: Vec<Sample> = Vec::new();
    let mut warm_samples: Vec<Sample> = Vec::new();
    let mut exact_samples: Vec<Sample> = Vec::new();
    let mut nomatch_samples: Vec<Sample> = Vec::new();
    for line in text.lines() {
        let value = parse_json(line).expect("cell jsonl line");
        let kind = value
            .get("kind")
            .and_then(|kind| kind.as_str())
            .unwrap_or_default();
        let sample = Sample {
            elapsed_us: value
                .get("elapsed_us")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
            read_bytes: value
                .get("read_bytes")
                .and_then(|v| v.as_u64())
                .unwrap_or(0),
            scan_us: value.get("scan_us").and_then(|v| v.as_f64()).unwrap_or(0.0),
            hydrate_us: value
                .get("hydrate_us")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
            search_us: value
                .get("search_us")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
        };
        match kind {
            "cold" => cold_samples.push(sample),
            "warm" => warm_samples.push(sample),
            "exact" => exact_samples.push(sample),
            "nomatch" => nomatch_samples.push(sample),
            _ => {}
        }
    }
    let cold = summarize(&cold_samples);
    let warm = summarize(&warm_samples);
    let cold_read_mean = mean(
        &cold_samples
            .iter()
            .map(|sample| sample.read_bytes as f64)
            .collect::<Vec<_>>(),
    );
    let read_ratio = if facts.payload_bytes > 0 {
        cold_read_mean / facts.payload_bytes as f64
    } else {
        0.0
    };
    let warm_scan = mean(
        &warm_samples
            .iter()
            .map(|sample| sample.scan_us)
            .collect::<Vec<_>>(),
    );
    let warm_hydrate = mean(
        &warm_samples
            .iter()
            .map(|sample| sample.hydrate_us)
            .collect::<Vec<_>>(),
    );
    let warm_search = mean(
        &warm_samples
            .iter()
            .map(|sample| sample.search_us)
            .collect::<Vec<_>>(),
    );
    let exact_mean = mean(
        &exact_samples
            .iter()
            .map(|sample| sample.elapsed_us)
            .collect::<Vec<_>>(),
    );
    let exact_per_row = if facts.rows > 0 {
        exact_mean / facts.rows as f64
    } else {
        0.0
    };
    let nomatch_mean = mean(
        &nomatch_samples
            .iter()
            .map(|sample| sample.elapsed_us)
            .collect::<Vec<_>>(),
    );
    let summary = format!(
        concat!(
            "{{\n",
            "  \"cell\": \"{}\",\n",
            "  \"layout\": \"{}\",\n",
            "  \"rows\": {},\n",
            "  \"shards\": {},\n",
            "  \"cap\": {},\n",
            "  \"stamp\": \"{}\",\n",
            "  \"payload_bytes\": {},\n",
            "  \"shard_bytes\": {},\n",
            "  \"read_bytes\": {},\n",
            "  \"scan_us\": {:.3},\n",
            "  \"hydrate_us\": {:.3},\n",
            "  \"search_us\": {:.3},\n",
            "  \"cold\": {{\"n\": {}, \"min\": {:.3}, \"p50\": {:.3}, \"p95\": {:.3}, \"max\": {:.3}, \"mean\": {:.3}, \"read_bytes_mean\": {:.3}, \"read_bytes_ratio\": {:.6}}},\n",
            "  \"warm\": {{\"n\": {}, \"min\": {:.3}, \"p50\": {:.3}, \"p95\": {:.3}, \"max\": {:.3}, \"mean\": {:.3}}},\n",
            "  \"exact_only\": {{\"n\": {}, \"mean_us\": {:.3}, \"us_per_row\": {:.6}}},\n",
            "  \"no_match\": {{\"n\": {}, \"mean_us\": {:.3}}}\n",
            "}}\n"
        ),
        cell.name,
        match cell.layout {
            Layout::Inline => "inline",
            Layout::Narrow => "narrow",
        },
        facts.rows,
        facts.shards,
        cell.cap,
        facts.stamp,
        facts.payload_bytes,
        facts.shard_bytes,
        cold_read_mean as u64,
        warm_scan,
        warm_hydrate,
        warm_search,
        cold.n,
        cold.min,
        cold.p50,
        cold.p95,
        cold.max,
        cold.mean,
        cold_read_mean,
        read_ratio,
        warm.n,
        warm.min,
        warm.p50,
        warm.p95,
        warm.max,
        warm.mean,
        exact_samples.len(),
        exact_mean,
        exact_per_row,
        nomatch_samples.len(),
        nomatch_mean,
    );
    std::fs::write(out.join(format!("{}.summary.json", cell.name)), summary)
        .expect("write summary");
    println!(
        "W8 AGG {} cold_n={} cold_p50_us={:.3} cold_p95_us={:.3} warm_n={} warm_p95_us={:.3} read_ratio={:.3}",
        cell.name, cold.n, cold.p50, cold.p95, warm.n, warm.p95, read_ratio
    );
}
