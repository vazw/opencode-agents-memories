#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::wasm_bindgen_test;
use web_ui::core_screens::{MemoryItem, TagsPage};
use web_ui::export_import::{
    EXPLORER_EXPORT_KIND, ExportInputs, IMPORT_APPLY_SUPPORTED, ImportError, ImportFormat,
    MAX_IMPORT_BYTES, check_import_len, export_document, export_download_disabled,
    export_file_name, export_pages_spec, export_preview_text, exportable_memories,
    format_exported_at, import_preview, should_continue,
};

#[wasm_bindgen_test]
fn test_export_document_has_the_explorer_shape() {
    let inputs = ExportInputs {
        memories: &[],
        tags: &TagsPage {
            project: vec![],
            user: vec![],
            memory_tags: vec![],
            memory_tag_total: 0,
        },
        profile: None,
        truncated: false,
    };
    let doc = export_document(&inputs, "20260101T000000Z");
    assert_eq!(doc["kind"], EXPLORER_EXPORT_KIND);
    assert_eq!(doc["schemaVersion"], 1);
    assert_eq!(doc["exportedAt"], "20260101T000000Z");
    assert!(doc.get("source").is_some());
    assert!(doc.get("memories").unwrap().is_array());
    assert!(doc.get("tags").is_some());
    assert!(doc.get("profile").is_some());
    assert_eq!(doc["truncated"], false);
}

#[wasm_bindgen_test]
fn test_export_document_excludes_prompt_rows() {
    let mem = MemoryItem {
        kind: Some("prompt".to_string()),
        id: "p1".to_string(),
        content: "prompt content".to_string(),
        memory_type: None,
        tags: vec![],
        created_at: "".to_string(),
        is_pinned: false,
        session_id: None,
        display_name: None,
    };
    let inputs = ExportInputs {
        memories: std::slice::from_ref(&mem),
        tags: &TagsPage {
            project: vec![],
            user: vec![],
            memory_tags: vec![],
            memory_tag_total: 0,
        },
        profile: None,
        truncated: false,
    };
    let doc = export_document(&inputs, "20260101T000000Z");
    assert_eq!(doc["memories"].as_array().unwrap().len(), 0);
    // The preview reads the very same row set, so it counts what the document
    // would contain — not every fetched row.
    assert_eq!(exportable_memories(&[mem]).len(), 0);
}

#[wasm_bindgen_test]
fn test_export_document_records_truncation() {
    let inputs = ExportInputs {
        memories: &[],
        tags: &TagsPage {
            project: vec![],
            user: vec![],
            memory_tags: vec![],
            memory_tag_total: 0,
        },
        profile: None,
        truncated: true,
    };
    let doc = export_document(&inputs, "20260101T000000Z");
    assert_eq!(doc["truncated"], true);
}

#[wasm_bindgen_test]
fn test_export_file_name_is_utc_stamped() {
    assert_eq!(
        export_file_name("20261021T114500Z"),
        "opencode-mem-export-20261021T114500Z.json"
    );
    // The stamp is derived from the clock, never a literal: the pure formatter
    // turns the wasm clock's epoch millis into the same UTC shape.
    assert_eq!(format_exported_at(1_792_583_100_000), "20261021T114500Z");
    assert_eq!(format_exported_at(1_767_225_600_000), "20260101T000000Z");
}

#[wasm_bindgen_test]
fn test_export_pages_spec_requests_the_readable_tier() {
    let spec = export_pages_spec(2, 50);
    assert_eq!(spec.path, "/api/memories");
    // query contains page, pageSize, includePrompts=false
    let q: std::collections::BTreeMap<&str, &str> = spec
        .query
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    assert_eq!(q.get("page"), Some(&"2"));
    assert_eq!(q.get("pageSize"), Some(&"50"));
    assert_eq!(q.get("includePrompts"), Some(&"false"));
}

#[wasm_bindgen_test]
fn test_should_continue_stops_on_a_short_page() {
    assert!(!should_continue(1, 200, 50, 1));
    assert!(should_continue(1, 200, 200, 3));
    assert!(!should_continue(25, 200, 200, 30));
}

