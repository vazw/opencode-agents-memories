'use strict';
// Issue 139 browser runner.
//
// Two modes, one executable:
//   --probe --dist=<abs> --md=<path>    probe whether Thaw Table and ProgressBar
//                                       forward `attr:` to their root element;
//                                       writes the report at <path>.
//   --walk --dist=<abs> --out=<json>    walk the ten routes, record the authored
//                                       tag census per route into <json>.
//   --self-test                         run the fixtures, print `SELFTEST n/n`.
//
// The runner owns its child processes: the static server (web-ui/scripts/
// serve-dist.mjs on 127.0.0.1:8139) and Chromium (headless, CDP on
// 127.0.0.1:9222). Both ports are bind-probed and released before either child
// spawns, and both pids are killed in a `finally` on every path.
//
// The API is never reached. A fetch override installed before any page script
// answers every fixture `(method, template)` pair; a request the fixture does
// not answer is recorded as an api miss, and the route it hit is NOT MEASURED.

const assert = require('node:assert/strict');
const { execFileSync, spawn } = require('node:child_process');
const crypto = require('node:crypto');
const fs = require('node:fs');
const net = require('node:net');
const path = require('node:path');

const HERE = __dirname;
const REPO_ROOT = path.resolve(HERE, '..', '..');
const SRC_DIR = path.join(HERE, '..', 'src');
const FIXTURE_PATH = path.join(HERE, 'fixtures', 'issue-139-census.json');
const STATIC_PORT = 8139;
const CDP_PORT = 9222;
const ORIGIN = `http://127.0.0.1:${STATIC_PORT}`;
const TOKEN = 'synthetic-issue139-marker';
const SETTLE_TIMEOUT_MS = 5000;
const EXCLUDED = ['script', 'style', 'noscript', 'template', 'link', 'meta', 'title', 'head'];

// ---------------------------------------------------------------------------
// Pure slices (each has a fixture in selfTest())
// ---------------------------------------------------------------------------

/// The three verdicts, from the facts the probe evaluates in the page.
function probeVerdicts(facts) {
  const table = facts.table || {};
  const tableLanded =
    table.found === true &&
    table.tagName === 'TABLE' &&
    Array.isArray(table.classList) &&
    table.classList.includes('thaw-table') &&
    Number(table.cellCount) >= 1;
  const progress = facts.progress || {};
  const progressLanded = progress.found === true && progress.tagName === 'DIV' && progress.role === 'progressbar';
  const labelLanded = progressLanded && progress.ariaLabel === 'probe';
  return {
    table: tableLanded ? 'landed' : 'dropped',
    progress: progressLanded ? 'landed' : 'dropped',
    label: labelLanded ? 'landed' : 'dropped',
  };
}

function escapeRegExp(text) {
  return text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/// A path template with `{x}` segments becomes an anchored regex.
function templateToRegExp(template) {
  const parts = template.split(/\{[^}]+\}/).map(escapeRegExp);
  return new RegExp(`^${parts.join('[^/]+')}$`);
}

/// The fixture entry answering `(method, pathname)`, or null.
function matchApi(entries, method, pathname) {
  for (const entry of entries) {
    if (entry.method !== method) continue;
    if (templateToRegExp(entry.template).test(pathname)) return entry;
  }
  return null;
}

/// `(method, template)` pairs for the `*_PATHS` templates plus `/api/health`.
function loadMethodPairs(pathsText) {
  const endpoints = [];
  const table = pathsText.slice(pathsText.indexOf('pub const ENDPOINTS'));
  for (const chunk of table.split('EndpointSpec').slice(1)) {
    const method = /method:\s*"([A-Z]+)"/.exec(chunk);
    const template = /template:\s*"([^"]+)"/.exec(chunk);
    if (method && template) endpoints.push({ method: method[1], template: template[1] });
  }
  const templates = new Set(['/api/health']);
  for (const block of pathsText.matchAll(/const [A-Z_]+_PATHS: &\[&str\] = &\[([\s\S]*?)\];/g)) {
    for (const literal of block[1].matchAll(/"([^"]+)"/g)) templates.add(literal[1]);
  }
  const pairs = [];
  for (const endpoint of endpoints) {
    if (templates.has(endpoint.template)) pairs.push(endpoint);
  }
  return pairs;
}

