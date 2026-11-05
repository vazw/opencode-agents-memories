// Readiness-gated route census + screenshots for the Issue 118 wave 2 evidence.
// Every count and screenshot is taken only after the route has been idle (no Thaw spinner and no
// `*-loading` testid) for 3 consecutive 100 ms samples; a busy capture fails the run.
// Run against a disposable engine serving this build and Chrome CDP on port 9222:
//   TEST_BASE_URL=http://127.0.0.1:49392 node web-ui/scripts/capture-evidence.cjs
const assert = require('node:assert/strict');
const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');
const base = process.env.TEST_BASE_URL || 'http://127.0.0.1:49392';
const outDir = process.env.EVIDENCE_DIR || path.join(__dirname, '..', '..', 'docs', 'evidence', 'webui-wave2-thaw-controls');
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
      busyAtCapture: q('[class*=thaw-spinner]') + q('[data-testid$="-loading"]')
    };
  })()`;
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
        return ${census};
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
    console.log('STEADY-STATE CENSUS OK');
  } finally { ws.close(); }
})().catch(e => { console.error('FAIL', e.message); process.exitCode = 1; });
