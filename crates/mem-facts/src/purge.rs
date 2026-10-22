//! Facts-tier erasure (W7.1, D6) — the ONE home of `purge_sources` (M21).
//!
//! # The contract (M17)
//!
//! `facts` and `fact_sources` are DISTINCT-row counts measured from the UNION
//! of the two predicates BEFORE any mutation:
//!
//! - the triple predicate `(project_id, session_id, agent_id)`, and
//! - the milestone predicate (every fact a pre-scanned, purged milestone
//!   records provenance for).
//!
//! Summing per-statement `changes` would double-count a fact matched by both.
//!
//! # The multi-source rule (M18)
//!
//! Purging one source deletes the whole fact, even when another source
//! survives, and with it EVERY provenance row of that fact. The shared
//! milestone is deleted by the episodic leg, so the surviving contributor's
//! summary is lost permanently; recovery is an explicit re-capture of that
//! session. Erasure wins over durability on purpose.
//!
//! # Scope (M21)
//!
//! Every statement is project-scoped. The orphan sweep in particular is
//! `WHERE project_id = ? AND fact_id NOT IN (SELECT id FROM facts WHERE
//! project_id = ?)` — a single-scope operation must never write across
//! projects.

use std::collections::BTreeSet;

use mem_store::db::{BatchMode, Statement, Value, query_one};

use crate::FactsError;
use crate::schema as sql;
use crate::store::FactsStore;

/// What one facts purge erased (counts only — never content).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FactsPurgeReport {
    /// Distinct fact rows removed.
    pub facts: u64,
    /// Distinct provenance rows removed.
    pub fact_sources: u64,
}

impl FactsPurgeReport {
    /// Whether nothing was erased.
    pub fn is_empty(&self) -> bool {
        self.facts == 0 && self.fact_sources == 0
    }
}

/// A comma-separated list of `n` bind placeholders (the ids are BOUND, never
/// interpolated).
fn placeholders(count: usize) -> String {
    std::iter::repeat_n("?", count)
        .collect::<Vec<_>>()
        .join(", ")
}

impl FactsStore {
    /// Erases one scope's facts and provenance in ONE write batch.
    ///
    /// `milestone_ids` are the milestone ids the episodic pre-scan found for
    /// the triple; each milestone's facts are deleted even when their
    /// session survives (so no `source_ref` dangles at a destroyed
    /// milestone). A second call returns zeros; an unknown scope returns
    /// zeros.
    pub async fn purge_sources(
        &self,
        project_id: &str,
        session_id: &str,
        agent_id: &str,
        milestone_ids: &[String],
    ) -> Result<FactsPurgeReport, FactsError> {
        let db = self.ready().await?;
        let mut deleted: BTreeSet<String> = BTreeSet::new();

        let triple_rows = db
            .query(
                sql::SELECT_PROVENANCE_FACT_IDS_FOR_TRIPLE,
                vec![
                    Value::Text(project_id.to_string()),
                    Value::Text(session_id.to_string()),
                    Value::Text(agent_id.to_string()),
                ],
            )
            .await?;
        for row in &triple_rows {
            if let Some(fact_id) = row.text("fact_id") {
                deleted.insert(fact_id);
            }
        }
        for milestone_id in milestone_ids {
            let rows = db
                .query(
                    sql::SELECT_PROVENANCE_FACT_IDS_FOR_MILESTONE,
                    vec![
                        Value::Text(project_id.to_string()),
                        Value::Text(milestone_id.clone()),
                    ],
                )
                .await?;
            for row in &rows {
                if let Some(fact_id) = row.text("fact_id") {
                    deleted.insert(fact_id);
                }
            }
        }
        let ids: Vec<String> = deleted.into_iter().collect();

        // Count BEFORE mutating: distinct rows from the union (M17).
        let facts = if ids.is_empty() {
            0
        } else {
            let statement = format!(
                "SELECT COUNT(*) AS count FROM facts WHERE id IN ({})",
                placeholders(ids.len())
            );
            let params = ids
                .iter()
                .map(|id| Value::Text(id.clone()))
                .collect::<Vec<_>>();
            query_one(db.as_ref(), &statement, params)
                .await?
                .and_then(|row| row.i64("count"))
                .unwrap_or(0)
                .max(0) as u64
        };
        let fact_sources = {
            let mut params = vec![
                Value::Text(project_id.to_string()),
                Value::Text(session_id.to_string()),
                Value::Text(agent_id.to_string()),
            ];
            let mut statement = String::from(
                "SELECT COUNT(*) AS count FROM fact_provenance WHERE (project_id = ? AND session_id = ? AND agent_id = ?)",
            );
            if !ids.is_empty() {
                statement.push_str(&format!(" OR fact_id IN ({})", placeholders(ids.len())));
                params.extend(ids.iter().map(|id| Value::Text(id.clone())));
            }
            query_one(db.as_ref(), &statement, params)
                .await?
                .and_then(|row| row.i64("count"))
                .unwrap_or(0)
                .max(0) as u64
        };

        let triple = vec![
            Value::Text(project_id.to_string()),
            Value::Text(session_id.to_string()),
            Value::Text(agent_id.to_string()),
        ];
        let mut statements = Vec::new();
        for id in &ids {
            statements.push(Statement::new(
                sql::DELETE_FACT_BY_ID,
                vec![Value::Text(id.clone())],
            ));
        }
        for id in &ids {
            let mut params = triple.clone();
            params.push(Value::Text(id.clone()));
            statements.push(Statement::new(
                sql::DELETE_PROVENANCE_BY_TRIPLE_OR_FACT,
                params,
            ));
        }
        // Also remove triple rows whose fact id was absent from `facts` (an
        // orphan provenance row) — the per-id statements above only cover
        // existing ids.
        statements.push(Statement::new(sql::DELETE_PROVENANCE_BY_TRIPLE, triple));
        statements.push(Statement::new(
            sql::DELETE_ORPHAN_PROVENANCE_SCOPED,
            vec![
                Value::Text(project_id.to_string()),
                Value::Text(project_id.to_string()),
            ],
        ));
        db.batch(statements, BatchMode::Write).await?;

        Ok(FactsPurgeReport {
            facts,
            fact_sources,
        })
    }
}
