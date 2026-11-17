'use strict';
// Issue 118 wave 5 browser matrix (Step 9).
//
// Five cases over a synthetic REST fixture intercepted before the wasm boots,
// so no engine and no store are involved. The readiness rule for a pre/post
// matrix: every case waits only on selectors that exist in BOTH bundles
// (`memory-content`, `memory-tags`, `export-download:not([disabled])`,
// `agents-project-select`) and never on a new testid — the pre run fails on an
// assertion that names the defect, never on a timeout.
//
// The synthetic `/api/memories` store deliberately leaks ONE prompt row into the
// walk: 6 227 exportable memories + 1 prompt = 6 228 fetched rows while the
// server reports `total: 6 227`, the shape a live server cannot produce (it
// filters prompts before `timeline.len()`, crates/mem-web/src/api/memories.rs).
// That leak is the only way a browser can observe fetched ≠ exportable, so the
// preview==blob equality kills a view that feeds the preview `all.len()`.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');

const base = process.env.TEST_BASE_URL || 'http://127.0.0.1:49431';
const selection = process.env.W5_TEST || 'all';
const dist = process.env.WEBUI_DIST;
const output = process.env.W5_OUTPUT_DIR ||
  path.join('.tmp/artifacts/issue-118-wave5', new Date().toISOString().replace(/[:.]/g, '-'));
const sha = bytes => crypto.createHash('sha256').update(bytes).digest('hex');
const q = id => `[data-testid="${id}"]`;
const results = [];
let call, evaluate, initId;

const sleep = ms => evaluate(`new Promise(r=>setTimeout(r,${ms}))`);

// The one readiness helper: a selector that exists in both bundles, plus the
// house "no spinner / no -loading block" quiet window (three clean samples).
async function ready(selector) {
  return evaluate(`new Promise((resolve,reject)=>{let clean=0;const start=Date.now();const timer=setInterval(()=>{const ok=!!document.querySelector(${JSON.stringify(selector)})&&!document.querySelector('.thaw-spinner,[data-testid$="-loading"]');clean=ok?clean+1:0;if(clean===3){clearInterval(timer);resolve(true)}else if(Date.now()-start>20000){clearInterval(timer);reject(Error('readiness timeout '+${JSON.stringify(selector)}))}},100)})`);
}

async function waitFor(expression, ms = 8000) {
  const deadline = Date.now() + ms;
  while (Date.now() < deadline) {
    if (await evaluate(expression)) return true;
    await sleep(50);
  }
  return false;
}

async function click(selector) {
  await evaluate(`(()=>{const e=document.querySelector(${JSON.stringify(selector)});if(!e)throw Error('missing '+${JSON.stringify(selector)});e.click()})()`);
}

async function text(selector) {
  return evaluate(`(()=>{const e=document.querySelector(${JSON.stringify(selector)});return e?e.textContent:null})()`);
}

// The download path builds a Blob and hands it to URL.createObjectURL; capture
// the Blob itself (installed with the fixture, before the app boots) and read
// the exact document the user would receive.
async function downloadedDocument() {
  return evaluate(`(async()=>{ if(!window.__lastBlob) return null; const text = await window.__lastBlob.text(); return JSON.parse(text); })()`);
}

// The accessible name as the browser computes it (not as an attribute guess).
async function accessibleName(selector) {
  const { result } = await call('Runtime.evaluate', {
    expression: `document.querySelector(${JSON.stringify(selector)})`,
  });
  if (!result || !result.objectId) return null;
  const { nodes } = await call('Accessibility.getPartialAXTree', {
    objectId: result.objectId,
    fetchRelatives: false,
  });
  const named = nodes.find(node => node.name && node.name.value);
  if (!named) return null;
  return { role: named.role ? named.role.value : null, name: named.name.value };
}

