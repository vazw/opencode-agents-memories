// Readiness-gated route census + screenshots for the Issue 118 waves 2/3 evidence.
// Every count and screenshot is taken only after the route has been idle (no Thaw spinner and no
// `*-loading` testid) for 3 consecutive 100 ms samples; a busy capture fails the run.
// Wave 3 additions: the Memories search Field label association is a HARD check —
// the search input must have a non-empty id and a `label[for]` pointing at it
// (thaw Field pins `for` to its internal uuid; the Input's explicit id is
// dropped so the uuid becomes both). A failure stops the run.
// Run against a disposable engine serving this build and Chrome CDP on port 9222:
//   TEST_BASE_URL=http://127.0.0.1:49392 node web-ui/scripts/capture-evidence.cjs
const assert = require('node:assert/strict');
const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');
const base = process.env.TEST_BASE_URL || 'http://127.0.0.1:49392';
const outDir = process.env.EVIDENCE_DIR || path.join(__dirname, '..', '..', 'docs', 'evidence', 'webui-wave3-worst-screens');
const routes = [
  ['/', 'home'], ['/project-memories', 'project-memories'], ['/tags', 'tags'], ['/user-profile', 'user-profile'],
  ['/settings', 'settings'], ['/export-import', 'export-import'], ['/skill-library', 'skill-library'],
  ['/sessions', 'sessions'], ['/agents', 'agents'], ['/stats', 'stats']
];
const get = url => new Promise((resolve, reject) => http.get(url, r => { let body = ''; r.on('data', c => body += c); r.on('end', () => resolve(JSON.parse(body))); }).on('error', reject));
(async () => {
  const page = (await get('http://127.0.0.1:9222/json')).find(p => p.type === 'page' && p.url.startsWith(base));
  assert(page, 'open the lane on CDP first');
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { ws.onopen = resolve; ws.onerror = reject; });
  let id = 0; const pending = new Map();
  ws.onmessage = event => { const m = JSON.parse(event.data); if (!pending.has(m.id)) return; const [resolve, reject] = pending.get(m.id); pending.delete(m.id); m.error ? reject(Error(m.error.message)) : resolve(m.result); };
  const call = (method, params = {}) => new Promise((resolve, reject) => { const n = ++id; pending.set(n, [resolve, reject]); ws.send(JSON.stringify({id:n, method, params})); });
  const evaluate = async expression => { const r = await call('Runtime.evaluate', {expression, awaitPromise:true, returnByValue:true}); if (r.exceptionDetails) throw Error(r.exceptionDetails.text); return r.result.value; };
  const shot = async name => {
    const {data} = await call('Page.captureScreenshot', {format:'png'});
    fs.writeFileSync(path.join(outDir, name + '.png'), Buffer.from(data, 'base64'));
  };
  const census = `(() => {
    const q = s => document.querySelectorAll(s).length;
    const settled = [...document.querySelectorAll('.state-loading,.state-empty,.state-error')];
    return {
      url: location.pathname,
      buttons: q('[class*=thaw-button]'), inputs: q('[class*=thaw-input]'), cards: q('[class*=thaw-card]'), tags: q('[class*=thaw-tag]'),
      spinner: q('[class*=thaw-spinner]'), msgbar: q('[class*=thaw-message-bar]'),
      rawButtons: [...document.querySelectorAll('button')].filter(b => !/thaw/.test(b.className)).length,
      stateBlocks: settled.length, stateBlocksMissingClass: settled.filter(e => !e.classList.contains('state-block')).length,
      profileSelect: q('select[data-testid="profile-identity-select"]'),
      agentsSelect: q('select[data-testid="agents-project-select"]'),
      sessionsSelect: q('select[data-testid="sessions-project-select"]'),
      agentsNoProjects: q('[data-testid="agents-no-projects"]'),
      memoriesPager: q('[data-testid="memories-pager"]'),
      memoriesList: q('[data-testid="memory-row"]'),
      busyAtCapture: q('[class*=thaw-spinner]') + q('[data-testid$="-loading"]')
    };
  })()`;
  // Wave 3, step 6: the discriminating page-size measurement. docHeight is
  // recorded per route; the /project-memories reading decides MEMORY_PAGE_SIZE:
  // ≤ 2500 px at 1280×800 keeps 10, a taller page drops it to 8 and re-measures.
  const docHeight = `(() => ({url: location.pathname, viewport: innerWidth + 'x' + innerHeight, docHeight: document.documentElement.scrollHeight}))()`;
  const rows = [];
  try {
    for (const [route, name] of routes) {
      await call('Page.navigate', {url: base + route});
      const row = await evaluate(`(async () => {
        const wait = ms => new Promise(r => setTimeout(r, ms));
        let quiet = 0;
        for (let i = 0; i < 300; i++) {
          const busy = document.querySelectorAll('[class*=thaw-spinner]').length + document.querySelectorAll('[data-testid$="-loading"]').length;
          quiet = busy === 0 ? quiet + 1 : 0;
          if (quiet >= 3) break;
          await wait(100);
        }
        return {...${census}, ...${docHeight}};
      })()`);
      assert.equal(row.busyAtCapture, 0, `${route} must be captured at steady state`);
      assert.equal(row.stateBlocksMissingClass, 0, `${route} StateBlock containers must carry .state-block`);
      await shot(name + '-r2');
      rows.push(row);
      console.log(JSON.stringify(row));
    }
    // The skill form is a fixture, not a route state: declare it explicitly and capture it separately.
    await call('Page.navigate', {url: base + '/skill-library'});
    const form = await evaluate(`(async () => {
      const wait = ms => new Promise(r => setTimeout(r, ms));
      const q = id => document.querySelector('[data-testid="' + id + '"]');
      for (let i = 0; i < 100 && !q('skill-create'); i++) await wait(100);
      if (!q('skill-create')) throw Error('skill library never mounted');
      q('skill-create').click();
      for (let i = 0; i < 100 && !q('skill-form'); i++) await wait(50);
      if (!q('skill-form')) throw Error('skill form never opened');
      return ${census};
    })()`);
    assert.equal(form.busyAtCapture, 0, '/skill-library form must be captured at steady state');
    await shot('skill-form-r2');
    console.log(JSON.stringify({...form, fixture:'skill-form-open'}));

    // Wave 3, step 5: the Memories search Field label association. HARD check,
    // no wrapper fallback — a failure stops the step. The mutant this kills is
    // the r1 shape (explicit id kept inside the Field): the control keeps its
    // id but label[for] stays pinned to the Field uuid, so the second clause
    // fails while everything else looks green. The Field uuid becomes BOTH the
    // control's id and the label's for, so we assert through the uuid pair.
    await call('Page.navigate', {url: base + '/project-memories'});
    const fieldCheck = await evaluate(`(async () => {
      const wait = ms => new Promise(r => setTimeout(r, ms));
      for (let i = 0; i < 300; i++) {
        const busy = document.querySelectorAll('[class*=thaw-spinner]').length + document.querySelectorAll('[data-testid$="-loading"]').length;
        if (busy === 0 && document.querySelector('.thaw-field input[name="q"]')) break;
        await wait(100);
      }
      // The testid lives on the thaw Input's outer <span> (its root element);
      // the control is the inner input named q. The Field's label carries for=
      // <uuid>; the association holds iff the control's id equals it.
      const root = document.querySelector('[data-testid="memories-search"]');
      const i = root ? root.querySelector('input') : document.querySelector('.thaw-field input[name="q"]');
      const label = document.querySelector('label[for]');
      return {
        searchRootPresent: root !== null,
        searchControlName: i ? i.name : null,
        searchControlId: i ? i.id : null,
        labelFor: label ? label.getAttribute('for') : null,
        labelForResolves: i && i.id !== '' && document.querySelector('label[for="' + i.id + '"]') !== null
      };
    })()`);
    assert.equal(fieldCheck.searchRootPresent, true, 'the Memories search input must render');
    assert.notEqual(fieldCheck.searchControlId, null, 'the search control must have an id (the Field uuid)');
    assert.notEqual(fieldCheck.searchControlId, '', 'the search control id must be non-empty');
    assert.equal(fieldCheck.searchControlName, 'q', 'the search control keeps name=q');
    assert.equal(fieldCheck.labelForResolves, true, 'label[for] must resolve to the search input id');
    console.log(JSON.stringify({...fieldCheck, fixture:'memories-field-label-association'}));
    console.log('STEADY-STATE CENSUS OK');
  } finally { ws.close(); }
})().catch(e => { console.error('FAIL', e.message); process.exitCode = 1; });
