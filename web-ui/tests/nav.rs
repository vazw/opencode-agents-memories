//! The navigation model: one entry per route, in route-table order, derived
//! from the table (a nav that silently drops a route fails these tests).
#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::wasm_bindgen_test;
use web_ui::nav::{NavEntry, ScreenState, nav_entries};
use web_ui::routes::{Owner, ROUTES, RouteId};

fn entry_for(route: RouteId) -> NavEntry {
    nav_entries()
        .into_iter()
        .find(|entry| entry.route == route)
        .unwrap_or_else(|| panic!("nav is missing {route:?}"))
}

#[wasm_bindgen_test]
fn test_nav_lists_every_declared_route_in_table_order() {
    let nav_routes: Vec<RouteId> = nav_entries().into_iter().map(|entry| entry.route).collect();
    let table_routes: Vec<RouteId> = ROUTES.iter().map(|spec| spec.id).collect();
    assert_eq!(nav_routes, table_routes);
}

#[wasm_bindgen_test]
fn test_nav_marks_the_shell_route_as_ready() {
    assert_eq!(entry_for(RouteId::Home).state, ScreenState::Ready);
    assert_eq!(entry_for(RouteId::Home).owner, Owner::ThisWave);
}

#[wasm_bindgen_test]
fn test_nav_marks_the_portability_screens_live() {
    // W6.5 re-homed the two portability screens from W6.4: both are Live.
    for id in [RouteId::ExportImport, RouteId::SkillLibrary] {
        assert_eq!(
            entry_for(id).state,
            ScreenState::Live {
                owner: Owner::W6_5,
                endpoints: web_ui::paths::endpoints_for(id),
            },
            "{id:?} is live under W6.5"
        );
        assert_eq!(entry_for(id).owner, Owner::W6_5, "{id:?} owner");
        assert!(
            !matches!(entry_for(id).state, ScreenState::Planned { .. }),
            "{id:?} must not render the planned notice"
        );
    }
}

#[wasm_bindgen_test]
fn test_nav_marks_the_core_screens_live() {
    // W6.7 landed: the four core screens fetch and render real endpoint data.
    for id in [
        RouteId::ProjectMemories,
        RouteId::Tags,
        RouteId::Profile,
        RouteId::Settings,
    ] {
        assert_eq!(
            entry_for(id).state,
            ScreenState::Live {
                owner: Owner::W6_7,
                endpoints: web_ui::paths::endpoints_for(id),
            },
            "{id:?} is live under W6.7"
        );
        assert_eq!(entry_for(id).owner, Owner::W6_7, "{id:?} owner");
        assert!(
            !matches!(entry_for(id).state, ScreenState::Planned { .. }),
            "{id:?} must not render the planned notice"
        );
    }
}

#[wasm_bindgen_test]
fn test_nav_marks_session_history_live() {
    // W6.3 landed: the screen fetches and renders the real timeline.
    assert_eq!(
        entry_for(RouteId::SessionHistory).state,
        ScreenState::Live {
            owner: Owner::W6_3,
            endpoints: web_ui::paths::endpoints_for(RouteId::SessionHistory),
        }
    );
    assert_eq!(entry_for(RouteId::SessionHistory).owner, Owner::W6_3);
    assert!(!matches!(
        entry_for(RouteId::SessionHistory).state,
        ScreenState::Planned { .. }
    ));
}

#[wasm_bindgen_test]
fn test_nav_agents_entry_is_live_with_the_project_and_milestone_endpoints() {
    // W6.6 landed the route; issue 116 added the project roster the picker
    // reads, so the screen now consumes both templates on mount. It never
    // renders the planned notice.
    let entry = entry_for(RouteId::Agents);
    assert_eq!(
        entry.state,
        ScreenState::Live {
            owner: Owner::W6_6,
            endpoints: &["/api/projects", "/api/projects/{project_id}/milestones",],
        }
    );
    assert_eq!(entry.owner, Owner::W6_6);
    assert_eq!(entry.label, "Agents");
    assert!(!matches!(entry.state, ScreenState::Planned { .. }));
}

#[wasm_bindgen_test]
fn test_nav_stats_entry_is_live_with_stats_endpoint() {
    // The stats screen lands with this wave: Live, consuming only /api/stats.
    assert_eq!(
        entry_for(RouteId::Stats).state,
        ScreenState::Live {
            owner: Owner::ThisWave,
            endpoints: &["/api/stats"],
        }
    );
    assert_eq!(entry_for(RouteId::Stats).owner, Owner::ThisWave);
    assert_eq!(entry_for(RouteId::Stats).label, "Stats");
    assert!(!matches!(
        entry_for(RouteId::Stats).state,
        ScreenState::Planned { .. }
    ));
}

#[wasm_bindgen_test]
fn test_nav_only_marks_the_landed_routes_ready_or_live() {
    let landed: Vec<RouteId> = nav_entries()
        .into_iter()
        .filter(|entry| matches!(entry.state, ScreenState::Ready | ScreenState::Live { .. }))
        .map(|entry| entry.route)
        .collect();
    assert_eq!(
        landed,
        vec![
            RouteId::Home,
            // W6.7: the four core screens render real data.
            RouteId::ProjectMemories,
            RouteId::Tags,
            RouteId::Profile,
            RouteId::Settings,
            // W6.5: the two portability screens re-homed this wave.
            RouteId::ExportImport,
            RouteId::SkillLibrary,
            // W6.3: the session timeline.
            RouteId::SessionHistory,
            // W6.6: the agents view fetches the milestone projection.
            RouteId::Agents,
            // This wave: the stats screen.
            RouteId::Stats,
        ]
    );
}

#[wasm_bindgen_test]
fn test_every_declared_route_is_ready_or_live() {
    for id in RouteId::ALL {
        assert!(
            matches!(
                entry_for(id).state,
                ScreenState::Ready | ScreenState::Live { .. }
            ),
            "{id:?} must not be Planned"
        );
    }
}

#[wasm_bindgen_test]
fn test_nav_labels_are_non_empty() {
    let entries = nav_entries();
    for (index, entry) in entries.iter().enumerate() {
        assert!(
            !entry.label.is_empty(),
            "{:?} has an empty label",
            entry.route
        );
        for other in &entries[index + 1..] {
            assert_ne!(entry.label, other.label, "duplicate label {}", entry.label);
        }
    }
}

#[wasm_bindgen_test]
fn test_nav_entries_point_at_their_route_path() {
    for entry in nav_entries() {
        assert_eq!(entry.path, web_ui::routes::route(entry.route).path);
    }
}

#[wasm_bindgen_test]
fn test_nav_entry_count_matches_the_route_table() {
    assert_eq!(nav_entries().len(), ROUTES.len());
    assert_eq!(nav_entries().len(), RouteId::ALL.len());
}