// The synthetic store, installed before every navigation. `scenario` selects
// which store answers `/api/memories`:
//   export       6 227 memories + 1 leaked prompt row, totalPages 32
//   export-short the same total, but page 3 answers 500
//   cards        two rows: a 1 550-char body and a five-tag row
//   agents       one project + its milestone (the roster drill-down)
function install(scenario) {
  return `(()=>{
    const MEMORY_TOTAL = 6227;
    const FULL_BODY = 'alpha bravo charlie delta echo '.repeat(50);
    const rowAt = (index, leak) => (leak && index === MEMORY_TOTAL)
      ? {type: 'prompt', id: 'prompt-leak', content: 'synthetic leaked prompt row',
         tags: [], createdAt: '1794000000000', isPinned: false, sessionId: 'fixture-session'}
      : {type: 'memory', id: 'mem-' + index, content: 'synthetic memory ' + index,
         memoryType: 'note', tags: [], createdAt: '1794000000000', isPinned: false};
    window.__scenario = ${JSON.stringify(scenario)};
    window.__requests = [];
    window.__lastBlob = null;
    const originalFetch = window.fetch.bind(window);
    const originalCreate = URL.createObjectURL.bind(URL);
    URL.createObjectURL = blob => { window.__lastBlob = blob; return originalCreate(blob); };
    const envelope = (data, status = 200) => new Response(
      JSON.stringify(status === 200 ? {success: true, data} : data),
      {status, headers: {'Content-Type': 'application/json'}},
    );
    window.fetch = async (input, options = {}) => {
      const url = new URL(typeof input === 'string' ? input : input.url, location.href);
      if (!url.pathname.startsWith('/api/')) return originalFetch(input, options);
      window.__requests.push({
        path: url.pathname,
        url: url.pathname + url.search,
        method: options.method || 'GET',
      });
      const scenario = window.__scenario;
      if (url.pathname === '/api/memories') {
        if (scenario === 'cards') {
          return envelope({items: [
            {type: 'memory', id: 'mem-long', content: FULL_BODY, memoryType: 'note',
             tags: ['longbody'], createdAt: '1794000000000', isPinned: false},
            {type: 'memory', id: 'mem-tags', content: 'five tag row', memoryType: 'note',
             tags: ['first', 'second', 'third', 'fourth', 'fifth'],
             createdAt: '1794000000000', isPinned: false},
          ], total: 2, page: 1, pageSize: 200, totalPages: 1});
        }
        const page = Number(url.searchParams.get('page') || '1');
        const size = Math.min(Number(url.searchParams.get('pageSize') || '20'), 200);
        if (scenario === 'export-short' && page >= 3) {
          return new Response(JSON.stringify({success: false, error: 'synthetic page failure'}),
            {status: 500, headers: {'Content-Type': 'application/json'}});
        }
        const leak = scenario === 'export';
        const fetched = MEMORY_TOTAL + (leak ? 1 : 0);
        const items = [];
        for (let index = (page - 1) * size; index < Math.min(page * size, fetched); index += 1) {
          items.push(rowAt(index, leak));
        }
        return envelope({
          items, total: MEMORY_TOTAL, page, pageSize: size,
          totalPages: Math.ceil(MEMORY_TOTAL / size),
        });
      }
      if (url.pathname === '/api/tags') {
        return envelope({project: [], user: [], memoryTags: [], memoryTagTotal: 0});
      }
      if (url.pathname === '/api/user-profile') {
        return envelope({exists: false, userId: 'fixture-user', message: 'No profile found yet.'});
      }
      if (url.pathname === '/api/projects') {
        return envelope([{projectId: 'fixture-project', sessionCount: 1, milestoneCount: 1,
          lastActivityAt: 1730462400000}]);
      }
      if (url.pathname.endsWith('/milestones')) {
        return envelope([{id: 'fixture-ms-1', projectId: 'fixture-project', agentId: 'fixture-agent',
          summary: 'Synthetic milestone summary', sourceCount: 1,
          windowStart: 1730462400000, windowEnd: 1730462400000, createdAt: 1730462400000,
          promotedFacts: [],
          contributors: [{sessionRowId: 'fixture-session-row-1', sessionId: 'fixture-session-1',
            parentSessionId: null, agentId: 'fixture-agent', agentName: 'Fixture Agent',
            createdAt: 1730462400000, summary: 'Synthetic session summary'}]}]);
      }
      return new Response(JSON.stringify({success: false, error: 'Unknown fixture path'}),
        {status: 599, headers: {'Content-Type': 'application/json'}});
    };
  })();`;
}

async function navigate(route, scenario) {
  if (initId) await call('Page.removeScriptToEvaluateOnNewDocument', {identifier: initId});
  ({identifier: initId} = await call('Page.addScriptToEvaluateOnNewDocument', {
    source: install(scenario),
  }));
  await call('Page.navigate', {url: base + route});
  await evaluate(`new Promise(r=>{if(document.readyState==='complete')r();else addEventListener('load',r,{once:true})})`);
}

const EXPORT_READY = `${q('export-download')}:not([disabled])`;
const CARD_READY = q('memory-content');
const AGENTS_READY = q('agents-project-select');
const walkRequests = `window.__requests.filter(r => r.path === '/api/memories').length`;
const longRow = `[...document.querySelectorAll('[data-testid="memory-row"]')]
  .find(li => (li.getAttribute('data-tags') || '').includes('longbody'))`;
