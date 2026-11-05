// Run against a disposable engine serving this build and Chrome CDP on port 9222.
// Regression: every StateBlock container must carry .state-block in the DOM (the class
// expression used to discard it, so the spacing rule never applied).
const assert = require('node:assert/strict');
const http = require('node:http');
const base = process.env.TEST_BASE_URL || 'http://127.0.0.1:49392';
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
  try {
    // 1. A settled StateBlock (empty store) must carry .state-block and its margin.
    await call('Page.navigate', {url:base + '/project-memories'});
    const settled = await evaluate(`(async () => {
      const wait = ms => new Promise(r => setTimeout(r, ms));
      for (let i = 0; i < 100 && !document.querySelector('[data-testid="memories-empty"],[data-testid="memories-error"]'); i++) await wait(100);
      const el = document.querySelector('[data-testid="memories-empty"],[data-testid="memories-error"]');
      if (!el) throw Error('no settled StateBlock');
      const cs = getComputedStyle(el);
      return { testid: el.getAttribute('data-testid'), className: el.className, hasStateBlock: el.classList.contains('state-block'), marginTop: cs.marginTop, marginBottom: cs.marginBottom, stateBlockCount: document.querySelectorAll('.state-block').length };
    })()`);
    assert.equal(settled.hasStateBlock, true, 'StateBlock container must carry .state-block');
    assert.equal(settled.marginTop, '12px', '.state-block margin-top must apply (0.75rem)');
    assert.equal(settled.marginBottom, '12px', '.state-block margin-bottom must apply (0.75rem)');
    assert.ok(settled.stateBlockCount > 0, 'page must render at least one .state-block');

    // 2. Side effects: the /agents loading container must keep the common class too (its own
    //    .agents-loading rule sets no margin, so .state-block supplies the spacing). The roster
    //    request is held open for 800 ms so the Loading arm is deterministic, not a race.
    await call('Fetch.enable', {patterns:[{urlPattern:'*/api/projects'}]});
    await call('Page.navigate', {url:base + '/agents'});
    const agents = await evaluate(`(async () => {
      const wait = ms => new Promise(r => setTimeout(r, ms));
      let el = null;
      for (let i = 0; i < 100 && !el; i++) { el = document.querySelector('[data-testid="agents-loading"]'); if (!el) await wait(20); }
      if (!el) throw Error('agents loading StateBlock never rendered');
      const cs = getComputedStyle(el);
      return { className: el.className, hasStateBlock: el.classList.contains('state-block'), marginTop: cs.marginTop, marginBottom: cs.marginBottom };
    })()`);
    await call('Fetch.disable');
    assert.equal(agents.hasStateBlock, true, '/agents loading StateBlock must carry .state-block');
    assert.equal(agents.marginTop, '12px', '/agents loading must keep the .state-block margin');
    const agentsSettled = await evaluate(`(async () => {
      const wait = ms => new Promise(r => setTimeout(r, ms));
      for (let i = 0; i < 100 && !document.querySelector('[data-testid="agents-no-projects"],[data-testid="agents-roster"]'); i++) await wait(100);
      const states = [...document.querySelectorAll('.state-loading,.state-empty,.state-error')];
      return { missing: states.filter(e => !e.classList.contains('state-block')).map(e => e.getAttribute('data-testid')), stateCount: states.length };
    })()`);
    assert.deepEqual(agentsSettled.missing, [], 'every settled /agents state container must carry .state-block');
    console.log('PASS StateBlock class', JSON.stringify({ settled, agents, agentsSettled }));
  } finally { ws.close(); }
})().catch(e => { console.error('FAIL', e.message); process.exitCode=1; });
