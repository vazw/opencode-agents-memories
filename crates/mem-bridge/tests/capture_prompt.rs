//! W5.3 — AC2's prompt/schema/skip oracle: the capture summarizer seam
//! parses the frozen `{summary, type, tags}` shape, normalizes tags and
//! short-circuits `type: "skip"`, and the frozen schema/system prompt
//! literals are exported for the engine composition.

use mem_bridge::capture::{CaptureSummaryOutput, SkipReason};
use mem_bridge::capture_prompt::parse_capture_output;

#[test]
fn test_capture_summary_extraction_schema_and_skip() {
    // (a) A technical capture parses with normalized tags.
    let raw = r#"{"summary": "Fixed the login retry.","type": "bug","tags": ["Auth", " bug "]}"#;
    let parsed = parse_capture_output(raw).expect("captures");
    match parsed {
        CaptureSummaryOutput::Capture {
            summary,
            memory_type,
            tags,
            ..
        } => {
            assert_eq!(summary, "Fixed the login retry.");
            assert_eq!(memory_type, "bug");
            assert_eq!(tags, vec!["auth".to_string(), "bug".to_string()]);
        }
        CaptureSummaryOutput::Skip(_) => panic!("a technical capture must not be Skip"),
    }

    // (b) `type == "skip"` short-circuits: whatever the other fields say,
    // nothing tier of anything gets written for it.
    let skip = parse_capture_output(r#"{"summary": "", "type": "skip", "tags": []}"#)
        .expect("skip output parses");
    assert_eq!(
        skip,
        CaptureSummaryOutput::Skip(SkipReason::ProviderSkipped)
    );

    // (c) A capture with an EMPTY summary fails closed (never a blank row).
    let blank = parse_capture_output(r#"{"summary": "   ", "type": "bug", "tags": []}"#);
    assert!(blank.is_err(), "empty summary is rejected");

    // (d) The provider wrapped its output in prose: the first balanced
    // object parses.
    let fenced = "Here is the capture:\n```json\n{\"summary\":\"ok\",\"type\":\"feature\",\"tags\":[]}\n```\n";
    let parsed = parse_capture_output(fenced).expect("fenced JSON parses");
    assert!(matches!(parsed, CaptureSummaryOutput::Capture { .. }));

    // (e) Non-JSON garbage fails closed (a retryable summarizer failure,
    // never a blank row).
    assert!(parse_capture_output("gibberish without braces").is_err());

    // (f) The frozen literals exist for the composition root.
    assert!(mem_bridge::capture_prompt::CAPTURE_SYSTEM_PROMPT.contains("type=\"skip\""));
    assert!(mem_bridge::capture_prompt::CAPTURE_OUTPUT_SCHEMA.contains("\"tags\""));
}

/// The summarizer's episodic extension (`filesTouched`, `completedWork`,
/// `decisions`, `openThreads`) is ADVERTISED by the frozen schema, so the
/// parser must CARRY it — sanitized and bounded, because `mem-episodic`'s
/// validation REJECTS (never coerces) empty/NUL/oversized entries.
#[test]
fn test_capture_output_carries_episodic_extension_fields() {
    use mem_episodic::record::WorkKind;

    let raw = r#"{
        "summary": "Wired the retry path.",
        "type": "bugFix",
        "tags": ["Auth"],
        "filesTouched": [" crates/a.rs ", "", "bad\u0000path", "crates/b.rs"],
        "completedWork": [
            {"kind": "feature", "description": "added retry"},
            {"kind": "bugFix", "description": "fixed the leak"},
            {"kind": "command", "description": "ran the suite"},
            {"kind": "mystery", "description": "unknown kind still lands"},
            {"kind": "feature", "description": "   "}
        ],
        "decisions": [{"description": "use JWT for auth"}, {"description": "  "}],
        "openThreads": [" wire the drain ", "", "still open"]
    }"#;

    let parsed = parse_capture_output(raw).expect("the extension parses");
    let CaptureSummaryOutput::Capture { extension, .. } = parsed else {
        panic!("a technical capture must not be Skip");
    };

    assert_eq!(
        extension.files_touched,
        vec!["crates/a.rs".to_string(), "crates/b.rs".to_string()],
        "trimmed, empties dropped, NUL entries dropped"
    );
    assert_eq!(
        extension.completed_work.len(),
        4,
        "empty description dropped"
    );
    assert_eq!(extension.completed_work[0].kind, WorkKind::Feature);
    assert_eq!(extension.completed_work[1].kind, WorkKind::BugFix);
    assert_eq!(extension.completed_work[2].kind, WorkKind::Command);
    assert_eq!(
        extension.completed_work[3].kind,
        WorkKind::Other,
        "an unknown kind maps to Other instead of failing the whole capture"
    );
    assert_eq!(
        extension.completed_work[3].description,
        "unknown kind still lands"
    );
    assert_eq!(
        extension.decisions,
        vec![mem_episodic::record::Decision {
            description: "use JWT for auth".to_string()
        }]
    );
    assert_eq!(
        extension.open_threads,
        vec!["wire the drain".to_string(), "still open".to_string()]
    );
}
