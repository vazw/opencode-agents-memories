// Issue 69 — the `skills` bridge: [registerSkills](../src/skills.js) unit
// oracles, then the driver cases over the real V2 `setup(ctx)`.
//
// The unit half imports the source module directly and fakes the host's
// `SkillEditor` with the INCREMENTAL model (M14: the live 2.2.0 host rebuilds
// from `initial()` when dirty, so the fake deliberately is the friendlier of
// the two editor models — the reconciliation must be correct on both).
// Nothing here spawns an engine.
//
// The driver half (Step 6) stages the stub engine and the package tree and
// runs through the shared harness, because `setup` resolves `./v2/plugin.js`
// inside the staged `dist/` and the child must not sit on `node:test`'s loop.

import test from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { registerSkills } from "../src/skills.js";
import {
  DEFAULT_STUB_CAPABILITIES,
  DRIVER_SOURCE,
  assertReaped,
  runDriver,
  stagePackageTree,
  stageStubEngine,
} from "./harness/bridge.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));

// The pre-69 hello: the four capabilities issue 68 left, `skills` absent.
const PRE_69_CAPABILITIES = { ...DEFAULT_STUB_CAPABILITIES };
delete PRE_69_CAPABILITIES.skills;

/** The absolute `SKILL.md` path a well-behaved engine would emit. */
const skillPath = (root, id) => path.join(root, "skills-content", id, "SKILL.md");

/** One catalogue entry in the engine's wire shape. */
function entry(id, extras = {}) {
  return {
    id,
    name: id.charAt(0).toUpperCase() + id.slice(1),
    path: path.join("/workspace/skills-content", id, "SKILL.md"),
    content: `body-${id}`,
    autoinvoke: false,
    ...extras,
  };
}

/**
 * The host's incremental `SkillEditor` fake: records every mutation and keeps
 * the state the host would merge into. `transform` runs the callback at
 * registration (`state.ts:264` notifies), which is what makes `adds` non-empty
 * after `registerSkills`'s first refresh.
 */
function fakeSkillDomain() {
  const skills = new Map();
  const adds = [];
  const updates = [];
  const removes = [];
  const state = { transformCalls: 0, disposeCalls: 0, reloadCalls: 0, stored: null };
  const editor = {
    list: () => [...skills.values()],
    get: (id) => skills.get(id),
    add: (skill) => {
      adds.push(structuredClone(skill));
      skills.set(skill.id, structuredClone(skill));
    },
    update: (id, mutate) => {
      updates.push(id);
      const current = structuredClone(skills.get(id));
      mutate(current);
      skills.set(id, current);
    },
    remove: (id) => {
      removes.push(id);
      skills.delete(id);
    },
  };
  return {
    ctx: {
      skill: {
        transform: async (callback) => {
          state.transformCalls += 1;
          state.stored = callback;
          callback(editor); // the host runs it at registration
          return {
            dispose: async () => {
              state.disposeCalls += 1;
            },
          };
        },
        reload: async () => {
          state.reloadCalls += 1;
          state.stored(editor);
        },
      },
    },
    skills,
    adds,
    updates,
    removes,
    state,
  };
}

// ── the driver helper ──────────────────────────────────────────────────

/**
 * Runs one `v2-skills` driver case over a staged package tree and returns the
 * driver record plus the temp root and host directory it used.
 *
 * `prepare({ tmp, directory })` arranges the stub catalogue and the fixture's
 * extra fields (`skills`, `skillsPath`, `events`, `skillsAfter`,
 * `skillsAfterPath`); `stub: false` stages no engine at all.
 */
