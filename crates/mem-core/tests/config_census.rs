use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use mem_core::config::FileConfig;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CensusRow {
    pub struct_name: String,
    pub field_name: String,
    pub status: String,
    pub disposition: String,
    pub upstream_ref: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CensusError {
    DuplicateRow {
        struct_name: String,
        field_name: String,
    },
    EmptyDisposition {
        struct_name: String,
        field_name: String,
    },
    MalformedRow(String),
    MissingField {
        struct_name: String,
        field_name: String,
    },
    WrongStruct {
        expected: String,
        actual: String,
        field_name: String,
    },
    FileNotFound(String),
}

impl std::fmt::Display for CensusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateRow {
                struct_name,
                field_name,
            } => {
                write!(f, "duplicate census row: {struct_name}.{field_name}")
            }
            Self::EmptyDisposition {
                struct_name,
                field_name,
            } => {
                write!(f, "empty disposition for {struct_name}.{field_name}")
            }
            Self::MalformedRow(line) => write!(f, "malformed table row: {line}"),
            Self::MissingField {
                struct_name,
                field_name,
            } => {
                write!(
                    f,
                    "schema field missing from census: {struct_name}.{field_name}"
                )
            }
            Self::WrongStruct {
                expected,
                actual,
                field_name,
            } => {
                write!(
                    f,
                    "wrong struct for field {field_name}: expected {expected}, got {actual}"
                )
            }
            Self::FileNotFound(path) => write!(f, "file not found: {path}"),
        }
    }
}

impl std::error::Error for CensusError {}

pub fn parse_census_table(markdown: &str) -> Result<Vec<CensusRow>, CensusError> {
    let mut rows = Vec::new();
    let mut seen = BTreeSet::new();

    for line in markdown.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('|') || !trimmed.ends_with('|') {
            continue;
        }

        let cells: Vec<&str> = trimmed.split('|').map(str::trim).collect();

        // Row format: | Struct | Field | Status | Reader / Disposition | Upstream Reference |
        // Splitting by '|' yields leading and trailing empty entries, so 7 cells total.
        if cells.len() != 7 {
            continue;
        }

        let struct_name = cells[1];
        let field_name = cells[2];
        let status = cells[3];
        let disposition = cells[4];
        let upstream_ref = cells[5];

        // Skip table headers and separator dashes
        if struct_name.eq_ignore_ascii_case("struct")
            || struct_name.starts_with("---")
            || struct_name.starts_with(':')
        {
            continue;
        }

        if struct_name.is_empty() || field_name.is_empty() {
            return Err(CensusError::MalformedRow(line.to_string()));
        }

        if disposition.is_empty() {
            return Err(CensusError::EmptyDisposition {
                struct_name: struct_name.to_string(),
                field_name: field_name.to_string(),
            });
        }

        let key = (struct_name.to_string(), field_name.to_string());
        if !seen.insert(key.clone()) {
            return Err(CensusError::DuplicateRow {
                struct_name: key.0,
                field_name: key.1,
            });
        }

        rows.push(CensusRow {
            struct_name: struct_name.to_string(),
            field_name: field_name.to_string(),
            status: status.to_string(),
            disposition: disposition.to_string(),
            upstream_ref: upstream_ref.to_string(),
        });
    }

    Ok(rows)
}

pub fn extract_schema_fields(schema_source: &str) -> Vec<(String, String)> {
    let target_structs = [
        "Config",
        "MemoryScope",
        "Compaction",
        "ChatMessage",
        "Episodic",
        "FeatureFlags",
        "SoundMemory",
        "Promotion",
    ];

    let mut result = Vec::new();

    for struct_name in target_structs {
        let pattern = format!("pub struct {struct_name}");
        if let Some(pos) = schema_source.find(&pattern) {
            let after = &schema_source[pos + pattern.len()..];
            if let Some(open) = after.find('{') {
                let inside = &after[open + 1..];
                if let Some(close) = inside.find('}') {
                    let body = &inside[..close];
                    for line in body.lines() {
                        let trimmed = line.trim();
                        if let Some((field, _)) = trimmed
                            .strip_prefix("pub ")
                            .and_then(|rest| rest.split_once(':'))
                        {
                            let field = field.trim();
                            if !field.is_empty() && field != "inert_keys_present" {
                                result.push((struct_name.to_string(), field.to_string()));
                            }
                        }
                    }
                }
            }
        }
    }

    result
}

pub fn validate_census_against_schema(
    rows: &[CensusRow],
    schema_fields: &[(String, String)],
) -> Result<(), CensusError> {
    let census_map: BTreeMap<(&str, &str), &CensusRow> = rows
        .iter()
        .map(|r| ((r.struct_name.as_str(), r.field_name.as_str()), r))
        .collect();

    for (struct_name, field_name) in schema_fields {
        if !census_map.contains_key(&(struct_name.as_str(), field_name.as_str())) {
            if let Some(wrong) = rows.iter().find(|r| r.field_name == *field_name) {
                return Err(CensusError::WrongStruct {
                    expected: struct_name.clone(),
                    actual: wrong.struct_name.clone(),
                    field_name: field_name.clone(),
                });
            }
            return Err(CensusError::MissingField {
                struct_name: struct_name.clone(),
                field_name: field_name.clone(),
            });
        }
    }

    Ok(())
}

