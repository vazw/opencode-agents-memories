// Issue 114 — the restore's agent gate after a compaction.
//
// The oracle the issue-112 wave lacked. Every existing restore fixture feeds a
// `context` that carries an agent-bearing assistant row — the PRE-compaction
// transcript shape. The host never returns that shape after a compaction:
// `ctx.session.context` is bounded to the latest completed compaction
// (`packages/core/src/session/store.ts:194` → `history.ts:319-323`), so the
// slice at restore time is `[compaction, idle]` and holds no agent at all.
//
// The fixture below is therefore the live post-compaction slice PLUS an
// answerable `session.get`. That pairing is what makes the case non-vacuous:
// without `get` the input is just today's tree (nothing to test); with it, the
// host provably CAN answer the agent, so a `null` session is the wrapper never
// asking — the shipped defect (`18fea68`).
//
// In-process on purpose: `createLegacyClient` + `readSession` +
// `resolveSessionAgent` are the whole chain, and no engine or subprocess is
// needed to reach it (~40 ms).
//
// Measured red on `18fea68`: `readSession -> null`,
// `resolveSessionAgent -> undefined`.

import test from "node:test";
import assert from "node:assert/strict";

import { createLegacyClient } from "../src/v2/legacy-client.js";
import {
  isChildSession,
  readSession,
  resolveSessionAgent,
  resolveSessionModel,
} from "../src/compaction.js";

const SESSION = "ses-A";

// The live slice at restore time: the boundary rows only.
const slice = [
  { id: "msg-6863", type: "compaction" },
  { id: "msg-6866", type: "idle" },
];

// What the host answers for `session.get` (`Session.Info`,
// `packages/schema/src/session.ts:32-63`).
const row = {
  agent: "main",
  model: { providerID: "opencode-go", id: "deepseek-v4.1-flash", variant: "high" },
  location: { directory: "/workspace/project" },
};

function contextReturning(sessionRow) {
  return {
    location: { directory: "/workspace/project" },
    session: {
      context: async () => slice,
      get: async ({ sessionID }) => {
        assert.equal(sessionID, SESSION, "the client asks the host for this session by id");
        return sessionRow;
      },
    },
  };
}

test("test_restore_agent_resolves_from_the_session_row_after_a_compaction", async () => {
  const client = createLegacyClient(contextReturning(row), undefined);

  const session = await readSession(client, SESSION);
  assert.notEqual(session, null, "the session row is reachable through the client");

  assert.equal(
    await resolveSessionAgent(client, SESSION, session),
    "main",
    "the agent comes from the session row, never from a slice that holds none"
  );

  assert.deepEqual(
    resolveSessionModel(client, SESSION, session),
    { model: { providerID: "opencode-go", modelID: "deepseek-v4.1-flash" }, variant: "high" },
    "the model reference maps to the compaction send shape"
  );
});

test("test_restore_agent_identifies_a_child_session_from_the_parent_id", async () => {
  const child = await readSession(
    createLegacyClient(contextReturning({ parentID: "ses-parent" }), undefined),
    SESSION
  );
  assert.equal(isChildSession(child), true, "a non-empty parentID is positive evidence of a child");

  const topLevel = await readSession(
    createLegacyClient(contextReturning({ parentID: "" }), undefined),
    SESSION
  );
  assert.equal(
    isChildSession(topLevel),
    false,
    "an empty parentID is not a child (the positive-evidence rule)"
  );
});