async function runSkillsDriver(t, { stub = true, capabilities = DEFAULT_STUB_CAPABILITIES, prepare } = {}) {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "om-skills-"));
  const pkgDir = stagePackageTree(tmp);
  const requestLogPath = path.join(tmp, "stub-requests.ndjson");
  const manifestPath = path.join(tmp, "stub-manifest.json");
  const donePath = path.join(tmp, "driver-done.json");
  const directory = path.join(tmp, "project");
  fs.mkdirSync(directory, { recursive: true });
  const arranged = prepare ? prepare({ tmp, directory }) : {};
  if (stub) {
    stageStubEngine(pkgDir, {
      requestLogPath,
      manifestPath,
      capabilities,
      skills: arranged.skills ?? [],
      skillsPath: arranged.skillsPath ?? null,
    });
    t.after(() => assertReaped(manifestPath));
  }
  const fixturePath = path.join(tmp, "fixture.json");
  fs.writeFileSync(
    fixturePath,
    JSON.stringify({
      location: { directory, project: { id: "project-1", directory } },
      events: arranged.events ?? [],
      skillsAfter: arranged.skillsAfter ?? null,
      skillsAfterPath: arranged.skillsAfterPath ?? null,
    })
  );
  const driverPath = path.join(tmp, "driver.mjs");
  fs.writeFileSync(driverPath, DRIVER_SOURCE);
  const result = await runDriver(
    driverPath,
    [pkgDir, fixturePath],
    {
      ...process.env,
      DRIVER_MODE: "v2-skills",
      DRIVER_STORAGE_ROOT: path.join(tmp, "storage"),
      DRIVER_DONE_PATH: donePath,
      STUB_LOG: requestLogPath,
    },
    30000
  );
  assert.equal(
    result.code,
    0,
    `the v2-skills driver failed (signal=${String(result.signal)}): stderr=${result.stderr}`
  );
  return { record: JSON.parse(fs.readFileSync(donePath, "utf8")), tmp, directory };
}

// ── the nine unit cases ────────────────────────────────────────────────

test("test_register_skills_adds_each_catalogue_entry_with_the_host_payload", async () => {
  const { ctx, adds } = fakeSkillDomain();
  const handle = await registerSkills(ctx, {
    readCatalogue: async () => [
      entry("alpha", { description: "Alpha desc", autoinvoke: true }),
      entry("beta"),
    ],
  });

  assert.deepEqual(adds, [
    {
      id: "alpha",
      name: "Alpha",
      path: "/workspace/skills-content/alpha/SKILL.md",
      content: "body-alpha",
      autoinvoke: true,
      description: "Alpha desc",
    },
    {
      id: "beta",
      name: "Beta",
      path: "/workspace/skills-content/beta/SKILL.md",
      content: "body-beta",
      autoinvoke: false,
    },
  ]);
  await handle.cleanup();
});

test("test_register_skills_removes_a_vanished_id_on_refresh_and_leaves_the_survivor", async () => {
  const { ctx, adds, updates, removes, skills } = fakeSkillDomain();
  const catalogue = [entry("alpha"), entry("beta")];
  const handle = await registerSkills(ctx, { readCatalogue: async () => catalogue });

  catalogue.splice(1, 1); // alpha survives, beta vanished
  await handle.refresh();

  assert.deepEqual(removes, ["beta"]);
  assert.ok(updates.includes("alpha"), "the survivor is updated, never re-added");
  assert.equal(adds.length, 2, "a refresh must not add the survivor again");
  assert.deepEqual([...skills.keys()], ["alpha"]);
  await handle.cleanup();
});

test("test_register_skills_clears_a_stale_description_on_refresh", async () => {
  const { ctx, skills } = fakeSkillDomain();
  let catalogue = [entry("alpha", { description: "old" })];
  const handle = await registerSkills(ctx, { readCatalogue: async () => catalogue });
  assert.equal(skills.get("alpha").description, "old");

  catalogue = [entry("alpha")];
  await handle.refresh();

  assert.equal(
    Object.hasOwn(skills.get("alpha"), "description"),
    false,
    "a wire entry without a description clears the host record's stale one"
  );
  await handle.cleanup();
});

test("test_register_skills_rejects_an_entry_with_a_relative_path", async () => {
  const { ctx, adds } = fakeSkillDomain();
  const handle = await registerSkills(ctx, {
    readCatalogue: async () => [
      entry("relative", { path: "skills-content/relative/SKILL.md" }),
      entry("absolute"),
    ],
  });

  assert.deepEqual(
    adds.map((skill) => skill.id),
    ["absolute"],
    "a relative path is never registered: the host's AbsolutePath brand is unchecked at runtime"
  );
  await handle.cleanup();
});