/// The pairs the fixture does not answer.
function fixtureCovers(entries, pairs) {
  const have = new Set(entries.map(entry => `${entry.method} ${entry.template}`));
  return pairs.filter(pair => !have.has(`${pair.method} ${pair.template}`));
}

/// The fetch override, as the source `Page.addScriptToEvaluateOnNewDocument`
/// installs before any page script runs.
function installScript(entries) {
  return `(() => {
  const entries = ${JSON.stringify(entries)};
  const token = ${JSON.stringify(TOKEN)};
  window.__OPENCODE_MEM_TOKEN__ = token;
  window.__apiInflight = 0;
  window.__apiMisses = [];
  const original = window.fetch.bind(window);
  const escapeRegExp = text => text.replace(/[.*+?^\${}()|[\\]\\\\]/g, '\\\\$&');
  const toRegExp = template => new RegExp('^' + template.split(/\\{[^}]+\\}/).map(escapeRegExp).join('[^/]+') + '$');
  const match = (method, pathname) => {
    for (const entry of entries) {
      if (entry.method !== method) continue;
      if (toRegExp(entry.template).test(pathname)) return entry;
    }
    return null;
  };
  window.fetch = async (input, options = {}) => {
    const url = new URL(typeof input === 'string' ? input : input.url, location.href);
    if (!url.pathname.startsWith('/api/')) return original(input, options);
    const method = options.method || 'GET';
    window.__apiInflight += 1;
    try {
      const entry = match(method, url.pathname);
      if (!entry) {
        window.__apiMisses.push(method + ' ' + url.pathname);
        return new Response(JSON.stringify({ success: false }), {
          status: 404,
          headers: { 'Content-Type': 'application/json' },
        });
      }
      return new Response(JSON.stringify(entry.response), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      });
    } finally {
      window.__apiInflight -= 1;
    }
  };
})();`;
}

/// The per-element walk, as injected. Emits one flat entry per element under
/// `document.body`; classification happens in Node.
function walkScript() {
  return `(() => {
  const excluded = new Set(${JSON.stringify(EXCLUDED)});
  const entries = [];
  const visit = (element, ancestorThawOwned, parentClass) => {
    const classes = Array.from(element.classList);
    const thawOwned = classes.some(token => token.startsWith('thaw-'));
    if (!excluded.has(element.localName)) {
      entries.push({
        localName: element.localName,
        classes,
        thawOwned,
        ancestorThawOwned,
        parentClass,
      });
    }
    for (const child of element.children) visit(child, ancestorThawOwned || thawOwned, element.className || '');
  };
  for (const child of document.body.children) visit(child, false, '');
  return {
    entries,
    bodyChildCount: document.body.children.length,
    loadingMarkers: !!document.querySelector('.thaw-spinner,[data-testid$="-loading"]'),
    plannedNotice: !!document.querySelector('[data-testid="planned-notice"]'),
    apiMisses: window.__apiMisses || [],
  };
})()`;
}

/// Classify the flat walk output: Thaw-owned elements are not authored; every
/// other element is, once, under its `localName` and `inThawSubtree` flag.
function classifyWalk(entries) {
  const authored = new Map();
  const unclassedUnderThaw = [];
  for (const entry of entries) {
    if (entry.thawOwned) continue;
    const key = `${entry.localName}\u0000${entry.ancestorThawOwned}`;
    const row = authored.get(key) || {
      tag: entry.localName,
      inThawSubtree: entry.ancestorThawOwned,
      count: 0,
    };
    row.count += 1;
    authored.set(key, row);
    if (entry.ancestorThawOwned) {
      unclassedUnderThaw.push({ localName: entry.localName, parentClass: entry.parentClass });
    }
  }
  const authoredTags = [...authored.values()].sort((a, b) =>
    a.tag === b.tag ? Number(a.inThawSubtree) - Number(b.inThawSubtree) : a.tag.localeCompare(b.tag)
  );
  return { authoredTags, unclassedUnderThaw };
}

