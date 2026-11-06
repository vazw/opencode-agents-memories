// Readiness-gated route census + screenshots for the Issue 118 waves 2/3 evidence.
// Every count and screenshot is taken only after the route has been idle (no Thaw spinner and no
// `*-loading` testid) for 3 consecutive 100 ms samples; a busy capture fails the run.
// Wave 3 additions: the Memories search Field label association is a HARD check —
// the search input must have a non-empty id and a `label[for]` pointing at it
// (thaw Field pins `for` to its internal uuid; the Input's explicit id is
// dropped so the uuid becomes both). A failure stops the run.
// Wave 3 rework (review-1 Finding 1): the Sessions arms lane — a page-level
// fetch override fabricates a non-empty roster (the empty-store census can
// never reach the roster-Ready arm where the timeline layer lives) and every
// arm transition is hard-checked. Set SESSIONS_ARMS_ONLY=1 to run just that
// lane (RED/GREEN iteration and review re-runs).
// Run against a disposable engine serving this build and Chrome CDP on port 9222:
//   TEST_BASE_URL=http://127.0.0.1:49392 node web-ui/scripts/capture-evidence.cjs
const assert = require('node:assert/strict');
const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');
const base = process.env.TEST_BASE_URL || 'http://127.0.0.1:49392';
const sessionsArmsOnly = process.env.SESSIONS_ARMS_ONLY === '1';
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
  await call('Page.enable');
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
  // Wave 3 rework (review-1 Finding 1): the Sessions arms lane. The roster
  // must be READY for the timeline layer to exist at all, so the census pass
  // (empty store → sessions-no-projects) can never observe it. This lane
  // fabricates the wire answers with a page-level fetch override installed
  // BEFORE the wasm boots (Page.addScriptToEvaluateOnNewDocument, one per
  // arm, removed after the arm settles; Page.enable must be called first —
  // MEASURED: without it the scripts never fire on navigation).
  // `new Response(...)` is required: plain JS-object stubs fail the wasm
  // `dyn_into::<web_sys::Response>` and misclassify as a roster failure.
  // Wire keys are camelCase; the envelope is {success,data}. Each arm carries
  // separate roster (GET /api/projects) and timeline
  // (GET /api/projects/{id}/sessions) answer specs — the roster 501 degrade
  // (sessions-roster-unavailable) is a ROSTER-arm shape; a 501 on the
  // timeline itself renders session-error like any timeline failure.
  // Killed mutant: the timeline closure reading rows/error/loading through
  // get_untracked() and being INVOKED once (`{timeline()}`) — the arm paints
  // mid-load and never recomputes, so `sessions-loading` pins forever. The
  // loading→rows assertion is the discriminator: RED on that tree, GREEN on
  // tracked reads + `{timeline}`.
  const interception = specJson => `
      (function () {
        const real = window.fetch.bind(window);
        const answer = (status, data, error) => new Response(
          JSON.stringify(error ? {success:false, error} : {success:true, data}),
          {status, headers: {'content-type': 'application/json'}});
        const spec = ${specJson};
        window.__sessionsCalls = [];
        window.fetch = (input, init) => {
          const url = typeof input === 'string' ? input : input.url;
          window.__sessionsCalls.push(url);
          const delay = ms => new Promise(r => setTimeout(r, ms));
          if (url.endsWith('/api/projects')) {
            const r = spec.roster;
            if (r.status !== 200) {
              return delay(r.delay || 0).then(() => answer(r.status, null, r.error));
            }
            return Promise.resolve(answer(200, [ { projectId: 'proj-lane-alpha',
              sessionCount: 2, milestoneCount: 0, lastActivityAt: 1794000000000 } ]));
          }
          if (url.includes('/api/projects/')) {
            const t = spec.timeline;
            if (t.status !== 200) {
              return delay(t.delay || 0).then(() => answer(t.status, null, t.error));
            }
            const rows = t.empty ? [] : [ { id: 'sess-lane-1', agentName: 'lane-agent',
                summary: 'lane session one', createdAt: 1794000000000, orphaned: false },
              { id: 'sess-lane-2', agentName: 'lane-agent',
                summary: 'lane session two', createdAt: 1794000001000, orphaned: true } ];
            return (t.delay ? delay(t.delay) : Promise.resolve())
              .then(() => answer(200, rows));
          }
          return real(input, init);
        };
      })();`;
  // Timing: the timeline fetch is issued by the mount effect as soon as the
  // roster resolves; with the 700 ms delay the panel is still in
  // `sessions-loading` at t≈+250 ms and must show rows by t≈+3 s IF the view
  // is reactive. The frozen tree never leaves `sessions-loading`.
  const qExpr = 'const q = id => document.querySelectorAll(\'[data-testid="\' + id + \'"]\').length;';
  const callsExpr = 'const timelineFetches = (window.__sessionsCalls || []).filter(u => u.includes(\'/api/projects/\')).length;';
  const sessionsArms = [
    ['loading-to-rows', { roster: { status: 200 }, timeline: { status: 200, delay: 700 } },
      `(async () => {
        const wait = ms => new Promise(r => setTimeout(r, ms));
        ${qExpr}
        let mid = null;
        for (let i = 0; i < 60; i++) {
          if (q('sessions-loading') > 0 && q('sessions-project-select') === 1) {
            mid = { loading: q('sessions-loading'), rows: q('session-row') };
            break;
          }
          await wait(50);
        }
        if (!mid) throw Error('never saw the mid-load state (picker + sessions-loading)');
        for (let i = 0; i < 50; i++) {
          if (q('session-row') >= 1 && q('sessions-loading') === 0) break;
          await wait(100);
        }
        return { midLoad: mid, rows: q('session-row'), loading: q('sessions-loading'),
                 picker: q('sessions-project-select'), orphanedBadges: q('orphaned-badge') };
      })()`],
    ['roster-unavailable', { roster: { status: 501, error: 'session service absent (501)' }, timeline: { status: 500, error: 'unreachable when the roster fails' } },
      `(async () => {
        const wait = ms => new Promise(r => setTimeout(r, ms));
        ${qExpr}
        ${callsExpr}
        for (let i = 0; i < 50; i++) { if (q('sessions-roster-unavailable') === 1) break; await wait(100); }
        return { rosterUnavailable: q('sessions-roster-unavailable'),
                 picker: q('sessions-project-select'),
                 timelineLayers: q('sessions-loading') + q('session-row'), timelineFetches };
      })()`],
    ['roster-error', { roster: { status: 500, error: 'roster exploded' }, timeline: { status: 500, error: 'unreachable when the roster fails' } },
      `(async () => {
        const wait = ms => new Promise(r => setTimeout(r, ms));
        ${qExpr}
        ${callsExpr}
        for (let i = 0; i < 50; i++) { if (q('sessions-roster-error') === 1) break; await wait(100); }
        return { rosterError: q('sessions-roster-error'), picker: q('sessions-project-select'), timelineFetches };
      })()`],
    ['timeline-error', { roster: { status: 200 }, timeline: { status: 500, error: 'timeline exploded' } },
      `(async () => {
        const wait = ms => new Promise(r => setTimeout(r, ms));
        ${qExpr}
        for (let i = 0; i < 50; i++) { if (q('session-error') === 1) break; await wait(100); }
        return { sessionError: q('session-error'), picker: q('sessions-project-select'),
                 loading: q('sessions-loading') };
      })()`],
    ['timeline-empty', { roster: { status: 200 }, timeline: { status: 200, empty: true } },
      `(async () => {
        const wait = ms => new Promise(r => setTimeout(r, ms));
        ${qExpr}
        for (let i = 0; i < 50; i++) { if (q('sessions-empty') === 1) break; await wait(100); }
        return { sessionsEmpty: q('sessions-empty'), picker: q('sessions-project-select') };
      })()`],
  ];
  const runSessionsArms = async () => {
    for (const [name, spec, probe] of sessionsArms) {
      const { identifier } = await call('Page.addScriptToEvaluateOnNewDocument', {
        source: interception(JSON.stringify(spec)) });
      try {
        await call('Page.navigate', { url: base + '/sessions' });
        const out = await evaluate(probe);
        if (name === 'loading-to-rows') {
          assert.ok(out.midLoad, 'the mid-load state (picker + sessions-loading) must be observable');
          assert.ok(out.midLoad.loading >= 1, 'sessions-loading must be visible mid-load');
          // THE discriminator: the frozen tree pins sessions-loading forever
          // (rows=0, loading=1 at t+3 s while the fetches have settled).
          assert.ok(out.rows >= 1, 'sessions-loading must transition to session-row once the timeline settles (frozen timeline layer: rows=' + out.rows + ', loading=' + out.loading + ')');
          assert.equal(out.loading, 0, 'sessions-loading must clear once rows render');
          assert.equal(out.picker, 1, 'the picker must stay present in the Ready arm');
        } else if (name === 'roster-unavailable') {
          assert.equal(out.rosterUnavailable, 1, 'roster 501 must render sessions-roster-unavailable');
          assert.equal(out.picker, 0, 'a failed roster suppresses the picker entirely');
          assert.equal(out.timelineLayers, 0, 'a failed roster suppresses the timeline layer');
          assert.equal(out.timelineFetches, 0, 'a roster failure must not fetch a timeline (no project to pick)');
        } else if (name === 'roster-error') {
          assert.equal(out.rosterError, 1, 'roster 500 must render sessions-roster-error');
          assert.equal(out.picker, 0, 'a failed roster suppresses the picker entirely');
          assert.equal(out.timelineFetches, 0, 'a roster failure must not fetch a timeline (no project to pick)');
        } else if (name === 'timeline-error') {
          assert.equal(out.sessionError, 1, 'timeline 500 must render session-error');
          assert.equal(out.picker, 1, 'a timeline failure must NOT disable the picker');
          assert.equal(out.loading, 0, 'sessions-loading must clear on a timeline failure');
        } else if (name === 'timeline-empty') {
          assert.equal(out.sessionsEmpty, 1, 'an empty timeline must render sessions-empty');
          assert.equal(out.picker, 1, 'the picker must stay present on the empty arm');
        }
        console.log(JSON.stringify({...out, fixture: 'sessions-arms-' + name}));
      } finally {
        await call('Page.removeScriptToEvaluateOnNewDocument', { identifier });
      }
    }
  };
  try {
    if (sessionsArmsOnly) {
      await runSessionsArms();
      console.log('SESSIONS ARMS OK');
      return;
    }
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

    // The Sessions arms lane (fabricated roster + per-arm timeline answers).
    await runSessionsArms();

    console.log('STEADY-STATE CENSUS OK');

  } finally { ws.close(); }
})().catch(e => { console.error('FAIL', e.message); process.exitCode = 1; });