test("test_register_skills_rejects_a_non_string_description", async () => {
  const { ctx, adds } = fakeSkillDomain();
  const handle = await registerSkills(ctx, {
    readCatalogue: async () => [entry("typed", { description: 123 }), entry("clean")],
  });

  assert.deepEqual(
    adds.map((skill) => skill.id),
    ["clean"],
    "description: 123 is malformed, not truthy-but-typed"
  );
  await handle.cleanup();
});

test("test_register_skills_is_a_no_op_without_a_skill_editor_surface", async () => {
  let reads = 0;
  const handle = await registerSkills(
    { location: {}, session: {}, event: {} },
    {
      readCatalogue: async () => {
        reads += 1;
        return [entry("alpha")];
      },
    }
  );

  assert.equal(reads, 0, "no skill domain means no read");
  await handle.refresh();
  await handle.cleanup();
});

test("test_register_skills_is_a_no_op_when_the_catalogue_is_unavailable", async () => {
  const { ctx, adds, state } = fakeSkillDomain();
  const handle = await registerSkills(ctx, { readCatalogue: async () => null });

  assert.equal(state.transformCalls, 0, "no catalogue means the transform is never called");
  assert.deepEqual(adds, []);
  await handle.refresh();
  assert.equal(state.transformCalls, 0);
  await handle.cleanup();
});

test("test_register_skills_disposes_the_registration_once", async () => {
  const { ctx, state } = fakeSkillDomain();
  const handle = await registerSkills(ctx, { readCatalogue: async () => [entry("alpha")] });
  await handle.cleanup();
  await handle.cleanup();
  assert.equal(state.disposeCalls, 1, "cleanup is idempotent");

  // A rejecting dispose must not reject the host's unload.
  const rejectingHandle = await registerSkills(
    {
      skill: {
        transform: async () => ({
          dispose: async () => {
            throw new Error("host dispose failed");
          },
        }),
        reload: async () => {},
      },
    },
    { readCatalogue: async () => [entry("alpha")] }
  );
  await rejectingHandle.cleanup();
});

test("test_register_skills_does_not_register_after_cleanup", async () => {
  const { ctx, adds, updates, removes, state } = fakeSkillDomain();
  let catalogue = [entry("alpha")];
  let deferred = false;
  let release = null;
  const readCatalogue = async () => {
    if (!deferred) return catalogue;
    return new Promise((resolve) => {
      release = () => resolve(catalogue);
    });
  };
  const handle = await registerSkills(ctx, { readCatalogue });

  deferred = true;
  const pending = handle.refresh(); // parks on the deferred read
  await handle.cleanup(); // aborts and disposes while the read is in flight
  catalogue = [entry("gamma")];
  release();
  await pending;

  assert.deepEqual(
    adds.map((skill) => skill.id),
    ["alpha"],
    "a read that lands after cleanup mutates nothing"
  );
  assert.deepEqual(updates, []);
  assert.deepEqual(removes, []);
  assert.equal(state.disposeCalls, 1);
});

// ── the five driver cases (the real V2 `setup(ctx)`) ───────────────────

test(
  "test_setup_registers_the_engine_catalogue_through_the_v2_entry",
  { timeout: 60000 },
  async (t) => {
    const { record, tmp, directory } = await runSkillsDriver(t, {
      prepare: ({ tmp: root }) => ({
        skills: [
          {
            id: "alpha",
            name: "Alpha",
            description: "First skill",
            path: skillPath(root, "alpha"),
            content: "body-alpha",
            autoinvoke: true,
          },
          {
            id: "beta",
            name: "Beta",
            path: skillPath(root, "beta"),
            content: "body-beta",
            autoinvoke: false,
          },
        ],
      }),
    });

    assert.equal(record.helloCount, 1, "one child, one hello");
    assert.deepEqual(
      record.catalogueRequests,
      [{ project_id: directory }],
      "the host directory is the project_id, and the request is session-free"
    );
    assert.deepEqual(record.adds, [
      {
        id: "alpha",
        name: "Alpha",
        description: "First skill",
        path: skillPath(tmp, "alpha"),
        content: "body-alpha",
        autoinvoke: true,
      },
      {
        id: "beta",
        name: "Beta",
        path: skillPath(tmp, "beta"),
        content: "body-beta",
        autoinvoke: false,
      },
    ]);
    assert.ok(record.elapsedMs < 8000, "the awaited setup stays inside the 5 s + 3 s bound");
    assert.deepEqual(record.consoleErrors, []);
  }
);