/// One route's status, from what the settle loop observed.
function routeOutcome(observed) {
  if (observed.loadingMarkers) return 'NOT MEASURED — still loading';
  if (observed.apiMisses.length > 0) return `NOT MEASURED — api miss: ${observed.apiMisses.join(', ')}`;
  if (observed.settled !== true) return 'NOT MEASURED — did not settle';
  if (observed.plannedNotice) return 'NOT MEASURED — planned notice';
  if (observed.bodyChildCount === 0) return 'NOT MEASURED — no body child';
  return 'measured';
}

// ---------------------------------------------------------------------------
// Filesystem + process helpers
// ---------------------------------------------------------------------------

function currentHead() {
  return execFileSync('git', ['rev-parse', '--short', 'HEAD'], { cwd: REPO_ROOT, encoding: 'utf8' }).trim();
}

function bundleSha(distDir) {
  const files = [];
  const walkDir = dir => {
    for (const name of fs.readdirSync(dir)) {
      const full = path.join(dir, name);
      if (fs.statSync(full).isDirectory()) walkDir(full);
      else if (name.endsWith('.wasm') || name.endsWith('.js')) files.push(full);
    }
  };
  walkDir(distDir);
  const lines = files
    .sort()
    .map(file => `${crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex')}  ${path.relative(distDir, file)}`);
  return crypto.createHash('sha256').update(lines.join('\n')).digest('hex').slice(0, 12);
}

function readRoutes(routesText) {
  const start = routesText.indexOf('pub const ROUTES');
  const block = start === -1 ? '' : routesText.slice(start);
  return [...block.matchAll(/path:\s*"([^"]+)"/g)].map(match => match[1]);
}

function bindProbe(port) {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.once('error', reject);
    server.listen(port, '127.0.0.1', () => {
      server.close(error => (error ? reject(error) : resolve()));
    });
  });
}

const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

function spawnChild(command, args, options = {}) {
  return spawn(command, args, { stdio: ['ignore', 'pipe', 'pipe'], ...options });
}

async function waitForCdp(timeoutMs = 20000) {
  const deadline = Date.now() + timeoutMs;
  let lastError = 'no attempt';
  while (Date.now() < deadline) {
    try {
      const response = await fetch(`http://127.0.0.1:${CDP_PORT}/json/version`);
      const body = await response.json();
      if (body.webSocketDebuggerUrl) return body.webSocketDebuggerUrl;
      lastError = 'no webSocketDebuggerUrl';
    } catch (error) {
      lastError = error.message;
    }
    await sleep(150);
  }
  throw new Error(`chromium CDP did not answer: ${lastError}`);
}

async function firstPageTarget() {
  const deadline = Date.now() + 10000;
  while (Date.now() < deadline) {
    const pages = await (await fetch(`http://127.0.0.1:${CDP_PORT}/json/list`)).json();
    const page = pages.find(candidate => candidate.type === 'page');
    if (page && page.webSocketDebuggerUrl) return page.webSocketDebuggerUrl;
    await sleep(150);
  }
  throw new Error('chromium exposes no page target');
}

