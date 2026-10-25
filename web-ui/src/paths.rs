//! The pinned REST path table.
//!
//! Transcribed from `example/opencode-mem/src/services/web-server.ts` (the
//! routing table) plus the paths the upstream Svelte client actually calls,
//! plus the two W6.3 port additions (roadmap §W6.3). Every entry is a
//! `(method, template)` pair; `{id}` (and the W6.3 `{project_id}` /
//! `{session_id}`) is a percent-encoded path parameter.
//!
//! The UI adds no endpoint, no scope parameter and no second API surface:
//! this table is the whole contract.

use crate::routes::RouteId;

/// The pinned endpoints, by identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EndpointId {
    Health,
    ListTags,
    ListMemories,
    CreateMemory,
    UpdateMemory,
    DeleteMemory,
    BulkDeleteMemories,
    PinMemory,
    UnpinMemory,
    Search,
    Stats,
    Cleanup,
    Deduplicate,
    DetectMigration,
    DetectTagMigration,
    RunTagMigrationBatch,
    TagMigrationProgress,
    RunMigration,
    DeletePrompt,
    BulkDeletePrompts,
    UserProfile,
    UserProfileChangelog,
    UserProfileSnapshot,
    RefreshUserProfile,
    AiCleanupUserProfile,
    ApplyAiCleanupUserProfile,
    UpdateUserProfileItem,
    ListSkills,
    CreateSkill,
    GetSkill,
    UpdateSkill,
    DeleteSkill,
    ListProjectSessions,
    DeleteProjectSession,
}

/// One pinned endpoint: its identity, its HTTP method and its path template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndpointSpec {
    pub id: EndpointId,
    pub method: &'static str,
    pub template: &'static str,
}

