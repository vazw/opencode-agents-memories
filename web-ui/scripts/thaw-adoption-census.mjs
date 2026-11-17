#!/usr/bin/env node
// The issue-139 Thaw adoption census.
//
// One measurement, three checks over three inputs:
//   scan   — every raw HTML element kind `web-ui/src` can author (comments and
//            string literals blanked; `<Table` is a component, not a kind).
//   ruling — the human ruling in `docs/evidence/thaw-adoption-census/ruling.md`
//            (two machine-readable regions: kinds and worklist).
//   probe  — the browser probe report (did Thaw forward `attr:` to its root?).
//   routes — the per-route browser walk (`routes.json`).
//
// Exit classes: 2 (harness / input broken) > 1 (violation) > 3 (drift) > 0.
// A failed input never stops the checks that do not read it; each skipped
// check prints `SKIP <check> <input>`.
//
// Self-test: `node web-ui/scripts/thaw-adoption-census.mjs --self-test`.

import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(HERE, '..', '..');

const DEFAULT_SRC = 'web-ui/src';
const KINDS = 'kinds';
const WORKLIST = 'worklist';
const MARKER = {
  [`${KINDS}:begin`]: '<!-- thaw-census:kinds:begin -->',
  [`${KINDS}:end`]: '<!-- thaw-census:kinds:end -->',
  [`${WORKLIST}:begin`]: '<!-- thaw-census:worklist:begin -->',
  [`${WORKLIST}:end`]: '<!-- thaw-census:worklist:end -->',
};

const CLASSES = new Set(['replace', 'keep-native', 'no-equivalent']);
const PROBE_DEPENDENT = ['table', 'thead', 'tbody', 'tr', 'td', 'progress'];
const OPENER_BEFORE = new Set([' ', '\t', '\n', '\r', '\f', '\v', '(', '{', '[', ',', '=', '>', '|', '&']);
const EXCLUDED_FROM_WALK = ['script', 'style', 'noscript', 'template', 'link', 'meta', 'title', 'head'];

// ---------------------------------------------------------------------------
// Scanner
// ---------------------------------------------------------------------------

