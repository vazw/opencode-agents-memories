//! Domain-doc presence: root CONTEXT-MAP + one CONTEXT.md stub per crate.

#[test]
fn test_context_map_lists_every_crate_and_stub_docs_exist() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let crate_names = [
        "mem-core",
        "mem-store",
        "mem-embed",
        "mem-ai",
        "mem-profile",
        "mem-episodic",
        "mem-skills",
        "mem-portability",
        "mem-web",
        "mem-bridge",
    ];

    for name in crate_names {
        let path = format!("{manifest_dir}/crates/{name}/CONTEXT.md");
        let contents = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("missing or unreadable domain doc {path}: {e}"));
        assert!(!contents.trim().is_empty(), "{path} must be non-empty");
        assert!(
            contents.contains("**Purpose:**"),
            "{path} must contain a **Purpose:** line"
        );
    }

    let map_path = format!("{manifest_dir}/CONTEXT-MAP.md");
    let map = std::fs::read_to_string(&map_path)
        .unwrap_or_else(|e| panic!("missing or unreadable {map_path}: {e}"));
    for name in crate_names {
        assert!(map.contains(name), "CONTEXT-MAP.md must list crate {name}");
        let stub_path = format!("crates/{name}/CONTEXT.md");
        assert!(
            map.contains(&stub_path),
            "CONTEXT-MAP.md must reference path {stub_path}"
        );
    }
}
