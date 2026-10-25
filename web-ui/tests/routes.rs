//! The client route table: the single source of truth for which paths the SPA
//! resolves client-side. Every test drives the public `web_ui::routes` module.
#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::wasm_bindgen_test;
use web_ui::routes::{Owner, ROUTES, RouteId, resolve, route};

#[wasm_bindgen_test]
fn test_route_table_declares_the_upstream_and_port_paths() {
    // Upstream-parity paths are transcribed from upstream's
    // `web/src/lib/routes.ts`; the port-local ones are declared by this wave.
    let expected = [
        (RouteId::Home, "/"),
        (RouteId::ProjectMemories, "/project-memories"),
        (RouteId::Tags, "/tags"),
        (RouteId::Profile, "/user-profile"),
        (RouteId::Settings, "/settings"),
        (RouteId::ExportImport, "/export-import"),
        (RouteId::SkillLibrary, "/skill-library"),
        (RouteId::SessionHistory, "/sessions"),
        (RouteId::Stats, "/stats"),
    ];

    assert_eq!(
        ROUTES.len(),
        expected.len(),
        "the route table has exactly nine entries"
    );
    assert_eq!(RouteId::ALL.len(), expected.len());

    for (id, path) in expected {
        let spec = ROUTES
            .iter()
            .find(|spec| spec.id == id)
            .unwrap_or_else(|| panic!("route table is missing {id:?}"));
        assert_eq!(spec.path, path, "{id:?} has the wrong path");
    }
}

#[wasm_bindgen_test]
fn test_route_table_paths_are_unique_and_root_relative() {
    for (index, spec) in ROUTES.iter().enumerate() {
        assert!(
            spec.path.starts_with('/'),
            "{:?} path is not root-relative",
            spec.id
        );
        for other in &ROUTES[index + 1..] {
            assert_ne!(spec.path, other.path, "duplicate path {}", spec.path);
        }
    }
}

#[wasm_bindgen_test]
fn test_route_table_declares_no_api_path() {
    for spec in ROUTES {
        assert!(
            !spec.path.starts_with("/api/"),
            "{:?} must never be a client route",
            spec.id
        );
    }
}

#[wasm_bindgen_test]
fn test_route_table_has_nine_entries_with_the_stats_route() {
    // Baseline 8 + the stats route (upstream parity path `/stats`).
    assert_eq!(RouteId::ALL.len(), 9, "baseline eight + stats");
    assert_eq!(ROUTES.len(), 9);
    let spec = route(RouteId::Stats);
    assert_eq!(spec.path, "/stats");
    assert_eq!(spec.label, "Stats");
    assert_eq!(
        spec.owner,
        Owner::ThisWave,
        "the stats route is this wave's"
    );
}

#[wasm_bindgen_test]
fn test_route_table_paths_are_spa_eligible() {
    for spec in ROUTES {
        let last_segment = spec.path.rsplit('/').next().unwrap_or_default();
        assert!(
            !last_segment.contains('.'),
            "{:?}'s last segment {:?} is not SPA-eligible",
            spec.id,
            last_segment
        );
    }
}

#[wasm_bindgen_test]
fn test_resolve_returns_the_route_for_a_declared_path() {
    assert_eq!(resolve("/project-memories"), Some(RouteId::ProjectMemories));
}

#[wasm_bindgen_test]
fn test_resolve_maps_stats_path_to_stats_route() {
    assert_eq!(resolve("/stats"), Some(RouteId::Stats));
    assert_eq!(resolve("/stats/"), Some(RouteId::Stats));
    assert_eq!(resolve("/stats?refresh=1"), Some(RouteId::Stats));
}

#[wasm_bindgen_test]
fn test_resolve_returns_none_for_an_unknown_path() {
    assert_eq!(resolve("/nope"), None);
}

#[wasm_bindgen_test]
fn test_resolve_returns_none_for_an_api_path() {
    assert_eq!(resolve("/api/memories"), None);
    assert_eq!(resolve("/api/memories/42/pin"), None);
}

#[wasm_bindgen_test]
fn test_resolve_normalizes_a_trailing_slash() {
    assert_eq!(resolve("/project-memories/"), resolve("/project-memories"));
    assert_eq!(
        resolve("/project-memories/"),
        Some(RouteId::ProjectMemories)
    );
}

#[wasm_bindgen_test]
fn test_home_is_the_shell_landing_route() {
    assert_eq!(resolve("/"), Some(RouteId::Home));
    assert_eq!(route(RouteId::Home).owner, Owner::ThisWave);
    assert!(!route(RouteId::Home).label.is_empty());
}

#[wasm_bindgen_test]
fn test_core_route_owners_are_w6_7() {
    // W6.7 re-homes the four core screens from the closed `Owner::W6_2` to
    // this wave. W6.3 keeps the sessions screen. W6.5 re-homes the two
    // portability screens; the retired W6.4 owner no longer exists at all.
    for id in [
        RouteId::ProjectMemories,
        RouteId::Tags,
        RouteId::Profile,
        RouteId::Settings,
    ] {
        let spec = route(id);
        assert_eq!(spec.owner, Owner::W6_7, "{id:?} is owned by W6.7");
        assert_eq!(spec.owner.label(), "W6.7", "{id:?} renders the W6.7 label");
    }
    assert_eq!(route(RouteId::SessionHistory).owner, Owner::W6_3);
    assert_eq!(route(RouteId::ExportImport).owner, Owner::W6_5);
    assert_eq!(route(RouteId::SkillLibrary).owner, Owner::W6_5);
}

#[wasm_bindgen_test]
fn test_route_accessor_is_total_for_every_declared_route() {
    for id in RouteId::ALL {
        assert_eq!(route(id).id, id, "{id:?} is not in the route table");
    }
}
