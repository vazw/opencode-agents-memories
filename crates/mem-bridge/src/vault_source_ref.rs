//! Single construction site for vault `source_ref` strings (W8a).
//!
//! `mem-vault` stores `source_ref` opaquely and validates shape only; the
//! tagged grammar lives here so the purge erasure leg (W8b) can match what
//! the feeder wrote. `vault_source_ref` is the ONLY place that builds these
//! strings — a second construction site would let the erasure leg silently
//! no-op.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultSourceKind {
    Milestone,
    Session,
    Fact,
    Manual,
}

impl VaultSourceKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Milestone => "milestone",
            Self::Session => "session",
            Self::Fact => "fact",
            Self::Manual => "manual",
        }
    }
}

pub fn vault_source_ref(kind: VaultSourceKind, id: &str) -> String {
    format!("{}:{}", kind.as_str(), id)
}