#[test]
fn test_census_parser_fails_when_field_only_in_prose() {
    let fixture = r#"
# Configuration Surface
Here we mention allow_unguarded_open in prose or comment, but omit it from the table.

| Struct | Field | Status | Reader / Disposition | Upstream Reference |
|---|---|---|---|---|
| Config | storage_path | live | mem-store/src/db.rs:25 | upstream/config.ts:10 |
"#;
    let rows = parse_census_table(fixture).expect("parse rows");
    let schema_fields = vec![
        ("Config".to_string(), "storage_path".to_string()),
        ("Config".to_string(), "allow_unguarded_open".to_string()),
    ];
    let err = validate_census_against_schema(&rows, &schema_fields).unwrap_err();
    assert_eq!(
        err,
        CensusError::MissingField {
            struct_name: "Config".to_string(),
            field_name: "allow_unguarded_open".to_string(),
        }
    );
}

#[test]
fn test_census_parser_fails_on_wrong_struct() {
    let fixture = r#"
| Struct | Field | Status | Reader / Disposition | Upstream Reference |
|---|---|---|---|---|
| Config | enabled | live | inject.rs:100 | index.ts:670 |
"#;
    let rows = parse_census_table(fixture).expect("parse rows");
    let schema_fields = vec![("ChatMessage".to_string(), "enabled".to_string())];
    let err = validate_census_against_schema(&rows, &schema_fields).unwrap_err();
    assert_eq!(
        err,
        CensusError::WrongStruct {
            expected: "ChatMessage".to_string(),
            actual: "Config".to_string(),
            field_name: "enabled".to_string(),
        }
    );
}

#[test]
fn test_census_parser_fails_on_duplicate_row() {
    let fixture = r#"
| Struct | Field | Status | Reader / Disposition | Upstream Reference |
|---|---|---|---|---|
| Config | storage_path | live | db.rs:10 | config.ts:1 |
| Config | storage_path | live | db.rs:20 | config.ts:2 |
"#;
    let err = parse_census_table(fixture).unwrap_err();
    assert_eq!(
        err,
        CensusError::DuplicateRow {
            struct_name: "Config".to_string(),
            field_name: "storage_path".to_string(),
        }
    );
}

#[test]
fn test_census_parser_fails_on_empty_disposition() {
    let fixture = r#"
| Struct | Field | Status | Reader / Disposition | Upstream Reference |
|---|---|---|---|---|
| Config | storage_path | live |   | config.ts:1 |
"#;
    let err = parse_census_table(fixture).unwrap_err();
    assert_eq!(
        err,
        CensusError::EmptyDisposition {
            struct_name: "Config".to_string(),
            field_name: "storage_path".to_string(),
        }
    );
}

#[test]
fn test_census_parser_fails_on_missing_field() {
    let fixture = r#"
| Struct | Field | Status | Reader / Disposition | Upstream Reference |
|---|---|---|---|---|
| Config | storage_path | live | db.rs:10 | config.ts:1 |
"#;
    let rows = parse_census_table(fixture).expect("parse rows");
    let schema_fields = vec![
        ("Config".to_string(), "storage_path".to_string()),
        ("Config".to_string(), "allow_unguarded_open".to_string()),
    ];
    let err = validate_census_against_schema(&rows, &schema_fields).unwrap_err();
    assert_eq!(
        err,
        CensusError::MissingField {
            struct_name: "Config".to_string(),
            field_name: "allow_unguarded_open".to_string(),
        }
    );
}

#[test]
fn test_census_covers_all_schema_fields() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let schema_path = manifest_dir.join("src/config/schema.rs");
    let schema_source = std::fs::read_to_string(&schema_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", schema_path.display()));

    let schema_fields = extract_schema_fields(&schema_source);
    assert_eq!(
        schema_fields.len(),
        102,
        "expected exactly 102 fields in schema (86 Config + 16 nested leaves)"
    );

    let doc_path = manifest_dir.join("../../docs/config-surface.md");
    if !doc_path.exists() {
        panic!("docs/config-surface.md not found at {}", doc_path.display());
    }
    let doc_content = std::fs::read_to_string(&doc_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", doc_path.display()));

    let rows = parse_census_table(&doc_content).expect("valid census table");
    assert!(
        rows.len() >= 102,
        "census table must contain at least 102 rows, got {}",
        rows.len()
    );

    validate_census_against_schema(&rows, &schema_fields)
        .expect("all schema fields present in census");
}

#[test]
fn test_inert_field_names_match_dead_census_rows() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let doc_path = manifest_dir.join("../../docs/config-surface.md");
    let doc_content = std::fs::read_to_string(&doc_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", doc_path.display()));

    let rows = parse_census_table(&doc_content).expect("valid census table");
    let dead_census_fields: BTreeSet<&str> = rows
        .iter()
        .filter(|r| r.status == "dead")
        .map(|r| r.field_name.as_str())
        .collect();

    let inert_fields: BTreeSet<&str> = FileConfig::inert_field_names().iter().copied().collect();

    assert_eq!(
        dead_census_fields.len(),
        20,
        "expected exactly 20 dead census fields in docs/config-surface.md"
    );
    assert_eq!(
        inert_fields, dead_census_fields,
        "FileConfig::inert_field_names() must match dead census rows 1:1"
    );
}
