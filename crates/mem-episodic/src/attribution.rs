//! Agent attribution (W4.3, D-29) — the pure port of upstream
//! `resolveSessionAgent` (`example/opencode-mem/src/index.ts` L75-140).
//!
//! Compaction must not reset the active agent, so the consolidation lane
//! needs to know WHICH agent a session belonged to. Upstream resolves that
//! from the live SDK client; this port resolves it from the same inputs the
//! upstream function reads, with the SAME precedence:
//!
//! 1. **the session agent wins outright** — returned trimmed, with no
//!    message scanned;
//! 2. otherwise **ONE newest-to-oldest scan** over the messages, where
//!    - a **user** message contributes its `agent`;
//!    - an **assistant** message is SKIPPED when it is a compaction summary
//!      (`summary === true`) or its mode is `"compaction"`; otherwise it
//!      contributes `agent ?? mode` (upstream's v2 assistant `agent` is
//!      preferred over `mode`);
//!    - every other role is skipped;
//! 3. otherwise `None`.
//!
//! The single scan is load-bearing: upstream runs ONE loop, so a NEWER
//! assistant message outranks an OLDER user message. A two-pass
//! implementation would return the older user agent and would lose the v2
//! assistant `agent` — exactly the identity this feature preserves.

/// The role of one conversation message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MessageRole {
    /// A user turn.
    User,
    /// An assistant turn.
    Assistant,
    /// Any other role (tool calls, system turns, …) — always skipped.
    #[default]
    Other,
}

/// The attribution-relevant fields of one message.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MessageInfo {
    /// The message role.
    pub role: MessageRole,
    /// The agent that produced the message (v2 hosts).
    pub agent: Option<String>,
    /// The mode the message ran under (v1 hosts).
    pub mode: Option<String>,
    /// Whether the message IS a compaction summary.
    pub is_compaction_summary: bool,
}

/// A trimmed, non-blank value (`None` when the value is absent or blank).
fn non_blank(value: &Option<String>) -> Option<String> {
    value
        .as_deref()
        .map(str::trim)
        .filter(|trimmed| !trimmed.is_empty())
        .map(str::to_string)
}

/// Resolves the session's active agent.
///
/// `session_agent` is the host's session-level agent (`session.get().agent`
/// upstream); `messages` is the session's message list in chronological
/// order (the scan runs newest-to-oldest).
///
/// The returned value is TRIMMED, exactly like upstream.
pub fn resolve_session_agent(
    session_agent: Option<&str>,
    messages: &[MessageInfo],
) -> Option<String> {
    // 1. The session agent wins outright — no message is scanned.
    if let Some(agent) = session_agent
        .map(str::trim)
        .filter(|agent| !agent.is_empty())
    {
        return Some(agent.to_string());
    }

    // 2. ONE newest-to-oldest scan.
    for message in messages.iter().rev() {
        match message.role {
            MessageRole::User => {
                if let Some(agent) = non_blank(&message.agent) {
                    return Some(agent);
                }
            }
            MessageRole::Assistant => {
                // Upstream skips compaction summaries and compaction-mode
                // assistant messages entirely.
                if message.is_compaction_summary || message.mode.as_deref() == Some("compaction") {
                    continue;
                }
                if let Some(agent) = non_blank(&message.agent) {
                    return Some(agent);
                }
                if let Some(mode) = non_blank(&message.mode) {
                    return Some(mode);
                }
                // NEITHER a non-blank agent NOR a non-blank mode: the message
                // contributes nothing and the scan CONTINUES.
            }
            MessageRole::Other => {}
        }
    }

    // 3. Nothing resolved.
    None
}

/// The agent to inject into a compaction request.
///
/// The injection happens if and only if the injection is enabled, at least
/// one memory is being injected, and the agent resolved to a non-blank
/// value.
pub fn compaction_injection_agent(
    enabled: bool,
    memory_count: usize,
    agent: Option<&str>,
) -> Option<&str> {
    if !enabled || memory_count == 0 {
        return None;
    }
    agent.map(str::trim).filter(|agent| !agent.is_empty())
}
