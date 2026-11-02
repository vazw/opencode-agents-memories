// V2 plugin entry (W5.2, blueprint D1): `{ id, setup(ctx) }` where `setup`
// returns an async cleanup. The host reaches this module only through
// `plugin/src/plugin.js:1260`'s `await import("./v2/plugin.js")` inside the
// `setup` the host calls via `server()`; the package's `./v2` export is inert
// (`packages/plugin/src/host.ts:57` resolves `./server` then `.`), so never
// assume the host loads `./v2` (issue 112, D-112-8).
// It is a DIFFERENT entry, not a variant of `server`: different input (`ctx`,
// not `PluginInput`), different output (a cleanup, not `{ hooks }`), different
// lifecycle.
//
// The shape of the bridge, in order: register the toast RPC descriptor (the
// only channel a V2 server plugin has for user-visible intent), build the
// narrow legacy client from the V2 context, run the very same V1 `server()`
// against it, and let the adapter translate V2 session hooks and events onto
// the resulting hooks. The engine surface therefore cannot fork — one child,
// one hello, the same six methods.
//
// Provider connectivity is deliberately absent (D7): it is unported and owned
// by issue 62, and the legacy client exposes no `provider` surface at all.
//
// Upstream reference: example/opencode-mem/src/v2/plugin.ts.

import { readSkillCatalogue, server } from "../plugin.js";
import { registerSkills } from "../skills.js";
import { registerV2Adapter } from "./adapter.js";
import { createLegacyClient } from "./legacy-client.js";
import { MemToast } from "./toast-rpc.js";

export const id = "opencode-mem";

async function registerToastRpc(ctx) {
  if (typeof ctx?.rpc?.register !== "function") return undefined;
  try {
    return await ctx.rpc.register(MemToast, {});
  } catch (error) {
    // A registry that refuses the descriptor is treated as absent: the legacy
    // client falls back to one console line, never a throw.
    console.error("opencode-mem toast rpc registration failed", error);
    return undefined;
  }
}

export async function setup(ctx) {
  const toastRpc = await registerToastRpc(ctx);
  const legacyClient = createLegacyClient(ctx, toastRpc);
  const v1 = await server({
    client: legacyClient,
    directory: ctx?.location?.directory ?? null,
    project: ctx?.location?.project ?? null,
    worktree: ctx?.location?.project?.directory ?? null,
    $: null,
  });
  const adapterCleanup = await registerV2Adapter(ctx, v1.hooks);
  // Issue 69: the catalogue is the engine's; the host's skill domain is where it
  // becomes visible. Fail-open: no engine, no capability, no transform → no-op.
  const skills = await registerSkills(ctx, {
    readCatalogue: () => readSkillCatalogue(ctx?.location?.directory ?? null),
  });
  return async () => {
    await skills.cleanup();
    await adapterCleanup();
  };
}

export default { id, setup };