/// The pinned `(method, template)` table.
pub const ENDPOINTS: &[EndpointSpec] = &[
    EndpointSpec {
        id: EndpointId::Health,
        method: "GET",
        template: "/api/health",
    },
    EndpointSpec {
        id: EndpointId::ListTags,
        method: "GET",
        template: "/api/tags",
    },
    EndpointSpec {
        id: EndpointId::ListMemories,
        method: "GET",
        template: "/api/memories",
    },
    EndpointSpec {
        id: EndpointId::CreateMemory,
        method: "POST",
        template: "/api/memories",
    },
    EndpointSpec {
        id: EndpointId::UpdateMemory,
        method: "PUT",
        template: "/api/memories/{id}",
    },
    EndpointSpec {
        id: EndpointId::DeleteMemory,
        method: "DELETE",
        template: "/api/memories/{id}",
    },
    EndpointSpec {
        id: EndpointId::BulkDeleteMemories,
        method: "POST",
        template: "/api/memories/bulk-delete",
    },
    EndpointSpec {
        id: EndpointId::PinMemory,
        method: "POST",
        template: "/api/memories/{id}/pin",
    },
    EndpointSpec {
        id: EndpointId::UnpinMemory,
        method: "POST",
        template: "/api/memories/{id}/unpin",
    },
    EndpointSpec {
        id: EndpointId::Search,
        method: "GET",
        template: "/api/search",
    },
    EndpointSpec {
        id: EndpointId::Stats,
        method: "GET",
        template: "/api/stats",
    },
    EndpointSpec {
        id: EndpointId::Cleanup,
        method: "POST",
        template: "/api/cleanup",
    },
    EndpointSpec {
        id: EndpointId::Deduplicate,
        method: "POST",
        template: "/api/deduplicate",
    },
    EndpointSpec {
        id: EndpointId::DetectMigration,
        method: "GET",
        template: "/api/migration/detect",
    },
    EndpointSpec {
        id: EndpointId::DetectTagMigration,
        method: "GET",
        template: "/api/migration/tags/detect",
    },
    EndpointSpec {
        id: EndpointId::RunTagMigrationBatch,
        method: "POST",
        template: "/api/migration/tags/run-batch",
    },
    EndpointSpec {
        id: EndpointId::TagMigrationProgress,
        method: "GET",
        template: "/api/migration/tags/progress",
    },
    EndpointSpec {
        id: EndpointId::RunMigration,
        method: "POST",
        template: "/api/migration/run",
    },
    EndpointSpec {
        id: EndpointId::DeletePrompt,
        method: "DELETE",
        template: "/api/prompts/{id}",
    },
    EndpointSpec {
        id: EndpointId::BulkDeletePrompts,
        method: "POST",
        template: "/api/prompts/bulk-delete",
    },
    EndpointSpec {
        id: EndpointId::UserProfile,
        method: "GET",
        template: "/api/user-profile",
    },
    EndpointSpec {
        id: EndpointId::UserProfileChangelog,
        method: "GET",
        template: "/api/user-profile/changelog",
    },
    EndpointSpec {
        id: EndpointId::UserProfileSnapshot,
        method: "GET",
        template: "/api/user-profile/snapshot",
    },
    EndpointSpec {
        id: EndpointId::RefreshUserProfile,
        method: "POST",
        template: "/api/user-profile/refresh",
    },
    EndpointSpec {
        id: EndpointId::AiCleanupUserProfile,
        method: "POST",
        template: "/api/user-profile/ai-cleanup",
    },
    EndpointSpec {
        id: EndpointId::ApplyAiCleanupUserProfile,
        method: "POST",
        template: "/api/user-profile/ai-cleanup/apply",
    },
    EndpointSpec {
        id: EndpointId::UpdateUserProfileItem,
        method: "PATCH",
        template: "/api/user-profile/item",
    },
    EndpointSpec {
        id: EndpointId::ListSkills,
        method: "GET",
        template: "/api/skills",
    },
    EndpointSpec {
        id: EndpointId::CreateSkill,
        method: "POST",
        template: "/api/skills",
    },
    EndpointSpec {
        id: EndpointId::GetSkill,
        method: "GET",
        template: "/api/skills/{id}",
    },
    EndpointSpec {
        id: EndpointId::UpdateSkill,
        method: "PUT",
        template: "/api/skills/{id}",
    },
    EndpointSpec {
        id: EndpointId::DeleteSkill,
        method: "DELETE",
        template: "/api/skills/{id}",
    },
    EndpointSpec {
        id: EndpointId::ListProjectSessions,
        method: "GET",
        template: "/api/projects/{project_id}/sessions",
    },
    EndpointSpec {
        id: EndpointId::DeleteProjectSession,
        method: "DELETE",
        template: "/api/projects/{project_id}/sessions/{session_id}",
    },
];

/// Compile-time totality: one row per `EndpointId`, in declaration order.
///
/// The `match` is exhaustive, so a variant missing from this list is `E0004`;
/// the length assert catches a forgotten row and the loop catches a reordered
/// table (which a length check alone cannot see). What it does NOT catch: a
/// variant added here *and* to the enum with its row still missing — that edit
/// compiles and fails at the first use of the id (see ADR-0016, Limitations).
const _: () = {
    match EndpointId::Health {
        EndpointId::Health
        | EndpointId::ListTags
        | EndpointId::ListMemories
        | EndpointId::CreateMemory
        | EndpointId::UpdateMemory
        | EndpointId::DeleteMemory
        | EndpointId::BulkDeleteMemories
        | EndpointId::PinMemory
        | EndpointId::UnpinMemory
        | EndpointId::Search
        | EndpointId::Stats
        | EndpointId::Cleanup
        | EndpointId::Deduplicate
        | EndpointId::DetectMigration
        | EndpointId::DetectTagMigration
        | EndpointId::RunTagMigrationBatch
        | EndpointId::TagMigrationProgress
        | EndpointId::RunMigration
        | EndpointId::DeletePrompt
        | EndpointId::BulkDeletePrompts
        | EndpointId::UserProfile
        | EndpointId::UserProfileChangelog
        | EndpointId::UserProfileSnapshot
        | EndpointId::RefreshUserProfile
        | EndpointId::AiCleanupUserProfile
        | EndpointId::ApplyAiCleanupUserProfile
        | EndpointId::UpdateUserProfileItem
        | EndpointId::ListSkills
        | EndpointId::CreateSkill
        | EndpointId::GetSkill
        | EndpointId::UpdateSkill
        | EndpointId::DeleteSkill
        | EndpointId::ListProjectSessions
        | EndpointId::DeleteProjectSession => {}
    }
    assert!(ENDPOINTS.len() == EndpointId::DeleteProjectSession as usize + 1);
    let mut i: usize = 0;
    while i < ENDPOINTS.len() {
        assert!(ENDPOINTS[i].id as usize == i);
        i += 1;
    }
};