const tagRow = `[...document.querySelectorAll('[data-testid="memory-row"]')]
  .find(li => (li.getAttribute('data-tags') || '').includes('fifth'))`;

const cases = {
  async export_walks_to_store_total() {
    await navigate('/export-import', 'export');
    await ready(EXPORT_READY);
    const requests = await evaluate(walkRequests);
    const preview = await text(q('export-preview'));
    if (requests !== 32 || preview !== 'memories: 6227') {
      throw Error(`the walk issued ${requests} requests and the preview reads ${JSON.stringify(preview)}; expected 32 requests and "memories: 6227"`);
    }
    await click(q('export-download'));
    const doc = await downloadedDocument();
    assert.ok(doc, 'the download produced a document');
    const rows = doc.memories.length;
    assert.equal(rows, 6227, `the blob holds ${rows} rows, expected 6 227`);
    // The equality is where a view feeding the preview `all.len()` dies: fetched
    // is 6 228, exportable is 6 227, and only the browser can see the call site.
    assert.equal(preview, `memories: ${rows}`, `preview ${JSON.stringify(preview)} != blob row count ${rows}`);
    assert.equal(doc.truncated, false, 'a complete walk is not truncated');
    return {requests, preview, rows, truncated: doc.truncated};
  },

  async export_discloses_a_short_walk() {
    await navigate('/export-import', 'export-short');
    await ready(EXPORT_READY);
    const requests = await evaluate(walkRequests);
    const preview = await text(q('export-preview'));
    await click(q('export-download'));
    const doc = await downloadedDocument();
    assert.ok(doc, 'the download produced a document');
    if (preview !== 'memories: 400 of 6227 — export limited' || doc.truncated !== true) {
      throw Error(`page 3 failed after ${requests} requests: preview reads ${JSON.stringify(preview)} and blob truncated=${JSON.stringify(doc.truncated)}; expected "memories: 400 of 6227 — export limited" and truncated=true`);
    }
    assert.equal(doc.memories.length, 400, 'the file holds the 400 rows the walk reached');
    return {requests, preview, rows: doc.memories.length, truncated: doc.truncated};
  },

  async memory_card_toggle_after_content() {
    await navigate('/project-memories', 'cards');
    await ready(CARD_READY);
    const fullBody = 'alpha bravo charlie delta echo '.repeat(50);
    const before = await evaluate(`(() => {
      const row = ${longRow};
      if (!row) return null;
      const content = row.querySelector('[data-testid="memory-content"]');
      const toggle = row.querySelector('[data-testid="memory-toggle"]');
      const style = getComputedStyle(content);
      return {
        previewChars: content.textContent.length,
        next: content.nextElementSibling ? content.nextElementSibling.tagName : null,
        toggles: row.querySelectorAll('[data-testid="memory-toggle"]').length,
        nextIsToggle: toggle ? content.nextElementSibling === toggle : false,
        containsToggle: toggle ? content.contains(toggle) : false,
        expanded: toggle ? toggle.getAttribute('aria-expanded') : null,
        label: toggle ? toggle.textContent.trim() : null,
        webkitLineClamp: style.webkitLineClamp,
        display: style.display,
        fits: content.scrollHeight <= content.clientHeight + 1,
      };
    })()`);
    if (!before) throw Error('FIXTURE: the long-body row never rendered');
    if (before.toggles !== 1) {
      throw Error(`the expanded card carries ${before.toggles} toggle(s) and the content's next element sibling is a ${before.next}; expected one toggle after the content`);
    }
    if (!before.nextIsToggle) throw Error(`the toggle is not the content's next element sibling (next is ${before.next})`);
    if (before.containsToggle) throw Error('the toggle sits INSIDE the content');
    if (before.expanded !== 'false' || before.label !== 'Show more') {
      throw Error(`collapsed toggle reads aria-expanded=${JSON.stringify(before.expanded)} text=${JSON.stringify(before.label)}`);
    }
    if (before.webkitLineClamp !== 'none') {
      throw Error(`webkitLineClamp is ${JSON.stringify(before.webkitLineClamp)}, expected "none" (clamp still applied)`);
    }
    if (before.display === '-webkit-box') {
      throw Error('content computes display: -webkit-box (clamp still applied)');
    }
    // The clamp-absent fallback when the CSSOM property is unavailable.
    if (!before.fits) {
      throw Error(`content scrollHeight ${'>'} clientHeight + 1: the box is still clamped`);
    }
    await click(q('memory-toggle'));
    const opened = await waitFor(`(() => {
      const row = ${longRow};
      const content = row.querySelector('[data-testid="memory-content"]');
      const toggle = row.querySelector('[data-testid="memory-toggle"]');
      return content.textContent.length === fullBodyLength && toggle.getAttribute('aria-expanded') === 'true';
    })()`.replace('fullBodyLength', String(fullBody.length)), 3000);
    if (!opened) throw Error('expanding the card never put the whole body in the one content div');
    const expandedText = await evaluate(`(() => {
      const row = ${longRow};
      const content = row.querySelector('[data-testid="memory-content"]');
      const toggle = row.querySelector('[data-testid="memory-toggle"]');
      return {length: content.textContent.length, nextIsToggle: content.nextElementSibling === toggle,
              label: toggle.textContent.trim()};
    })()`);
    if (expandedText.length !== fullBody.length) {
      throw Error(`expanded content is ${expandedText.length} chars, expected the whole ${fullBody.length}`);
    }
    if (!expandedText.nextIsToggle) throw Error('the toggle moved away from the content while expanded');
    if (expandedText.label !== 'Show less') throw Error(`expanded toggle reads ${JSON.stringify(expandedText.label)}`);
    await click(q('memory-toggle'));
    const collapsed = await waitFor(`(() => {
      const row = ${longRow};
      const toggle = row.querySelector('[data-testid="memory-toggle"]');
      return toggle.getAttribute('aria-expanded') === 'false';
    })()`, 3000);
    if (!collapsed) throw Error('the toggle never returned to aria-expanded="false"');
    const collapsedChars = await evaluate(`(${longRow}).querySelector('[data-testid="memory-content"]').textContent.length`);
    if (collapsedChars !== before.previewChars) {
      throw Error(`collapsing left ${collapsedChars} chars, expected the ${before.previewChars}-char preview`);
    }
    return {
      previewChars: before.previewChars, expandedChars: expandedText.length,
      webkitLineClamp: before.webkitLineClamp, display: before.display,
      ariaExpanded: ['false', 'true', 'false'],
    };
  },

  async memory_card_tags_reachable() {
    await navigate('/project-memories', 'cards');
    await ready(CARD_READY);
    const before = await evaluate(`(() => {
      const row = ${tagRow};
      if (!row) return null;
      const more = row.querySelector('[data-testid="memory-tag-more"]') ||
        [...row.querySelectorAll('[data-testid="memory-tags"] span')]
          .find(el => el.textContent.trim() === '+1 more') || null;
      return {
        pills: row.querySelectorAll('[data-testid="memory-tag-pill"]').length,
        tag: more ? more.tagName : null,
        expanded: more ? more.getAttribute('aria-expanded') : null,
        label: more ? more.getAttribute('aria-label') : null,
        text: more ? more.textContent.trim() : null,
      };
    })()`);
    if (!before) throw Error('FIXTURE: the five-tag row never rendered');
    if (before.tag !== 'BUTTON') {
      throw Error(`the +1 more control is a ${before.tag}, not a BUTTON (hidden tags are unreachable)`);
    }
    if (before.pills !== 4) throw Error(`${before.pills} visible pills before activation, expected 4`);
    if (before.expanded !== 'false') throw Error(`the +1 more control's aria-expanded is ${JSON.stringify(before.expanded)}`);
    if (before.label !== '1 more tags: fifth') throw Error(`the +1 more control's aria-label is ${JSON.stringify(before.label)}`);
    if (before.text !== '+1 more') throw Error(`the +1 more control's text is ${JSON.stringify(before.text)}`);
    await click(q('memory-tag-more'));
    const revealed = await waitFor(`(() => {
      const row = ${tagRow};
      const more = row.querySelectorAll('[data-testid="memory-tag-more"]').length;
      const pills = [...row.querySelectorAll('[data-testid="memory-tag-pill"]')];
      return (more === 0 && pills.length === 5) ? true : false;
    })()`, 3000);
    if (!revealed) throw Error('activating +1 more never revealed the fifth chip in place');
    const last = await evaluate(`(() => {
      const row = ${tagRow};
      const pills = [...row.querySelectorAll('[data-testid="memory-tag-pill"]')];
      return {length: pills.length, label: pills.at(-1).getAttribute('aria-label')};
    })()`);
    if (last.label !== 'Filter by tag: fifth') throw Error(`the revealed chip is ${JSON.stringify(last.label)}`);
    const beforeCalls = await evaluate(`window.__requests.filter(r => r.url.includes('tags=fifth')).length`);
    await evaluate(`(() => {
      const row = ${tagRow};
      const pills = [...row.querySelectorAll('[data-testid="memory-tag-pill"]')];
      pills.at(-1).click();
    })()`);
    const filtered = await waitFor(`window.__requests.filter(r => r.url.includes('tags=fifth')).length > ${beforeCalls}`, 5000);
    if (!filtered) throw Error('clicking the revealed chip issued no tags=fifth request');
    return {pills: [before.pills, last.length], last: last.label, filtered: true};
  },

  async agents_select_has_accessible_name() {
    await navigate('/agents', 'agents');
    await ready(AGENTS_READY);
    const seen = await evaluate(`(() => {
      const select = document.querySelector('[data-testid="agents-project-select"]');
      if (!select) return null;
      return {
        tag: select.tagName,
        labels: select.labels ? select.labels.length : 0,
        label: select.labels && select.labels[0] ? select.labels[0].textContent.trim() : null,
      };
    })()`);
    if (!seen) throw Error('FIXTURE: the project picker never rendered');
    if (seen.tag !== 'SELECT') throw Error(`the picker is a ${seen.tag}, not the native SELECT the live contract pins`);
    if (seen.labels !== 1 || seen.label !== 'Project') {
      throw Error(`the picker's labels.length === ${seen.labels} (label ${JSON.stringify(seen.label)}); expected one label reading "Project"`);
    }
    await click('label[for="agents-project-select"]');
    const focused = await evaluate(`document.activeElement === document.querySelector('[data-testid="agents-project-select"]')`);
    if (!focused) throw Error('clicking the label did not focus the select');
    const ax = await accessibleName(q('agents-project-select'));
    if (!ax || ax.name !== 'Project') {
      throw Error(`the accessibility tree names the picker ${JSON.stringify(ax ? ax.name : null)}; expected "Project"`);
    }
    return {tag: seen.tag, labels: seen.labels, label: seen.label, axRole: ax.role, axName: ax.name};
  },
};

