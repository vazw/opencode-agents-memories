//! The navigation model.
//!
//! Pure data derived from the route table: one entry per route, in table order.
//! A planned screen is visibly marked in the shell but stays navigable.

use crate::paths::endpoints_for;
use crate::routes::{ROUTES, RouteId, resolve, route};

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
    /// A screen whose owning wave has landed: it renders its own panel and,
    /// when it declares endpoints, fetches them on mount. `Agents` (W6.6) is
    /// the milestone-roster example; its documented `Unavailable` state is the
    /// rendered 501 degrade, not a deferred data path.
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
        RouteId::Agents => ScreenState::Live {
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

/// The route path a pathname selects in the nav, normalised (`/tags/` and
/// `/tags?x` both select `/tags`). `None` for `/api/**` and unknown paths.
pub fn selected_path(pathname: &str) -> Option<&'static str> {
    resolve(pathname).map(|id| route(id).path)
}

/// Whether a click is the plain primary-button click the shell routes in-app.
/// Modified or non-primary clicks keep the browser's own new-tab behaviour.
pub fn is_plain_left_click(button: i16, ctrl: bool, meta: bool, shift: bool, alt: bool) -> bool {
    button == 0 && !(ctrl || meta || shift || alt)
}

/// The icon of a nav entry (an `icondata_ai` glyph; the visible label is always
/// rendered beside it, so the icon is never the sole accessible name).
pub fn nav_icon(route: RouteId) -> icondata_core::Icon {
    match route {
        RouteId::Home => icondata_ai::AiHomeOutlined,
        RouteId::ProjectMemories => icondata_ai::AiDatabaseOutlined,
        RouteId::Tags => icondata_ai::AiTagsOutlined,
        RouteId::Profile => icondata_ai::AiUserOutlined,
        RouteId::Settings => icondata_ai::AiSettingOutlined,
        RouteId::ExportImport => icondata_ai::AiSwapOutlined,
        RouteId::SkillLibrary => icondata_ai::AiBookOutlined,
        RouteId::SessionHistory => icondata_ai::AiHistoryOutlined,
        RouteId::Agents => icondata_ai::AiTeamOutlined,
        RouteId::Stats => icondata_ai::AiBarChartOutlined,
    }
}
