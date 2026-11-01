#!/usr/bin/env node
// The class-coverage gate for the Memory Explorer stylesheet.
//
// Contract: every class token the Rust under `web-ui/src` can emit must have at
// least one rule in `web-ui/public/styles.css`. The extractor is a *whitelist
// classifier*: each `class=` site is classified by the token after `=` and
// anything it cannot classify is a residue that exits 1 — never a skip. The
// closed end of the whitelist is deliberate: a carrier form nobody has invented
// yet is refused, not silently dropped.
//
// Carrier forms the classifier accepts:
//   `class="a b"`                  literal   — split on whitespace
//   `class=NAME_CLASS`             const     — resolved through the file's
//                                              `const NAME_CLASS: &str = "…"`
//   `class={ … }`                  brace     — balanced-brace block; every
//                                              string literal and every
//                                              resolved `[A-Z][A-Z0-9_]*` ident
//                                              inside contributes tokens
//   `class:foo`                    modifier  — its own token, not a site
// Anything else (a tuple, a closure, `format!(…)`, a lowercase local, a raw
// string) is a residue and a hard failure.
//
// Output contract:
//   sites=N literal=N const=N brace=N residue=N
//   emitted=N hooks=N missing=N dead=N unresolved=N
// `unresolved` counts a site that produced **no** token (an empty literal, an
// empty block, a const that resolves to nothing). Such a site is not a residue
// — the classifier recognised its form — but it silently contributes nothing,
// so it is printed rather than inferred. A residue always contributes nothing
// too, so `unresolved == 0` is implied by `residue == 0`; it is printed because
// the wave's acceptance criterion names it.
//
// Run from the repo root or from `web-ui/`; both paths are derived from this
// file's own location.

import { readdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const webUiRoot = resolve(here, "..");
const srcDir = join(webUiRoot, "src");
const cssPath = join(webUiRoot, "public", "styles.css");

const args = process.argv.slice(2);
const runSelfTest = args.includes("--self-test");
const asJson = args.includes("--json");
let minEmitted = 175;
for (const arg of args) {
  if (arg.startsWith("--min-emitted=")) {
    minEmitted = Number.parseInt(arg.slice("--min-emitted=".length), 10);
    if (!Number.isFinite(minEmitted)) {
      console.error(`bad --min-emitted value: ${arg}`);
      process.exit(2);
    }
  }
}

const IDENT = /[A-Za-z0-9_]/;
const UPPER = /[A-Z]/;

/** 1-based line number of a character offset. */
function lineAt(source, offset) {
  let line = 1;
  for (let i = 0; i < offset; i += 1) {
    if (source.charCodeAt(i) === 10) line += 1;
  }
  return line;
}

/** Read a `"…"` literal starting at `start`; returns [text, nextOffset]. */
function readQuoted(source, start) {
  let i = start + 1;
  let text = "";
  while (i < source.length && source[i] !== '"') {
    if (source[i] === "\\" && i + 1 < source.length) {
      text += source[i + 1];
      i += 2;
      continue;
    }
    text += source[i];
    i += 1;
  }
  return [text, i + 1];
}

/** Read an identifier starting at `start`; returns [text, nextOffset]. */
function readIdent(source, start) {
  let i = start;
  while (i < source.length && IDENT.test(source[i])) i += 1;
  return [source.slice(start, i), i];
}

function splitTokens(text) {
  return text.split(/\s+/).filter(Boolean);
}

function skipSpace(source, start) {
  let i = start;
  while (i < source.length && /\s/.test(source[i])) i += 1;
  return i;
}

/** The const map of one Rust source text: NAME -> "value". */
function constMap(source) {
  const map = new Map();
  const pattern = /(?:pub\s+)?const\s+([A-Z_][A-Z0-9_]*)\s*:\s*&str\s*=\s*"([^"]*)"/g;
  for (const match of source.matchAll(pattern)) map.set(match[1], match[2]);
  return map;
}

/**
 * Scan a balanced `{ … }` block starting at `open` (which must be `{`).
 *
 * Returns `{ tokens, unresolvedIdents, next }`. Depth moves only on braces
 * outside string literals and comments, so `if c { "x" } else { "y" }` yields
 * both literals — the discriminator against a `\{([^}]*)\}` scan.
 */