#[wasm_bindgen_test]
fn test_import_preview_counts_an_explorer_export() {
    let mem = MemoryItem {
        kind: Some("memory".to_string()),
        id: "m1".to_string(),
        content: "x".to_string(),
        memory_type: Some("note".to_string()),
        tags: vec![],
        created_at: "".to_string(),
        is_pinned: false,
        session_id: None,
        display_name: None,
    };
    let inputs = ExportInputs {
        memories: &[mem],
        tags: &TagsPage {
            project: vec![],
            user: vec![],
            memory_tags: vec![],
            memory_tag_total: 0,
        },
        profile: None,
        truncated: false,
    };
    let doc = export_document(&inputs, "20260101T000000Z");
    let bytes = serde_json::to_string(&doc).unwrap().into_bytes();
    let preview = import_preview(&bytes).unwrap();
    assert_eq!(preview.format, ImportFormat::ExplorerExport);
    assert_eq!(preview.memories, 1);
}

#[wasm_bindgen_test]
fn test_import_preview_counts_a_portability_bundle() {
    let bundle = serde_json::json!({
        "schemaVersion": 1,
        "exportedAt": "x",
        "plugin": {"package":"p","version":"1"},
        "source": {"x":1},
        "embedding": {"model":"m","dimensions":1},
        "memories": [{"id":"m1","content":"x"},{"id":"m2","content":"y"}],
        "milestones": [{"id":"ms1"}],
        "staging": {"sessions":[]},
        "profiles": [{"id":"p1"}],
    });
    let bytes = serde_json::to_string(&bundle).unwrap().into_bytes();
    let preview = import_preview(&bytes).unwrap();
    assert_eq!(preview.format, ImportFormat::PortabilityBundle);
    assert_eq!(preview.memories, 2);
    assert_eq!(preview.milestones, 1);
}

#[wasm_bindgen_test]
fn test_import_preview_rejects_malformed_json() {
    assert!(import_preview(b"not json").is_err());
    // TooLarge: the length check is exercised directly, so the test never
    // materialises a 100 MiB buffer inside the wasm heap.
    assert_eq!(
        check_import_len(MAX_IMPORT_BYTES + 1),
        Err(ImportError::TooLarge)
    );
    assert_eq!(check_import_len(0), Ok(()));
    assert_eq!(check_import_len(MAX_IMPORT_BYTES), Ok(()));
}

#[wasm_bindgen_test]
fn test_import_preview_rejects_an_unknown_document() {
    let bytes = b"{}".to_vec();
    assert!(import_preview(&bytes).is_err());
    // schemaVersion:2
    let v = serde_json::json!({"kind": EXPLORER_EXPORT_KIND, "schemaVersion": 2, "memories": []});
    let bytes = serde_json::to_string(&v).unwrap().into_bytes();
    let res = import_preview(&bytes);
    assert!(res.is_err());
}

#[wasm_bindgen_test]
fn test_import_preview_reports_a_missing_memories_tier() {
    let v = serde_json::json!({"kind": EXPLORER_EXPORT_KIND, "schemaVersion": 1});
    let bytes = serde_json::to_string(&v).unwrap().into_bytes();
    let res = import_preview(&bytes);
    assert!(res.is_err());
    // content-less row -> skipped:1
    let v2 = serde_json::json!({"kind": EXPLORER_EXPORT_KIND, "schemaVersion": 1, "memories": [{"id":"m1"}]});
    let bytes2 = serde_json::to_string(&v2).unwrap().into_bytes();
    let p = import_preview(&bytes2).unwrap();
    assert_eq!(p.skipped, 1);
}

#[wasm_bindgen_test]
fn test_import_apply_is_documented_unsupported() {
    const { assert!(!IMPORT_APPLY_SUPPORTED) };
}

// ---------------------------------------------------------------------------
// The export's loading policy (U7)
// ---------------------------------------------------------------------------

#[wasm_bindgen_test]
fn test_export_preview_does_not_report_zero_before_load() {
    // Mutant: `format!("memories: {count}")` unconditionally, which is today's
    // `memories: 0` while 24 pages are still arriving.
    assert_eq!(export_preview_text(false, 0), "Loading export…");
    assert_eq!(
        export_preview_text(false, 3215),
        "Loading export…",
        "a half-filled count is not reported as final either"
    );
}

#[wasm_bindgen_test]
fn test_export_preview_reports_count_after_load() {
    // Mutant: always "Loading export…" — a preview that never settles.
    assert_eq!(export_preview_text(true, 0), "memories: 0");
    assert_eq!(export_preview_text(true, 3215), "memories: 3215");
}

#[wasm_bindgen_test]
fn test_export_download_is_disabled_until_loaded() {
    // Mutant: returns false unconditionally — an early click downloads an empty
    // export while the pages load.
    assert!(export_download_disabled(false));
}

#[wasm_bindgen_test]
fn test_export_download_is_enabled_after_load() {
    // Mutant: returns true unconditionally — Download is never usable.
    assert!(!export_download_disabled(true));
}
