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
//
// Issue 129 residuals (r9) add two more self-gating lanes. Each writes a markdown
// table of ACTUAL outcomes and exits non-zero when those outcomes do not match the
// declared classification for the tree it ran on:
//   PILL_CONTRACT_ONLY=1 LANE_EXPECT=pre|post  testids / thaw-button / .chip retirement
//   DIALOG_ONLY=1        LANE_EXPECT=pre|post  dialog role+name, Esc, focus trap+return,
//                                              maintenance confirm-in-dialog, counts
// `LANE_TABLE=<file>` receives the table; `LANE_PAGE_ID=<cdp target id>` pins the
// lane's tab (so a concurrent lane on the same browser is not stolen).
// `WEBUI_DIST=<dir>` is the built bundle the table declares (id + sha256).
const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');
const base = process.env.TEST_BASE_URL || 'http://127.0.0.1:49392';
const sessionsArmsOnly = process.env.SESSIONS_ARMS_ONLY === '1';
const pillsOnly = process.env.PILL_CONTRACT_ONLY === '1';
const dialogOnly = process.env.DIALOG_ONLY === '1';
const laneExpect = process.env.LANE_EXPECT || 'post';
const lanePageId = process.env.LANE_PAGE_ID || '';
const outDir = process.env.EVIDENCE_DIR || path.join(__dirname, '..', '..', 'docs', 'evidence', 'webui-wave3-worst-screens');
const routes = [
  ['/', 'home'], ['/project-memories', 'project-memories'], ['/tags', 'tags'], ['/user-profile', 'user-profile'],
  ['/settings', 'settings'], ['/export-import', 'export-import'], ['/skill-library', 'skill-library'],
  ['/sessions', 'sessions'], ['/agents', 'agents'], ['/stats', 'stats']
];
const get = url => new Promise((resolve, reject) => http.get(url, r => { let body = ''; r.on('data', c => body += c); r.on('end', () => resolve(JSON.parse(body))); }).on('error', reject));
(async () => {
  const pages = (await get('http://127.0.0.1:9222/json')).filter(p => p.type === 'page');
  const page = lanePageId ? pages.find(p => p.id === lanePageId) : pages.find(p => p.url.startsWith(base));
  assert(page, lanePageId ? 'LANE_PAGE_ID must name an open CDP page' : 'open the lane on CDP first');
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { ws.onopen = resolve; ws.onerror = reject; });
  let id = 0; const pending = new Map();
  ws.onmessage = event => { const m = JSON.parse(event.data); if (!pending.has(m.id)) return; const [resolve, reject] = pending.get(m.id); pending.delete(m.id); m.error ? reject(Error(m.error.message)) : resolve(m.result); };
  // A CDP call must never wedge the lane: a navigation that lands mid-dispatch
  // can swallow a reply, so every call carries its own watchdog.
  const call = (method, params = {}) => new Promise((resolve, reject) => {
    const n = ++id;
    const timer = setTimeout(() => {
      pending.delete(n);
      reject(Error(`CDP ${method} did not answer within 20s`));
    }, 20000);
    pending.set(n, [value => { clearTimeout(timer); resolve(value); },
                    error => { clearTimeout(timer); reject(error); }]);
    ws.send(JSON.stringify({id:n, method, params}));
  });
  await call('Page.enable');
  const evaluate = async expression => { const r = await call('Runtime.evaluate', {expression, awaitPromise:true, returnByValue:true}); if (r.exceptionDetails) { const d = r.exceptionDetails; throw Error(`${d.text}${d.exception && d.exception.description ? ': ' + d.exception.description : ''}`); } return r.result.value; };
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
  // =========================================================================
  // Issue 129 residuals (r9): the pill-contract and dialog lanes.
  //
  // Both lanes run on one intercepted-REST fixture (`new Response`, installed
  // before the wasm boots) so the DOM reaches its actor without a seeded store,
  // and both are self-gating: `LANE_EXPECT=pre` requires every RED row to FAIL
  // and every CONTROL row to PASS; `LANE_EXPECT=post` requires all PASS. The
  // table records the ACTUAL outcome — a RED row that passes pre-change is a
  // non-discriminating assertion (exit 1), and a CONTROL that fails pre-change
  // is a broken fixture (exit 1).
  // =========================================================================
  const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
  const waitFor = async (expression, ms = 8000) => {
    const deadline = Date.now() + ms;
    while (Date.now() < deadline) {
      const value = await evaluate(expression);
      if (value) return value;
      await sleep(50);
    }
    return null;
  };
  const distDir = process.env.WEBUI_DIST || path.join(__dirname, '..', 'dist');
  const wasmFile = fs.readdirSync(distDir).find(name => name.endsWith('_bg.wasm'));
  assert(wasmFile, 'the declared bundle must hold a *_bg.wasm');
  const bundleId = (wasmFile.match(/web-ui-([0-9a-f]+)_bg\.wasm/) || [null, 'unknown'])[1];
  const bundleSha = crypto.createHash('sha256').update(fs.readFileSync(path.join(distDir, wasmFile))).digest('hex');
  const laneRows = [];
  const laneRecord = (row, kind, fixture, outcome, detail) => {
    laneRows.push({row, kind, fixture, outcome, detail});
    console.log(JSON.stringify({row, kind, fixture, outcome, detail, bundle: bundleId}));
  };
  const laneRun = async (row, kind, fixture, body) => {
    try {
      const detail = await body();
      laneRecord(row, kind, fixture, 'PASS', detail);
    } catch (failure) {
      const message = failure && failure.message ? failure.message : String(failure);
      laneRecord(row, kind, fixture, message.startsWith('FIXTURE:') ? 'STOP' : 'FAIL', message);
    }
  };
  const finishLane = name => {
    const lines = [
      `# ${name} lane — bundle ${bundleId} (sha256 ${bundleSha})`,
      `LANE_EXPECT=${laneExpect}`,
      '',
      '| row | kind | fixture | outcome | expected | detail |',
      '| --- | --- | --- | --- | --- | --- |',
    ];
    let mismatches = 0;
    let stopped = false;
    for (const row of laneRows) {
      const expected = laneExpect === 'pre' ? (row.kind === 'RED' ? 'FAIL' : 'PASS') : 'PASS';
      if (row.outcome === 'STOP') stopped = true;
      else if (row.outcome !== expected) mismatches += 1;
      const detail = String(row.detail).replace(/\|/g, '\\|').replace(/\n/g, ' ');
      lines.push(`| ${row.row} | ${row.kind} | ${row.fixture} | ${row.outcome} | ${expected} | ${detail} |`);
    }
    const text = `${lines.join('\n')}\n`;
    if (process.env.LANE_TABLE) fs.writeFileSync(process.env.LANE_TABLE, text);
    console.log(text);
    console.log(`LANE ${name} bundle=${bundleId} expect=${laneExpect} rows=${laneRows.length} mismatches=${mismatches} stopped=${stopped}`);
    if (mismatches > 0 || stopped) process.exitCode = 1;
  };
  // Real pointer input at the element's centre (never element.click()).
  // A real pointer click needs a target that is really there: the async status
  // blocks shift the page after the first paint, so a centre read once can land
  // on whatever moved underneath. Scroll it into view, hit-test the centre, and
  // retry briefly; a covered target is a fixture failure with a reason, never a
  // silent click on the wrong element.
  const clickAt = async selector => {
    let spot = null;
    let why = 'not rendered';
    for (let attempt = 0; attempt < 4 && !spot; attempt += 1) {
      const seen = await evaluate(`(() => {
        const el = document.querySelector(${JSON.stringify(selector)});
        if (!el) return {why: 'not rendered'};
        el.scrollIntoView({block: 'center', inline: 'center'});
        const r = el.getBoundingClientRect();
        const x = r.left + r.width / 2;
        const y = r.top + r.height / 2;
        if (r.width === 0 || r.height === 0) return {why: 'zero size'};
        const hit = document.elementFromPoint(x, y);
        if (!hit || !(el === hit || el.contains(hit))) return {why: 'covered at its centre'};
        return {x, y};
      })()`);
      if (seen.x !== undefined) spot = seen;
      else {
        why = seen.why;
        await sleep(100);
      }
    }
    if (!spot) throw Error(`FIXTURE: ${selector} never became clickable (${why})`);
    for (const type of ['mouseMoved', 'mousePressed', 'mouseReleased']) {
      await call('Input.dispatchMouseEvent', {
        type, x: spot.x, y: spot.y, button: type === 'mouseMoved' ? 'none' : 'left', clickCount: 1,
      });
    }
  };
  const clickPoint = async (x, y) => {
    for (const type of ['mouseMoved', 'mousePressed', 'mouseReleased']) {
      await call('Input.dispatchMouseEvent', {
        type, x, y, button: type === 'mouseMoved' ? 'none' : 'left', clickCount: 1,
      });
    }
  };
  // Real key events: Tab/Enter/Escape must be able to perform their browser
  // default action, so they go through the Input domain, not a JS dispatch.
  // `keyDown` (not `rawKeyDown`) so the browser runs the key's default action —
  // Tab moves focus, Enter/Space activate the focused button.
  const pressKey = async (key, code, virtualKeyCode, modifiers = 0, text) => {
    const base = {key, code, windowsVirtualKeyCode: virtualKeyCode, nativeVirtualKeyCode: virtualKeyCode, modifiers};
    await call('Input.dispatchKeyEvent', text ? {type: 'keyDown', text, ...base} : {type: 'keyDown', ...base});
    await call('Input.dispatchKeyEvent', {type: 'keyUp', ...base});
  };
  const fixtureSource = `
    (function () {
      var real = window.fetch.bind(window);
      function answer(status, data, error) {
        var body = error ? {success: false, error} : {success: true, data};
        if (window.__laneAnswers) {
          window.__laneAnswers.push({status: status, body: JSON.stringify(body).slice(0, 160)});
        }
        return new Response(JSON.stringify(body),
          {status: status, headers: {'content-type': 'application/json'}});
      }
      var spec = {};
      try { spec = JSON.parse(window.localStorage.getItem('__laneSpec') || '{}') || {}; }
      catch (e) { spec = {}; }
      window.__laneCalls = [];
      window.__laneAnswers = [];
      window.__laneDeleted = false;
      window.__laneSessionsDeleted = false;
      window.__laneSkillsDeleted = false;
      window.__laneCleanupCount = 0;
      window.__laneHold = 'none';
      var release = null;
      window.__laneRelease = function () {
        var done = release;
        release = null;
        window.__laneHold = 'none';
        if (done) done();
      };
      function hold() { return new Promise(function (resolve) { release = resolve; }); }
      function urlOf(input) { return typeof input === 'string' ? input : input.url; }
      function methodOf(input, init) {
        if (init && init.method) return String(init.method).toUpperCase();
        if (typeof input !== 'string' && input.method) return String(input.method).toUpperCase();
        return 'GET';
      }
      function queryOf(url) { return new URL(url, location.origin); }
      function memoriesAnswer(url) {
        var tags = queryOf(url).searchParams.getAll('tags');
        var items = window.__laneDeleted ? [] : (spec.memories || []);
        if (tags.length > 0) {
          items = items.filter(function (row) {
            return tags.every(function (tag) { return (row.tags || []).indexOf(tag) !== -1; });
          });
        }
        return answer(200, {items: items, total: items.length, page: 1, pageSize: 8, totalPages: 1});
      }
      window.fetch = function (input, init) {
        var url = urlOf(input);
        if (url.indexOf('/api/') === -1) return real(input, init);
        var method = methodOf(input, init);
        var path = queryOf(url).pathname;
        window.__laneCalls.push({method: method, url: url});
        if (path === '/api/memories' && method === 'GET') return Promise.resolve(memoriesAnswer(url));
        if (/^\\/api\\/memories\\/[^/]+$/.test(path) && method === 'DELETE') {
          window.__laneDeleted = true;
          if (window.__laneHold === 'delete') return hold().then(function () { return answer(200, {}); });
          return Promise.resolve(answer(200, {}));
        }
        if (path === '/api/tags') {
          return Promise.resolve(answer(200, {project: [], user: [], memoryTags: [], memoryTagTotal: 0}));
        }
        if (path === '/api/projects') {
          return Promise.resolve(answer(200, [{projectId: 'proj-lane-alpha', sessionCount: 1,
            milestoneCount: 0, lastActivityAt: 1794000000000}]));
        }
        if (/^\\/api\\/projects\\/[^/]+\\/sessions$/.test(path) && method === 'GET') {
          var sessions = window.__laneSessionsDeleted ? [] : [{id: 'sess-lane-1', agentName: 'lane-agent',
            summary: 'lane session one', createdAt: 1794000000000, orphaned: false}];
          return Promise.resolve(answer(200, sessions));
        }
        if (/^\\/api\\/projects\\/[^/]+\\/sessions\\/[^/]+$/.test(path) && method === 'DELETE') {
          window.__laneSessionsDeleted = true;
          if (window.__laneHold === 'delete') return hold().then(function () { return answer(200, {}); });
          return Promise.resolve(answer(200, {}));
        }
        if (path === '/api/skills' && method === 'GET') {
          var skills = window.__laneSkillsDeleted ? [] : [{id: 'skill-lane-1', name: 'lane skill',
            description: 'lane description', content: 'lane content', autoinvoke: true, enabled: true,
            createdAt: '2026-11-06T00:00:00Z', updatedAt: '2026-11-06T00:00:00Z'}];
          return Promise.resolve(answer(200, {items: skills}));
        }
        if (/^\\/api\\/skills\\/[^/]+$/.test(path) && method === 'DELETE') {
          window.__laneSkillsDeleted = true;
          if (window.__laneHold === 'delete') return hold().then(function () { return answer(200, {}); });
          return Promise.resolve(answer(200, {}));
        }
        if (path === '/api/health') {
          return Promise.resolve(new Response(
            JSON.stringify({success: true, status: 'ok', authEnabled: false}),
            {status: 200, headers: {'content-type': 'application/json'}}));
        }
        if (path === '/api/stats') {
          return Promise.resolve(answer(200, {total: 0, byScope: {user: 0, project: 0}, byType: {},
            generatedAt: '2026-11-06T00:00:00Z', shards: [], profile: null, daily: [], dailyError: null}));
        }
        if (path === '/api/cleanup' && method === 'POST') {
          window.__laneCleanupCount += 1;
          var report = {deletedCount: 0, userCount: 0, projectCount: 0, promptsDeleted: 0,
            protectedMemoriesSkipped: 0};
          if (window.__laneHold === 'cleanup') return hold().then(function () { return answer(200, report); });
          return Promise.resolve(answer(200, report));
        }
        return real(input, init);
      };
    })();`;
  const PILL_SPEC = {
    memories: [
      {type: 'memory', id: 'mem-lane-1', content: 'lane memory one carrying the rust tag',
        memoryType: 'note', tags: ['rust'], createdAt: '1794000000000', isPinned: false},
      {type: 'memory', id: 'mem-lane-2', content: 'lane memory two carrying six tags',
        memoryType: 'note', tags: ['alpha', 'beta', 'gamma', 'delta', 'epsilon', 'zeta'],
        createdAt: '1794000000000', isPinned: false},
      {type: 'memory', id: 'mem-lane-3', content: 'lane memory three carrying no tag',
        memoryType: 'note', tags: [], createdAt: '1794000000000', isPinned: false},
    ],
  };
  const DIALOG_SPEC = {
    memories: [
      {type: 'memory', id: 'mem-lane-1', content: 'lane memory one carrying the rust tag',
        memoryType: 'note', tags: ['rust'], createdAt: '1794000000000', isPinned: false},
    ],
  };
  const laneSetSpec = async spec => {
    await evaluate(`window.localStorage.setItem('__laneSpec', ${JSON.stringify(JSON.stringify(spec))})`);
  };
  const laneBoot = async (route, readyExpression, ms = 10000) => {
    await call('Page.navigate', {url: base + route});
    const landed = await waitFor(
      `location.origin === ${JSON.stringify(base)} && location.pathname === ${JSON.stringify(route)}` +
      ` && document.readyState === 'complete'`, ms);
    if (!landed) throw Error(`FIXTURE: ${route} never loaded`);
    const ready = await waitFor(readyExpression, ms);
    if (!ready) {
      const seen = await evaluate(`(() => ({
        text: document.body ? document.body.innerText.slice(0, 240) : 'no body',
        testids: [...document.querySelectorAll('[data-testid]')].map(e => e.getAttribute('data-testid')),
        laneInstalled: typeof window.__laneCalls,
        calls: (window.__laneCalls || []).map(c => c.method + ' ' + c.url),
      }))()`);
      throw Error(`FIXTURE: ${route} never rendered (${readyExpression}) — ${JSON.stringify(seen)}`);
    }
    return ready;
  };
  const laneGoto = laneBoot;
  const ARMED_SURFACE = `document.querySelectorAll('[data-testid="confirm-gate"],[data-testid="confirm-dialog"]')`;
  const armedCount = `(() => ${ARMED_SURFACE}.length)()`;
  const armRows = route => `document.querySelectorAll('[data-testid="memory-row"]').length`;
  const goneExpr = selector => `document.querySelectorAll(${JSON.stringify(selector)}).length === 0`;
  // The armed-surface selector guard every dialog row starts with: exactly one
  // surface, Confirm and Cancel exactly once each. A zero count is a fixture
  // failure (STOP), never a RED.
  const armGuard = async () => {
    const guard = await waitFor(`(() => {
      const surface = ${ARMED_SURFACE}.length;
      const confirm = document.querySelectorAll('[data-testid="confirm-gate-confirm"]').length;
      const cancel = document.querySelectorAll('[data-testid="confirm-gate-cancel"]').length;
      return surface === 1 && confirm === 1 && cancel === 1
        ? {surface, confirm, cancel} : null;
    })()`, 3000);
    if (!guard) throw Error('FIXTURE: the armed confirmation surface never matched exactly once');
    return guard;
  };
  const armMemoryDelete = async () => {
    await laneGoto('/project-memories', armRows('/project-memories'));
    await clickAt('[data-testid="memory-delete"]');
    return armGuard();
  };
  const cancelArmed = async () => {
    await clickAt('[data-testid="confirm-gate-cancel"]');
    const gone = await waitFor(goneExpr('[data-testid="confirm-gate"]') + ' || ' +
      goneExpr('[data-testid="confirm-dialog"]'), 3000);
    if (!gone) throw Error('FIXTURE: the armed surface never closed');
  };
  const laneCalls = () => evaluate('window.__laneCalls.map(c => c.method + " " + c.url)');
  const laneDeleteCalls = async () => (await laneCalls()).filter(entry => entry.startsWith('DELETE ')).length;
  const focusInsideArmed = `(() => {
    const active = document.activeElement;
    const box = document.querySelector('[role="dialog"]') ||
      document.querySelector('[data-testid="confirm-gate"]');
    if (!box || !active) return false;
    return box === active || box.contains(active);
  })()`;

  const runPillLane = async () => {
    await call('Page.navigate', {url: base + '/'});
    await waitFor(`location.origin === ${JSON.stringify(base)} && document.readyState === 'complete'`);
    await laneSetSpec(PILL_SPEC);
    const cardPills = `document.querySelectorAll('[data-testid="memory-tags"] button')`;
    const memoryReady = `document.querySelectorAll('[data-testid="memory-row"]').length === 3`;
    const clickRust = () => clickAt('[data-testid="memory-tags"] button[aria-label="Filter by tag: rust"]');
    const filterPill = `document.querySelectorAll('button[aria-label^="Remove tag:"]')`;
    const rowCount = `document.querySelectorAll('[data-testid="memory-row"]').length`;
    const seen = () => evaluate(`(() => ({
      filters: ${filterPill}.length, rows: ${rowCount},
      calls: window.__laneCalls.map(c => c.method + ' ' + c.url),
      answers: window.__laneAnswers.slice(-4),
    }))()`);
    // The card list is fetched asynchronously after the render that shows the
    // filters, so a row must wait for the SETTLED state, not for the first paint.
    const openFiltered = async () => {
      await laneGoto('/project-memories', memoryReady);
      await clickRust();
      const settled = await waitFor(`(${filterPill}.length === 1 && ${rowCount} === 1)`, 6000);
      if (!settled) throw Error(`FIXTURE: the tag filter never settled — ${JSON.stringify(await seen())}`);
      return settled;
    };

    await laneRun('pill_is_thaw_button', 'RED', 'card + filter pills', async () => {
      // The card pills are counted on the UNFILTERED list (a filter narrows it to
      // one record, one pill); the active filter pill is counted after the click.
      await laneGoto('/project-memories', memoryReady);
      const cards = await evaluate(`(() => {
        const card = [...${cardPills}];
        return {count: card.length, raw: card.filter(b => !/thaw-button/.test(b.className)).length};
      })()`);
      if (cards.count < 5) throw Error(`FIXTURE: only ${cards.count} card pills rendered`);
      await clickRust();
      const settled = await waitFor(`(${filterPill}.length === 1 && ${rowCount} === 1)`, 6000);
      if (!settled) throw Error(`FIXTURE: the tag filter never settled — ${JSON.stringify(await seen())}`);
      const filter = await evaluate(`(() => {
        const pill = ${filterPill}[0];
        return {raw: /thaw-button/.test(pill.className) ? 0 : 1};
      })()`);
      if (cards.raw !== 0 || filter.raw !== 0) {
        throw Error(`${cards.raw} card pill(s) of ${cards.count} and ${filter.raw} filter pill(s) are not thaw-button`);
      }
      return `card=${cards.count} filter=1 all thaw-button`;
    });

    await laneRun('pill_no_chip_class', 'RED', 'card + filter pills', async () => {
      await openFiltered();
      const chip = await evaluate(`document.querySelectorAll('.chip').length`);
      if (chip !== 0) throw Error(`${chip} element(s) still carry class \"chip\"`);
      return 'no .chip element in the document';
    });

    await laneRun('pill_no_chip_selector', 'RED', 'stylesheet text', async () => {
      // Scope: the BUNDLE under test (`WEBUI_DIST` or the working build), so a
      // re-run against a preserved pre bundle greps THAT bundle's stylesheet
      // and not the working tree (review-1 MEDIUM: the source-scoped read
      // reported PASS against the pre bundle because the tree had already
      // retired `.chip`). The source file is the fallback only when the dist
      // carries no styles entry, and the detail line says which was read.
      const bundleCss = fs.readdirSync(distDir).find(name => name.startsWith('styles') && name.endsWith('.css'));
      const scope = bundleCss
        ? `bundle ${path.join(distDir, bundleCss)}`
        : 'SOURCE (no styles entry in the dist)';
      const css = fs.readFileSync(bundleCss ? path.join(distDir, bundleCss) : path.join(__dirname, '..', 'public', 'styles.css'), 'utf8');
      const rule = (css.match(/\.chip/g) || []).length;
      if (rule !== 0) throw Error(`grep -c '\\.chip' prints ${rule} in ${scope}`);
      return `grep -c '\\.chip' prints 0 in ${scope}`;
    });

    await laneRun('pill_has_stable_testid', 'RED', 'card + filter pills', async () => {
      await laneGoto('/project-memories', memoryReady);
      const cards = await evaluate(`(() => ({
        card: ${cardPills}.length,
        stable: document.querySelectorAll('[data-testid="memory-tag-pill"]').length,
      }))()`);
      if (cards.card < 5) throw Error(`FIXTURE: only ${cards.card} card pills rendered`);
      await clickRust();
      const settled = await waitFor(`(${filterPill}.length === 1 && ${rowCount} === 1)`, 6000);
      if (!settled) throw Error(`FIXTURE: the tag filter never settled — ${JSON.stringify(await seen())}`);
      const stable = await evaluate(`document.querySelectorAll('[data-testid="memory-filter-pill"]').length`);
      // Expected to fail on both counts of 0 pre-change: the testids ARE the change.
      if (cards.stable !== cards.card || stable !== 1) {
        throw Error(`memory-tag-pill=${cards.stable} of ${cards.card} card pills, ` +
          `memory-filter-pill=${stable} of 1 filter pill`);
      }
      return `memory-tag-pill=${cards.stable} memory-filter-pill=${stable}`;
    });

    await laneRun('pill_accessible_name_preserved', 'CONTROL', 'card + filter pills', async () => {
      await openFiltered();
      const names = await evaluate(`(() => {
        const card = document.querySelector('[data-testid="memory-tags"] button[aria-label="Filter by tag: rust"]');
        const filter = ${filterPill};
        return {card: card ? card.getAttribute('aria-label') : null,
                filter: filter.length === 1 ? filter[0].getAttribute('aria-label') : null,
                nested: [...document.querySelectorAll('[data-testid="memory-tags"] button [aria-label]')].length};
      })()`);
      if (names.card !== 'Filter by tag: rust') throw Error(`card pill aria-label is ${JSON.stringify(names.card)}`);
      if (names.filter !== 'Remove tag: rust') throw Error(`filter pill aria-label is ${JSON.stringify(names.filter)}`);
      return 'card=\"Filter by tag: rust\" filter=\"Remove tag: rust\"';
    });

    await laneRun('pill_click_filters_as_before', 'CONTROL', 'tagged + untagged records', async () => {
      await laneGoto('/project-memories', memoryReady);
      await evaluate('window.__laneCalls = []');
      await clickRust();
      const state = await waitFor(`(() => {
        const filters = ${filterPill}.length;
        const rows = ${rowCount};
        return (filters === 1 && rows === 1) ? {filters, rows, calls: window.__laneCalls.map(c => c.url)} : null;
      })()`, 6000);
      if (!state) throw Error(`FIXTURE: the click never settled — ${JSON.stringify(await seen())}`);
      const tagged = state.calls.filter(url => url.includes('/api/memories') && /[?&]tags=rust(&|$)/.test(url));
      if (tagged.length === 0) throw Error('no tag-filtered /api/memories request was issued');
      if (state.rows !== 1) throw Error(`rows narrowed to ${state.rows}, expected the one rust record`);
      return `rows=${state.rows} tagged requests=${tagged.length}`;
    });

    await laneRun('pill_duplicate_filter_idempotent', 'CONTROL', 'rust pill clicked twice', async () => {
      await openFiltered();
      const before = (await laneCalls()).filter(url => /[?&]tags=rust(&|$)/.test(url)).length;
      await clickRust();
      await sleep(600);
      const after = (await laneCalls()).filter(url => /[?&]tags=rust(&|$)/.test(url)).length;
      const filters = await evaluate(`${filterPill}.length`);
      if (filters !== 1) throw Error(`${filters} active filter pills after the duplicate click`);
      if (after - before > 1) throw Error(`${after - before} tag-filtered requests after the second click`);
      return `filters=${filters} extra tagged requests=${after - before}`;
    });

    await laneRun('pill_remove_filter_as_before', 'CONTROL', 'rust filter removed', async () => {
      await openFiltered();
      const before = (await laneCalls()).length;
      await clickAt('button[aria-label^="Remove tag:"]');
      const state = await waitFor(`(() => {
        const filters = ${filterPill}.length;
        const rows = ${rowCount};
        return (filters === 0 && rows === 3)
          ? {filters, rows, calls: window.__laneCalls.slice(${before}).map(c => c.url)} : null;
      })()`, 6000);
      if (!state) throw Error(`FIXTURE: the removal never settled — ${JSON.stringify(await seen())}`);
      const unfiltered = state.calls.filter(url => url.includes('/api/memories') && !/[?&]tags=/.test(url));
      if (unfiltered.length === 0) throw Error('no unfiltered /api/memories request after the removal');
      return `filters=0 rows=${state.rows} unfiltered requests=${unfiltered.length}`;
    });

    await laneRun('pill_overflow_survives', 'CONTROL', 'six-tag record', async () => {
      await laneGoto('/project-memories', memoryReady);
      const overflow = await evaluate(`(() => {
        const row = [...document.querySelectorAll('[data-testid="memory-row"]')]
          .find(li => (li.getAttribute('data-tags') || '').includes('epsilon'));
        if (!row) return null;
        const pills = row.querySelectorAll('[data-testid="memory-tags"] button').length;
        const span = [...row.querySelectorAll('[data-testid="memory-tags"] span')]
          .find(el => el.textContent.trim() === '+2 more');
        return {pills, overflow: Boolean(span), overflowTag: span ? span.tagName : null};
      })()`);
      if (!overflow) throw Error('FIXTURE: the six-tag record never rendered');
      if (overflow.pills !== 4) throw Error(`${overflow.pills} pills on the six-tag record, expected 4`);
      if (!overflow.overflow) throw Error('the \"+2 more\" overflow label is missing');
      if (overflow.overflowTag === 'BUTTON') throw Error('the overflow label is a button');
      return `pills=${overflow.pills} overflow=<${overflow.overflowTag.toLowerCase()}>`;
    });

    await laneRun('pill_keyboard_activation', 'CONTROL', 'Enter on the focused rust pill', async () => {
      await laneGoto('/project-memories', memoryReady);
      const focused = await evaluate(`(() => {
        const pill = document.querySelector('[data-testid="memory-tags"] button[aria-label="Filter by tag: rust"]');
        if (!pill) return false;
        pill.focus();
        return document.activeElement === pill;
      })()`);
      if (!focused) throw Error('FIXTURE: the card pill could not take focus');
      await evaluate('window.__laneCalls = []');
      await pressKey('Enter', 'Enter', 13, 0, '\r');
      const state = await waitFor(`(() => {
        const filters = ${filterPill}.length;
        const rows = ${rowCount};
        return (filters === 1 && rows === 1) ? {filters, rows, calls: window.__laneCalls.map(c => c.url)} : null;
      })()`, 6000);
      if (!state) throw Error(`FIXTURE: Enter produced no settled active filter — ${JSON.stringify(await seen())}`);
      const tagged = state.calls.filter(url => /[?&]tags=rust(&|$)/.test(url));
      if (tagged.length === 0) throw Error('Enter did not issue the tag-filtered request');
      return `filters=${state.filters} tagged requests=${tagged.length}`;
    });

    await laneRun('pill_type_button_no_submit', 'CONTROL', 'card pill type attribute', async () => {
      await laneGoto('/project-memories', memoryReady);
      const types = await evaluate(`(() => {
        const pills = [...${cardPills}];
        return {count: pills.length, types: pills.map(p => p.getAttribute('type'))};
      })()`);
      if (types.count < 3) throw Error('FIXTURE: the card pills never rendered');
      const wrong = types.types.filter(type => type !== 'button');
      if (wrong.length > 0) throw Error(`${wrong.length} pill(s) have type=${JSON.stringify(wrong[0])}`);
      await clickRust();
      const path = await waitFor(`(${filterPill}.length === 1) ? location.pathname : null`, 6000);
      if (path !== '/project-memories') throw Error(`activation navigated to ${path}`);
      return `type=button on ${types.count} pills, path unchanged`;
    });

    finishLane('PILL_CONTRACT');
  };

  const runDialogLane = async () => {
    await call('Page.navigate', {url: base + '/'});
    await waitFor(`location.origin === ${JSON.stringify(base)} && document.readyState === 'complete'`);
    await laneSetSpec(DIALOG_SPEC);
    const settingsReady = `document.querySelectorAll('[data-testid="maintenance-action"]').length`;
    const sessionsReady = `document.querySelectorAll('[data-testid="delete-session"]').length`;
    const skillsReady = `document.querySelectorAll('[data-testid="skill-delete"]').length`;
    const armCleanup = async () => {
      await laneGoto('/settings', settingsReady);
      await clickAt('[data-testid="maintenance-action"]');
      const armed = await waitFor(`document.querySelectorAll('[data-testid="maintenance-armed"]').length === 1`, 3000);
      if (!armed) throw Error('FIXTURE: the cleanup arm notice never appeared');
      return armed;
    };
    const maintenanceCancel = async () => {
      const pick = await evaluate(`(() => {
        const dialog = document.querySelectorAll('[data-testid="maintenance-dialog-cancel"]');
        if (dialog.length === 1) return {selector: '[data-testid="maintenance-dialog-cancel"]'};
        const inline = document.querySelectorAll('[data-testid="maintenance-cancel"]');
        if (inline.length === 1) return {selector: '[data-testid="maintenance-cancel"]'};
        return {selector: null, dialog: dialog.length, inline: inline.length};
      })()`);
      if (!pick.selector) {
        throw Error(`FIXTURE: no maintenance cancel control resolved (dialog=${pick.dialog} inline=${pick.inline})`);
      }
      await clickAt(pick.selector);
      return pick.selector;
    };

    await laneRun('dialog_role_and_name', 'RED', 'memory, session, skill rows', async () => {
      const sites = [
        ['/project-memories', '[data-testid="memory-delete"]', armRows('/project-memories')],
        ['/sessions', '[data-testid="delete-session"]', sessionsReady],
        ['/skill-library', '[data-testid="skill-delete"]', skillsReady],
      ];
      const details = [];
      for (const [route, selector, ready] of sites) {
        await laneGoto(route, ready);
        await clickAt(selector);
        await armGuard();
        // The accessible name is computed the way a screen reader does it:
        // `aria-labelledby` first (the title it points at), `aria-label` second.
        const seen = await evaluate(`(() => {
          const dialogs = [...document.querySelectorAll('[role="dialog"]')];
          if (dialogs.length !== 1) return {dialogs: dialogs.length};
          const dialog = dialogs[0];
          const title = dialog.querySelector('h2');
          const labelledby = (dialog.getAttribute('aria-labelledby') || '')
            .split(/\\s+/).filter(Boolean)
            .map(id => { const el = document.getElementById(id); return el ? el.textContent.trim() : ''; })
            .filter(Boolean).join(' ');
          const label = dialog.getAttribute('aria-label') || '';
          return {dialogs: 1, name: labelledby || label, label, labelledby,
                  attrs: [...dialog.attributes].map(a => a.name).join(' '),
                  title: title ? title.textContent.trim() : ''};
        })()`);
        if (seen.dialogs !== 1) throw Error(`${route}: ${seen.dialogs} [role="dialog"] element(s)`);
        if (!seen.name) {
          throw Error(`${route}: the dialog has no accessible name (attributes: ${JSON.stringify(seen.attrs)})`);
        }
        if (seen.name !== seen.title) {
          throw Error(`${route}: accessible name ${JSON.stringify(seen.name)} != title ${JSON.stringify(seen.title)}`);
        }
        details.push(`${route}:${JSON.stringify(seen.name)}`);
        await cancelArmed();
      }
      return details.join(' ');
    });

    await laneRun('dialog_escape_disarms', 'RED', 'memory row, Escape', async () => {
      await armMemoryDelete();
      await evaluate('window.__laneCalls = []');
      await pressKey('Escape', 'Escape', 27);
      const gone = await waitFor(`(${armedCount}) === 0`, 3000);
      if (!gone) throw Error('the armed gate/dialog remains after Escape');
      const usable = await evaluate(`(() => {
        const button = document.querySelector('[data-testid="memory-delete"]');
        return button ? {present: true, disabled: button.disabled} : {present: false};
      })()`);
      if (!usable.present) throw Error('the row Delete button did not come back');
      if (usable.disabled) throw Error('the row Delete button is disabled after Escape');
      const deletes = await laneDeleteCalls();
      if (deletes !== 0) throw Error(`Escape issued ${deletes} DELETE request(s)`);
      return 'surface gone, Delete usable, no DELETE issued';
    });

    await laneRun('dialog_focus_trapped_forward_and_reverse', 'RED', 'memory row, Tab and Shift-Tab', async () => {
      await armMemoryDelete();
      const last = await evaluate(`(() => {
        const button = document.querySelector('[data-testid="confirm-gate-cancel"]');
        if (!button) return false;
        button.focus();
        return document.activeElement === button;
      })()`);
      if (!last) throw Error('FIXTURE: the last action could not take focus');
      await pressKey('Tab', 'Tab', 9);
      const forward = await evaluate(focusInsideArmed);
      await evaluate(`document.querySelector('[data-testid="confirm-gate-confirm"]').focus()`);
      await pressKey('Tab', 'Tab', 9, 8);
      const reverse = await evaluate(focusInsideArmed);
      if (!forward) throw Error('Tab from the last action left the armed surface');
      if (!reverse) throw Error('Shift-Tab from the first action left the armed surface');
      return 'forward and reverse Tab stayed inside';
    });

    await laneRun('dialog_focus_returns_to_invoker', 'RED', 'memory row, Cancel', async () => {
      await laneGoto('/project-memories', armRows('/project-memories'));
      const marked = await evaluate(`(() => {
        const button = document.querySelector('[data-testid="memory-delete"]');
        if (!button) return false;
        button.setAttribute('data-lane-invoker', 'memory-delete');
        return true;
      })()`);
      if (!marked) throw Error('FIXTURE: the memory Delete button never rendered');
      await clickAt('[data-testid="memory-delete"]');
      await armGuard();
      await clickAt('[data-testid="confirm-gate-cancel"]');
      const back = await waitFor(`(() => {
        const active = document.activeElement;
        return Boolean(active && active.getAttribute('data-lane-invoker') === 'memory-delete');
      })()`, 3000);
      if (!back) {
        const where = await evaluate(`(() => {
          const active = document.activeElement;
          return active ? active.tagName + ' ' + (active.getAttribute('data-testid') || '') : 'none';
        })()`);
        throw Error(`focus went to ${where}, not the original Delete node`);
      }
      return 'focus returned to the same Delete node';
    });

    await laneRun('confirm_removed_invoker_focus_fallback', 'RED', 'DELETE held, row removed', async () => {
      await laneGoto('/project-memories', armRows('/project-memories'));
      await clickAt('[data-testid="memory-delete"]');
      await armGuard();
      await evaluate("window.__laneHold = 'delete'");
      await clickAt('[data-testid="confirm-gate-confirm"]');
      const held = await waitFor(`window.__laneCalls.filter(c => c.method === 'DELETE').length === 1`, 3000);
      if (!held) throw Error('FIXTURE: the held DELETE never reached the fixture');
      await evaluate('window.__laneRelease()');
      const removed = await waitFor(goneExpr('[data-testid="memory-row"]'), 5000);
      if (!removed) throw Error('FIXTURE: the deleted row never left the list');
      const fallback = await waitFor(`(() => {
        const active = document.activeElement;
        return Boolean(active && active.getAttribute('data-testid') === 'memories-reload');
      })()`, 3000);
      if (!fallback) {
        const where = await evaluate(`document.activeElement ? document.activeElement.tagName : 'none'`);
        throw Error(`focus stayed on ${where}, not the memories Reload fallback`);
      }
      return 'focus on [data-testid=memories-reload]';
    });

    await laneRun('maintenance_confirm_in_dialog', 'RED', 'cleanup armed', async () => {
      await armCleanup();
      const found = await evaluate(`(() => {
        const dialogs = [...document.querySelectorAll('[role="dialog"]')];
        const confirm = document.querySelectorAll('[data-testid="maintenance-dialog-confirm"]').length;
        const inDialog = dialogs.some(d => d.querySelector('[data-testid="maintenance-dialog-confirm"]'));
        return {dialogs: dialogs.length, confirm, inDialog};
      })()`);
      if (found.dialogs === 0 || !found.inDialog) {
        throw Error(`no dialog holds the final confirm (dialogs=${found.dialogs} confirm=${found.confirm})`);
      }
      const detail = `dialogs=${found.dialogs} confirm=${found.confirm}`;
      await maintenanceCancel();
      return detail;
    });

    await laneRun('maintenance_focus_returns', 'RED', 'cleanup armed, cancel then Escape', async () => {
      await armCleanup();
      await maintenanceCancel();
      const settled = await waitFor(`(() => {
        const dialog = document.querySelectorAll('[role="dialog"]').length;
        const action = document.querySelector('[data-testid="maintenance-action"]');
        return (dialog === 0 && action && !action.disabled) ? {dialog, disabled: action.disabled} : null;
      })()`, 5000);
      if (!settled) throw Error('FIXTURE: the dialog never closed with the action re-enabled');
      const back = await waitFor(`(() => {
        const active = document.activeElement;
        return Boolean(active && active.getAttribute('data-testid') === 'maintenance-action');
      })()`, 3000);
      if (!back) {
        const where = await evaluate(`document.activeElement ? document.activeElement.tagName : 'none'`);
        throw Error(`focus stayed on ${where}, not the maintenance action`);
      }
      const esc = await evaluate(`document.querySelectorAll('[role="dialog"]').length`);
      return `focus on maintenance-action (esc path: dialog present=${esc > 0})`;
    });

    await laneRun('maintenance_inline_action_inert_while_armed', 'RED', 'cleanup armed', async () => {
      await armCleanup();
      const inert = await evaluate(`(() => {
        const action = document.querySelector('[data-testid="maintenance-action"]');
        return action ? {disabled: action.disabled, label: action.textContent.trim()} : null;
      })()`);
      if (!inert) throw Error('FIXTURE: the maintenance action never rendered');
      if (inert.disabled !== true) throw Error(`the inline action is still enabled while armed (${inert.label})`);
      const detail = `disabled while armed (${inert.label})`;
      await maintenanceCancel();
      return detail;
    });

    await laneRun('dialog_cancel_disarms', 'CONTROL', 'memory row, Cancel', async () => {
      await armMemoryDelete();
      await evaluate('window.__laneCalls = []');
      await clickAt('[data-testid="confirm-gate-cancel"]');
      const gone = await waitFor(`(${armedCount}) === 0`, 3000);
      if (!gone) throw Error('the armed surface remains after Cancel');
      const deletes = await laneDeleteCalls();
      if (deletes !== 0) throw Error(`Cancel issued ${deletes} DELETE request(s)`);
      return 'surface gone, no DELETE issued';
    });

    await laneRun('dialog_mask_does_not_close', 'CONTROL', 'memory row, outside click', async () => {
      await armMemoryDelete();
      const spot = await evaluate(`(() => {
        const box = document.querySelector('[role="dialog"]') ||
          document.querySelector('[data-testid="confirm-gate"]');
        if (!box) return null;
        const r = box.getBoundingClientRect();
        const y = r.bottom + 60 < innerHeight ? r.bottom + 60 : Math.max(8, r.top - 60);
        const x = Math.min(innerWidth - 8, Math.max(8, r.left + r.width / 2));
        const hit = document.elementFromPoint(x, y);
        return {x, y, hit: hit ? hit.tagName + '.' + hit.className : 'none'};
      })()`);
      if (!spot) throw Error('FIXTURE: the armed surface never rendered');
      await evaluate('window.__laneCalls = []');
      await clickPoint(spot.x, spot.y);
      await sleep(250);
      const surface = await evaluate(armedCount);
      if (surface !== 1) throw Error(`the confirmation closed on an outside click (surface=${surface})`);
      const deletes = await laneDeleteCalls();
      if (deletes !== 0) throw Error(`the outside click issued ${deletes} DELETE request(s)`);
      const path = await evaluate('location.pathname');
      if (path !== '/project-memories') throw Error(`the outside click navigated to ${path}`);
      return `clicked ${spot.x},${spot.y} on ${spot.hit}; surface kept`;
    });

    await laneRun('dialog_confirm_executes_exactly_once', 'CONTROL', 'DELETE held, Confirm twice', async () => {
      await armMemoryDelete();
      await evaluate("window.__laneHold = 'delete'");
      await evaluate('window.__laneCalls = []');
      await clickAt('[data-testid="confirm-gate-confirm"]');
      await clickAt('[data-testid="confirm-gate-confirm"]').catch(() => {});
      await sleep(300);
      const deletes = await laneDeleteCalls();
      await evaluate('window.__laneRelease && window.__laneRelease()').catch(() => {});
      if (deletes !== 1) throw Error(`${deletes} DELETE requests for one confirmation`);
      return 'exactly one DELETE';
    });

    await laneRun('confirm_navigation_does_not_steal_focus', 'CONTROL', 'focus moved before resolve', async () => {
      await armMemoryDelete();
      await evaluate("window.__laneHold = 'delete'");
      await clickAt('[data-testid="confirm-gate-confirm"]');
      const held = await waitFor(`window.__laneCalls.filter(c => c.method === 'DELETE').length === 1`, 3000);
      if (!held) throw Error('FIXTURE: the held DELETE never reached the fixture');
      const moved = await evaluate(`(() => {
        const button = document.querySelector('[data-testid="memories-reload"]');
        if (!button) return false;
        button.focus();
        return document.activeElement === button;
      })()`);
      if (!moved) throw Error('FIXTURE: the Reload control could not take focus');
      await evaluate('window.__laneRelease()');
      await waitFor(goneExpr('[data-testid="memory-row"]'), 5000);
      await sleep(250);
      const kept = await evaluate(`(() => {
        const active = document.activeElement;
        return Boolean(active && active.getAttribute('data-testid') === 'memories-reload');
      })()`);
      if (!kept) throw Error('the async completion stole focus from the moved control');
      return 'focus stayed on memories-reload';
    });

    await laneRun('maintenance_dialog_preserves_notice', 'CONTROL', 'cleanup armed then cancelled', async () => {
      await armCleanup();
      const notice = await evaluate(`(() => {
        const el = document.querySelector('[data-testid="maintenance-armed"]');
        return el ? el.textContent.trim() : null;
      })()`);
      if (!notice) throw Error('FIXTURE: the armed notice carries no text');
      await evaluate('window.__laneCalls = []');
      const control = await maintenanceCancel();
      const idle = await waitFor(goneExpr('[data-testid="maintenance-armed"]'), 3000);
      if (!idle) throw Error('the armed notice remains after cancelling');
      const posts = (await laneCalls()).filter(entry => entry.startsWith('POST ') && entry.includes('/api/cleanup'));
      if (posts.length !== 0) throw Error(`cancel issued ${posts.length} POST /api/cleanup`);
      return `notice via ${control}, no POST`;
    });

    await laneRun('maintenance_exactly_one_cleanup_post', 'CONTROL', 'every confirm path in turn', async () => {
      await armCleanup();
      await evaluate('window.__laneCalls = []');
      if (laneExpect === 'pre') {
        // Pre-change the inline action IS the only confirm path (blueprint
        // Step 6: "pre: the inline action") — click it once and require
        // exactly one POST /api/cleanup. The post-side probe logic below must
        // never run here: pre-change the inline action is enabled and nothing
        // covers it, so "probe issued 0 POST" would fail a correct tree.
        await clickAt('[data-testid="maintenance-action"]');
        await sleep(250);
        const posts = (await laneCalls()).filter(entry => entry.startsWith('POST ') && entry.includes('/api/cleanup'));
        if (posts.length !== 1) throw Error(`${posts.length} POST /api/cleanup from the inline action (pre)`);
        return 'exactly one POST /api/cleanup via the inline action (pre: no dialog)';
      }
      // Post: modal backdrop covers the inline action — the probe click only
      // fires when the action is really reachable, and either way this path
      // must issue no POST; the dialog confirm is the one confirm path.
      const probe = await evaluate(`(() => {
        const action = document.querySelector('[data-testid="maintenance-action"]');
        if (!action) return {why: 'absent'};
        action.scrollIntoView({block: 'center'});
        const r = action.getBoundingClientRect();
        const x = r.left + r.width / 2;
        const y = r.top + r.height / 2;
        const hit = document.elementFromPoint(x, y);
        return {x, y, disabled: action.disabled,
                reachable: Boolean(hit && (action === hit || action.contains(hit)))};
      })()`);
      if (probe.why) throw Error('FIXTURE: the maintenance action never rendered');
      if (probe.reachable) await clickPoint(probe.x, probe.y);
      await sleep(250);
      const probed = (await laneCalls()).filter(entry => entry.startsWith('POST ') && entry.includes('/api/cleanup')).length;
      if (probed !== 0) throw Error(`the inline action issued ${probed} POST /api/cleanup`);
      const dialogConfirm = await evaluate(`document.querySelectorAll('[data-testid="maintenance-dialog-confirm"]').length`);
      if (dialogConfirm === 1) {
        await clickAt('[data-testid="maintenance-dialog-confirm"]');
        await clickAt('[data-testid="maintenance-dialog-confirm"]').catch(() => {});
      }
      const counted = await waitFor(`window.__laneCalls.filter(c => c.method === 'POST' && c.url.includes('/api/cleanup')).length`, 3000);
      const posts = (await laneCalls()).filter(entry => entry.startsWith('POST ') && entry.includes('/api/cleanup'));
      if (posts.length !== 1) {
        throw Error(`${posts.length} POST /api/cleanup (dialog confirm present=${dialogConfirm}, first=${counted})`);
      }
      return `exactly one POST /api/cleanup (dialog confirm present=${dialogConfirm}, inline probe ${probe.reachable ? 'fired' : 'covered'})`;
    });

    finishLane('DIALOG');
  };

  try {
    if (pillsOnly) {
      const {identifier} = await call('Page.addScriptToEvaluateOnNewDocument', {source: fixtureSource});
      try {
        await runPillLane();
      } finally {
        await call('Page.removeScriptToEvaluateOnNewDocument', {identifier});
      }
      return;
    }
    if (dialogOnly) {
      const {identifier} = await call('Page.addScriptToEvaluateOnNewDocument', {source: fixtureSource});
      try {
        await runDialogLane();
      } finally {
        await call('Page.removeScriptToEvaluateOnNewDocument', {identifier});
      }
      return;
    }
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