/// The pinned spec for an id.
///
/// In range for every variant that has a row: `ENDPOINTS` holds one row per
/// variant in declaration order, so `id as usize` is a valid index there. The
/// check does not cover a variant added to the enum *and* to the tripwire with
/// no row — that edit compiles and panics at the first use of the id
/// (Limitation 1, probe P5).
pub fn endpoint(id: EndpointId) -> EndpointSpec {
    ENDPOINTS[id as usize]
}

const MEMORY_PATHS: &[&str] = &[
    "/api/memories",
    "/api/memories/{id}",
    "/api/memories/bulk-delete",
    "/api/memories/{id}/pin",
    "/api/memories/{id}/unpin",
    "/api/tags",
    "/api/search",
    "/api/stats",
];

const TAG_PATHS: &[&str] = &["/api/tags", "/api/memories", "/api/search"];

const PROFILE_PATHS: &[&str] = &[
    "/api/user-profile",
    "/api/user-profile/changelog",
    "/api/user-profile/snapshot",
    "/api/user-profile/refresh",
    "/api/user-profile/ai-cleanup",
    "/api/user-profile/ai-cleanup/apply",
    "/api/user-profile/item",
];

const SETTINGS_PATHS: &[&str] = &[
    "/api/stats",
    "/api/cleanup",
    "/api/deduplicate",
    "/api/migration/detect",
    "/api/migration/run",
    "/api/migration/tags/detect",
    "/api/migration/tags/run-batch",
    "/api/migration/tags/progress",
];

const EXPORT_IMPORT_PATHS: &[&str] = &[
    "/api/memories",
    "/api/tags",
    "/api/user-profile",
    "/api/prompts/{id}",
    "/api/prompts/bulk-delete",
];

const SKILL_PATHS: &[&str] = &["/api/skills", "/api/skills/{id}"];

const SESSION_PATHS: &[&str] = &[
    "/api/projects/{project_id}/sessions",
    "/api/projects/{project_id}/sessions/{session_id}",
];

const STATS_PATHS: &[&str] = &["/api/stats"];

/// The path templates a planned screen will consume, in a stable order.
pub fn endpoints_for(route: RouteId) -> &'static [&'static str] {
    match route {
        RouteId::Home => &[],
        RouteId::ProjectMemories => MEMORY_PATHS,
        RouteId::Tags => TAG_PATHS,
        RouteId::Profile => PROFILE_PATHS,
        RouteId::Settings => SETTINGS_PATHS,
        RouteId::ExportImport => EXPORT_IMPORT_PATHS,
        RouteId::SkillLibrary => SKILL_PATHS,
        RouteId::SessionHistory => SESSION_PATHS,
        RouteId::Stats => STATS_PATHS,
    }
}

/// A pinned path template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathTemplate(&'static str);

impl PathTemplate {
    pub const fn new(template: &'static str) -> Self {
        Self(template)
    }

    pub const fn as_str(self) -> &'static str {
        self.0
    }

    /// Fill the `{id}` parameter, percent-encoding the value.
    pub fn fill(self, id: &str) -> String {
        fill(self.0, id)
    }
}

impl std::fmt::Display for PathTemplate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// Fill a `{id}` path parameter, percent-encoding the value as a single path
/// segment (so `/` becomes `%2F` and a space becomes `%20`).
pub fn fill(template: &str, id: &str) -> String {
    template.replace("{id}", &encode_component(id))
}

/// Percent-encode an RFC 3986 component using the unreserved set only.
pub fn encode_component(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for &byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                encoded.push('%');
                encoded.push(HEX[(byte >> 4) as usize] as char);
                encoded.push(HEX[(byte & 0x0f) as usize] as char);
            }
        }
    }
    encoded
}
