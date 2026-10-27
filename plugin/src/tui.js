// TUI companion entry (W5.2, blueprint D4) — a host *TUI* plugin, not a server
// plugin: it subscribes to the `toast` event of the shared RPC descriptor and
// renders it with `ctx.ui.toast.show`. That is the only user-visible channel a
// V2 server plugin has (it owns no `ui.toast` of its own), so this file closes
// the loop `plugin/src/v2/toast-rpc.js` opens.
//
// It is a plugin-package module, not a Rust crate: upstream's companion is 18
// lines of host-plugin JS with zero Rust in it, and adding a thirteenth crate
// for it would force `tests/workspace_deps.rs` and the root manifests to move
// in a wave that adds no dependency edge.
//
// Upstream reference: example/opencode-mem/src/tui.ts.

import { MemToast } from "./v2/toast-rpc.js";

export const id = "opencode-mem-tui";

/** Subscribes to the toast event; returns the host's unsubscribe handle. */
export function setup(ctx) {
  return ctx.client.rpc(MemToast).events.on("toast", (event) => {
    ctx.ui.toast.show(event.data);
  });
}

export default { id, setup };