function connectCdp(wsUrl) {
  return new Promise((resolve, reject) => {
    const socket = new WebSocket(wsUrl);
    const pending = new Map();
    let nextId = 0;
    socket.addEventListener('message', event => {
      const message = JSON.parse(event.data);
      const waiter = message.id ? pending.get(message.id) : undefined;
      if (!waiter) return;
      pending.delete(message.id);
      if (message.error) waiter.reject(new Error(message.error.message));
      else waiter.resolve(message.result);
    });
    socket.addEventListener('error', () => reject(new Error('CDP socket error')));
    socket.addEventListener('open', () => {
      resolve({
        send(method, params = {}) {
          return new Promise((resolveCall, rejectCall) => {
            const id = ++nextId;
            pending.set(id, { resolve: resolveCall, reject: rejectCall });
            socket.send(JSON.stringify({ id, method, params }));
          });
        },
        async evaluate(expression) {
          const result = await this.send('Runtime.evaluate', {
            expression,
            awaitPromise: true,
            returnByValue: true,
          });
          if (result.exceptionDetails) {
            const description = result.exceptionDetails.exception?.description || result.exceptionDetails.text;
            throw new Error(`evaluate threw: ${description}`);
          }
          return result.result.value;
        },
        close() {
          try {
            socket.close();
          } catch {
            /* already closed */
          }
        },
      });
    });
  });
}

async function waitForLoad(cdp) {
  const deadline = Date.now() + 20000;
  while (Date.now() < deadline) {
    if ((await cdp.evaluate('document.readyState')) === 'complete') return;
    await sleep(100);
  }
  throw new Error('document did not reach readyState complete');
}

function killChild(child) {
  if (!child || child.killed || child.exitCode !== null) return;
  try {
    child.kill('SIGKILL');
  } catch {
    /* already gone */
  }
}

function writeHarnessReport(mdPath, stage, reason) {
  fs.mkdirSync(path.dirname(mdPath), { recursive: true });
  fs.writeFileSync(
    mdPath,
    [
      '# Probe — Thaw Table and ProgressBar attribute forwarding',
      '',
      `probe-harness: FAILED at ${stage}: ${reason}`,
      '',
    ].join('\n')
  );
}

function writeWalkReport(stage, reason) {
  const reportPath = path.join(REPO_ROOT, '.tmp', 'scratch', 'issue139', 'walk-report.md');
  fs.mkdirSync(path.dirname(reportPath), { recursive: true });
  fs.writeFileSync(reportPath, ['# Issue 139 walk harness', '', `walk-harness: FAILED at ${stage}: ${reason}`, ''].join('\n'));
  return reportPath;
}

/// The shared spawn/finally frame both modes use.
async function withBrowser(distDir, body) {
  for (const port of [STATIC_PORT, CDP_PORT]) {
    try {
      await bindProbe(port);
    } catch (error) {
      return { failure: { stage: 'port', reason: `port ${port}: ${error.message}` } };
    }
  }
  const profileDir = path.join(REPO_ROOT, '.tmp', 'scratch', 'issue139', 'chrome-profile');
  fs.mkdirSync(profileDir, { recursive: true });
  const serve = spawnChild(process.execPath, [path.join(HERE, 'serve-dist.mjs'), '--root', distDir, '--port', String(STATIC_PORT)]);
  const chromium = spawnChild('/usr/bin/chromium', [
    '--headless=new',
    '--remote-debugging-address=127.0.0.1',
    `--remote-debugging-port=${CDP_PORT}`,
    `--user-data-dir=${profileDir}`,
    '--no-first-run',
    '--disable-gpu',
    'about:blank',
  ]);
  let cdp = null;
  try {
    let wsUrl;
    try {
      // /json/version is the readiness probe; the CDP session itself attaches to
      // the page target, which is the only one that answers Page.* commands.
      await waitForCdp();
      wsUrl = await firstPageTarget();
    } catch (error) {
      return { failure: { stage: 'chromium', reason: error.message } };
    }
    try {
      cdp = await connectCdp(wsUrl);
    } catch (error) {
      return { failure: { stage: 'chromium', reason: error.message } };
    }
    return await body(cdp);
  } finally {
    if (cdp) cdp.close();
    killChild(serve);
    killChild(chromium);
    await sleep(100);
  }
}

// ---------------------------------------------------------------------------
// Probe mode
// ---------------------------------------------------------------------------

