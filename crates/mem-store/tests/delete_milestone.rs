//! W6.4 — the milestone delete statement.
//!
//! The portability import's replace mode needs a `DELETE` that precedes
//! the milestone re-INSERT in the same atomic batch. The statement is
//! net-new domain SQL (there is no upstream oracle), so it lives in
//! `mem_store::episodic_sql` next to `INSERT_MILESTONE` and is covered
//! here.
//!
//! The delete is keyed by the milestone **id** alone: the import rewrites
//! the milestone's `project_id` to the TARGET's project, so a
//! project-scoped predicate would leave the source row behind.

mod common;

use mem_store::db::Value;
use mem_store::episodic_sql::{DELETE_MILESTONE, INSERT_MILESTONE, episodic_milestones_ddl};

use common::{cleanup_temp_dir, open_real_db, store_config};

/// The embedding rides as JSON text through `vector32(?)`; the fixture
/// store config uses two dimensions.
const VECTOR_TEXT: &str = "[0.0,0.0]";

/// The milestone id used by the fixture.
const MILESTONE_ID: &str = "milestone-1";

/// Inserts one milestone row.
async fn insert_milestone(db: &mem_store::db::DbHandle) {
    db.execute(
        INSERT_MILESTONE,
        vec![
            Value::Text(MILESTONE_ID.to_string()),
            Value::Text("project-1".to_string()),
            Value::Text("agent-1".to_string()),
            Value::Text("the summary".to_string()),
            Value::Text("[]".to_string()),
            Value::Integer(0),
            Value::Integer(1),
            Value::Integer(2),
            Value::Text(VECTOR_TEXT.to_string()),
            Value::Integer(3),
        ],
    )
    .await
    .expect("insert milestone");
}

/// The milestone ids in the table.
async fn milestone_ids(db: &mem_store::db::DbHandle) -> Vec<String> {
    let rows = db
        .query(
            "SELECT id FROM episodic_milestones ORDER BY id ASC",
            Vec::new(),
        )
        .await
        .expect("select milestones");
    rows.iter()
        .filter_map(|row| row.text("id"))
        .collect::<Vec<String>>()
}

#[tokio::test]
async fn test_delete_milestone_removes_the_row() {
    let config = store_config("delete-milestone");
    let db = open_real_db(&config, "episodic.db").await.expect("open");
    db.execute(
        &episodic_milestones_ddl(config.embedding_dimensions),
        Vec::new(),
    )
    .await
    .expect("milestones ddl");

    insert_milestone(&db).await;
    assert_eq!(milestone_ids(&db).await, vec![MILESTONE_ID.to_string()]);

    // The delete removes exactly the addressed row.
    let result = db
        .execute(
            DELETE_MILESTONE,
            vec![Value::Text(MILESTONE_ID.to_string())],
        )
        .await
        .expect("delete milestone");
    assert_eq!(result.changes, 1, "the row was removed");
    assert!(
        milestone_ids(&db).await.is_empty(),
        "the milestone table is empty after the delete"
    );

    // A second delete matches nothing (the statement is idempotent).
    let result = db
        .execute(
            DELETE_MILESTONE,
            vec![Value::Text(MILESTONE_ID.to_string())],
        )
        .await
        .expect("delete again");
    assert_eq!(result.changes, 0, "the second delete changes nothing");

    let _ = cleanup_temp_dir(&config.storage_path);
}

#[tokio::test]
async fn test_delete_milestone_leaves_the_other_rows() {
    let config = store_config("delete-milestone-scope");
    let db = open_real_db(&config, "episodic.db").await.expect("open");
    db.execute(
        &episodic_milestones_ddl(config.embedding_dimensions),
        Vec::new(),
    )
    .await
    .expect("milestones ddl");

    insert_milestone(&db).await;
    // A second row, in another project.
    db.execute(
        INSERT_MILESTONE,
        vec![
            Value::Text("milestone-2".to_string()),
            Value::Text("project-2".to_string()),
            Value::Text("agent-1".to_string()),
            Value::Text("the other summary".to_string()),
            Value::Text("[]".to_string()),
            Value::Integer(0),
            Value::Integer(1),
            Value::Integer(2),
            Value::Text(VECTOR_TEXT.to_string()),
            Value::Integer(3),
        ],
    )
    .await
    .expect("insert the second milestone");

    let result = db
        .execute(
            DELETE_MILESTONE,
            vec![Value::Text(MILESTONE_ID.to_string())],
        )
        .await
        .expect("delete milestone");
    assert_eq!(result.changes, 1);
    assert_eq!(
        milestone_ids(&db).await,
        vec!["milestone-2".to_string()],
        "the other milestone survived"
    );

    let _ = cleanup_temp_dir(&config.storage_path);
}