function scanBlock(source, open, consts) {
  const tokens = [];
  const unresolvedIdents = [];
  let depth = 0;
  let i = open;
  while (i < source.length) {
    const ch = source[i];
    if (ch === '"') {
      const [text, next] = readQuoted(source, i);
      tokens.push(...splitTokens(text));
      i = next;
      continue;
    }
    if (ch === "/" && source[i + 1] === "/") {
      const newline = source.indexOf("\n", i);
      i = newline === -1 ? source.length : newline + 1;
      continue;
    }
    if (ch === "/" && source[i + 1] === "*") {
      const end = source.indexOf("*/", i + 2);
      i = end === -1 ? source.length : end + 2;
      continue;
    }
    if (ch === "{") {
      depth += 1;
      i += 1;
      continue;
    }
    if (ch === "}") {
      depth -= 1;
      i += 1;
      if (depth === 0) return { tokens, unresolvedIdents, next: i };
      continue;
    }
    if (UPPER.test(ch) && !IDENT.test(source[i - 1] ?? "")) {
      const [name, next] = readIdent(source, i);
      if (consts.has(name)) tokens.push(...splitTokens(consts.get(name)));
      else unresolvedIdents.push(name);
      i = next;
      continue;
    }
    i += 1;
  }
  return { tokens, unresolvedIdents, next: i };
}

/**
 * Extract every class token from one Rust source text.
 *
 * Returns `{ tokens, sites, literal, const, brace, modifier, residues,
 * unresolved }`, where a site is one `class=` occurrence, a residue is
 * `{ form, raw, offset }`, and `unresolved` counts sites that produced no token
 * through a recognised form.
 */
function extract(source) {
  const consts = constMap(source);
  const tokens = new Set();
  const residues = [];
  let literal = 0;
  let named = 0;
  let brace = 0;
  let modifier = 0;
  let unresolved = 0;

  const finder = /class=/g;
  for (let match = finder.exec(source); match; match = finder.exec(source)) {
    const start = skipSpace(source, match.index + "class=".length);
    const first = source[start];

    if (first === '"') {
      const [text, next] = readQuoted(source, start);
      const found = splitTokens(text);
      if (found.length === 0) unresolved += 1;
      for (const token of found) tokens.add(token);
      literal += 1;
      finder.lastIndex = next;
      continue;
    }

    if (first === "{") {
      const block = scanBlock(source, start, consts);
      if (block.tokens.length === 0) unresolved += 1;
      for (const token of block.tokens) tokens.add(token);
      brace += 1;
      finder.lastIndex = block.next;
      continue;
    }

    if (UPPER.test(first ?? "")) {
      const [name, next] = readIdent(source, start);
      if (consts.has(name)) {
        const found = splitTokens(consts.get(name));
        if (found.length === 0) unresolved += 1;
        for (const token of found) tokens.add(token);
      } else {
        residues.push({
          form: "ident",
          raw: source.slice(start, start + 60),
          offset: start,
        });
      }
      named += 1;
      finder.lastIndex = next;
      continue;
    }

    // Not a whitelisted carrier: classify the form for the failure message.
    let form = "other";
    let rawEnd = start + 1;
    if (first === "(") {
      form = "tuple";
    } else if (/[A-Za-z_]/.test(first ?? "")) {
      const [word, next] = readIdent(source, start);
      if (word === "move") {
        form = "closure";
      } else if ((word === "r" || word === "br" || word === "cr") && source[next] === '"') {
        form = "raw";
      } else {
        let probe = skipSpace(source, next);
        if (source[probe] === "!") probe = skipSpace(source, probe + 1);
        form = source[probe] === "(" ? "call" : "ident";
      }
      rawEnd = next;
    }
    residues.push({ form, raw: source.slice(start, Math.max(rawEnd, start + 60)), offset: start });
    finder.lastIndex = rawEnd;
  }

  // `class:foo` is a modifier, not a `class=` site, and its own token.
  for (const match of source.matchAll(/class:([A-Za-z][A-Za-z0-9_-]*)/g)) {
    tokens.add(match[1]);
    modifier += 1;
  }

  return {
    tokens,
    sites: literal + named + brace,
    literal,
    const: named,
    brace,
    modifier,
    residues,
    unresolved,
  };
}

