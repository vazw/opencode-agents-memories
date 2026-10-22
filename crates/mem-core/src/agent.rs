//! The ADR-0009 agent-role contract: the closed role set, its wire header,
//! and the D-4 access matrix as executable code.
//!
//! # Why the role set is closed
//!
//! Q6 (owner decision): `AgentRole` is a closed enum. `AgentRole::parse`
//! enforces membership before any write, so no `CHECK` constraint is needed
//! on `agent_sessions.agent_role` — a `CHECK` never makes a database
//! unopenable (`apply_schema_migrations` compares `PRAGMA user_version`
//! only), and adding a role would cost an SQLite table rebuild for a
//! guarantee this parser already provides.
//!
//! # Why there is no extractor here
//!
//! D3 (owner decision): the wave ships the role type, the matrix and the
//! header name; it does **not** ship an axum extractor and enforces no role
//! on any surface. Enforcement without role↔credential binding would be
//! theatre. `AGENT_ROLE_HEADER` is the lowercase HTTP header name later
//! surfaces will read.

/// The lowercase HTTP header carrying the agent role.
pub const AGENT_ROLE_HEADER: &str = "x-agent-role";

/// The four agent roles (ADR-0009 D-4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AgentRole {
    /// The primary agent: owns the session, sees everything in scope.
    Main,
    /// An exploratory sub-agent: reads existing memory, writes its own
    /// episodic rows.
    Explorer,
    /// A reviewing sub-agent: scoped reads of episodic memory.
    Reviewer,
    /// A task sub-agent: reads facts, writes nothing but its own session.
    Worker,
}

impl AgentRole {
    /// Every role, in the ADR's row order.
    pub const ALL: [AgentRole; 4] = [
        AgentRole::Main,
        AgentRole::Explorer,
        AgentRole::Reviewer,
        AgentRole::Worker,
    ];

    /// The canonical wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Explorer => "explorer",
            Self::Reviewer => "reviewer",
            Self::Worker => "worker",
        }
    }

    /// Parses a wire string: surrounding whitespace is trimmed and the
    /// comparison is ASCII case-insensitive. Unknown values are rejected
    /// with an error that never echoes the input.
    #[allow(clippy::should_implement_trait)]
    pub fn parse(text: &str) -> Result<Self, AgentRoleError> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err(AgentRoleError::Invalid);
        }
        Self::ALL
            .iter()
            .copied()
            .find(|role| role.as_str().eq_ignore_ascii_case(trimmed))
            .ok_or(AgentRoleError::Invalid)
    }

    /// The ADR-0009 D-4 access row for this role.
    ///
    /// Column order: `profile | facts | own_episodic | other_episodic |
    /// vault`.
    pub fn access(self) -> AgentAccess {
        match self {
            Self::Main => AgentAccess {
                profile: Access::ReadWrite,
                facts: Access::ReadWrite,
                own_episodic: Access::ReadWrite,
                other_episodic: Access::Read,
                vault: Access::Read,
            },
            Self::Explorer => AgentAccess {
                profile: Access::Read,
                facts: Access::Read,
                own_episodic: Access::ReadWrite,
                other_episodic: Access::None,
                vault: Access::None,
            },
            Self::Reviewer => AgentAccess {
                profile: Access::Read,
                facts: Access::Read,
                own_episodic: Access::ReadScoped,
                other_episodic: Access::ReadScoped,
                vault: Access::None,
            },
            Self::Worker => AgentAccess {
                profile: Access::None,
                facts: Access::Read,
                own_episodic: Access::None,
                other_episodic: Access::None,
                vault: Access::None,
            },
        }
    }
}

impl std::fmt::Display for AgentRole {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for AgentRole {
    type Err = AgentRoleError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::parse(text)
    }
}

/// A rejected agent-role value.
///
/// `Display` is FIXED and never echoes the untrusted input: the value may
/// be attacker-controlled and echoing it would turn an error line into a log
/// injection / content-leak seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentRoleError {
    /// The value is not one of the four wire strings.
    Invalid,
}

impl std::fmt::Display for AgentRoleError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .write_str("invalid agent role (expected one of: main, explorer, reviewer, worker)")
    }
}

impl std::error::Error for AgentRoleError {}

/// One cell of the access matrix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// No access at all.
    None,
    /// Read everything in the scope.
    Read,
    /// Read only rows the caller itself owns or contributes to.
    ReadScoped,
    /// Read and write.
    ReadWrite,
}

impl Access {
    /// Whether reads are permitted at all.
    pub fn can_read(self) -> bool {
        !matches!(self, Self::None)
    }

    /// Whether writes are permitted.
    pub fn can_write(self) -> bool {
        matches!(self, Self::ReadWrite)
    }

    /// Whether reads are restricted to the caller's own rows.
    pub fn is_scoped(self) -> bool {
        matches!(self, Self::ReadScoped)
    }
}

/// One role's five-column access row (ADR-0009 D-4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentAccess {
    /// User-profile tier.
    pub profile: Access,
    /// Facts tier.
    pub facts: Access,
    /// The caller's own episodic rows.
    pub own_episodic: Access,
    /// Other agents' episodic rows.
    pub other_episodic: Access,
    /// The vault tier (not shipped in this wave; the matrix is the ADR's).
    pub vault: Access,
}
