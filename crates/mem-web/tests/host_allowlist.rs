//! D-8 — the Host-header allowlist seam (pure, no server).
//!
//! The upstream project has no counterpart for this file: the allowlist is a
//! DNS-rebinding hardening measure that the port adds on top of upstream's
//! CORS gate. Per `docs/agents/testing.md` a file with no upstream oracle gets
//! no registry row — this wave's test plan names it.

use mem_web::host_allowlist::HostAllowlist;

#[test]
fn test_host_allowlist_accepts_loopback_names_with_and_without_port() {
    let allowlist = HostAllowlist::for_bind_host("127.0.0.1");
    for host in [
        "localhost",
        "localhost:4747",
        "127.0.0.1",
        "127.0.0.1:4747",
        "[::1]",
        "[::1]:4747",
        "::1",
    ] {
        assert!(
            allowlist.allows(Some(host)),
            "the loopback name {host} must be allowed"
        );
    }
}

#[test]
fn test_host_allowlist_accepts_the_configured_bind_host() {
    let named = HostAllowlist::for_bind_host("mem.local");
    assert!(named.allows(Some("mem.local")));
    assert!(named.allows(Some("mem.local:4747")));
    assert!(!named.allows(Some("other.local")));

    let lan = HostAllowlist::for_bind_host("192.168.1.10");
    assert!(lan.allows(Some("192.168.1.10:4747")));
    assert!(lan.allows(Some("192.168.1.10")));
    assert!(!lan.allows(Some("192.168.1.11")));
}

#[test]
fn test_host_allowlist_rejects_foreign_host_names() {
    let allowlist = HostAllowlist::for_bind_host("127.0.0.1");
    for host in [
        "evil.example",
        "evil.example:4747",
        "localhost.evil.example",
        "127.0.0.1.evil.example",
    ] {
        assert!(
            !allowlist.allows(Some(host)),
            "the foreign name {host} must be refused"
        );
    }
}

#[test]
fn test_host_allowlist_rejects_missing_and_empty_host_header() {
    let allowlist = HostAllowlist::for_bind_host("127.0.0.1");
    assert!(!allowlist.allows(None));
    assert!(!allowlist.allows(Some("")));
    assert!(!allowlist.allows(Some("   ")));
}

#[test]
fn test_host_allowlist_normalizes_case_and_ipv6_brackets() {
    let allowlist = HostAllowlist::for_bind_host("127.0.0.1");
    assert!(allowlist.allows(Some("LOCALHOST:4747")));
    assert!(allowlist.allows(Some("LOCALHOST")));
    assert!(allowlist.allows(Some("[::1]")));
    assert!(allowlist.allows(Some("::1")));
    assert!(allowlist.allows(Some("[::1]:4747")));
}

#[test]
fn test_host_allowlist_is_not_enforced_for_wildcard_bind_host() {
    for wildcard in ["0.0.0.0", "::", "[::]"] {
        let allowlist = HostAllowlist::for_bind_host(wildcard);
        assert!(
            !allowlist.is_enforced(),
            "the wildcard bind {wildcard} must not enforce the allowlist"
        );
        assert!(allowlist.names().is_empty());
        assert!(allowlist.allows(Some("evil.example")));
        assert!(allowlist.allows(None));
    }

    let loopback = HostAllowlist::for_bind_host("127.0.0.1");
    assert!(loopback.is_enforced());
    assert_eq!(
        loopback.names(),
        &[
            "127.0.0.1".to_owned(),
            "localhost".to_owned(),
            "::1".to_owned()
        ]
    );
}