/** The class hooks defined by the stylesheet, at any nesting depth. */
function cssHooks(cssText) {
  const text = cssText.replace(/\/\*[\s\S]*?\*\//g, "");
  const hooks = new Set();
  let start = 0;
  for (let i = 0; i < text.length; i += 1) {
    if (text[i] === "{") {
      const prelude = text.slice(start, i).trim();
      if (!prelude.startsWith("@")) {
        for (const match of prelude.matchAll(/\.([A-Za-z_][A-Za-z0-9_-]*)/g)) {
          hooks.add(match[1]);
        }
      }
      start = i + 1;
    } else if (text[i] === "}") {
      start = i + 1;
    }
  }
  return hooks;
}

function walkRust(dir) {
  const out = [];
  for (const entry of readdirSync(dir, { withFileTypes: true, recursive: true })) {
    if (entry.isFile() && entry.name.endsWith(".rs")) {
      out.push(join(entry.parentPath ?? entry.path, entry.name));
    }
  }
  return out.sort();
}

const SELF_TESTS = [
  {
    name: "literal",
    source: '<p class="a b">',
    tokens: ["a", "b"],
    residues: 0,
  },
  {
    name: "const",
    source: 'const FOO_CLASS: &str = "foo";\n<p class=FOO_CLASS>',
    tokens: ["foo"],
    residues: 0,
  },
  {
    name: "brace-both-branches",
    source: '<p class={if c { "x-ok" } else { "x-missing" }}>',
    tokens: ["x-missing", "x-ok"],
    residues: 0,
  },
  {
    name: "brace-mixed-const",
    source: 'const BAR_CLASS: &str = "bar";\n<p class={if c { "y" } else { BAR_CLASS }}>',
    tokens: ["bar", "y"],
    residues: 0,
  },
  {
    name: "lowercase-ident",
    source: "<p class=cls>",
    tokens: [],
    residues: 1,
    forms: ["ident"],
  },
  {
    name: "tuple",
    source: '<p class=("w", move || cond)>',
    tokens: [],
    residues: 1,
    forms: ["tuple"],
  },
  {
    name: "closure",
    source: '<p class=move || "v">',
    tokens: [],
    residues: 1,
    forms: ["closure"],
  },
  {
    name: "call",
    source: '<p class=format!("u-{i}")>',
    tokens: [],
    residues: 1,
    forms: ["call"],
  },
];

function runSelfTests() {
  let passed = 0;
  for (const test of SELF_TESTS) {
    const got = extract(test.source);
    const tokens = [...got.tokens].sort();
    const expected = [...test.tokens].sort();
    const forms = got.residues.map((residue) => residue.form);
    const ok =
      tokens.length === expected.length &&
      tokens.every((token, index) => token === expected[index]) &&
      got.residues.length === test.residues &&
      (test.forms === undefined ||
        (forms.length === test.forms.length && forms.every((form, index) => form === test.forms[index])));
    if (ok) {
      passed += 1;
    } else {
      console.error(
        `SELFTEST-FAIL ${test.name}: tokens=[${tokens}] residues=${got.residues.length}` +
          `${forms.length ? ` forms=[${forms}]` : ""}` +
          ` (want [${expected}] / ${test.residues}${test.forms ? ` [${test.forms}]` : ""})`,
      );
    }
  }
  console.log(`SELFTEST ${passed}/${SELF_TESTS.length}`);
  return passed === SELF_TESTS.length;
}

if (runSelfTest && !runSelfTests()) process.exit(1);

const files = walkRust(srcDir);
const totals = { sites: 0, literal: 0, const: 0, brace: 0, modifier: 0, unresolved: 0 };
const emitted = new Set();
const residueLines = [];
for (const file of files) {
  const source = readFileSync(file, "utf8");
  const result = extract(source);
  totals.sites += result.sites;
  totals.literal += result.literal;
  totals.const += result.const;
  totals.brace += result.brace;
  totals.modifier += result.modifier;
  totals.unresolved += result.unresolved;
  for (const token of result.tokens) emitted.add(token);
  for (const residue of result.residues) {
    const relative = file.slice(webUiRoot.length + 1);
    residueLines.push(
      `${relative}:${lineAt(source, residue.offset)} form=${residue.form} ${residue.raw.slice(0, 60)}`,
    );
  }
}

const hooks = cssHooks(readFileSync(cssPath, "utf8"));
const missing = [...emitted].filter((token) => !hooks.has(token)).sort();
const dead = [...hooks].filter((token) => !emitted.has(token)).sort();

console.log(
  `sites=${totals.sites} literal=${totals.literal} const=${totals.const} brace=${totals.brace} residue=${residueLines.length}`,
);
console.log(
  `emitted=${emitted.size} hooks=${hooks.size} missing=${missing.length} dead=${dead.length} unresolved=${totals.unresolved}`,
);
for (const residue of residueLines) console.log(`RESIDUE ${residue}`);
for (const token of missing) console.log(`MISSING ${token}`);
for (const token of dead) console.log(`DEAD ${token}`);
if (asJson) {
  console.log(
    `JSON ${JSON.stringify({ emitted: emitted.size, hooks: hooks.size, missing, dead, sites: totals })}`,
  );
}

const failures = [];
if (residueLines.length > 0) failures.push(`${residueLines.length} unclassified class= site(s)`);
if (missing.length > 0) failures.push(`${missing.length} emitted class(es) have no CSS rule`);
if (dead.length > 0) failures.push(`${dead.length} CSS rule(s) target no emitted class`);
if (totals.unresolved > 0) failures.push(`${totals.unresolved} class= site(s) produced no token`);
if (emitted.size < minEmitted) {
  failures.push(`emitted=${emitted.size} is below the ratchet --min-emitted=${minEmitted}`);
}
if (failures.length > 0) {
  for (const failure of failures) console.error(`FAIL ${failure}`);
  process.exit(1);
}