async function runProbe(distDir, mdPath) {
  const head = currentHead();
  const outcome = await withBrowser(distDir, async cdp => {
    await cdp.send('Page.enable');
    await cdp.send('Runtime.enable');
    try {
      await cdp.send('Page.navigate', { url: `${ORIGIN}/` });
      await waitForLoad(cdp);
      const deadline = Date.now() + 10000;
      while (Date.now() < deadline) {
        const present = await cdp.evaluate(
          `!!document.querySelector('[data-testid="probe-table"],[data-testid="probe-progress"]')`
        );
        if (present) break;
        await sleep(150);
      }
      await sleep(250);
      const facts = await cdp.evaluate(`(() => {
        const table = document.querySelector('[data-testid="probe-table"]');
        const progress = document.querySelector('[data-testid="probe-progress"]');
        return {
          table: {
            found: !!table,
            tagName: table ? table.tagName : null,
            classList: table ? Array.from(table.classList) : [],
            cellCount: document.querySelectorAll('.thaw-table-cell').length,
          },
          progress: {
            found: !!progress,
            tagName: progress ? progress.tagName : null,
            role: progress ? progress.getAttribute('role') : null,
            ariaLabel: progress ? progress.getAttribute('aria-label') : null,
          },
        };
      })()`);
      return { facts };
    } catch (error) {
      return { failure: { stage: 'evaluate', reason: error.message } };
    }
  });

  if (outcome.failure) {
    writeHarnessReport(mdPath, outcome.failure.stage, outcome.failure.reason);
    return 2;
  }
  const verdicts = probeVerdicts(outcome.facts);
  const report = [
    '# Probe — Thaw Table and ProgressBar attribute forwarding',
    '',
    `HEAD: ${head}`,
    `bundle sha256: ${bundleSha(distDir)}`,
    '',
    '## Raw evaluate output',
    '',
    '```json',
    JSON.stringify(outcome.facts, null, 2),
    '```',
    '',
    '## Verdicts',
    '',
    `probe-table: ${verdicts.table}`,
    `probe-progress: ${verdicts.progress}`,
    `probe-progress-label: ${verdicts.label}`,
    '',
  ].join('\n');
  fs.mkdirSync(path.dirname(mdPath), { recursive: true });
  fs.writeFileSync(mdPath, report);
  process.stdout.write(`${report}\n`);
  return 0;
}

// ---------------------------------------------------------------------------
// Walk mode
// ---------------------------------------------------------------------------

