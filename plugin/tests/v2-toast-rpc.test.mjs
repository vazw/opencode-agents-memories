// W5.2 `js` shard — the toast RPC contract, ported from
// example/opencode-mem/src/v2/toast-rpc.ts. Upstream has no test file for it,
// so all four cases are port additions (blueprint D6.2): the descriptor shape
// (which the TUI companion subscribes to by identity), the `{ body }`
// normalizer the V1 client capability feeds it, the blank-message reject, and
// the variant/duration filtering that keeps a hostile payload off the host UI.
//
// The module is imported dynamically in each case so a missing module reports
// once per case (ERR_MODULE_NOT_FOUND) instead of failing the file at load.

import test from "node:test";
import assert from "node:assert/strict";

const TOAST_MODULE = "../src/v2/toast-rpc.js";

test("test_v2_toast_rpc_descriptor_matches_the_upstream_shape", async () => {
  const { MemToast, defineRpc } = await import(TOAST_MODULE);

  assert.deepEqual(MemToast, {
    id: "opencode-mem",
    methods: {},
    events: {
      toast: {
        schema: {
          type: "object",
          properties: {
            title: { type: "string" },
            message: { type: "string" },
            variant: { type: "string" },
            duration: { type: "number" },
            sessionID: { type: "string" },
          },
          required: ["message"],
          additionalProperties: false,
        },
      },
    },
  });
  // The brand seam (D3): identity today, the single place to satisfy a
  // branded `Rpc.define` later. Identity is what the companion matches on.
  assert.equal(defineRpc(MemToast), MemToast, "defineRpc is the identity seam");
});

test("test_v2_toast_rpc_normalizes_the_v1_body_envelope", async () => {
  const { toastPayload } = await import(TOAST_MODULE);

  const body = {
    title: "Memory Captured",
    message: "Project memory saved from conversation",
    variant: "success",
    duration: 3000,
    sessionID: "ses-1",
  };
  assert.deepEqual(toastPayload({ body }), body, "the V1 capability envelope is unwrapped");
  assert.deepEqual(toastPayload(body), body, "a bare body is accepted as-is");
});

test("test_v2_toast_rpc_returns_null_when_the_message_is_blank", async () => {
  const { toastPayload } = await import(TOAST_MODULE);

  assert.equal(toastPayload({ body: { message: "" } }), null, "an empty message is not a toast");
  assert.equal(toastPayload({ body: { title: "only a title" } }), null, "message is required");
  assert.equal(toastPayload({}), null, "an empty input is not a toast");
  assert.equal(toastPayload(null), null, "a null input is not a toast");
});

test("test_v2_toast_rpc_drops_unknown_variants_and_non_finite_durations", async () => {
  const { toastPayload } = await import(TOAST_MODULE);

  assert.deepEqual(
    toastPayload({
      body: {
        message: "message survives",
        title: "",
        variant: "bogus",
        duration: Number.POSITIVE_INFINITY,
        sessionID: "",
        extra: "smuggled",
        path: "/etc/passwd",
      },
    }),
    { message: "message survives" },
    "unknown variant, blank title/sessionID and a non-finite duration are dropped"
  );
  assert.deepEqual(
    toastPayload({ body: { message: "number only", duration: Number.NaN } }),
    { message: "number only" },
    "NaN is not a finite duration"
  );
  assert.deepEqual(
    toastPayload({ body: { message: "error toast", variant: "error", duration: 0 } }),
    { message: "error toast", variant: "error", duration: 0 },
    "a zero duration is finite and kept"
  );
});
