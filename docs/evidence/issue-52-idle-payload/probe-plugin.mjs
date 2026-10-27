// Issue 52 — the 52.5 probe plugin. NOT the repo's `plugin/` tree.
//
// A `chat.message` hook that pushes our own synthetic block onto the current
// user message's parts (mirroring `plugin/src/plugin.js`'s push at its
// `chat.message` hook), and an `event` hook that on `session.idle` calls
// `client.session.messages({ path: { id } })` and writes the raw response JSON
// to `$PROBE_OUT`.
//
// The question it answers: does the host return our own `synthetic: true`
// block inside `session.messages()`? If yes, the port's idle capture
// re-captures its own block (a self-feeding loop), and issue 66 must also drop
// the port's known block headers defensively.

export const server = async function server(input) {
  const client = input && input.client;
  const hooks = {
    "chat.message": async (_hookInput, output) => {
      try {
        if (output && output.message && Array.isArray(output.message.parts)) {
          output.message.parts.push({
            type: "text",
            text: "<probe-block>probe-block-marker</probe-block>",
            synthetic: true,
          });
        }
      } catch {
        // fail open: the probe must never break the host
      }
    },
    event: async (hookInput) => {
      try {
        const event =
          hookInput && hookInput.event && typeof hookInput.event === "object"
            ? hookInput.event
            : null;
        const properties = event ? event.properties : null;
        let sessionId = null;
        if (
          hookInput &&
          typeof hookInput.sessionID === "string" &&
          hookInput.sessionID.length > 0
        ) {
          sessionId = hookInput.sessionID;
        } else if (properties && typeof properties.sessionID === "string") {
          sessionId = properties.sessionID;
        }
        if (sessionId === null || !properties || properties.type !== "session.idle") {
          return;
        }
        const out = typeof process !== "undefined" ? process.env.PROBE_OUT : undefined;
        if (typeof out !== "string" || out.length === 0) return;
        if (!client || typeof client.session?.messages !== "function") return;
        const response = await client.session.messages({ path: { id: sessionId } });
        const fs = await import("node:fs");
        fs.writeFileSync(out, JSON.stringify(response));
      } catch {
        // fail open
      }
    },
  };
  return { hooks };
};