test(
  "test_setup_sends_no_skills_request_when_the_engine_does_not_advertise_the_capability",
  { timeout: 60000 },
  async (t) => {
    const { record } = await runSkillsDriver(t, {
      capabilities: PRE_69_CAPABILITIES,
      prepare: ({ tmp: root }) => ({
        skills: [
          {
            id: "alpha",
            name: "Alpha",
            path: skillPath(root, "alpha"),
            content: "body-alpha",
            autoinvoke: true,
          },
        ],
      }),
    });

    assert.equal(record.helloCount, 1, "the handshake still happens");
    assert.deepEqual(
      record.catalogueRequests,
      [],
      "a pre-69 engine is never asked: an absent flag is not an invitation"
    );
    assert.deepEqual(record.adds, []);
  }
);

test(
  "test_setup_registers_no_entry_the_engine_did_not_make_absolute",
  { timeout: 60000 },
  async (t) => {
    const { record, tmp } = await runSkillsDriver(t, {
      prepare: ({ tmp: root }) => ({
        skills: [
          {
            id: "relative",
            name: "Relative",
            path: "skills-content/relative/SKILL.md",
            content: "body-relative",
            autoinvoke: false,
          },
          {
            id: "absolute",
            name: "Absolute",
            path: skillPath(root, "absolute"),
            content: "body-absolute",
            autoinvoke: false,
          },
        ],
      }),
    });

    assert.deepEqual(
      record.adds.map((skill) => skill.id),
      ["absolute"],
      "the absolute-path guard survives the JSON boundary"
    );
    assert.equal(record.adds[0].path, skillPath(tmp, "absolute"));
  }
);

test(
  "test_setup_stays_fail_open_with_a_skill_surface_and_no_engine",
  { timeout: 60000 },
  async (t) => {
    const { record } = await runSkillsDriver(t, { stub: false });

    assert.equal(record.helloCount, 0, "no engine means no wire at all");
    assert.deepEqual(record.catalogueRequests, []);
    assert.deepEqual(record.adds, [], "setup resolves and registers nothing");
    assert.deepEqual(record.consoleErrors, []);
  }
);

test(
  "test_setup_re_registers_on_skill_updated_and_removes_the_vanished_id",
  { timeout: 60000 },
  async (t) => {
    const { record, tmp } = await runSkillsDriver(t, {
      prepare: ({ tmp: root }) => {
        const skillsPath = path.join(root, "catalogue.json");
        const initial = [
          {
            id: "alpha",
            name: "Alpha",
            path: skillPath(root, "alpha"),
            content: "body-alpha",
            autoinvoke: true,
          },
          {
            id: "beta",
            name: "Beta",
            path: skillPath(root, "beta"),
            content: "body-beta",
            autoinvoke: false,
          },
        ];
        fs.writeFileSync(skillsPath, JSON.stringify(initial));
        return {
          skills: initial,
          skillsPath,
          events: [{ type: "skill.updated" }],
          skillsAfterPath: skillsPath,
          skillsAfter: [initial[0]],
        };
      },
    });

    assert.equal(
      record.catalogueRequests.length,
      2,
      "setup reads once; the host's skill.updated triggers exactly one re-read"
    );
    assert.equal(record.adds.length, 2, "the survivor is updated, never re-added");
    assert.ok(record.updates.includes("alpha"));
    assert.deepEqual(record.removes, ["beta"], "the vanished id is removed");
    assert.equal(record.disposed, true, "cleanup disposed the registration");
  }
);