async function runWalk(distDir, outPath) {
  const entries = JSON.parse(fs.readFileSync(FIXTURE_PATH, 'utf8'));
  const pairs = loadMethodPairs(fs.readFileSync(path.join(SRC_DIR, 'paths.rs'), 'utf8'));
  const missing = fixtureCovers(entries, pairs);
  if (missing.length > 0) {
    process.stderr.write(`fixture incomplete: ${missing.map(pair => `${pair.method} ${pair.template}`).join(', ')}\n`);
    return 2;
  }
  const routes = readRoutes(fs.readFileSync(path.join(SRC_DIR, 'routes.rs'), 'utf8'));
  const script = installScript(entries);

  const outcome = await withBrowser(distDir, async cdp => {
    await cdp.send('Page.enable');
    await cdp.send('Runtime.enable');
    const records = [];
    for (const route of routes) {
      let initId = null;
      try {
        const installed = await cdp.send('Page.addScriptToEvaluateOnNewDocument', { source: script });
        initId = installed.identifier;
        await cdp.send('Page.navigate', { url: `${ORIGIN}${route}` });
        await waitForLoad(cdp);
        const settled = await cdp.evaluate(`new Promise(resolve => {
          const started = Date.now();
          let previous = null;
          const signature = () => {
            const entries = [];
            const visit = el => {
              entries.push(el.localName + ':' + (el.classList && el.classList.length ? Array.from(el.classList).some(t => t.startsWith('thaw-')) : false));
              for (const child of el.children) visit(child);
            };
            for (const child of document.body.children) visit(child);
            return entries.join('|');
          };
          const tick = () => {
            const loading = !!document.querySelector('.thaw-spinner,[data-testid$="-loading"]');
            const inflight = (window.__apiInflight || 0) > 0;
            const signatureNow = signature();
            if (!loading && !inflight) {
              if (signatureNow === previous) return resolve({ settled: true });
              previous = signatureNow;
            } else {
              previous = null;
            }
            if (Date.now() - started > ${SETTLE_TIMEOUT_MS}) {
              return resolve({ settled: false, loading, inflight, bodyChildCount: document.body.children.length });
            }
            setTimeout(tick, 250);
          };
          tick();
        })`);
        const observed = await cdp.evaluate(walkScript());
        const status = routeOutcome({
          settled: settled.settled,
          loadingMarkers: settled.loading === true || observed.loadingMarkers,
          plannedNotice: observed.plannedNotice,
          bodyChildCount: observed.bodyChildCount,
          apiMisses: observed.apiMisses,
        });
        const classified = status === 'measured' ? classifyWalk(observed.entries) : { authoredTags: [], unclassedUnderThaw: [] };
        records.push({
          route,
          status,
          settled: settled.settled === true,
          authoredTags: classified.authoredTags,
          unclassedUnderThaw: classified.unclassedUnderThaw,
          apiMisses: observed.apiMisses,
        });
        process.stderr.write(`${route} ${status} tags=${classified.authoredTags.length}\n`);
      } finally {
        if (initId) await cdp.send('Page.removeScriptToEvaluateOnNewDocument', { identifier: initId }).catch(() => {});
      }
    }
    return { records };
  });

  if (outcome.failure) {
    const reportPath = writeWalkReport(outcome.failure.stage, outcome.failure.reason);
    process.stderr.write(`walk harness failed: ${reportPath}\n`);
    return 2;
  }
  fs.mkdirSync(path.dirname(outPath), { recursive: true });
  fs.writeFileSync(outPath, `${JSON.stringify(outcome.records, null, 2)}\n`);
  process.stdout.write(`walked ${outcome.records.length} routes → ${outPath}\n`);
  return 0;
}

// ---------------------------------------------------------------------------
// Self-test
// ---------------------------------------------------------------------------

