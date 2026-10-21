//! The placeholder state + the baseline theme.
//!
//! Both seams are pure so they run in node mode: the placeholder logic is
//! `notice_for`, and the theme preference is read/written through the
//! in-memory `ThemeStore` fake below. The real `web_sys::Storage` binding is a
//! one-line adapter exercised by the Step 9 browser check.
#![cfg(target_arch = "wasm32")]

use std::cell::RefCell;

use wasm_bindgen_test::wasm_bindgen_test;
use web_ui::nav::{ScreenState, state_for};
use web_ui::paths::endpoints_for;
use web_ui::routes::{Owner, RouteId};
use web_ui::screens::notice_for;
use web_ui::theme::{THEME_STORAGE_KEY, Theme, ThemeStore, read_preference, store_preference};

#[derive(Default)]
struct MemoryThemeStore {
    stored: RefCell<Option<(String, String)>>,
}

impl MemoryThemeStore {
    fn with(key: &str, value: &str) -> Self {
        Self {
            stored: RefCell::new(Some((key.to_string(), value.to_string()))),
        }
    }
}

impl ThemeStore for MemoryThemeStore {
    fn get(&self, key: &str) -> Option<String> {
        self.stored
            .borrow()
            .as_ref()
            .filter(|(stored_key, _)| stored_key == key)
            .map(|(_, value)| value.clone())
    }

    fn set(&self, key: &str, value: &str) {
        *self.stored.borrow_mut() = Some((key.to_string(), value.to_string()));
    }
}

#[wasm_bindgen_test]
fn test_ready_screen_state_has_no_placeholder_notice() {
    assert_eq!(notice_for(&ScreenState::Ready), None);
    assert_eq!(notice_for(&state_for(RouteId::Home)), None);
}

#[wasm_bindgen_test]
fn test_planned_screen_state_names_the_owning_wave() {
    let state = ScreenState::Planned {
        owner: Owner::W6_4,
        endpoints: &["/api/skills"],
    };
    let notice = notice_for(&state).expect("a planned screen renders a notice");
    assert_eq!(notice.owner, Owner::W6_4);
    assert_eq!(notice.owner.label(), "W6.4");
    assert_eq!(
        notice_for(&state_for(RouteId::SkillLibrary))
            .expect("the skill library is still planned")
            .owner,
        Owner::W6_4
    );
    // The four core screens are NOT planned any more (W6.7).
    for id in [
        RouteId::ProjectMemories,
        RouteId::Tags,
        RouteId::Profile,
        RouteId::Settings,
    ] {
        assert_eq!(notice_for(&state_for(id)), None, "{id:?} renders data");
    }
}

#[wasm_bindgen_test]
fn test_live_screen_state_has_no_placeholder_notice() {
    // W6.3 landed: the session-history screen renders data, not a notice.
    assert_eq!(notice_for(&state_for(RouteId::SessionHistory)), None);
    assert!(matches!(
        state_for(RouteId::SessionHistory),
        ScreenState::Live {
            owner: Owner::W6_3,
            ..
        }
    ));
}

#[wasm_bindgen_test]
fn test_planned_screen_state_lists_the_endpoints_it_will_consume() {
    let notice = notice_for(&state_for(RouteId::SkillLibrary)).expect("planned");
    assert_eq!(notice.endpoints, endpoints_for(RouteId::SkillLibrary));
    assert!(!notice.endpoints.is_empty());
    assert!(notice.endpoints.contains(&"/api/skills"));
}

#[wasm_bindgen_test]
fn test_theme_defaults_to_dark() {
    let store = MemoryThemeStore::default();
    assert_eq!(read_preference(&store), Theme::Dark);
}

#[wasm_bindgen_test]
fn test_theme_preference_round_trips_through_the_theme_key() {
    let store = MemoryThemeStore::default();
    store_preference(&store, Theme::Light);
    assert_eq!(read_preference(&store), Theme::Light);

    let stored = store
        .stored
        .borrow()
        .clone()
        .expect("a preference was stored");
    assert_eq!(stored.0, THEME_STORAGE_KEY);
    assert_eq!(stored.0, "opencode-mem-theme");
    assert_eq!(stored.1, "light");
}

#[wasm_bindgen_test]
fn test_theme_preference_ignores_an_unknown_stored_value() {
    let store = MemoryThemeStore::with(THEME_STORAGE_KEY, "purple");
    assert_eq!(read_preference(&store), Theme::Dark);
}
