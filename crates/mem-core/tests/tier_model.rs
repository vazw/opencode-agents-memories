//! W7.1 — the five-tier memory model and the ADR-0010 D-3 compression budget,
//! executable.
//!
//! The Vault row is the ADR table's, not a shipped crate (W7.2 owns the
//! crate): this wave pins the model, not the tier's storage.

use mem_core::tier::{CompressionLevel, FactTier, MemoryTier, compression_level_for};

#[test]
fn test_memory_tier_wire_strings() {
    let expected = [
        (MemoryTier::Profile, "profile"),
        (MemoryTier::Semantic, "semantic"),
        (MemoryTier::Procedural, "procedural"),
        (MemoryTier::Resource, "resource"),
        (MemoryTier::Episodic, "episodic"),
        (MemoryTier::Vault, "vault"),
    ];
    assert_eq!(MemoryTier::ALL.len(), 6);
    for (tier, wire) in expected {
        assert_eq!(tier.as_str(), wire);
        assert_eq!(MemoryTier::parse(wire), Some(tier));
    }
    // Round-trip over the whole set.
    for tier in MemoryTier::ALL {
        assert_eq!(MemoryTier::parse(tier.as_str()), Some(tier));
    }
    assert_eq!(MemoryTier::parse(""), None);
    assert_eq!(MemoryTier::parse("facts"), None);
    assert_eq!(MemoryTier::parse("Profile"), None);
}

/// ADR-0010 D-3: Profile→L2; Semantic/Procedural/Resource→L1; Episodic→L4;
/// Vault→L0.
#[test]
fn test_compression_budget_matches_adr_0010() {
    assert_eq!(
        compression_level_for(MemoryTier::Profile),
        CompressionLevel::L2
    );
    assert_eq!(
        compression_level_for(MemoryTier::Semantic),
        CompressionLevel::L1
    );
    assert_eq!(
        compression_level_for(MemoryTier::Procedural),
        CompressionLevel::L1
    );
    assert_eq!(
        compression_level_for(MemoryTier::Resource),
        CompressionLevel::L1
    );
    assert_eq!(
        compression_level_for(MemoryTier::Episodic),
        CompressionLevel::L4
    );
    assert_eq!(
        compression_level_for(MemoryTier::Vault),
        CompressionLevel::L0
    );
}

#[test]
fn test_compression_level_parse_table() {
    let accepted = [
        ("L0", CompressionLevel::L0),
        ("L1", CompressionLevel::L1),
        ("L2", CompressionLevel::L2),
        ("L3", CompressionLevel::L3),
        ("L4", CompressionLevel::L4),
    ];
    for (wire, level) in accepted {
        assert_eq!(CompressionLevel::parse(wire), Some(level));
        assert_eq!(level.as_str(), wire);
    }
    for rejected in ["l0", "L5", "", "L", "0"] {
        assert_eq!(CompressionLevel::parse(rejected), None, "{rejected:?}");
    }
}

#[test]
fn test_fact_tier_maps_to_its_memory_tier() {
    for fact_tier in [FactTier::Semantic, FactTier::Procedural, FactTier::Resource] {
        let tier = fact_tier.as_tier();
        assert!(tier.is_fact_tier(), "{tier:?} is a fact tier");
        assert_eq!(compression_level_for(tier), CompressionLevel::L1);
    }
    assert_eq!(FactTier::Semantic.as_tier(), MemoryTier::Semantic);
    assert_eq!(FactTier::Procedural.as_tier(), MemoryTier::Procedural);
    assert_eq!(FactTier::Resource.as_tier(), MemoryTier::Resource);

    // Non-fact tiers are exactly the other three.
    for tier in [MemoryTier::Profile, MemoryTier::Episodic, MemoryTier::Vault] {
        assert!(!tier.is_fact_tier(), "{tier:?} is not a fact tier");
    }
}