async function selfTest() {
  const results = [];
  const check = async (name, body) => {
    try {
      await body();
      results.push([name, true, '']);
    } catch (error) {
      results.push([name, false, error.message]);
    }
  };

  const tableFacts = overrides => ({
    table: { found: true, tagName: 'TABLE', classList: ['thaw-table'], cellCount: 1, ...overrides },
    progress: { found: true, tagName: 'DIV', role: 'progressbar', ariaLabel: 'probe' },
  });

  await check('probe_table_landed', async () => {
    assert.deepEqual(probeVerdicts(tableFacts()), { table: 'landed', progress: 'landed', label: 'landed' });
  });
  await check('probe_table_dropped_without_cells', async () => {
    assert.equal(probeVerdicts(tableFacts({ cellCount: 0 })).table, 'dropped');
  });
  await check('probe_table_dropped_wrong_tag', async () => {
    assert.equal(probeVerdicts(tableFacts({ tagName: 'DIV' })).table, 'dropped');
    assert.equal(probeVerdicts(tableFacts({ classList: ['not-thaw-table'] })).table, 'dropped');
    assert.equal(probeVerdicts(tableFacts({ found: false })).table, 'dropped');
  });
  await check('probe_progress_needs_testid', async () => {
    const verdicts = probeVerdicts({
      table: tableFacts().table,
      progress: { found: false, tagName: null, role: null, ariaLabel: null },
    });
    assert.equal(verdicts.progress, 'dropped');
    assert.equal(verdicts.label, 'dropped');
  });
  await check('probe_label_needs_landed_progress', async () => {
    const other = probeVerdicts({
      table: tableFacts().table,
      progress: { found: true, tagName: 'DIV', role: 'progressbar', ariaLabel: 'other' },
    });
    assert.equal(other.progress, 'landed');
    assert.equal(other.label, 'dropped');
    const unlanded = probeVerdicts({
      table: tableFacts().table,
      progress: { found: true, tagName: 'SPAN', role: 'progressbar', ariaLabel: 'probe' },
    });
    assert.equal(unlanded.progress, 'dropped');
    assert.equal(unlanded.label, 'dropped');
  });
  await check('probe_port_released_before_spawn', async () => {
    for (const port of [STATIC_PORT, CDP_PORT]) {
      await bindProbe(port);
      await bindProbe(port);
    }
  });
  await check('template_match_sessions', async () => {
    assert.ok(templateToRegExp('/api/projects/{project_id}/sessions').test('/api/projects/abc/sessions'));
    assert.ok(!templateToRegExp('/api/projects/{project_id}/sessions').test('/api/projects/abc'));
    assert.ok(!templateToRegExp('/api/projects/{project_id}/sessions').test('/api/projects/abc/sessions/xyz'));
  });
  await check('authored_span_under_thaw_card', async () => {
    const { authoredTags } = classifyWalk([
      { localName: 'div', classes: ['thaw-card'], thawOwned: true, ancestorThawOwned: false, parentClass: '' },
      { localName: 'span', classes: [], thawOwned: false, ancestorThawOwned: true, parentClass: 'thaw-card' },
    ]);
    assert.deepEqual(authoredTags, [{ tag: 'span', inThawSubtree: true, count: 1 }]);
  });
  await check('thaw_internal_excluded', async () => {
    const { authoredTags } = classifyWalk([
      {
        localName: 'button',
        classes: ['thaw-table-header-cell__button'],
        thawOwned: true,
        ancestorThawOwned: false,
        parentClass: 'thaw-table-header-cell',
      },
    ]);
    assert.deepEqual(authoredTags, []);
  });
  await check('thaw_card_span_counted_once', async () => {
    const result = classifyWalk([
      { localName: 'div', classes: ['thaw-card'], thawOwned: true, ancestorThawOwned: false, parentClass: '' },
      { localName: 'span', classes: [], thawOwned: false, ancestorThawOwned: true, parentClass: 'thaw-card' },
    ]);
    assert.deepEqual(result.authoredTags, [{ tag: 'span', inThawSubtree: true, count: 1 }]);
    assert.deepEqual(result.unclassedUnderThaw, [{ localName: 'span', parentClass: 'thaw-card' }]);
  });
  await check('route_not_measured_on_api_miss', async () => {
    const status = routeOutcome({
      settled: true,
      loadingMarkers: false,
      plannedNotice: false,
      bodyChildCount: 1,
      apiMisses: ['GET /api/x'],
    });
    assert.ok(status.startsWith('NOT MEASURED'));
  });
  await check('route_loading_not_measured', async () => {
    assert.equal(
      routeOutcome({ settled: false, loadingMarkers: true, plannedNotice: false, bodyChildCount: 1, apiMisses: [] }),
      'NOT MEASURED — still loading'
    );
    assert.equal(
      routeOutcome({ settled: true, loadingMarkers: true, plannedNotice: false, bodyChildCount: 1, apiMisses: [] }),
      'NOT MEASURED — still loading'
    );
  });
  const sandboxFixture = [
    { method: 'GET', template: '/api/stats', response: { success: true, data: { total: 7 } } },
    { method: 'GET', template: '/api/projects/{project_id}/sessions', response: { success: true, data: [] } },
  ];
  const sandbox = () => {
    const calls = [];
    const window = {
      fetch: async input => {
        calls.push(String(input));
        return new Response('original', { status: 200 });
      },
    };
    new Function('window', 'location', 'Headers', 'Response', installScript(sandboxFixture))(
      window,
      { href: 'http://127.0.0.1:8139/' },
      Headers,
      Response
    );
    return { window, calls };
  };
  await check('token_header_set', async () => {
    const { window } = sandbox();
    assert.equal(window.__OPENCODE_MEM_TOKEN__, TOKEN);
  });
  await check('non_api_passthrough', async () => {
    const { window, calls } = sandbox();
    const original = await window.fetch('/app.wasm');
    assert.equal(await original.text(), 'original');
    assert.deepEqual(calls, ['/app.wasm']);
    assert.equal(window.__apiInflight, 0);
    const stats = await window.fetch('/api/stats');
    assert.deepEqual(await stats.json(), { success: true, data: { total: 7 } });
    assert.deepEqual(window.__apiMisses, []);
    const sessions = await window.fetch('/api/projects/abc/sessions');
    assert.deepEqual(await sessions.json(), { success: true, data: [] });
    assert.equal(window.__apiInflight, 0);
    const miss = await window.fetch('/api/nope');
    assert.equal(miss.status, 404);
    assert.deepEqual(window.__apiMisses, ['GET /api/nope']);
  });
  await check('fixture_covers_templates', async () => {
    const entries = JSON.parse(fs.readFileSync(FIXTURE_PATH, 'utf8'));
    const pairs = loadMethodPairs(fs.readFileSync(path.join(SRC_DIR, 'paths.rs'), 'utf8'));
    assert.deepEqual(fixtureCovers(entries, pairs), []);
    const trimTo = pairs.slice(0, -1);
    assert.deepEqual(fixtureCovers(entries, trimTo), []);
    const missingOne = fixtureCovers(entries.slice(1), pairs);
    assert.equal(missingOne.length, 1);
    assert.deepEqual(missingOne[0], pairs.find(pair => !entries.slice(1).some(e => e.method === pair.method && e.template === pair.template)));
  });

  const failed = results.filter(([, ok]) => !ok);
  for (const [name, ok, message] of results) {
    if (!ok) process.stderr.write(`FAIL ${name}: ${message}\n`);
  }
  process.stdout.write(`SELFTEST ${results.length - failed.length}/${results.length}\n`);
  return failed.length === 0 ? 0 : 1;
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

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

async function main(argv) {
  const options = parseArgs(argv);
  const known = new Set(['self-test', 'probe', 'walk', 'dist', 'md', 'out', 'harness-failure']);
  for (const key of Object.keys(options)) {
    if (!known.has(key)) {
      process.stderr.write(`unknown argument: ${options.unknown || `--${key}`}\n`);
      return 2;
    }
  }
  if (options['self-test']) return selfTest();
  if (options['harness-failure']) {
    const [stage, ...rest] = String(options['harness-failure']).split(':');
    const mdPath = options.md || path.join(REPO_ROOT, 'docs', 'evidence', 'thaw-adoption-census', 'probe-table-progress.md');
    writeHarnessReport(mdPath, stage, rest.join(':').trim() || 'no reason given');
    return 2;
  }
  if (options.probe && options.walk) {
    process.stderr.write('--probe and --walk are exclusive\n');
    return 2;
  }
  if (options.probe) {
    if (!options.dist || !options.md) {
      process.stderr.write('--probe needs --dist=<abs> and --md=<path>\n');
      return 2;
    }
    return runProbe(options.dist, options.md);
  }
  if (options.walk) {
    if (!options.dist || !options.out) {
      process.stderr.write('--walk needs --dist=<abs> and --out=<json>\n');
      return 2;
    }
    return runWalk(options.dist, options.out);
  }
  process.stderr.write('usage: thaw-census-browser.cjs --self-test | --probe --dist=<abs> --md=<path> | --walk --dist=<abs> --out=<json>\n');
  return 2;
}

if (require.main === module) {
  main(process.argv.slice(2)).then(
    code => {
      process.exitCode = code;
    },
    error => {
      process.stderr.write(`${error && error.stack ? error.stack : String(error)}\n`);
      process.exitCode = 2;
    }
  );
}

module.exports = {
  probeVerdicts,
  templateToRegExp,
  matchApi,
  loadMethodPairs,
  fixtureCovers,
  classifyWalk,
  routeOutcome,
  installScript,
  walkScript,
  readRoutes,
};
