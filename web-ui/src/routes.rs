//! The client route table.
//!
//! This is the single source of truth for which paths the SPA resolves
//! client-side (`mem-web`'s SPA fallback serves the shell for exactly these
//! paths). `Home` is the shell's own landing route; every other entry is a
//! landed screen (`nav::state_for` answers `Live` for all of them), so no route
//! renders the placeholder. The `Planned` screen state and its notice remain
//! constructible for synthetic tests and for a future screen lane, but no
//! route-table row produces one.
//!
//! Upstream-parity paths are transcribed from upstream's
//! `example/opencode-mem/web/src/lib/routes.ts`; the port-local paths are
//! declared by this wave and are the contract the later screen lanes use.

/// Which wave owns a screen. `ThisWave` is the shell slice's own landing route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Owner {
    ThisWave,
    W6_7,
    W6_3,
    W6_5,
}

impl Owner {
    /// The wave label rendered in the placeholder notice.
    pub fn label(self) -> &'static str {
        match self {
            Self::ThisWave => "this wave",
            Self::W6_7 => "W6.7",
            Self::W6_3 => "W6.3",
            Self::W6_5 => "W6.5",
        }
    }
}

/// The client-side route identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RouteId {
    Home,
    ProjectMemories,
    Tags,
    Profile,
    Settings,
    ExportImport,
    SkillLibrary,
    SessionHistory,
}

impl RouteId {
    /// Every declared route, in route-table order.
    pub const ALL: [Self; 8] = [
        Self::Home,
        Self::ProjectMemories,
        Self::Tags,
        Self::Profile,
        Self::Settings,
        Self::ExportImport,
        Self::SkillLibrary,
        Self::SessionHistory,
    ];

    /// A stable, human-readable identifier (used by tests and labels only).
    pub fn name(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::ProjectMemories => "project-memories",
            Self::Tags => "tags",
            Self::Profile => "profile",
            Self::Settings => "settings",
            Self::ExportImport => "export-import",
            Self::SkillLibrary => "skill-library",
            Self::SessionHistory => "session-history",
        }
    }
}

/// One row of the client route table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteSpec {
    pub id: RouteId,
    pub path: &'static str,
    pub label: &'static str,
    pub owner: Owner,
}

/// The route table. Order is the navigation order.
pub const ROUTES: &[RouteSpec] = &[
    RouteSpec {
        id: RouteId::Home,
        path: "/",
        label: "Home",
        owner: Owner::ThisWave,
    },
    RouteSpec {
        id: RouteId::ProjectMemories,
        path: "/project-memories",
        label: "Memories",
        owner: Owner::W6_7,
    },
    RouteSpec {
        id: RouteId::Tags,
        path: "/tags",
        label: "Tags",
        owner: Owner::W6_7,
    },
    RouteSpec {
        id: RouteId::Profile,
        path: "/user-profile",
        label: "Profile",
        owner: Owner::W6_7,
    },
    RouteSpec {
        id: RouteId::Settings,
        path: "/settings",
        label: "Settings",
        owner: Owner::W6_7,
    },
    RouteSpec {
        id: RouteId::ExportImport,
        path: "/export-import",
        label: "Export / Import",
        owner: Owner::W6_5,
    },
    RouteSpec {
        id: RouteId::SkillLibrary,
        path: "/skill-library",
        label: "Skills",
        owner: Owner::W6_5,
    },
    RouteSpec {
        id: RouteId::SessionHistory,
        path: "/sessions",
        label: "Sessions",
        owner: Owner::W6_3,
    },
];

/// The route spec for an id. Every `RouteId` has exactly one row.
pub fn route(id: RouteId) -> &'static RouteSpec {
    ROUTES
        .iter()
        .find(|spec| spec.id == id)
        .expect("every RouteId has a route-table row")
}

/// Resolve a browser pathname to a declared client route.
///
/// - a trailing slash is normalised (`/project-memories/` resolves like
///   `/project-memories`);
/// - a query string or fragment is ignored;
/// - `/api/**` is never a client route (the API surface never falls back to the
///   shell, and the UI must never navigate to it).
pub fn resolve(path: &str) -> Option<RouteId> {
    let path = path.split(['?', '#']).next().unwrap_or(path);
    if path == "/api" || path.starts_with("/api/") {
        return None;
    }
    let normalized = normalize(path);
    ROUTES
        .iter()
        .find(|spec| spec.path == normalized)
        .map(|spec| spec.id)
}

fn normalize(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() { "/" } else { trimmed }
}
