// Toast RPC descriptor — the shared server↔TUI contract for toast intents
// (W5.2, blueprint D3). A V2 *server* plugin has no `ui.toast`; it emits the
// `toast` event over the host RPC registry, and the `./tui` companion renders
// it with `ctx.ui.toast.show`.
//
// Upstream builds this with `Rpc.define` imported from `@opencode/plugin/rpc`.
// The port cannot import a host package (bundle-boundary: every specifier must
// be `node:*` or relative), so `defineRpc` is the identity seam — if the host
// ever demands a branded definition, this one function is where it is
// satisfied.

/** Identity today; the single brand seam for the host's `Rpc.define`. */
export function defineRpc(descriptor) {
  return descriptor;
}

/** Upstream `MemToast` verbatim (`example/opencode-mem/src/v2/toast-rpc.ts:8-27`). */
export const MemToast = defineRpc({
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

const VARIANTS = new Set(["info", "success", "warning", "error"]);

/**
 * Normalizes a V1 toast capability input (`{ body }`) or a bare body into the
 * descriptor's payload. The message is the only required field; every other
 * field is dropped unless it has the schema's shape, so a hostile payload
 * cannot smuggle anything the TUI would render.
 */
export function toastPayload(input) {
  const body = input?.body ?? input ?? {};
  const message = typeof body.message === "string" ? body.message : "";
  if (!message) return null;

  const payload = { message };
  if (typeof body.title === "string" && body.title) payload.title = body.title;
  if (VARIANTS.has(body.variant)) payload.variant = body.variant;
  if (typeof body.duration === "number" && Number.isFinite(body.duration)) {
    payload.duration = body.duration;
  }
  if (typeof body.sessionID === "string" && body.sessionID) payload.sessionID = body.sessionID;
  return payload;
}
