//! W4.3 — agent attribution survives compaction (AC-1, PARTIAL port of
//! upstream `tests/compaction-agent-preservation.test.ts`).
//!
//! The upstream file drives `resolveSessionAgent` through a mocked SDK
//! client. This port exposes the same decision as a PURE function
//! ([`mem_episodic::attribution::resolve_session_agent`]) that takes the
//! session agent and the message list, so the precedence is asserted
//! directly. The precedence matches upstream `resolveSessionAgent`
//! (`example/opencode-mem/src/index.ts` L75-140) EXACTLY (D-29).
//!
//! Disposition: PARTIAL port. The host-bound cases of the upstream file
//! (`noReply`, the `synthetic` part flag, Tags-line dedup and the
//! model/variant passthrough) are deferred to W5.3 and W5.4.

use mem_episodic::attribution::{
    MessageInfo, MessageRole, compaction_injection_agent, resolve_session_agent,
};

fn user(agent: Option<&str>) -> MessageInfo {
    MessageInfo {
        role: MessageRole::User,
        agent: agent.map(str::to_string),
        mode: None,
        is_compaction_summary: false,
    }
}

fn assistant(agent: Option<&str>, mode: Option<&str>) -> MessageInfo {
    MessageInfo {
        role: MessageRole::Assistant,
        agent: agent.map(str::to_string),
        mode: mode.map(str::to_string),
        is_compaction_summary: false,
    }
}

fn compaction_summary(agent: Option<&str>, mode: Option<&str>) -> MessageInfo {
    MessageInfo {
        role: MessageRole::Assistant,
        agent: agent.map(str::to_string),
        mode: mode.map(str::to_string),
        is_compaction_summary: true,
    }
}

#[test]
fn test_resolve_session_agent_prefers_session_agent_over_messages() {
    // upstream: "prefers session.get().agent when available"
    let messages = [user(Some("build"))];
    assert_eq!(
        resolve_session_agent(Some("orchestrator"), &messages),
        Some("orchestrator".to_string())
    );
}

#[test]
fn test_resolve_session_agent_falls_back_to_latest_user_message_agent() {
    // upstream: "falls back to the latest user message agent"
    let messages = [
        user(Some("build")),
        assistant(None, Some("build")),
        user(Some("my-orchestrator")),
    ];
    assert_eq!(
        resolve_session_agent(None, &messages),
        Some("my-orchestrator".to_string())
    );
}

#[test]
fn test_resolve_session_agent_uses_latest_assistant_mode_when_no_user_agent() {
    // upstream: "uses assistant mode when no user agent is present"
    let messages = [
        assistant(None, Some("build")),
        compaction_summary(None, Some("compaction")),
    ];
    assert_eq!(
        resolve_session_agent(None, &messages),
        Some("build".to_string())
    );
}

#[test]
fn test_resolve_session_agent_skips_compaction_summary_messages() {
    // upstream: "skips compaction summary messages" — ported in BOTH upstream
    // skip shapes (`summary === true` and `mode === "compaction"`).
    let by_summary_flag = [
        user(Some("custom-agent")),
        compaction_summary(None, Some("build")),
    ];
    assert_eq!(
        resolve_session_agent(None, &by_summary_flag),
        Some("custom-agent".to_string())
    );

    let by_compaction_mode = [
        user(Some("custom-agent")),
        assistant(None, Some("compaction")),
    ];
    assert_eq!(
        resolve_session_agent(None, &by_compaction_mode),
        Some("custom-agent".to_string())
    );
}

#[test]
fn test_resolve_session_agent_prefers_newer_assistant_over_older_user_message() {
    // Upstream runs ONE newest->oldest scan, so the newer assistant message
    // outranks the older user message. A two-pass implementation would
    // wrongly return "build".
    let messages = [user(Some("build")), assistant(None, Some("orchestrator"))];
    assert_eq!(
        resolve_session_agent(None, &messages),
        Some("orchestrator".to_string())
    );
}

#[test]
fn test_resolve_session_agent_prefers_assistant_agent_over_mode() {
    // Upstream's v2 assistant `agent` is preferred over `mode`.
    let messages = [assistant(Some("v2-agent"), Some("build"))];
    assert_eq!(
        resolve_session_agent(None, &messages),
        Some("v2-agent".to_string())
    );
}

#[test]
fn test_resolve_session_agent_skips_assistant_without_agent_or_mode() {
    // An assistant message with NEITHER a non-blank agent NOR a non-blank
    // mode contributes nothing and the scan CONTINUES.
    let messages = [
        user(Some("build")),
        MessageInfo {
            role: MessageRole::Assistant,
            agent: Some("   ".to_string()),
            mode: Some(String::new()),
            is_compaction_summary: false,
        },
    ];
    assert_eq!(
        resolve_session_agent(None, &messages),
        Some("build".to_string())
    );
}

#[test]
fn test_resolve_session_agent_returns_trimmed_agent() {
    // A session agent of "  build  " -> "build".
    assert_eq!(
        resolve_session_agent(Some("  build  "), &[]),
        Some("build".to_string())
    );
    // A message agent of "  build  " -> "build".
    let messages = [user(Some("  build  "))];
    assert_eq!(
        resolve_session_agent(None, &messages),
        Some("build".to_string())
    );
    // A whitespace-only agent is blank: the session agent falls through to
    // the message scan.
    let messages = [user(Some("build"))];
    assert_eq!(
        resolve_session_agent(Some("   "), &messages),
        Some("build".to_string())
    );
    // A whitespace-only message agent is blank: the scan continues.
    let messages = [user(Some("   ")), user(Some("build"))];
    assert_eq!(
        resolve_session_agent(None, &messages),
        Some("build".to_string())
    );
}

#[test]
fn test_resolve_session_agent_returns_none_when_no_agent_is_resolvable() {
    let messages = [MessageInfo {
        role: MessageRole::Other,
        agent: Some("ignored".to_string()),
        mode: Some("ignored".to_string()),
        is_compaction_summary: false,
    }];
    assert_eq!(resolve_session_agent(None, &messages), None);
    assert_eq!(resolve_session_agent(Some("  "), &[]), None);
}

#[test]
fn test_compaction_injection_passes_resolved_agent() {
    assert_eq!(
        compaction_injection_agent(true, 3, Some("build")),
        Some("build")
    );
}

#[test]
fn test_compaction_injection_noop_when_agent_unresolved() {
    assert_eq!(compaction_injection_agent(true, 3, None), None);
    assert_eq!(compaction_injection_agent(true, 3, Some("   ")), None);
}

#[test]
fn test_compaction_injection_noop_when_disabled_or_no_memories() {
    assert_eq!(compaction_injection_agent(false, 3, Some("build")), None);
    assert_eq!(compaction_injection_agent(true, 0, Some("build")), None);
}
