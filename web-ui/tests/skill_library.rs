#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::wasm_bindgen_test;
use web_ui::skill_library::{
    DraftError, SkillDraft, SkillItem, SkillRowView, SkillsPage, SkillsView, allocate_skill_id,
    create_skill_spec, delete_skill_spec, filter_skills, get_skill_spec, list_skills_spec,
    skill_body, skills_error_view, skills_view, slugify_skill_id, update_skill_spec,
    validate_draft,
};

#[wasm_bindgen_test]
fn test_skills_empty_copy_distinguishes_library_and_filter() {
    use web_ui::skill_library::skills_empty_copy;
    assert_eq!(skills_empty_copy(0, 0), Some("No skills yet"));
    assert_eq!(skills_empty_copy(2, 0), Some("No skills match your search"));
    assert_eq!(skills_empty_copy(2, 1), None);
}

#[wasm_bindgen_test]
fn test_list_and_get_skill_specs_match_the_pinned_templates() {
    let spec = list_skills_spec();
    assert_eq!(spec.method, "GET");
    assert_eq!(spec.path, "/api/skills");
    assert!(spec.query.is_empty());
    assert!(spec.body.is_none());

    let spec2 = get_skill_spec("abc");
    assert_eq!(spec2.method, "GET");
    assert_eq!(spec2.path, "/api/skills/abc");
    // percent-encoded segment
    let spec3 = get_skill_spec("a/b c");
    assert_eq!(spec3.path, "/api/skills/a%2Fb%20c");
}

#[wasm_bindgen_test]
fn test_create_skill_spec_posts_the_upstream_body() {
    let draft = SkillDraft {
        id: "my-skill".to_string(),
        name: "My Skill".to_string(),
        description: "desc".to_string(),
        content: "# body".to_string(),
        autoinvoke: true,
        enabled: true,
    };
    let spec = create_skill_spec(&draft);
    assert_eq!(spec.method, "POST");
    assert_eq!(spec.path, "/api/skills");
    let body = spec.body.unwrap();
    assert_eq!(body["id"], "my-skill");
    assert_eq!(body["name"], "My Skill");
    assert_eq!(body["description"], "desc");
    assert_eq!(body["content"], "# body");
    assert_eq!(body["autoinvoke"], true);
    assert_eq!(body["enabled"], true);
}

#[wasm_bindgen_test]
fn test_update_skill_spec_puts_the_fields_without_the_id() {
    let draft = SkillDraft {
        id: "my-skill".to_string(),
        name: "Renamed".to_string(),
        description: "d".to_string(),
        content: "# c".to_string(),
        autoinvoke: false,
        enabled: true,
    };
    let spec = update_skill_spec("my-skill", &draft);
    assert_eq!(spec.method, "PUT");
    assert_eq!(spec.path, "/api/skills/my-skill");
    let body = spec.body.unwrap();
    assert!(body.get("id").is_none());
    assert_eq!(body["name"], "Renamed");
}

#[wasm_bindgen_test]
fn test_delete_skill_spec_matches_the_pinned_template() {
    let spec = delete_skill_spec("my-skill");
    assert_eq!(spec.method, "DELETE");
    assert_eq!(spec.path, "/api/skills/my-skill");
}

#[wasm_bindgen_test]
fn test_skill_id_is_one_percent_encoded_segment() {
    // fill() behaviour already; check spec path encoding for create?
    // Test the encode via get_skill_spec
    let spec = get_skill_spec("a/b c");
    assert_eq!(spec.path, "/api/skills/a%2Fb%20c");
    let spec2 = get_skill_spec("..%2F..");
    // % -> %25, so ..%2F.. -> ..%252F..
    assert_eq!(spec2.path, "/api/skills/..%252F..");
}

#[wasm_bindgen_test]
fn test_allocate_skill_id_slugifies_the_name() {
    assert_eq!(slugify_skill_id("Ship It!"), "ship-it");
    assert_eq!(slugify_skill_id(""), "");
    // unusable -> empty slug -> allocate uses "skill"
    assert_eq!(allocate_skill_id("", &[]), "skill");
    // 70-char name -> <=64
    let long = "a".repeat(70);
    let slug = slugify_skill_id(&long);
    assert!(slug.len() <= 64);
    // hyphen boundary re-trim
    let name = format!("{}-", "a".repeat(63));
    let s = slugify_skill_id(&name);
    assert!(!s.ends_with('-'));
    // non-ASCII: the documented Latin fold replaces NFKD ("Café" -> "cafe",
    // not "caf" — the é folds to `e` instead of becoming a separator).
    assert_eq!(slugify_skill_id("Café"), "cafe");
    assert_eq!(slugify_skill_id("Über Straße"), "uber-strasse");
    assert_eq!(slugify_skill_id("Ñoño"), "nono");
    assert_eq!(allocate_skill_id("Café", &[]), "cafe");
}

