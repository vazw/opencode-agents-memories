//! W7.1 — launch modes: the MCP spellings parse, report unimplemented, and
//! the existing modes keep their behaviour. The process-level refusal (exit
//! 78 before any config read) lives in `tests/workspace_smoke.rs`.

use mem_bridge::engine::LaunchMode;

#[test]
fn test_mcp_modes_parse_and_report_unimplemented() {
    for (text, expected) in [
        ("mcp-stdio", LaunchMode::McpStdio),
        ("mcp-http", LaunchMode::McpHttp),
    ] {
        let mode = LaunchMode::parse(text).expect("the MCP spellings parse");
        assert_eq!(mode, expected);
        assert_eq!(mode.as_str(), text, "as_str round-trips");
        assert!(
            !mode.is_implemented(),
            "{text} is recognised but not implemented until W7.6"
        );
        assert!(!mode.installs_signal_handlers(), "{text} installs nothing");
    }
}

#[test]
fn test_existing_modes_are_unchanged() {
    let stdio = LaunchMode::parse("stdio").expect("stdio parses");
    assert_eq!(stdio, LaunchMode::Stdio);
    assert_eq!(stdio.as_str(), "stdio");
    assert!(stdio.is_implemented());
    assert!(!stdio.installs_signal_handlers());
    assert_eq!(LaunchMode::default(), LaunchMode::Stdio);

    let standalone = LaunchMode::parse("standalone").expect("standalone parses");
    assert_eq!(standalone, LaunchMode::Standalone);
    assert_eq!(standalone.as_str(), "standalone");
    assert!(standalone.is_implemented());
    assert!(standalone.installs_signal_handlers());

    // The env-gated test mode keeps its gate; when the gate IS set its
    // behaviour is covered by `tests/engine_shutdown.rs`.
    let gated = std::env::var("OPENCODE_MEM_HANG_ON_SHUTDOWN").is_ok();
    match LaunchMode::parse("hang-on-shutdown") {
        Ok(mode) => {
            assert!(gated, "the env gate still governs hang-on-shutdown");
            assert_eq!(mode, LaunchMode::HangOnShutdown);
            assert!(mode.is_implemented());
            assert!(!mode.installs_signal_handlers());
        }
        Err(message) => {
            assert!(!gated, "the gate is unset, so the mode is rejected");
            assert!(message.contains("hang-on-shutdown"), "{message}");
        }
    }
}

#[test]
fn test_unknown_mode_message_lists_mcp_modes() {
    let message = LaunchMode::parse("bogus").expect_err("an unknown mode is rejected");
    assert_eq!(
        message,
        "unknown mode \"bogus\" (stdio | standalone | hang-on-shutdown | mcp-stdio | mcp-http, env-gated)"
    );
    assert!(message.contains("mcp-stdio"));
    assert!(message.contains("mcp-http"));
    assert!(message.contains("env-gated"));
}
