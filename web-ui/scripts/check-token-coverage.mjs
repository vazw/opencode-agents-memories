#!/usr/bin/env node
// Two-sided `--app-*` contract: every `var(--app-x)` in public/styles.css has an
// emitter in src/**/*.rs (`--app-x:`), and every emitter has a reference.
// Usage: node scripts/check-token-coverage.mjs [--self-test]
import { readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

export function refs(css) {
  return new Set([...css.matchAll(/var\(\s*(--app-[a-z0-9-]+)/g)].map((m) => m[1]));
}

export function emitters(rs) {
  return new Set([...rs.matchAll(/(--app-[a-z0-9-]+)\s*:/g)].map((m) => m[1]));
}

export function compare(refSet, emitSet) {
  const missing = [...refSet].filter((n) => !emitSet.has(n)).sort();
  const dead = [...emitSet].filter((n) => !refSet.has(n)).sort();
  return { missing, dead };
}

function walk(dir) {
  return readdirSync(dir).flatMap((name) => {
    const path = join(dir, name);
    return statSync(path).isDirectory() ? walk(path) : path.endsWith(".rs") ? [path] : [];
  });
}

function selfTest() {
  const fixtures = [
    {
      name: "reference without emitter",
      css: ".a { margin: var(--app-gutter, 1px); }",
      rs: 'const X: &str = "--app-measure: 72ch;";',
      want: { missing: ["--app-gutter"], dead: ["--app-measure"] },
    },
    {
      name: "emitter without reference",
      css: ".a { width: var(--app-measure, 72ch); }",
      rs: 'const X: &str = "--app-measure: 72ch; --app-orphan: 1px;";',
      want: { missing: [], dead: ["--app-orphan"] },
    },
  ];
  let ok = 0;
  for (const f of fixtures) {
    const got = compare(refs(f.css), emitters(f.rs));
    if (JSON.stringify(got) === JSON.stringify(f.want)) ok += 1;
    else console.error(`fixture failed: ${f.name}`, got);
  }
  console.log(`SELFTEST ${ok}/${fixtures.length}`);
  return ok === fixtures.length;
}

const selfOk = process.argv.includes("--self-test") ? selfTest() : true;
const css = readFileSync(join(root, "public/styles.css"), "utf8");
const rs = walk(join(root, "src")).map((p) => readFileSync(p, "utf8")).join("\n");
const { missing, dead } = compare(refs(css), emitters(rs));
if (missing.length) console.error("missing emitters:", missing.join(", "));
if (dead.length) console.error("dead emitters:", dead.join(", "));
console.log(`missing=${missing.length} dead=${dead.length}`);
process.exit(selfOk && !missing.length && !dead.length ? 0 : 1);