#[wasm_bindgen_test]
fn test_allocate_skill_id_dedupes_a_taken_id() {
    let taken = vec!["ship-it".to_string()];
    assert_eq!(allocate_skill_id("Ship It!", &taken), "ship-it-2");
    // 64-char base taken -> 62 a's + -2
    let base = "a".repeat(64);
    let taken2 = vec![base.clone()];
    let result = allocate_skill_id(&base, &taken2);
    assert_eq!(result.len(), 64);
    assert!(result.ends_with("-2"));
}

#[wasm_bindgen_test]
fn test_skills_view_maps_the_items_payload() {
    let json = r#"{"items":[{"id":"s1","name":"N","description":"d","content":"c","autoinvoke":true,"enabled":true,"containerTag":null,"createdAt":"2020","updatedAt":"2021"}]}"#;
    let page: SkillsPage = serde_json::from_str(json).unwrap();
    match skills_view(&page) {
        SkillsView::Loaded { rows } => {
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].id, "s1");
            assert_eq!(rows[0].name, "N");
            assert_eq!(rows[0].updated_at, "2021");
        }
        _ => panic!("expected loaded"),
    }
}

#[wasm_bindgen_test]
fn test_skills_view_empty_and_error_variants() {
    let page = SkillsPage { items: vec![] };
    assert!(matches!(skills_view(&page), SkillsView::Empty));
    // The failure branch the panel uses: a detail/list failure renders the
    // error string verbatim inside `skills-error`.
    match skills_error_view("skills unavailable") {
        SkillsView::Error(message) => assert_eq!(message, "skills unavailable"),
        other => panic!("expected the error variant, got {other:?}"),
    }
}

#[wasm_bindgen_test]
fn test_filter_skills_matches_id_name_and_description() {
    let rows = vec![
        SkillRowView {
            id: "abc".into(),
            name: "X".into(),
            description: "d".into(),
            updated_at: "".into(),
            enabled: true,
            autoinvoke: true,
        },
        SkillRowView {
            id: "zzz".into(),
            name: "Ship".into(),
            description: "desc".into(),
            updated_at: "".into(),
            enabled: true,
            autoinvoke: true,
        },
    ];
    let r = filter_skills(&rows, "abc");
    assert_eq!(r.len(), 1);
    let r = filter_skills(&rows, "Ship");
    assert_eq!(r.len(), 1);
    let r = filter_skills(&rows, "desc");
    assert_eq!(r.len(), 1);
}

#[wasm_bindgen_test]
fn test_filter_skills_is_case_insensitive_and_trims() {
    let rows = vec![SkillRowView {
        id: "x".into(),
        name: "Ship It!".into(),
        description: "".into(),
        updated_at: "".into(),
        enabled: true,
        autoinvoke: true,
    }];
    let r = filter_skills(&rows, "  SHIP  ");
    assert_eq!(r.len(), 1);
}

#[wasm_bindgen_test]
fn test_validate_draft_requires_name_and_content() {
    let mut draft = SkillDraft {
        id: "x".into(),
        name: "".into(),
        description: "".into(),
        content: "c".into(),
        autoinvoke: true,
        enabled: true,
    };
    assert_eq!(validate_draft(&draft), Err(DraftError::NameRequired));
    draft.name = "n".into();
    draft.content = "".into();
    assert_eq!(validate_draft(&draft), Err(DraftError::ContentRequired));
    // whitespace-only
    draft.name = "   ".into();
    draft.content = "c".into();
    assert_eq!(validate_draft(&draft), Err(DraftError::NameRequired));
    draft.name = "n".into();
    draft.content = " \n\t".into();
    assert_eq!(validate_draft(&draft), Err(DraftError::ContentRequired));
}

#[wasm_bindgen_test]
fn test_skill_item_decodes_the_endpoint_projection() {
    let json = r##"{"id":"s1","name":"N","description":"d","content":"# body","autoinvoke":true,"enabled":true,"containerTag":null,"createdAt":"2020","updatedAt":"2021"}"##;
    let item: SkillItem = serde_json::from_str(json).unwrap();
    assert_eq!(item.content, "# body");
    // by-id: data alone (not wrapped in items) decodes with content
    assert_eq!(item.id, "s1");
}

#[wasm_bindgen_test]
fn test_skill_body_omits_the_id_when_updating() {
    let draft = SkillDraft {
        id: "x".into(),
        name: "n".into(),
        description: "d".into(),
        content: "c".into(),
        autoinvoke: true,
        enabled: true,
    };
    let body = skill_body(&draft, false);
    assert!(body.get("id").is_none());
    let body2 = skill_body(&draft, true);
    assert_eq!(body2["id"], "x");
}
