//! W8a — `vault_source_ref` helper contract (AC-8).

use mem_bridge::vault_source_ref::{VaultSourceKind, vault_source_ref};

// TODO(W8b): equality half (producer output == purge erasure input) deferred
// until the purge vault leg lands.
#[test]
fn test_vault_source_ref_format_and_determinism() {
    let id = "proj:session:agent";
    let first = vault_source_ref(VaultSourceKind::Session, id);
    let second = vault_source_ref(VaultSourceKind::Session, id);
    assert_eq!(first, format!("session:{id}"));
    assert_eq!(first, second, "deterministic across calls");
    // other kinds map to their tagged prefix
    assert_eq!(
        vault_source_ref(VaultSourceKind::Milestone, "m1"),
        "milestone:m1"
    );
    assert_eq!(vault_source_ref(VaultSourceKind::Fact, "f1"), "fact:f1");
    assert_eq!(vault_source_ref(VaultSourceKind::Manual, "x"), "manual:x");
}