/// Blank comments and string literals, preserving length so indexes and line
/// numbers survive. `r#"` is not a string we can blank safely — it is a gap.
function blankSource(text) {
  if (/r#"/.test(text)) return { error: 'scanner gap' };
  const out = [];
  let i = 0;
  // Blanked spans keep their newlines: line numbers are read off this text.
  const push = original => out.push(original.replace(/[^\n]/g, ' '));
  while (i < text.length) {
    const char = text[i];
    const next = text[i + 1];
    if (char === '/' && next === '/') {
      const start = i;
      while (i < text.length && text[i] !== '\n') i += 1;
      push(text.slice(start, i));
      continue;
    }
    if (char === '/' && next === '*') {
      const start = i;
      let depth = 0;
      while (i < text.length) {
        if (text[i] === '/' && text[i + 1] === '*') {
          depth += 1;
          i += 2;
          continue;
        }
        if (text[i] === '*' && text[i + 1] === '/') {
          depth -= 1;
          i += 2;
          if (depth === 0) break;
          continue;
        }
        i += 1;
      }
      push(text.slice(start, i));
      continue;
    }
    if (char === '"') {
      const start = i;
      i += 1;
      while (i < text.length && text[i] !== '"') {
        if (text[i] === '\\') i += 1;
        i += 1;
      }
      i += 1;
      push(text.slice(start, i));
      continue;
    }
    out.push(char);
    i += 1;
  }
  return { text: out.join('') };
}

/// The attribute names of the opener starting at `start` (the `<`), scanning
/// quote-, brace- and paren-aware so `=>`, `>` inside a quoted value and a
/// closure body do not truncate the tag.
function attrsOf(text, start) {
  const names = [];
  let i = start + 1;
  while (i < text.length && /[a-z0-9]/.test(text[i])) i += 1;
  const skipSpace = () => {
    while (i < text.length && /\s/.test(text[i])) i += 1;
  };
  const endsHere = () => {
    if (text[i] === '>') return true;
    return text[i] === '/' && text[i + 1] === '>';
  };
  while (i < text.length) {
    skipSpace();
    if (i >= text.length || endsHere()) break;
    const nameStart = i;
    while (i < text.length && /[-A-Za-z0-9_:.@]/.test(text[i])) i += 1;
    if (i === nameStart) {
      i += 1;
      continue;
    }
    names.push(text.slice(nameStart, i));
    skipSpace();
    if (text[i] !== '=') continue;
    i += 1;
    skipSpace();
    if (text[i] === '"' || text[i] === "'") {
      const quote = text[i];
      i += 1;
      while (i < text.length && text[i] !== quote) {
        if (text[i] === '\\') i += 1;
        i += 1;
      }
      i += 1;
      continue;
    }
    if (text[i] === '{') {
      let depth = 0;
      while (i < text.length) {
        if (text[i] === '"' || text[i] === "'") {
          const quote = text[i];
          i += 1;
          while (i < text.length && text[i] !== quote) i += 1;
        } else if (text[i] === '{') {
          depth += 1;
        } else if (text[i] === '}') {
          depth -= 1;
          if (depth === 0) {
            i += 1;
            break;
          }
        }
        i += 1;
      }
      continue;
    }
    // A bare value: consume to the next attribute boundary at depth 0.
    let parens = 0;
    while (i < text.length) {
      const char = text[i];
      if (char === '(' || char === '[') parens += 1;
      else if (char === ')' || char === ']') parens = Math.max(0, parens - 1);
      else if (parens === 0) {
        if (char === '>' || (char === '/' && text[i + 1] === '>')) break;
        if (/\s/.test(char)) {
          const rest = text.slice(i);
          if (/^\s+[A-Za-z_:@][-A-Za-z0-9_:.@]*(\s*=|[\s/]*>)/.test(rest)) break;
        }
      }
      i += 1;
    }
  }
  return names;
}

/// Every authored opener in one file: `{file, line, tag, attrs}`.
function scanSource(text, file) {
  const blanked = blankSource(text);
  if (blanked.error) return { error: blanked.error };
  const openers = [];
  for (let index = 0; index < blanked.text.length; index += 1) {
    if (blanked.text[index] !== '<') continue;
    const previous = index === 0 ? '' : blanked.text[index - 1];
    if (index !== 0 && !OPENER_BEFORE.has(previous)) continue;
    const match = /^([a-z][a-z0-9]*)(?=[\s/>]|$)/.exec(blanked.text.slice(index + 1, index + 40));
    if (!match) continue;
    const line = blanked.text.slice(0, index).split('\n').length;
    // Attributes are read from the original text: the blanked copy has no
    // quoted values left, and only an opener's own span is read here.
    openers.push({ file, line, tag: match[1], attrs: attrsOf(text, index) });
  }
  return { openers };
}

function listSourceFiles(srcDir) {
  const files = [];
  const walk = dir => {
    for (const name of readdirSync(dir).sort()) {
      const full = join(dir, name);
      if (statSync(full).isDirectory()) walk(full);
      else if (name.endsWith('.rs')) files.push(full);
    }
  };
  walk(srcDir);
  return files;
}

/// Scan a directory. `kinds` maps a tag to its occurrences; `files` are the
/// relative names the worklist may name.
function scanSourceDir(srcDir) {
  if (!existsSync(srcDir)) return { error: `source directory missing: ${srcDir}` };
  const files = listSourceFiles(srcDir);
  const kinds = new Map();
  const relative = new Set();
  for (const file of files) {
    const name = file.slice(srcDir.length + 1).split(/[\\/]/).join('/');
    relative.add(name);
    const scanned = scanSource(readFileSync(file, 'utf8'), name);
    if (scanned.error) return { error: `${scanned.error}: ${name}` };
    for (const opener of scanned.openers) {
      const list = kinds.get(opener.tag) || [];
      list.push(opener);
      kinds.set(opener.tag, list);
    }
  }
  // Occurrences read in document order: by line, then by file name.
  for (const list of kinds.values()) list.sort((a, b) => (a.line === b.line ? a.file.localeCompare(b.file) : a.line - b.line));
  return { kinds, files: relative, count: [...kinds.values()].reduce((total, list) => total + list.length, 0) };
}

// ---------------------------------------------------------------------------
// Ruling
// ---------------------------------------------------------------------------

const cellsOf = line => line.split('|').slice(1, -1).map(cell => cell.trim());

function occurrencesText(occurrences) {
  return occurrences.map(occurrence => `${occurrence.file}:${occurrence.line}`).join(', ');
}

/// Parse the ruling. Every rule is a hard error (exit 2) or a value.
function parseRuling(text, sourceFiles) {
  const counts = Object.fromEntries(Object.entries(MARKER).map(([key, marker]) => [key, text.split(marker).length - 1]));
  if (Object.values(counts).every(count => count === 0)) return { error: 'missing' };
  if (Object.values(counts).some(count => count === 0)) return { error: 'marker' };
  if (Object.values(counts).some(count => count > 1)) return { error: 'marker' };

  const region = (beginKey, endKey) => {
    const begin = text.indexOf(MARKER[beginKey]) + MARKER[beginKey].length;
    const end = text.indexOf(MARKER[endKey]);
    if (end < begin) return { error: 'marker' };
    return { lines: text.slice(begin, end).split('\n') };
  };

  const kindsRegion = region(`${KINDS}:begin`, `${KINDS}:end`);
  if (kindsRegion.error) return kindsRegion;
  const worklistRegion = region(`${WORKLIST}:begin`, `${WORKLIST}:end`);
  if (worklistRegion.error) return worklistRegion;

  const kinds = new Map();
  for (const raw of kindsRegion.lines) {
    const line = raw.trim();
    if (!line.startsWith('|')) continue;
    const cells = cellsOf(line);
    if (cells[0] === 'kind' || cells[0].startsWith('-')) continue;
    if (cells.length !== 5) return { error: `row shape: ${line}` };
    const [kind, klass, attrs, reason, basis] = cells;
    if (!CLASSES.has(klass)) return { error: `class: ${line}` };
    const allowed = attrs === '-' ? [] : attrs.split(',').map(name => name.trim()).filter(Boolean);
    if (klass !== 'replace' && attrs !== '-') return { error: `attrs: ${line}` };
    if (kinds.has(kind)) return { error: `duplicate: ${kind}` };
    kinds.set(kind, { kind, klass, attrs: allowed, attrsText: attrs, reason, basis, line });
  }

  const worklist = [];
  for (const raw of worklistRegion.lines) {
    const line = raw.trim();
    if (!line.startsWith('|')) continue;
    const cells = cellsOf(line);
    if (cells[0] === 'file' || cells[0].startsWith('-')) continue;
    if (cells.length !== 3) return { error: `row shape: ${line}` };
    const [file, kind, countText] = cells;
    if (!/^\d+$/.test(countText)) return { error: `count: ${line}` };
    if (!sourceFiles.has(file)) return { error: `worklist file absent from source: ${file}` };
    const row = kinds.get(kind);
    if (!row || row.klass !== 'replace') return { error: `worklist kind not replace: ${kind}` };
    worklist.push({ file, kind, count: Number.parseInt(countText, 10), line });
  }
  return { kinds, worklist };
}

// ---------------------------------------------------------------------------
// Probe check
// ---------------------------------------------------------------------------

const VERDICT = /^probe-(table|progress|progress-label):[ \t]*([A-Za-z]+)[ \t]*$/gm;
const BRANCH_ATTRS = {
  table: ['class', 'data-testid'],
  thead: ['class'],
  tbody: ['class'],
  tr: ['class'],
  td: ['class'],
  progress: ['aria-label', 'class', 'value', 'max'],
};

function sameNameSet(left, right) {
  const a = [...left].sort().join(',');
  const b = [...right].sort().join(',');
  return a === b;
}

/// Check the probe report against the ruling. `head` is the current HEAD.
function checkProbe(reportText, ruling, head) {
  const findings = [];
  if (!reportText.trim()) {
    findings.push({ exit: 2, scope: 'probe', where: 'report', message: 'probe' });
    return { findings, verdicts: null };
  }
  if (!/^HEAD:\s*\S+/m.test(reportText) || !/^bundle sha256:\s*[0-9a-f]{12}\s*$/m.test(reportText)) {
    findings.push({ exit: 2, scope: 'probe', where: 'report', message: 'probe' });
    return { findings, verdicts: null };
  }
  if (/^probe-harness:\s*FAILED/m.test(reportText)) {
    findings.push({ exit: 2, scope: 'probe', where: 'report', message: 'probe' });
    return { findings, verdicts: null };
  }
  const verdicts = { table: null, progress: null, label: null };
  const seen = new Set();
  for (const match of reportText.matchAll(VERDICT)) {
    const name = match[1];
    if (seen.has(name)) {
      findings.push({ exit: 2, scope: 'probe', where: name, message: 'probe duplicate' });
      return { findings, verdicts: null };
    }
    seen.add(name);
    const value = match[2];
    if (value !== 'landed' && value !== 'dropped') {
      findings.push({ exit: 2, scope: 'probe', where: name, message: `probe value (${value})` });
      return { findings, verdicts: null };
    }
    verdicts[name === 'progress-label' ? 'label' : name] = value;
  }
  for (const [name, key] of [['table', 'table'], ['progress', 'progress'], ['progress-label', 'label']]) {
    if (verdicts[key] === null) {
      findings.push({ exit: 2, scope: 'probe', where: name, message: 'probe' });
      return { findings, verdicts: null };
    }
  }
  if (verdicts.label === 'landed' && verdicts.progress === 'dropped') {
    findings.push({ exit: 2, scope: 'probe', where: 'progress-label', message: 'probe inconsistent' });
    return { findings, verdicts: null };
  }

  const reportHead = /^HEAD:\s*(\S+)/m.exec(reportText)[1];
  if (reportHead !== head) {
    findings.push({ exit: 3, scope: 'probe', where: 'report', message: `probe stale (${reportHead} ≠ ${head})` });
  }

  const tableReplace = verdicts.table === 'landed';
  const progressReplace = verdicts.progress === 'landed' && verdicts.label === 'landed';
  const branch = {
    table: tableReplace ? 'replace' : 'keep-native',
    thead: tableReplace ? 'replace' : 'no-equivalent',
    tbody: tableReplace ? 'replace' : 'no-equivalent',
    tr: tableReplace ? 'replace' : 'no-equivalent',
    td: tableReplace ? 'replace' : 'no-equivalent',
    progress: progressReplace ? 'replace' : 'no-equivalent',
  };
  for (const kind of PROBE_DEPENDENT) {
    const row = ruling.kinds.get(kind);
    if (!row) continue;
    if (row.klass !== branch[kind]) {
      findings.push({
        exit: 1,
        scope: kind,
        where: 'ruling',
        message: `probe contradiction (branch ${branch[kind]}, ruling ${row.klass})`,
      });
      continue;
    }
    if (branch[kind] === 'replace' && !sameNameSet(row.attrs, BRANCH_ATTRS[kind])) {
      findings.push({
        exit: 1,
        scope: kind,
        where: 'ruling',
        message: `probe contradiction (attrs ${BRANCH_ATTRS[kind].join(',')}, ruling ${row.attrsText})`,
      });
    }
  }
  return { findings, verdicts };
}

// ---------------------------------------------------------------------------
// Policy check
// ---------------------------------------------------------------------------

function checkPolicy(scan, ruling) {
  const findings = [];
  for (const [kind, occurrences] of scan.kinds) {
    if (!ruling.kinds.has(kind)) {
      findings.push({ exit: 1, scope: kind, where: 'unclassified', message: `(${occurrencesText(occurrences)})` });
    }
  }
  for (const [kind, row] of ruling.kinds) {
    if (!scan.kinds.has(kind)) {
      findings.push({ exit: 3, scope: kind, where: 'ruling', message: 'stale' });
      continue;
    }
    if (row.klass !== 'replace') continue;
    const byFile = new Map();
    for (const occurrence of scan.kinds.get(kind)) {
      byFile.set(occurrence.file, (byFile.get(occurrence.file) || 0) + 1);
    }
    for (const [file, count] of byFile) {
      const allowed = ruling.worklist.find(entry => entry.file === file && entry.kind === kind);
      const worklistCount = allowed ? allowed.count : 0;
      if (count > worklistCount) {
        findings.push({
          exit: 1,
          scope: kind,
          where: file,
          message: `replace outside worklist (${count} > ${worklistCount})`,
        });
      }
    }
  }
  for (const entry of ruling.worklist) {
    const sourceCount = (scan.kinds.get(entry.kind) || []).filter(occurrence => occurrence.file === entry.file).length;
    if (entry.count > sourceCount) {
      findings.push({
        exit: 3,
        scope: entry.kind,
        where: entry.file,
        message: `worklist overstated (${entry.count} > ${sourceCount})`,
      });
    }
  }
  for (const [kind, row] of ruling.kinds) {
    if (row.klass !== 'replace') continue;
    for (const occurrence of scan.kinds.get(kind) || []) {
      for (const attribute of occurrence.attrs) {
        if (!row.attrs.includes(attribute)) {
          findings.push({
            exit: 1,
            scope: kind,
            where: `${occurrence.file}:${occurrence.line}`,
            message: `attribute not in attrs-allowed (${attribute})`,
          });
        }
      }
    }
  }
  return { findings };
}

// ---------------------------------------------------------------------------
// Route gate
// ---------------------------------------------------------------------------

function checkRoutes(routes, universe) {
  const findings = [];
  const perRoute = [];
  for (const route of routes) {
    const status = String(route.status || '');
    if (!/^measured$/i.test(status)) {
      perRoute.push({ route: route.route, status, counts: null, thawInternal: [], unclassedUnderThaw: [] });
      continue;
    }
    const counts = new Map();
    const thawInternal = new Set();
    const unclassed = new Set();
    for (const entry of route.authoredTags || []) {
      if (universe.has(entry.tag)) {
        counts.set(entry.tag, (counts.get(entry.tag) || 0) + entry.count);
      } else if (entry.inThawSubtree) {
        thawInternal.add(entry.tag);
      } else {
        findings.push({
          exit: 1,
          scope: route.route,
          where: entry.tag,
          message: 'unrecognised authored tag (route)',
        });
      }
    }
    for (const entry of route.unclassedUnderThaw || []) unclassed.add(entry.localName);
    perRoute.push({
      route: route.route,
      status,
      counts,
      thawInternal: [...thawInternal].sort(),
      unclassedUnderThaw: [...unclassed].sort(),
    });
  }
  return { findings, perRoute };
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

const EXIT_ORDER = [0, 3, 1, 2];

function highestExit(findings) {
  let exit = 0;
  for (const finding of findings) {
    if (EXIT_ORDER.indexOf(finding.exit) > EXIT_ORDER.indexOf(exit)) exit = finding.exit;
  }
  return exit;
}

function currentHead() {
  return execFileSync('git', ['rev-parse', '--short', 'HEAD'], { cwd: REPO_ROOT, encoding: 'utf8' }).trim();
}

function parseArgs(argv) {
  const options = {};
  for (const arg of argv) {
    const match = /^--([a-z-]+)=(.*)$/.exec(arg);
    if (match) {
      options[match[1]] = match[2];
      continue;
    }
    const bare = /^--([a-z-]+)$/.exec(arg);
    if (bare) {
      options[bare[1]] = true;
      continue;
    }
    options.unknown = arg;
  }
  return options;
}

function readIfExists(file) {
  if (!existsSync(file)) return { missing: true };
  return { text: readFileSync(file, 'utf8') };
}

/// Run the census. Returns `{code, findings, summary, perRoute, verdicts}` and
/// writes the report lines through `io.write`.
function main(argv, io = {}, deps = {}) {
  const write = io.write || (line => process.stdout.write(`${line}\n`));
  const head = deps.head || currentHead;
  const options = parseArgs(argv);
  const known = new Set(['self-test', 'src', 'ruling', 'probe', 'routes']);

  if (options.unknown !== undefined) {
    write(`EXIT2 mode - unknown flag ${options.unknown}`);
    write(`census kinds=0 unclassified=0 stale=0 replace=0 worklist=0 violations=0 drift=0 harness=1 exit=2`);
    return { code: 2, findings: [{ exit: 2, scope: 'mode', where: '-', message: `unknown flag ${options.unknown}` }] };
  }
  const ruled = options.ruling !== undefined;
  const routed = options.routes !== undefined;
  const probed = options.probe !== undefined;
  const sourced = options.src !== undefined;
  let modeError = null;
  if (!sourced && ruled) modeError = '--ruling without --src';
  else if (!sourced && routed) modeError = '--routes without --src';
  else if (probed && !ruled) modeError = '--probe without --ruling';
  else if (sourced && !ruled && !routed) modeError = '--src alone';
  else if (!sourced && !ruled && !routed) modeError = 'no mode selected';
  if (modeError) {
    write(`EXIT2 mode - ${modeError}`);
    write(`census kinds=0 unclassified=0 stale=0 replace=0 worklist=0 violations=0 drift=0 harness=1 exit=2`);
    return { code: 2, findings: [{ exit: 2, scope: 'mode', where: '-', message: modeError }] };
  }

  const srcDir = resolve(REPO_ROOT, options.src);
  const findings = [];
  const skipped = [];
  const scan = scanSourceDir(srcDir);
  if (scan.error) {
    findings.push({ exit: 2, scope: 'scan', where: options.src, message: scan.error });
  }
  const scanOk = !scan.error;

  let ruling = null;
  let rulingFailed = false;
  if (ruled) {
    const file = readIfExists(resolve(REPO_ROOT, options.ruling));
    if (file.missing) {
      findings.push({ exit: 2, scope: 'ruling', where: options.ruling, message: 'missing' });
      rulingFailed = true;
    } else if (!scanOk) {
      // The rows parse on their own, but the worklist is validated against the
      // source file set, so a scan gap makes the ruling unparseable.
      rulingFailed = true;
    } else {
      const parsed = parseRuling(file.text, scan.files);
      if (parsed.error) {
        findings.push({ exit: 2, scope: 'ruling', where: options.ruling, message: parsed.error });
        rulingFailed = true;
      } else {
        ruling = parsed;
      }
    }
  }

  let probe = null;
  let probeFailed = false;
  if (probed) {
    const file = readIfExists(resolve(REPO_ROOT, options.probe));
    if (file.missing) {
      findings.push({ exit: 2, scope: 'probe', where: options.probe, message: 'probe' });
      probeFailed = true;
    } else {
      probe = file.text;
    }
  }

  let routes = null;
  let routesFailed = false;
  if (routed) {
    const file = readIfExists(resolve(REPO_ROOT, options.routes));
    if (file.missing) {
      findings.push({ exit: 2, scope: 'routes', where: options.routes, message: 'missing' });
      routesFailed = true;
    } else {
      try {
        routes = JSON.parse(file.text);
      } catch (error) {
        findings.push({ exit: 2, scope: 'routes', where: options.routes, message: `unparseable (${error.message})` });
        routesFailed = true;
      }
    }
  }

  // (c) checkProbe reads the ruling and the probe.
  let verdicts = null;
  if (probed) {
    if (rulingFailed) skipped.push('checkProbe ruling');
    else if (probeFailed) skipped.push('checkProbe probe');
    else {
      const result = checkProbe(probe, ruling, head());
      verdicts = result.verdicts;
      findings.push(...result.findings);
    }
  }
  // (d) checkPolicy reads the scan and the ruling.
  if (ruled) {
    if (rulingFailed) skipped.push('checkPolicy ruling');
    else if (!scanOk) skipped.push('checkPolicy scan');
    else findings.push(...checkPolicy(scan, ruling).findings);
  }
  // (e) checkRoutes reads the scan and the routes.
  let perRoute = [];
  if (routesFailed) skipped.push('checkRoutes routes');
  else if (routes) {
    if (!scanOk) skipped.push('checkRoutes scan');
    else {
      const result = checkRoutes(routes, new Set(scan.kinds.keys()));
      findings.push(...result.findings);
      perRoute = result.perRoute;
    }
  }

  // Findings were pushed in check order: inputs (a)/(b), then checkProbe (c),
  // then checkPolicy (d), then checkRoutes (e).
  const all = findings;
  if (routes && scanOk) {
    for (const route of perRoute) {
      if (route.status === 'measured') {
        const counts = [...route.counts.entries()].sort().map(([tag, count]) => `${tag}=${count}`).join(' ');
        write(`ROUTE ${route.route} ${counts}`);
        if (route.thawInternal.length) write(`THAWINTERNAL ${route.route} ${route.thawInternal.join(',')}`);
        if (route.unclassedUnderThaw.length) write(`UNCLASSED ${route.route} ${route.unclassedUnderThaw.join(',')}`);
      } else {
        write(`ROUTE ${route.route} ${route.status}`);
      }
    }
  }
  for (const skip of skipped) write(`SKIP ${skip}`);
  for (const finding of all) {
    write(`EXIT${finding.exit} ${finding.scope} ${finding.where} ${finding.message}`);
  }

  const exit = highestExit(all);
  const summary = {
    kinds: scanOk ? scan.kinds.size : 0,
    unclassified: all.filter(finding => finding.exit === 1 && finding.where === 'unclassified').length,
    stale: all.filter(finding => finding.exit === 3).length,
    replace: ruling ? [...ruling.kinds.values()].filter(row => row.klass === 'replace').length : 0,
    worklist: ruling ? ruling.worklist.length : 0,
    violations: all.filter(finding => finding.exit === 1).length,
    drift: all.filter(finding => finding.exit === 3).length,
    harness: all.filter(finding => finding.exit === 2).length,
    exit,
  };
  write(
    `census kinds=${summary.kinds} unclassified=${summary.unclassified} stale=${summary.stale} replace=${summary.replace} worklist=${summary.worklist} violations=${summary.violations} drift=${summary.drift} harness=${summary.harness} exit=${summary.exit}`
  );
  return { code: exit, findings: all, summary, perRoute, verdicts, scanCount: scanOk ? scan.count : 0 };
}

// ---------------------------------------------------------------------------
// Self-test
// ---------------------------------------------------------------------------

function selfTest() {
  const results = [];
  const check = (name, body) => {
    try {
      body();
      results.push([name, true, '']);
    } catch (error) {
      results.push([name, false, error.message]);
    }
  };
  const equal = (actual, expected, label) => {
    const a = JSON.stringify(actual);
    const b = JSON.stringify(expected);
    if (a !== b) throw new Error(`${label || 'value'}: expected ${b}, got ${a}`);
  };
  const ok = (value, label) => {
    if (!value) throw new Error(`${label || 'assertion'} failed`);
  };

  const rule = (rows, worklistRows = []) => {
    const body = rows.map(cells => `| ${cells.join(' | ')} |`).join('\n');
    const work = worklistRows.map(cells => `| ${cells.join(' | ')} |`).join('\n');
    return [
      '<!-- thaw-census:kinds:begin -->',
      '| kind | class | attrs-allowed | reason | basis |',
      '| --- | --- | --- | --- | --- |',
      body,
      '<!-- thaw-census:kinds:end -->',
      '<!-- thaw-census:worklist:begin -->',
      '| file | kind | count |',
      '| --- | --- | --- |',
      work,
      '<!-- thaw-census:worklist:end -->',
    ].join('\n');
  };
  const files = new Set(['x.rs', 'y.rs', 'core_screens.rs']);
  const parseOk = (text) => {
    const parsed = parseRuling(text, files);
    ok(!parsed.error, `parse failed: ${parsed.error}`);
    return parsed;
  };
  const scanKinds = (text, file = 'x.rs') => {
    const scanned = scanSource(text, file);
    ok(!scanned.error, `scan failed: ${scanned.error}`);
    return [...new Set(scanned.openers.map(opener => opener.tag))].sort();
  };
  const sink = () => {
    const lines = [];
    return { lines, io: { write: line => lines.push(line) } };
  };

  // --- scanner ---
  check('scan_ignores_comment_tags', () => {
    equal(scanKinds('// <td>\n<td class="a">'), ['td']);
    equal(scanSource('// <td>\n<td class="a">', 'x.rs').openers.length, 1);
  });
  check('scan_ignores_string_tags', () => {
    equal(scanSource('"<p>not markup</p>" plus <p>', 'x.rs').openers.length, 1);
    equal(scanKinds('"<p>not markup</p>" plus <p>'), ['p']);
  });
  check('scan_requires_tag_boundary', () => {
    equal(scanKinds('let v: Vec<td> = x;\n<td>'), ['td']);
  });
  check('scan_excludes_thaw_components', () => {
    equal(scanKinds('<Table>\n<table>'), ['table']);
  });
  check('scan_multiline_opener', () => {
    equal(scanKinds('<progress\n    class="x"><select\n    id="y">'), ['progress', 'select']);
  });
  check('scan_turbofish_not_opener', () => {
    equal(scanKinds('let a = parse::<i64>(); let b = None::<usize>;'), []);
  });
  check('scan_paren_brace_opener', () => {
    equal(scanKinds('foo(<div class="a"/>)'), ['div']);
    equal(scanKinds('view! {<span>'), ['span']);
    equal(scanKinds('foo(<div class="a"/>) and view! {<span>'), ['div', 'span']);
  });
  check('scan_unspaced_comparison_not_opener', () => {
    equal(scanKinds('if (x)<span {}'), []);
    equal(scanKinds("if c == 'a'<p {}"), []);
  });
  check('scan_closing_brace_not_opener', () => {
    equal(scanKinds('if { 1 }<td {}'), []);
  });
  check('scan_generic_and_comparison_stay_clean', () => {
    for (const text of [
      'if n < p {',
      'if v[0]<table.len()',
      'a <= b',
      '1 << table',
      'Vec<td>',
      'Option< u8 >',
      '|x| x < y',
      "<'a>",
      'foo::<i64>',
      'None::<usize>',
    ]) {
      equal(scanKinds(text), [], text);
    }
    equal(scanKinds('x <p {'), ['p']);
    equal(scanKinds('view! { <div class="x">'), ['div']);
  });
  check('scan_raw_string_gap', () => {
    equal(scanSource('let s = r#"<td>"#;', 'x.rs').error, 'scanner gap');
  });

  // --- attrs ---
  check('attrs_survive_closure_arrow', () => {
    const text = '<input on:change=move |e| set(event_target_checked(&e) > 0) data-testid="t" />';
    equal(scanSource(text, 'x.rs').openers[0].attrs, ['on:change', 'data-testid']);
  });
  check('attrs_quoted_gt', () => {
    equal(scanSource('<td title="a>b" class="x">', 'x.rs').openers[0].attrs, ['title', 'class']);
  });

  // --- parse ---
  check('parse_rejects_duplicate_kind', () => {
    const text = rule([
      ['td', 'keep-native', '-', 'r', 'b'],
      ['td', 'replace', 'class', 'r', 'b'],
    ]);
    equal(parseRuling(text, files).error, 'duplicate: td');
  });
  check('parse_rejects_undecided_class', () => {
    equal(parseRuling(rule([['table', 'undecided', '-', 'r', 'b']]), files).error === 'class: | table | undecided | - | r | b |', true);
  });
  check('parse_rejects_attrs_on_non_replace', () => {
    equal(parseRuling(rule([['select', 'keep-native', 'class', 'r', 'b']]), files).error === 'attrs: | select | keep-native | class | r | b |', true);
  });
  check('parse_rejects_bad_row_shape', () => {
    equal(parseRuling(rule([['td', 'replace', 'class', 'reason-only']]), files).error === 'row shape: | td | replace | class | reason-only |', true);
  });
  check('parse_begin_without_end', () => {
    const text = '<!-- thaw-census:kinds:begin -->\n| td | keep-native | - | r | b |\n<!-- thaw-census:worklist:begin -->\n<!-- thaw-census:worklist:end -->';
    equal(parseRuling(text, files).error, 'marker');
  });
  check('parse_prose_pipe_ignored', () => {
    const text = `prose | td | replace | not | a | row\n${rule([['p', 'no-equivalent', '-', 'r', 'b']])}`;
    const parsed = parseOk(text);
    equal([...parsed.kinds.keys()], ['p']);
  });
  check('parse_worklist_file_absent', () => {
    const text = rule([['td', 'replace', 'class', 'r', 'b']], [['ghost.rs', 'td', '1']]);
    equal(parseRuling(text, files).error, 'worklist file absent from source: ghost.rs');
  });
  check('parse_worklist_kind_not_replace', () => {
    const text = rule([['p', 'keep-native', '-', 'r', 'b']], [['x.rs', 'p', '1']]);
    equal(parseRuling(text, files).error, 'worklist kind not replace: p');
  });
  check('parse_missing_markers', () => {
    equal(parseRuling('| td | replace | class | r | b |', files).error, 'missing');
  });
  check('parse_rejects_bad_count', () => {
    const text = rule([['td', 'replace', 'class', 'r', 'b']], [['x.rs', 'td', 'three']]);
    equal(parseRuling(text, files).error === 'count: | x.rs | td | three |', true);
  });

  // --- probe ---
  const probeReport = (overrides = {}) => {
    const head = overrides.head || 'abc1234';
    const table = overrides.table || 'landed';
    const progress = overrides.progress || 'landed';
    const label = overrides.label || 'landed';
    const lines = [
      '# Probe',
      '',
      `HEAD: ${head}`,
      'bundle sha256: 0123456789ab',
      '',
      'probe-table: ' + table,
      'probe-progress: ' + progress,
      'probe-progress-label: ' + label,
    ];
    if (overrides.harness) lines.push(`probe-harness: FAILED at build: ${overrides.harness}`);
    return lines.join('\n');
  };
  const landedRuling = () =>
    parseOk(
      rule([
        ['table', 'replace', 'class,data-testid', 'r', 'PROBE probe-table'],
        ['thead', 'replace', 'class', 'r', 'PROBE probe-table'],
        ['tbody', 'replace', 'class', 'r', 'PROBE probe-table'],
        ['tr', 'replace', 'class', 'r', 'PROBE probe-table'],
        ['td', 'replace', 'class', 'r', 'PROBE probe-table'],
        ['progress', 'replace', 'aria-label,class,value,max', 'r', 'PROBE probe-progress'],
        ['th', 'no-equivalent', '-', 'r', 'READ table/mod.rs:23'],
      ])
    );
  check('probe_missing_line', () => {
    const text = probeReport()
      .split('\n')
      .filter(line => !line.startsWith('probe-progress-label:'))
      .join('\n');
    const result = checkProbe(text, landedRuling(), 'abc1234');
    equal(result.findings[0].exit, 2);
    equal(result.findings[0].message, 'probe');
  });
  check('probe_missing_head_or_bundle', () => {
    const text = probeReport().replace(/^HEAD:.*$/m, '');
    equal(checkProbe(text, landedRuling(), 'abc1234').findings[0].exit, 2);
    const noBundle = probeReport().replace(/^bundle sha256:.*$/m, '');
    equal(checkProbe(noBundle, landedRuling(), 'abc1234').findings[0].exit, 2);
  });
  check('probe_duplicate_verdict', () => {
    const text = `${probeReport()}\nprobe-table: landed`;
    equal(checkProbe(text, landedRuling(), 'abc1234').findings[0].message, 'probe duplicate');
  });
  check('probe_bad_value', () => {
    equal(checkProbe(probeReport({ table: 'maybe' }), landedRuling(), 'abc1234').findings[0].message, 'probe value (maybe)');
  });
  check('probe_label_without_progress', () => {
    const text = probeReport({ progress: 'dropped', label: 'landed' });
    equal(checkProbe(text, landedRuling(), 'abc1234').findings[0].message, 'probe inconsistent');
  });
  check('probe_harness_failed', () => {
    const text = probeReport({ harness: 'build' });
    equal(checkProbe(text, landedRuling(), 'abc1234').findings[0].exit, 2);
  });
  check('probe_stale_head', () => {
    const text = probeReport({ head: '0000000' });
    const result = checkProbe(text, landedRuling(), 'abc1234');
    equal(result.findings.length, 1);
    equal(result.findings[0].exit, 3);
    ok(result.findings[0].message.startsWith('probe stale'));
  });
  check('probe_contradicts_row', () => {
    const ruling = parseOk(rule([['table', 'keep-native', '-', 'r', 'b']]));
    const result = checkProbe(probeReport(), ruling, 'abc1234');
    equal(result.findings[0].exit, 1);
    ok(result.findings[0].message.startsWith('probe contradiction'));
  });
  check('probe_attrs_contradict', () => {
    const ruling = parseOk(rule([['table', 'replace', 'class', 'r', 'b']]));
    const result = checkProbe(probeReport(), ruling, 'abc1234');
    equal(result.findings[0].exit, 1);
    ok(result.findings[0].message.includes('attrs'));
  });
  check('probe_progress_attrs_match_head', () => {
    const short = parseOk(rule([['progress', 'replace', 'aria-label,class', 'r', 'b']]));
    equal(checkProbe(probeReport(), short, 'abc1234').findings[0].exit, 1);
    const full = parseOk(rule([['progress', 'replace', 'aria-label,class,value,max', 'r', 'b']]));
    equal(checkProbe(probeReport(), full, 'abc1234').findings, []);
  });

  // --- policy ---
  const scanOf = text => {
    const scanned = scanSource(text, 'x.rs');
    ok(!scanned.error);
    const kinds = new Map();
    for (const opener of scanned.openers) {
      const list = kinds.get(opener.tag) || [];
      list.push(opener);
      kinds.set(opener.tag, list);
    }
    return { kinds, files: new Set(['x.rs']), count: scanned.openers.length };
  };
  check('policy_unclassified_per_kind', () => {
    const ruling = parseOk(
      rule([
        ['dt', 'no-equivalent', '-', 'r', 'b'],
        ['dd', 'no-equivalent', '-', 'r', 'b'],
      ])
    );
    const occurrences = [
      { file: 'screens.rs', line: 213, tag: 'dl', attrs: [] },
      { file: 'core_screens.rs', line: 2880, tag: 'dl', attrs: [] },
      { file: 'core_screens.rs', line: 2998, tag: 'dl', attrs: [] },
    ];
    const scan = {
      kinds: new Map([
        ['dl', occurrences],
        ['dt', [{ file: 'core_screens.rs', line: 2881, tag: 'dt', attrs: [] }]],
        ['dd', [{ file: 'core_screens.rs', line: 2882, tag: 'dd', attrs: [] }]],
      ]),
      files: new Set(['screens.rs', 'core_screens.rs']),
      count: 5,
    };
    const result = checkPolicy(scan, ruling);
    equal(result.findings.length, 1);
    equal(result.findings[0].exit, 1);
    equal(result.findings[0].scope, 'dl');
    equal(result.findings[0].message, '(screens.rs:213, core_screens.rs:2880, core_screens.rs:2998)');
  });
  check('policy_replace_outside_worklist', () => {
    const ruling = parseOk(rule([['td', 'replace', 'class', 'r', 'b']]));
    const result = checkPolicy(scanOf('<td>'), ruling);
    equal(result.findings[0].exit, 1);
    equal(result.findings[0].message, 'replace outside worklist (1 > 0)');
  });
  check('policy_replace_count_grew', () => {
    const ruling = parseRuling(rule([['td', 'replace', 'class', 'r', 'b']], [['x.rs', 'td', '1']]), new Set(['x.rs']));
    ok(!ruling.error, ruling.error);
    const result = checkPolicy(scanOf('<td><td>'), ruling);
    equal(result.findings[0].exit, 1);
    equal(result.findings[0].message, 'replace outside worklist (2 > 1)');
  });
  check('policy_replace_attr_forbidden', () => {
    const ruling = parseOk(rule([['td', 'replace', 'class', 'r', 'b']], [['x.rs', 'td', '1']]));
    const result = checkPolicy(scanOf('<td data-testid="x">'), ruling);
    equal(result.findings[0].message, 'attribute not in attrs-allowed (data-testid)');
  });
  check('drift_stale_kind', () => {
    const ruling = parseOk(rule([['p', 'no-equivalent', '-', 'r', 'b'], ['marquee', 'no-equivalent', '-', 'r', 'b']]));
    const result = checkPolicy(scanOf('<p>'), ruling);
    equal(result.findings[0].exit, 3);
    equal(result.findings[0].scope, 'marquee');
    equal(result.findings[0].message, 'stale');
  });
  check('drift_worklist_overstated', () => {
    const ruling = parseOk(rule([['td', 'replace', 'class', 'r', 'b']], [['x.rs', 'td', '3']]));
    const result = checkPolicy(scanOf('<td><td>'), ruling);
    equal(result.findings[0].exit, 3);
    equal(result.findings[0].message, 'worklist overstated (3 > 2)');
  });

  // --- routes ---
  const universe = new Set(['p', 'span', 'td', 'div']);
  check('route_thaw_internal_svg_not_gated', () => {
    const routes = [
      {
        route: '/',
        status: 'measured',
        authoredTags: [
          { tag: 'svg', inThawSubtree: true, count: 2 },
          { tag: 'path', inThawSubtree: true, count: 2 },
        ],
        unclassedUnderThaw: [{ localName: 'svg', parentClass: 'thaw-icon' }],
      },
    ];
    const result = checkRoutes(routes, universe);
    equal(result.findings, []);
    equal(result.perRoute[0].thawInternal, ['path', 'svg']);
  });
  check('route_counts_once_thaw_span', () => {
    const routes = [
      {
        route: '/',
        status: 'measured',
        authoredTags: [{ tag: 'span', inThawSubtree: true, count: 1 }],
        unclassedUnderThaw: [{ localName: 'span', parentClass: 'thaw-card' }],
      },
    ];
    const result = checkRoutes(routes, universe);
    equal(result.perRoute[0].counts.get('span'), 1);
    equal(result.perRoute[0].unclassedUnderThaw, ['span']);
  });
  check('routes_unrecognised_outside_thaw', () => {
    const routes = [{ route: '/', status: 'measured', authoredTags: [{ tag: 'svg', inThawSubtree: false, count: 1 }], unclassedUnderThaw: [] }];
    const result = checkRoutes(routes, universe);
    equal(result.findings[0].exit, 1);
    equal(result.findings[0].message, 'unrecognised authored tag (route)');
  });
  check('routes_not_measured_ungated', () => {
    const routes = [{ route: '/x', status: 'NOT MEASURED — did not settle', authoredTags: [], unclassedUnderThaw: [] }];
    json_equal(checkRoutes(routes, universe).findings, []);
  });
  function json_equal(a, b) {
    equal(a, b);
  }

  // --- CLI + combined ---
  const scratch = join(REPO_ROOT, '.tmp', 'scratch', 'issue139', 'self-test');
  mkdirSync(scratch, { recursive: true });
  const writeFile = (name, text) => {
    const file = join(scratch, name);
    writeFileSync(file, text);
    return file;
  };
  const run = (args, deps = {}) => {
    const { lines, io } = sink();
    const result = main(args, io, { head: () => 'abc1234', ...deps });
    return { result, lines };
  };

  check('exit_precedence', () => {
    const src = join(scratch, 'prec-src');
    mkdirSync(src, { recursive: true });
    writeFileSync(join(src, 'a.rs'), '<span>');
    const ruling = writeFile('prec-ruling.md', rule([]));
    const probe = writeFile('prec-probe.md', probeReport({ head: '0000000', table: 'dropped', progress: 'dropped', label: 'dropped' }));
    const { result, lines } = run([`--src=${src}`, `--ruling=${ruling}`, `--probe=${probe}`]);
    equal(result.code, 1);
    const stale = lines.findIndex(line => line.startsWith('EXIT3 probe'));
    const violation = lines.findIndex(line => line.startsWith('EXIT1 span'));
    ok(stale !== -1 && violation !== -1, `both findings printed: ${lines.join(' | ')}`);
    ok(stale < violation, 'the exit-3 finding precedes the exit-1 finding');
  });
  check('cli_unknown_flag', () => {
    equal(run(['--snapshot=x']).result.code, 2);
  });
  check('cli_mode_ruling_without_src', () => {
    equal(run(['--ruling=x.md']).result.code, 2);
  });
  check('cli_mode_routes_without_src', () => {
    equal(run(['--routes=x.json']).result.code, 2);
  });
  check('cli_mode_probe_without_ruling', () => {
    equal(run(['--src=web-ui/src', '--probe=x.md']).result.code, 2);
  });
  check('cli_mode_src_alone', () => {
    equal(run(['--src=web-ui/src']).result.code, 2);
  });
  check('cli_absent_probe_silent', () => {
    const src = join(scratch, 'silent-src');
    mkdirSync(src, { recursive: true });
    writeFileSync(join(src, 'a.rs'), '<p>');
    const ruling = writeFile('silent-ruling.md', rule([['p', 'no-equivalent', '-', 'r', 'b']]));
    const { result, lines } = run([`--src=${src}`, `--ruling=${ruling}`]);
    equal(result.code, 0);
    ok(!lines.some(line => line.startsWith('SKIP')), `no SKIP line: ${lines.join(' | ')}`);
  });

  const controlSrc = () => {
    const src = join(scratch, 'control-src');
    mkdirSync(src, { recursive: true });
    writeFileSync(join(src, 'a.rs'), '<span>');
    return src;
  };
  const controlRoutes = () =>
    writeFile(
      'control-routes.json',
      JSON.stringify([{ route: '/', status: 'measured', authoredTags: [{ tag: 'span', inThawSubtree: true, count: 1 }], unclassedUnderThaw: [] }])
    );
  check('combined_policy1_route0_is_1', () => {
    const src = controlSrc();
    const ruling = writeFile('control-ruling.md', rule([]));
    const routes = controlRoutes();
    const combined = run([`--src=${src}`, `--ruling=${ruling}`, `--routes=${routes}`]);
    equal(combined.result.code, 1);
    ok(
      combined.lines.some(line => line.startsWith('EXIT1 span') && line.includes('unclassified')),
      `span unclassified line: ${combined.lines.join(' | ')}`
    );
    const routeOnly = run([`--src=${src}`, `--routes=${routes}`]);
    equal(routeOnly.result.code, 0);
  });
  check('policy_control_span_in_thaw', () => {
    const src = controlSrc();
    const ruling = writeFile('control-ruling-2.md', rule([]));
    const routes = controlRoutes();
    const combined = run([`--src=${src}`, `--ruling=${ruling}`, `--routes=${routes}`]);
    equal(combined.result.code, 1);
    ok(combined.result.findings.some(finding => finding.scope === 'span' && finding.where === 'unclassified'));
  });
  check('combined_policy3_route1_is_1', () => {
    const src = join(scratch, 'p3-src');
    mkdirSync(src, { recursive: true });
    writeFileSync(join(src, 'a.rs'), '<p>');
    const ruling = writeFile(
      'p3-ruling.md',
      rule([
        ['p', 'no-equivalent', '-', 'r', 'b'],
        ['marquee', 'no-equivalent', '-', 'r', 'b'],
      ])
    );
    const routes = writeFile(
      'p3-routes.json',
      JSON.stringify([{ route: '/', status: 'measured', authoredTags: [{ tag: 'svg', inThawSubtree: false, count: 1 }], unclassedUnderThaw: [] }])
    );
    const { result, lines } = run([`--src=${src}`, `--ruling=${ruling}`, `--routes=${routes}`]);
    equal(result.code, 1);
    ok(lines.some(line => line.startsWith('EXIT3 marquee') && line.includes('stale')), `literal stale line: ${lines.join(' | ')}`);
    ok(lines.some(line => line.startsWith('EXIT1 /') && line.includes('unrecognised authored tag (route)')), `literal route line: ${lines.join(' | ')}`);
  });
  check('combined_parse2_with_route1_is_2', () => {
    const src = join(scratch, 'p2-src');
    mkdirSync(src, { recursive: true });
    writeFileSync(join(src, 'a.rs'), '<td>');
    const ruling = writeFile('p2-ruling.md', rule([['td', 'undecided', '-', 'r', 'b']]));
    const routes = writeFile(
      'p2-routes.json',
      JSON.stringify([{ route: '/', status: 'measured', authoredTags: [{ tag: 'svg', inThawSubtree: false, count: 1 }], unclassedUnderThaw: [] }])
    );
    const { result, lines } = run([`--src=${src}`, `--ruling=${ruling}`, `--routes=${routes}`]);
    equal(result.code, 2);
    ok(lines.some(line => line.startsWith('EXIT2 ruling') && line.includes('class')), `literal class line: ${lines.join(' | ')}`);
    ok(lines.some(line => line.startsWith('EXIT1 /') && line.includes('unrecognised authored tag (route)')), `route still checked: ${lines.join(' | ')}`);
    ok(lines.some(line => line === 'SKIP checkPolicy ruling'), `skip line: ${lines.join(' | ')}`);
  });

  const failed = results.filter(([, passed]) => !passed);
  for (const [name, passed, message] of results) {
    if (!passed) process.stderr.write(`FAIL ${name}: ${message}\n`);
  }
  process.stdout.write(`SELFTEST ${results.length - failed.length}/${results.length}\n`);
  return failed.length === 0 ? 0 : 1;
}

// ---------------------------------------------------------------------------
// Entry
// ---------------------------------------------------------------------------

if (process.argv[1] && resolve(process.argv[1]) === resolve(fileURLToPath(import.meta.url))) {
  if (process.argv.slice(2).includes('--self-test')) {
    process.exitCode = selfTest();
  } else {
    process.exitCode = main(process.argv.slice(2)).code;
  }
}

export {
  attrsOf,
  blankSource,
  checkPolicy,
  checkProbe,
  checkRoutes,
  checkRoutes as runRouteGate,
  currentHead,
  main,
  parseRuling,
  scanSource,
  scanSourceDir,
  selfTest,
};
