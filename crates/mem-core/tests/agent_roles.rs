//! W7.1 — the ADR-0009 agent-role contract: the closed role set, the wire
//! header, and the D-4 access matrix.
//!
//! Behaviour is asserted through the public seam only (`AgentRole::{parse,
//! as_str, access, ALL}` and `Access`'s predicates). No axum extractor is
//! involved anywhere (D3).

use mem_core::agent::{AGENT_ROLE_HEADER, Access, AgentAccess, AgentRole};

fn access_of(role: AgentRole) -> AgentAccess {
    role.access()
}

#[test]
fn test_agent_role_parse_table() {
    // Accepted: the four wire strings, case-insensitively and trimmed.
    let cases = [
        ("main", AgentRole::Main),
        ("MAIN", AgentRole::Main),
        ("  main  ", AgentRole::Main),
        ("explorer", AgentRole::Explorer),
        ("Explorer", AgentRole::Explorer),
        ("reviewer", AgentRole::Reviewer),
        ("REVIEWER", AgentRole::Reviewer),
        ("worker", AgentRole::Worker),
        ("\tworker\t", AgentRole::Worker),
    ];
    for (text, expected) in cases {
        assert_eq!(
            AgentRole::parse(text).ok(),
            Some(expected),
            "parse({text:?}) must accept the wire string"
        );
    }

    for rejected in ["", "   ", "boss", "mai", "worker ", "\u{0}worker"] {
        if rejected == "worker " {
            // Trailing whitespace is trimmed and therefore accepted.
            assert_eq!(AgentRole::parse(rejected).ok(), Some(AgentRole::Worker));
            continue;
        }
        assert!(
            AgentRole::parse(rejected).is_err(),
            "parse({rejected:?}) must be rejected"
        );
    }
}

#[test]
fn test_agent_role_as_str_roundtrip() {
    for role in AgentRole::ALL {
        assert_eq!(
            AgentRole::parse(role.as_str()),
            Ok(role),
            "{role:?} must round-trip through its wire string"
        );
    }
    assert_eq!(AgentRole::Main.as_str(), "main");
    assert_eq!(AgentRole::Explorer.as_str(), "explorer");
    assert_eq!(AgentRole::Reviewer.as_str(), "reviewer");
    assert_eq!(AgentRole::Worker.as_str(), "worker");
    assert_eq!(AGENT_ROLE_HEADER, "x-agent-role");
    // The header is the lowercase HTTP form.
    assert_eq!(AGENT_ROLE_HEADER, AGENT_ROLE_HEADER.to_ascii_lowercase());
}

/// ADR-0009 D-4, column order:
/// `profile | facts | own_episodic | other_episodic | vault`.
#[test]
fn test_access_matrix_matches_adr_0009_matrix() {
    use Access::{None as N, Read as R, ReadScoped as RS, ReadWrite as RW};

    let expected = [
        (AgentRole::Main, [RW, RW, RW, R, R]),
        (AgentRole::Explorer, [R, R, RW, N, N]),
        (AgentRole::Reviewer, [R, R, RS, RS, N]),
        (AgentRole::Worker, [N, R, N, N, N]),
    ];
    for (role, columns) in expected {
        let AgentAccess {
            profile,
            facts,
            own_episodic,
            other_episodic,
            vault,
        } = access_of(role);
        assert_eq!(
            [profile, facts, own_episodic, other_episodic, vault],
            columns,
            "ADR-0009 D-4 row for {role:?}"
        );
    }

    // The predicate semantics of each cell kind.
    assert!(!Access::None.can_read() && !Access::None.can_write() && !Access::None.is_scoped());
    assert!(Access::Read.can_read() && !Access::Read.can_write() && !Access::Read.is_scoped());
    assert!(Access::ReadScoped.can_read() && !Access::ReadScoped.can_write());
    assert!(Access::ReadScoped.is_scoped());
    assert!(Access::ReadWrite.can_read() && Access::ReadWrite.can_write());
    assert!(!Access::ReadWrite.is_scoped());
}

#[test]
fn test_agent_role_error_never_echoes_the_input() {
    let secret = "top-secret-agent-role-xyz";
    let error = AgentRole::parse(secret).expect_err("an unknown role is rejected");
    let rendered = error.to_string();
    assert!(
        !rendered.contains(secret),
        "the error must not echo the untrusted input: {rendered}"
    );
    assert!(
        !rendered.contains("top-secret"),
        "the error must not echo any part of the input: {rendered}"
    );
    assert!(!rendered.trim().is_empty());
}

#[test]
fn test_agent_role_order_is_stable() {
    assert_eq!(
        AgentRole::ALL,
        [
            AgentRole::Main,
            AgentRole::Explorer,
            AgentRole::Reviewer,
            AgentRole::Worker
        ]
    );
    let wire: Vec<&str> = AgentRole::ALL.iter().map(|role| role.as_str()).collect();
    assert_eq!(wire, ["main", "explorer", "reviewer", "worker"]);
}
