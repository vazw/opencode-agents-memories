// W5.2 `js` shard — the TUI companion, ported from
// example/opencode-mem/tests/tui-companion.test.ts. Four cases: the upstream
// case splits into subscribe+show and unsubscribe, plus two port additions
// (the entry shape, and a client→companion round trip through the toast RPC).
//
// The companion imports `src/` (module test); only loader-contract tests import
// `dist/`. It renders the `toast` event with `ctx.ui.toast.show` — a V2 server
// plugin has no `ui.toast`, so this is the only user-visible channel.

import test from "node:test";
import assert from "node:assert/strict";

const TUI_MODULE = "../src/tui.js";
const TOAST_MODULE = "../src/v2/toast-rpc.js";
const CLIENT_MODULE = "../src/v2/legacy-client.js";

function createCompanionHost() {
  const shown = [];
  let subscribedDefinition;
  let subscribedName;
  let handler;
  const host = {
    client: {
      rpc: (definition) => {
        subscribedDefinition = definition;
        return {
          events: {
            on: (name, next) => {
              subscribedName = name;
              handler = next;
              return () => {
                handler = undefined;
              };
            },
          },
        };
      },
    },
    ui: {
      toast: {
        show: (options) => {
          shown.push(options);
        },
      },
    },
  };
  return {
    host,
    shown,
    subscribed: () => ({ definition: subscribedDefinition, name: subscribedName }),
    emit: (event) => handler?.(event),
    subscribedHandler: () => handler,
  };
}

test("test_tui_companion_exports_id_and_setup", async () => {
  const mod = await import(TUI_MODULE);
  assert.equal(mod.id, "opencode-mem-tui");
  assert.equal(typeof mod.setup, "function");
  assert.equal(mod.default.id, "opencode-mem-tui");
  assert.equal(mod.default.setup, mod.setup);
});

test("test_tui_companion_subscribes_to_toast_events_and_shows_them", async () => {
  const { default: tuiPlugin } = await import(TUI_MODULE);
  const { MemToast } = await import(TOAST_MODULE);
  const harness = createCompanionHost();

  tuiPlugin.setup(harness.host);

  assert.equal(harness.subscribed().definition, MemToast, "the companion matches the descriptor by identity");
  assert.equal(harness.subscribed().name, "toast");

  harness.emit({
    data: {
      title: "Memory Captured",
      message: "Project memory saved from conversation",
      variant: "success",
      duration: 3000,
    },
  });
  assert.deepEqual(harness.shown, [
    {
      title: "Memory Captured",
      message: "Project memory saved from conversation",
      variant: "success",
      duration: 3000,
    },
  ]);
});

test("test_tui_companion_returns_the_host_unsubscribe", async () => {
  const { default: tuiPlugin } = await import(TUI_MODULE);
  const harness = createCompanionHost();

  const cleanup = tuiPlugin.setup(harness.host);
  assert.equal(typeof cleanup, "function", "setup returns the host's unsubscribe");
  assert.equal(typeof harness.subscribedHandler(), "function");

  cleanup();
  assert.equal(harness.subscribedHandler(), undefined, "the unsubscribe is the host's own handle");
});

test("test_tui_companion_renders_a_toast_emitted_by_the_legacy_client", async () => {
  const { default: tuiPlugin } = await import(TUI_MODULE);
  const { createLegacyClient } = await import(CLIENT_MODULE);
  const harness = createCompanionHost();
  const cleanup = tuiPlugin.setup(harness.host);

  // The server-side emitter hands the payload to the same handler the
  // companion registered — the full client → RPC → companion → host path.
  const toastRpc = {
    events: {
      emit: async (name, data) => {
        assert.equal(name, "toast");
        harness.emit({ data });
      },
    },
  };
  const client = createLegacyClient({}, toastRpc);

  const result = await client.tui.showToast({
    body: { message: "Memory injection failed; memory may be disabled.", variant: "warning" },
  });

  assert.deepEqual(result, { data: true });
  assert.deepEqual(harness.shown, [
    { message: "Memory injection failed; memory may be disabled.", variant: "warning" },
  ]);

  cleanup();
});
