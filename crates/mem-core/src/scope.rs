//! Memory-scope algebra, ported from upstream
//! `src/services/memory-scope.ts` (subject of
//! `tests/memory-scope-helper.test.ts`).
//!
//! A container tag has the frozen format `{prefix}_{user|project}_{16hex}`;
//! the hash is the first 16 hex characters of SHA-256 over an identity
//! string (see [`crate::sha256`]). Parsing branch order is the contract:
//! fewer than three segments → format error, then the scope segment is
//! validated, then the hash. No permissive fallbacks exist — rejection is
//! the contract, because a scope hash can later reach filesystem shard
//! paths (W1.2), so traversal-shaped inputs must never survive this seam.
//!
//! Documented divergence (blueprint): upstream returns plain
//! `string` hashes guarded by `assertSafeScopeHash` at call sites; here
//! [`ScopeHash`] is a newtype whose invariant (16 lowercase hex) makes
//! traversal segments structurally unrepresentable, and the wildcard
//! ("every shard of this scope", upstream's empty string) is
//! [`ScopeRef`] with `hash: None` instead of `hash: ""`.

use std::fmt;
use std::str::FromStr;

use crate::config::MemoryDefaultScope;
use crate::error::ScopeError;

/// A validated memory-scope hash: exactly 16 lowercase hex characters
/// (upstream `/^[a-f0-9]{16}$/`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ScopeHash(String);

impl ScopeHash {
    /// Validates and wraps a candidate hash (upstream
    /// `assertSafeScopeHash` returning the value instead of asserting).
    pub fn parse(candidate: &str) -> Result<Self, ScopeError> {
        if !is_valid_scope_hash(candidate) {
            return Err(ScopeError::InvalidScopeHash {
                hash: candidate.to_string(),
            });
        }
        Ok(Self(candidate.to_string()))
    }

    /// The validated 16-character hash string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ScopeHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for ScopeHash {
    type Err = ScopeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

/// The canonical memory scopes (upstream `"user" | "project"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    User,
    Project,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
        }
    }
}

/// The request-level scope selection (upstream `MemoryScope`:
/// `"project" | "all-projects"`; renamed to avoid colliding with
/// `config::MemoryScope`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopeSelection {
    Project,
    AllProjects,
}

impl ScopeSelection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Project => "project",
            Self::AllProjects => "all-projects",
        }
    }
}

impl From<MemoryDefaultScope> for ScopeSelection {
    fn from(value: MemoryDefaultScope) -> Self {
        match value {
            MemoryDefaultScope::Project => Self::Project,
            MemoryDefaultScope::AllProjects => Self::AllProjects,
        }
    }
}

/// A resolved memory-scope reference (upstream `MemoryScopeRef`).
/// `hash: None` is upstream's empty string — "every shard of this scope"
/// (the wildcard). It is a query selector, never a filesystem path
/// segment; shard-path builders must take the [`ScopeHash`] newtype.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeRef {
    pub scope: Scope,
    pub hash: Option<ScopeHash>,
}

/// Strict hash check: exactly 16 lowercase hex characters. Deliberately
/// does NOT use `char::is_ascii_hexdigit` — that accepts uppercase, while
/// the upstream regex does not.
pub fn is_valid_scope_hash(candidate: &str) -> bool {
    candidate.len() == 16
        && candidate
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Validates a scope hash, mirroring upstream `assertSafeScopeHash`
/// (throws `Invalid scope hash: ...`).
pub fn assert_safe_scope_hash(candidate: &str) -> Result<ScopeHash, ScopeError> {
    ScopeHash::parse(candidate)
}

/// Parses a container tag into its scope + hash (upstream
/// `extractScopeFromContainerTag`). Branch order is the contract:
/// fewer than three `_`-separated segments → [`ScopeError::InvalidContainerTag`];
/// the second-to-last segment must be `user`/`project` →
/// [`ScopeError::InvalidContainerTagScope`]; the last segment must pass the
/// strict hash check → [`ScopeError::InvalidContainerTagHash`]. The prefix
/// may itself contain underscores (only the last two segments are read).
pub fn extract_scope_from_container_tag(container_tag: &str) -> Result<ScopeRef, ScopeError> {
    split_container_tag(container_tag).map(|(scope, hash)| ScopeRef {
        scope,
        hash: Some(hash),
    })
}

/// Returns the scope **and** the validated hash, so a caller that needs both
/// (the HTTP guard) cannot fall back to an `Option` for the hash half.
/// Branch order is identical to [`extract_scope_from_container_tag`]; that
/// function delegates here so the two can never drift.
pub fn split_container_tag(container_tag: &str) -> Result<(Scope, ScopeHash), ScopeError> {
    let parts: Vec<&str> = container_tag.split('_').collect();
    if parts.len() < 3 {
        return Err(ScopeError::InvalidContainerTag {
            container_tag: container_tag.to_string(),
        });
    }

    let hash_segment = parts[parts.len() - 1];
    let scope_segment = parts[parts.len() - 2];

    let scope = match scope_segment {
        "user" => Scope::User,
        "project" => Scope::Project,
        _ => {
            return Err(ScopeError::InvalidContainerTagScope {
                scope: scope_segment.to_string(),
                container_tag: container_tag.to_string(),
            });
        }
    };

    let hash = match ScopeHash::parse(hash_segment) {
        Ok(hash) => hash,
        Err(_) => {
            return Err(ScopeError::InvalidContainerTagHash {
                container_tag: container_tag.to_string(),
            });
        }
    };

    Ok((scope, hash))
}

/// Non-throwing variant (upstream `tryExtractScopeFromContainerTag`).
pub fn try_extract_scope_from_container_tag(container_tag: &str) -> Option<ScopeRef> {
    extract_scope_from_container_tag(container_tag).ok()
}

/// Resolves the effective scope selection: an explicit request-level
/// argument beats the resolved config default (upstream
/// `args.scope ?? CONFIG.memory.defaultScope`; upstream's second fallback —
/// the client's `scope: MemoryScope = "project"` default parameter — is
/// inside `config_default`, because `Config` is always resolved with
/// `memory.defaultScope` defaulting to `Project`).
pub fn resolve_scope_selection(
    argument: Option<ScopeSelection>,
    config_default: ScopeSelection,
) -> ScopeSelection {
    argument.unwrap_or(config_default)
}

/// Expands a scope selection into the shard references to walk (upstream
/// `resolveMemoryScope`). `all-projects` spans both canonical scopes as
/// user-then-project wildcards — user-scope memories must not be silently
/// excluded; `project` resolves the container tag's own scope + hash.
pub fn resolve_memory_scope(
    selection: ScopeSelection,
    container_tag: &str,
) -> Result<Vec<ScopeRef>, ScopeError> {
    match selection {
        ScopeSelection::AllProjects => Ok(vec![
            ScopeRef {
                scope: Scope::User,
                hash: None,
            },
            ScopeRef {
                scope: Scope::Project,
                hash: None,
            },
        ]),
        ScopeSelection::Project => Ok(vec![extract_scope_from_container_tag(container_tag)?]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_container_tag_separates_scope_and_hash() {
        let (scope, hash) =
            split_container_tag("opencode_project_0123456789abcdef").expect("valid tag");
        assert_eq!(scope, Scope::Project);
        assert_eq!(hash.as_str(), "0123456789abcdef");

        assert!(split_container_tag("bad").is_err());
    }
}
