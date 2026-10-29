//! NET-NEW registry statements for issue 43 W4 (ADR-0022).
//!
//! These statements have **no upstream counterpart**: `sql.rs` is the
//! verbatim-upstream locus pinned by `golden_sql.rs`, so the W4 additions
//! live here instead.
//!
//! The invariant they enforce is *exactly one active shard per
//! `(scope, scope_hash)`*:
//!
//! - [`SHARDS_REPAIR_EXTRA_ACTIVE`] retires every active row that has a
//!   higher-index active row in its own tuple (the survivor is the row
//!   `GET_ACTIVE_SHARD` already returned).
//! - [`SHARDS_ONE_ACTIVE_INDEX`] is the database backstop: a partial unique
//!   index over the active rows only.
//! - [`MARK_SHARD_INACTIVE_IN_SCOPE`] is the rotation's retire, scoped so a
//!   stale or foreign id can never deactivate another tenant's shard.
//! - [`SET_SHARD_ACTIVE`] is the resolver's reactivation of the highest
//!   registered row of a zero-active tuple.

/// The constraint: at most one active row per `(scope, scope_hash)`.
///
/// `IF NOT EXISTS` keeps the v2 migration re-runnable after a crash between
/// the batch commit and the version pragma.
pub const SHARDS_ONE_ACTIVE_INDEX: &str = r#"
      CREATE UNIQUE INDEX IF NOT EXISTS idx_shards_one_active
      ON shards(scope, scope_hash) WHERE is_active = 1
    "#;

/// The v2 repair, run in the SAME transaction as the index.
///
/// Retires every active row that has a higher-index active row in its own
/// tuple; the highest-index row survives — the row `GET_ACTIVE_SHARD`
/// (`ORDER BY shard_index DESC LIMIT 1`), the legacy migrator's
/// highest-index map and the inventory default already chose.
pub const SHARDS_REPAIR_EXTRA_ACTIVE: &str = r#"
      UPDATE shards SET is_active = 0
      WHERE is_active = 1
        AND EXISTS (
          SELECT 1 FROM shards AS newer
          WHERE newer.scope = shards.scope
            AND newer.scope_hash = shards.scope_hash
            AND newer.is_active = 1
            AND newer.shard_index > shards.shard_index
        )
    "#;

/// The rotation's retire: by id AND scope, never the upstream id-only
/// `MARK_SHARD_INACTIVE` (which would let a stale id deactivate another
/// tenant's shard).
pub const MARK_SHARD_INACTIVE_IN_SCOPE: &str =
    r#"UPDATE shards SET is_active = 0 WHERE id = ? AND scope = ? AND scope_hash = ?"#;

/// The reactivation of a zero-active tuple's highest-index row.
pub const SET_SHARD_ACTIVE: &str = r#"UPDATE shards SET is_active = 1 WHERE id = ?"#;
