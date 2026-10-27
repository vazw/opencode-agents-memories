//! W5.4 — cross-language descriptor drift guard (blueprint test 9).
//!
//! **`gate: no`, `testable: no`.** The shared fixture is an authored artifact:
//! `include_str!` of an absent fixture is a compile error, so this binary
//! cannot exist before `plugin/tests/fixtures/recall-tool.json` is authored and
//! cannot be red-then-green inside a step. It is green on authoring by
//! construction and becomes a real oracle from the second edit onward — it
//! fails if `to_json_schema()` changes without the fixture, or if the tool name
//! or description moves.

use mem_episodic::tool_schema::{
    RECALL_PROJECT_SESSIONS_DESCRIPTION, RECALL_PROJECT_SESSIONS_TOOL_NAME,
    RecallProjectSessionsArgs,
};
use serde_json::Value;

fn fixture() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugin/tests/fixtures/recall-tool.json"
    )))
    .expect("the shared descriptor fixture must parse")
}

#[test]
fn test_tool_descriptor_matches_the_shared_fixture() {
    let fixture = fixture();
    assert_eq!(fixture["name"], RECALL_PROJECT_SESSIONS_TOOL_NAME);
    assert_eq!(fixture["description"], RECALL_PROJECT_SESSIONS_DESCRIPTION);
    assert_eq!(
        RecallProjectSessionsArgs::to_json_schema(),
        fixture["input"],
        "the wrapper descriptor and the W4.2 schema must not drift"
    );
}
