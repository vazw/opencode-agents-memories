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
    W6_6,
}

impl Owner {
    /// The wave label rendered in the placeholder notice.
    pub fn label(self) -> &'static str {
        match self {
            Self::ThisWave => "this wave",
            Self::W6_7 => "W6.7",
            Self::W6_3 => "W6.3",
            Self::W6_5 => "W6.5",
            Self::W6_6 => "W6.6",
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
    Agents,
    Stats,
}

impl RouteId {
    /// Every declared route, in route-table order.
    pub const ALL: [Self; 10] = [
        Self::Home,
        Self::ProjectMemories,
        Self::Tags,
        Self::Profile,
        Self::Settings,
        Self::ExportImport,
        Self::SkillLibrary,
        Self::SessionHistory,
        Self::Agents,
        Self::Stats,
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
            Self::Agents => "agents",
            Self::Stats => "stats",
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
    RouteSpec {
        id: RouteId::Agents,
        path: "/agents",
        label: "Agents",
        owner: Owner::W6_6,
    },
    // Upstream parity path (`web/src/lib/routes.ts`). `Owner::ThisWave` marks
    // the provenance; the label renders only on `Planned` placeholders and the
    // stats screen is `Live`, so the owner label never renders.
    RouteSpec {
        id: RouteId::Stats,
        path: "/stats",
        label: "Stats",
        owner: Owner::ThisWave,
    },
];

/// Compile-time totality: one row per `RouteId`, in declaration order, and
/// `RouteId::ALL` in discriminant order. A new variant is already a compile
/// error in `paths::endpoints_for` and `nav::state_for` (both match
/// exhaustively), so no separate tripwire is needed here.
const _: () = {
    assert!(ROUTES.len() == RouteId::Stats as usize + 1);
    assert!(RouteId::ALL.len() == ROUTES.len());
    let mut i: usize = 0;
    while i < ROUTES.len() {
        assert!(RouteId::ALL[i] as usize == i);
        assert!(ROUTES[i].id as usize == i);
        i += 1;
    }
};

/// The route spec for an id.
///
/// In range for every `RouteId` that has a row: length, `RouteId::ALL` and
/// declaration order are pinned by the `const` block above, and a new variant is
/// a compile error in `paths::endpoints_for` and `nav::state_for` until both are
/// updated. The edit that updates both and adds no row compiles and panics at
/// the first use (Limitation 6, probe P6).
pub fn route(id: RouteId) -> &'static RouteSpec {
    &ROUTES[id as usize]
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
