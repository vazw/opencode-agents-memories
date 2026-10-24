//! The navigation model.
//!
//! Pure data derived from the route table: one entry per route, in table order.
//! A planned screen is visibly marked in the shell but stays navigable.

use crate::paths::endpoints_for;
use crate::routes::{ROUTES, RouteId, route};

pub use crate::routes::Owner;

/// What a route resolves to today.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenState {
    /// The shell's own landing screen.
    Ready,
    /// A declared screen that a later wave owns; it renders the placeholder.
    Planned {
        owner: Owner,
        endpoints: &'static [&'static str],
    },
    /// A screen whose owning wave has landed: it fetches and renders real data.
    Live {
        owner: Owner,
        endpoints: &'static [&'static str],
    },
}

/// One navigation entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NavEntry {
    pub route: RouteId,
    pub path: &'static str,
    pub label: &'static str,
    pub owner: Owner,
    pub state: ScreenState,
}

/// The state of a route today.
///
/// `Home` is the shell's own landing screen; every other route is `Live`.
pub fn state_for(id: RouteId) -> ScreenState {
    let spec = route(id);
    match id {
        RouteId::Home => ScreenState::Ready,
        RouteId::ProjectMemories | RouteId::Tags | RouteId::Profile | RouteId::Settings => {
            ScreenState::Live {
                owner: spec.owner,
                endpoints: endpoints_for(id),
            }
        }
        RouteId::SessionHistory => ScreenState::Live {
            owner: spec.owner,
            endpoints: endpoints_for(id),
        },
        RouteId::Stats => ScreenState::Live {
            owner: spec.owner,
            endpoints: endpoints_for(id),
        },
        RouteId::ExportImport => ScreenState::Live {
            owner: spec.owner,
            endpoints: endpoints_for(id),
        },
        RouteId::SkillLibrary => ScreenState::Live {
            owner: spec.owner,
            endpoints: endpoints_for(id),
        },
    }
}

/// One entry per route, in the route table's order.
pub fn nav_entries() -> Vec<NavEntry> {
    ROUTES
        .iter()
        .map(|spec| NavEntry {
            route: spec.id,
            path: spec.path,
            label: spec.label,
            owner: spec.owner,
            state: state_for(spec.id),
        })
        .collect()
}
