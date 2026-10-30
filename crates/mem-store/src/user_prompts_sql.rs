//! `user_prompts` v2 column statements (issue 43 W5, ADR-0021).
//!
//! NET-NEW: upstream builds the same `ALTER TABLE user_prompts ADD COLUMN …`
//! strings in TypeScript inside `ensureUserPromptColumns`, so there is no
//! frozen byte contract to preserve here (unlike `sql.rs`, the byte-frozen
//! upstream SQL locus).
//!
//! The *apply* is deliberately not a
//! [`crate::schema_migrations::SchemaMigration`]: a static `Vec<Statement>`
//! cannot carry a `PRAGMA table_info` result, and a fresh file's v1 DDL
//! already creates these three columns — a static v2 would duplicate-ALTER
//! every fresh database. [`crate::schema_migrations::apply_user_prompts_schema`]
//! owns the probe and applies this table itself (the `apply_episodic_schema` /
//! `EPISODIC_MAX_KNOWN = 6` split precedent).

/// The v2 column set: `(name, declaration)` in ALTER order.
pub const USER_PROMPTS_V2_COLUMNS: [(&str, &str); 3] = [
    ("capture_attempts", "INTEGER DEFAULT 0"),
    ("provider_id", "TEXT"),
    ("model_id", "TEXT"),
];

/// One `ALTER TABLE user_prompts ADD COLUMN` statement.
pub fn user_prompts_v2_alter(name: &str, declaration: &str) -> String {
    format!("ALTER TABLE user_prompts ADD COLUMN {name} {declaration}")
}