(async () => {
  assert.ok(selection === 'all' || Object.hasOwn(cases, selection), 'unknown W5_TEST ' + selection);
  assert.ok(process.env.LANE_PAGE_ID, 'LANE_PAGE_ID must be the dedicated CDP target id');
  assert.ok(dist, 'WEBUI_DIST must point at the served dist (bundle hashes go into results.json)');
  const pages = await (await fetch('http://127.0.0.1:9222/json')).json();
  const page = pages.find(p => p.id === process.env.LANE_PAGE_ID && p.type === 'page');
  assert.ok(page, 'dedicated target id not found; MCP ordinal is not a CDP target id');
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { ws.onopen = resolve; ws.onerror = reject; });
  let serial = 0;
  const pending = new Map();
  ws.onmessage = e => {
    const m = JSON.parse(e.data);
    if (pending.has(m.id)) {
      const { resolve, reject } = pending.get(m.id);
      pending.delete(m.id);
      m.error ? reject(Error(m.error.message)) : resolve(m.result);
    }
  };
  call = (method, params = {}) => new Promise((resolve, reject) => {
    const id = ++serial;
    pending.set(id, {resolve, reject});
    ws.send(JSON.stringify({id, method, params}));
  });
  evaluate = async expression => {
    const r = await call('Runtime.evaluate', {expression, awaitPromise: true, returnByValue: true});
    if (r.exceptionDetails) throw Error(JSON.stringify(r.exceptionDetails));
    return r.result.value;
  };
  const bundle = {};
  for (const entry of fs.readdirSync(dist)) {
    const full = path.join(dist, entry);
    if (fs.statSync(full).isFile()) bundle[entry] = sha(fs.readFileSync(full));
  }
  fs.mkdirSync(output, {recursive: true});
  await call('Page.enable');
  await call('Runtime.enable');
  await call('Accessibility.enable').catch(() => {});
  try {
    for (const name of selection === 'all' ? Object.keys(cases) : [selection]) {
      try {
        const extra = await cases[name]();
        results.push({name, status: 'PASS', ...(extra || {})});
        console.log('PASS ' + name);
      } catch (e) {
        results.push({name, status: 'FAIL', error: e.stack});
        console.error('FAIL ' + name + ': ' + e.message);
      }
    }
  } finally {
    fs.writeFileSync(path.join(output, 'results.json'), JSON.stringify({
      utc: new Date().toISOString(),
      page: page.id,
      base,
      scenario: selection,
      scriptSha256: sha(fs.readFileSync(__filename)),
      bundle,
      results,
    }, null, 2) + '\n');
    if (initId) await call('Page.removeScriptToEvaluateOnNewDocument', {identifier: initId});
    ws.close();
  }
  if (results.some(r => r.status === 'FAIL')) process.exitCode = 1;
})().catch(e => { console.error(e); process.exitCode = 1; });
