#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::wasm_bindgen_test;
use web_ui::core_screens::{MemoryItem, TagsPage};
use web_ui::export_import::{
    EXPLORER_EXPORT_KIND, ExportInputs, IMPORT_APPLY_SUPPORTED, ImportError, ImportFormat,
    MAX_IMPORT_BYTES, check_import_len, export_document, export_download_disabled,
    export_file_name, export_pages_spec, export_preview_text, export_truncated,
    exportable_memories, format_exported_at, import_preview, should_continue,
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
    // Mutant: `received <= page_size` — a full last page loops one page too far.
    assert!(!should_continue(1, 200, 50, 1));
    assert!(should_continue(1, 200, 200, 3));
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
    assert_eq!(export_preview_text(false, 0, Some(0)), "Loading export…");
    assert_eq!(
        export_preview_text(false, 3215, Some(6227)),
        "Loading export…",
        "a half-filled count is not reported as final either"
    );
}

#[wasm_bindgen_test]
fn test_export_preview_reports_count_after_load() {
    // Mutant: always "Loading export…" — a preview that never settles.
    assert_eq!(export_preview_text(true, 0, Some(0)), "memories: 0");
    assert_eq!(
        export_preview_text(true, 3215, Some(3215)),
        "memories: 3215"
    );
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

// ---------------------------------------------------------------------------
// Issue 118 wave 5 (F1a): the walk reaches the store total
// ---------------------------------------------------------------------------

#[wasm_bindgen_test]
fn test_export_walk_reaches_the_store_total() {
    // 6 227 rows at page size 200 is 32 pages. Page 25 is a FULL page with seven
    // pages still to come: the walk must continue past the retired 25-page cap.
    assert!(
        should_continue(25, 200, 200, 32),
        "a full page 25 of 32 continues"
    );
}

#[wasm_bindgen_test]
fn test_export_walk_stops_at_the_server_page_count() {
    // The server's own page count is authoritative: page 32 of 32 is the last.
    assert!(should_continue(31, 200, 200, 32), "page 31 of 32 continues");
    assert!(
        !should_continue(32, 200, 200, 32),
        "page 32 of 32 stops — no 33rd request"
    );
}

#[wasm_bindgen_test]
fn test_export_walk_stops_at_the_work_ceiling() {
    // Mutant: drop the EXPORT_MAX_PAGES guard — a 1 M-row store walks unbounded.
    assert!(
        should_continue(249, 200, 200, 400),
        "page 249 is still work the ceiling allows"
    );
    assert!(
        !should_continue(250, 200, 200, 400),
        "page 250 is the named work ceiling"
    );
}

// ---------------------------------------------------------------------------
// Issue 118 wave 5 (F1b): the preview never prints a count the file lacks
// ---------------------------------------------------------------------------

#[wasm_bindgen_test]
fn test_export_preview_discloses_a_short_walk() {
    // Mutant: the flat two-argument `memories: {count}` of the retired preview.
    assert_eq!(
        export_preview_text(true, 5000, Some(6227)),
        "memories: 5000 of 6227 — export limited"
    );
}

#[wasm_bindgen_test]
fn test_export_preview_states_no_limit_when_complete() {
    // Mutant: always append " — export limited" (cries wolf on every export).
    assert_eq!(
        export_preview_text(true, 6227, Some(6227)),
        "memories: 6227"
    );
    assert_eq!(export_preview_text(true, 0, Some(0)), "memories: 0");
}

#[wasm_bindgen_test]
fn test_export_preview_survives_a_store_that_grew_mid_walk() {
    // Mutant: `count != total` as the truncation test — a store that grew while
    // the walk ran is not a truncation; the file is complete.
    assert_eq!(
        export_preview_text(true, 6229, Some(6227)),
        "memories: 6229"
    );
}

#[wasm_bindgen_test]
fn test_export_preview_reports_an_unknown_total() {
    // Mutant: the bare `memories: {count}` for `None` (the fall-through r3 removed).
    assert_eq!(
        export_preview_text(true, 0, None),
        "memories: 0 — store total unavailable; export may be incomplete"
    );
}

#[wasm_bindgen_test]
fn test_export_truncation_is_derived_from_the_counts() {
    // Mutant: a hand-set flag that only fires at the ceiling — the mid-walk
    // error path then reports `false` over a 400-row file.
    assert!(
        export_truncated(400, Some(6227)),
        "a short walk is truncated"
    );
    assert!(
        !export_truncated(6227, Some(6227)),
        "a complete walk is not"
    );
    assert!(export_truncated(0, None), "no page ever arrived is short");
    assert!(
        !export_truncated(0, Some(0)),
        "a real empty store is complete"
    );
    // Mutant: `<` as `!=` — a store that GREW mid-walk over-delivers (the file
    // holds more than the stale total), which is complete, not truncated.
    assert!(
        !export_truncated(6229, Some(6227)),
        "an over-delivered walk is complete, not truncated"
    );
}

#[wasm_bindgen_test]
fn test_export_document_row_count_equals_the_preview_count() {
    // The pure invariant: 3 fetched rows of which 1 is a prompt leave 2 in the
    // document, and the preview formats exactly the number it is given. The
    // fetched-vs-exportable mutant lives in the VIEW call site — a pure test
    // cannot see it; the CDP case `export_walks_to_store_total` kills it.
    let prompt = MemoryItem {
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
    let mut mem = prompt.clone();
    mem.kind = Some("memory".to_string());
    mem.id = "m1".to_string();
    let mut mem2 = mem.clone();
    mem2.id = "m2".to_string();
    let fetched = vec![mem, mem2, prompt];
    let inputs = ExportInputs {
        memories: &fetched,
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
    let rows = doc["memories"].as_array().expect("an array").len();
    assert_eq!(rows, 2, "the document holds the two exportable rows");
    assert_eq!(export_preview_text(true, rows, Some(rows)), "memories: 2");
    // Mutant: a preview formatting `count + 1` (or a stale literal) breaks here.
    assert_ne!(export_preview_text(true, rows, Some(rows)), "memories: 3");
}
