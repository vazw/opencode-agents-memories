// Issue 69 — the V2 skill-domain registration: one read of the engine's
// catalogue, registered on the host through `ctx.skill.transform`, re-read on
// the host's `skill.updated` event.
//
// The engine owns `skills.db`; the host owns skill state. This module is the
// bridge between them and follows the same fail-open law as
// `registerMemoryTools`: no skill domain, no catalogue, a throwing transform
// or a rejecting reload/dispose all leave capture and injection untouched.
//
// Two races are closed explicitly:
//   - `refresh` re-checks `disposed` AFTER its await, so a read that lands
//     after `cleanup` mutates nothing (and never registers a transform);
//   - `cleanup` awaits the in-flight read before disposing the registration,
//     so the host's unload cannot race the read.
//
// `skill.updated` events coalesce: one extra read per in-flight read, a burst
// spread across reads can chain, reads never run in parallel, and the last
// event is never lost.

import { isAbsolute } from "node:path";

import { logWarn, shortError } from "./plugin.js";

/**
 * Validates one catalogue entry before it reaches the host editor.
 * Returns `null` for a usable entry, otherwise the log reason:
 * `"malformed"` or `"relative_path"`.
 */
function validate(entry) {
  if (entry === null || typeof entry !== "object" || Array.isArray(entry)) return "malformed";
  if (typeof entry.id !== "string" || entry.id.length === 0) return "malformed";
  if (entry.id.includes("/") || entry.id.includes("\\")) return "malformed";
  if (typeof entry.name !== "string" || entry.name.length === 0) return "malformed";
  if (typeof entry.content !== "string") return "malformed";
  if (typeof entry.path !== "string") return "malformed";
  if (entry.description !== undefined && typeof entry.description !== "string") {
    return "malformed";
  }
  if (!isAbsolute(entry.path)) return "relative_path";
  return null;
}

/**
 * Registers the engine catalogue on the host's skill domain and returns
 * `{ refresh, cleanup }`. Never throws: every failure is a logged line and a
 * no-op.
 */
export async function registerSkills(ctx, { readCatalogue } = {}) {
  const noop = { refresh: async () => {}, cleanup: async () => {} };
  const transform = ctx?.skill?.transform;
  if (typeof transform !== "function") return noop; // fail-open: no skill domain
  if (typeof readCatalogue !== "function") return noop;

  const registeredIds = new Set();
  const entries = []; // shared with the registered callback; refresh swaps it
  const controller = new AbortController();
  let registration = null;
  let disposed = false;
  let inFlight = null; // the one coalesced refresh, if any
  let pending = false; // an event arrived while a refresh was in flight

  function apply(editor) {
    // The installed 2.2.0 host passes a SkillEditor with all five methods. Any
    // other shape (an older host's draft object) is left untouched rather than
    // throwing inside it.
    if (typeof editor?.get !== "function" || typeof editor?.add !== "function") return;
    if (typeof editor.remove !== "function" || typeof editor.update !== "function") return;
    const nextIds = new Set(entries.map((entry) => entry.id));
    for (const id of registeredIds) {
      if (!nextIds.has(id) && editor.get(id)) editor.remove(id);
    }
    for (const entry of entries) {
      const payload = {
        id: entry.id,
        name: entry.name,
        path: entry.path,
        content: entry.content,
        autoinvoke: entry.autoinvoke === true,
      };
      if (typeof entry.description === "string" && entry.description.length > 0) {
        payload.description = entry.description;
      }
      if (editor.get(entry.id)) {
        editor.update(entry.id, (current) => {
          Object.assign(current, payload);
          // A wire entry without a description must clear a stale one; the host
          // merges into the existing record rather than replacing it.
          if (payload.description === undefined) delete current.description;
        });
      } else {
        editor.add(payload);
      }
    }
    registeredIds.clear();
    for (const id of nextIds) registeredIds.add(id);
  }

  async function refresh() {
    if (disposed) return;
    const catalogue = await readCatalogue();
    // Cleanup may run while the read is in flight: without this second guard an
    // in-flight refresh would register a NEW transform after disposal (when the
    // initial registration failed) or call reload on the disposed one.
    if (disposed) return;
    if (catalogue === null) return; // capability absent / engine down
    const accepted = [];
    const rejected = new Map(); // reason -> count
    for (const entry of catalogue) {
      const reason = validate(entry); // "malformed" | "relative_path" | null
      if (reason === null) accepted.push(entry);
      else rejected.set(reason, (rejected.get(reason) ?? 0) + 1);
    }
    for (const [reason, count] of rejected) logWarn("skills.entry_rejected", { reason, count });
    entries.splice(0, entries.length, ...accepted);
    if (registration === null) {
      try {
        registration = await transform(apply);
      } catch (error) {
        logWarn("skills.registration_failed", { message: shortError(error) });
      }
    } else if (typeof ctx.skill.reload === "function") {
      try {
        await ctx.skill.reload(); // re-runs the stored callback
      } catch (error) {
        logWarn("skills.reload_failed", { message: shortError(error) });
      }
    }
  }

  function scheduleRefresh() {
    if (disposed) return;
    if (inFlight !== null) {
      pending = true; // coalesce, do not drop
      return;
    }
    inFlight = (async () => {
      try {
        do {
          pending = false;
          await refresh();
        } while (pending && !disposed);
      } catch (error) {
        logWarn("skills.refresh_failed", { message: shortError(error) });
      } finally {
        inFlight = null;
        if (pending && !disposed) scheduleRefresh(); // an event in the teardown window
      }
    })();
  }

  try {
    await refresh();
  } catch (error) {
    logWarn("skills.refresh_failed", { message: shortError(error) });
  }

  // A host skill-state change re-reads the catalogue. Our own registration's
  // event predates this subscription (the host published it while `transform`
  // resolved, before we subscribed; the bus has no replay), so the plugin does
  // not re-read it. A later host change triggers the read, and the host's JSON
  // dedupe (M15) suppresses an identical publish — no loop.
  if (typeof ctx?.event?.subscribe === "function") {
    void (async () => {
      try {
        for await (const event of ctx.event.subscribe({
          types: ["skill.updated"],
          signal: controller.signal,
        })) {
          const type = event?.type ?? event?.payload?.type;
          if (type === "skill.updated") scheduleRefresh();
        }
      } catch {
        // An aborted or broken stream keeps the first registration.
      }
    })();
  }

  return {
    refresh,
    cleanup: async () => {
      if (disposed) return;
      disposed = true;
      controller.abort();
      // A refresh started before cleanup must finish before the host state is
      // disposed; its post-await `disposed` guard then makes it a no-op.
      if (inFlight !== null) {
        try {
          await inFlight;
        } catch {
          // the loop already logged it
        }
      }
      if (registration && typeof registration.dispose === "function") {
        try {
          await registration.dispose();
        } catch (error) {
          // The host loader awaits cleanup: a rejected dispose would reject the
          // unload, so it is logged and swallowed like every other failure.
          logWarn("skills.dispose_failed", { message: shortError(error) });
        }
      }
    },
  };
}
