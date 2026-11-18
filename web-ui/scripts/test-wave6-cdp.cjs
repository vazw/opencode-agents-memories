'use strict';
// Issue 118 wave 6 CDP lanes (G3 ×6, G4 ×2, G5 ×5 = 13 gate rows), plus issue
// 140's thaw-adoption rows in G6 (5 cases: table geometry, table identity,
// progress contracts, native selects, clean render).
// Bootstrap follows test-wave4-cdp.cjs: a dedicated CDP target (LANE_PAGE_ID),
// Runtime.evaluate helper, Page.addScriptToEvaluateOnNewDocument fetch
// override, serve-dist.mjs static server. The fixture answers /api/memories
// (pinned total 6221 / pageSize 8 — the sourced range constant), /api/search
// (total 42), /api/tags, /api/stats (8 daily rows + 30 shards), /api/projects,
// /api/health and /api/user-profile; /api/user-profiles feeds the shell's
// profile picker. window.__requests records {path, query, method, status} per
// /api/*; console errors and uncaught exceptions land in `pageErrors`.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const fixturePath = path.join(__dirname, 'fixtures/issue-118-wave6.json');
const fixture = JSON.parse(fs.readFileSync(fixturePath));
const base = process.env.TEST_BASE_URL || 'http://127.0.0.1:49418';
const selection = process.env.W6_TEST || 'all';
const output = process.env.W6_OUTPUT_DIR || path.join('.tmp/artifacts/issue-118-wave6', new Date().toISOString().replace(/[:.]/g, '-'));
const dist = process.env.WEBUI_DIST;
const sha = bytes => crypto.createHash('sha256').update(bytes).digest('hex');
const q = id => `[data-testid="${id}"]`;
const results = [];
let call, evaluate, initId;
// Console errors and uncaught exceptions seen since the last navigate(). Fed by
// the Runtime.* events below; `stats_renders_clean` asserts it stays empty.
let pageErrors = [];
// Pacing lives in Node, never in a Runtime.evaluate roundtrip: a slow CDP
// roundtrip would stretch the "50 ms apart" keystrokes past the 300 ms
// debounce window and turn the burst into N legitimate fires.
const sleep = ms => new Promise(r => setTimeout(r, ms));
const settleFrame = () => evaluate('new Promise(r=>requestAnimationFrame(()=>setTimeout(r,20)))');

async function ready(selector) {
  return evaluate(`new Promise((resolve,reject)=>{let clean=0;const samples=[];const start=Date.now();const timer=setInterval(()=>{const ok=!!document.querySelector(${JSON.stringify(selector)})&&!document.querySelector('.thaw-spinner,[data-testid$="-loading"]');samples.push({elapsed:Date.now()-start,clean});clean=ok?clean+1:0;if(clean===3){clearInterval(timer);resolve(samples)}else if(Date.now()-start>20000){clearInterval(timer);reject(Error('readiness timeout '+${JSON.stringify(selector)}+' | '+document.body.innerText.slice(0,120)))}},100)})`);
}
// The memories lanes gate on the pinned range text: the listing fetch has
// answered and its projection rendered.
async function readyMemories() {
  return evaluate(`new Promise((resolve,reject)=>{const start=Date.now();const timer=setInterval(()=>{const el=document.querySelector('[data-testid="memories-range"]');if(el&&/of 6221$/.test(el.textContent.trim())){clearInterval(timer);resolve(Date.now()-start)}else if(Date.now()-start>20000){clearInterval(timer);reject(Error('memories readiness timeout | '+document.body.innerText.slice(0,120)))}},100)})`);
}
function install(scenario) {
  return `(()=>{const f=${JSON.stringify(fixture)};window.__scenario=${JSON.stringify(scenario)};window.__requests=[];window.__OPENCODE_MEM_TOKEN__='synthetic-wave6-marker';const original=window.fetch.bind(window);window.fetch=async(input,options={})=>{const u=new URL(typeof input==='string'?input:input.url,location.href);if(!u.pathname.startsWith('/api/'))return original(input,options);const method=options.method||'GET';const headers=new Headers(options.headers);let data,status=200;if(u.pathname==='/api/memories')data=f.memories;else if(u.pathname==='/api/search')data=f.search;else if(u.pathname==='/api/tags')data=f.tags;else if(u.pathname==='/api/stats')data=f.stats;else if(u.pathname==='/api/projects')data=f.projects;else if(u.pathname==='/api/health')data=f.health;else if(u.pathname==='/api/user-profile')data=f.profile;else if(u.pathname==='/api/user-profiles')data=f.roster;else if(u.pathname==='/api/cleanup/status'&&method==='GET'){if(window.__scenario==='degraded'){status=501;data={success:false,error:'the endpoint is not available in this build'}}else data=window.__ranOnce?f.cleanupStatus:window.__scenario==='never'?f.cleanupStatusNever:window.__scenario==='auto-running'?f.cleanupStatusRunning:f.cleanupStatus}else if(u.pathname==='/api/cleanup'&&method==='POST'){window.__ranOnce=true;data=f.cleanupReport}else{status=404;data={success:false}}window.__requests.push({path:u.pathname,query:u.search,method,tokenPresent:headers.get('x-opencode-mem-token')==='synthetic-wave6-marker',status});await new Promise(r=>setTimeout(r,30));return new Response(JSON.stringify(u.pathname==='/api/health'?data:{success:true,data}),{status,headers:{'Content-Type':'application/json'}})};})()`;
}
async function navigate(route, scenario = 'normal', width = 1280, theme = 'light') {
  pageErrors = [];
  if (initId) await call('Page.removeScriptToEvaluateOnNewDocument', { identifier: initId });
  ({ identifier: initId } = await call('Page.addScriptToEvaluateOnNewDocument', { source: install(scenario) + `;localStorage.setItem('opencode-mem-theme',${JSON.stringify(theme)});` }));
  await call('Emulation.setDeviceMetricsOverride', { width, height: 800, deviceScaleFactor: 1, mobile: false });
  await call('Page.navigate', { url: base + route });
  await evaluate(`new Promise(r=>{if(document.readyState==='complete')r();else addEventListener('load',r,{once:true})})`);
}
async function realKey(key, code, vk, text) {
  await call('Input.dispatchKeyEvent', { type: 'keyDown', key, code, windowsVirtualKeyCode: vk, ...(text ? { text } : {}) });
  await call('Input.dispatchKeyEvent', { type: 'keyUp', key, code, windowsVirtualKeyCode: vk });
}
async function click(selector) {
  await evaluate(`(()=>{const e=document.querySelector(${JSON.stringify(selector)});if(!e)throw Error('missing '+${JSON.stringify(selector)});e.click()})()`);
}
async function nested(selector) {
  return evaluate(`(()=>{const root=document.querySelector(${JSON.stringify(selector)});if(!root)throw Error('missing '+${JSON.stringify(selector)});const el=root.matches('input,textarea')?root:root.querySelector('input,textarea');if(!el)throw Error('no control inside '+${JSON.stringify(selector)});return el.tagName})()`);
}
async function input(selector, value) {
  await nested(selector);
  await evaluate(`(()=>{const root=document.querySelector(${JSON.stringify(selector)});const el=root.matches('input,textarea')?root:root.querySelector('input,textarea');el.value=${JSON.stringify(value)};el.dispatchEvent(new Event('input',{bubbles:true}))})()`);
}
const searchCalls = async from => evaluate(`window.__requests.slice(${from}).filter(r=>r.path==='/api/search')`);
const errorsSurface = `.stats-errors-surface`;
// The errors trigger lives in the first daily row that has buckets (the
// fixture gives every even row one); the lane opens THAT popover.
async function openErrorsPopover() {
  await click(q('stats-errors-trigger'));
  await sleep(120);
}
const cases = {
  // --- G3: the search row -------------------------------------------------
  async search_field_is_bounded() {
    await navigate('/project-memories');
    await readyMemories();
    const rects = await evaluate(`(()=>{const f=document.querySelector('[data-testid="memories-search"]').getBoundingClientRect();const p=document.querySelector('[data-testid="memories-panel"]').getBoundingClientRect();return{field:f.width,panel:p.width}})()`);
    assert.ok(rects.field <= 360, `field ${rects.field} > 360`);
    assert.ok(rects.field <= 0.5 * rects.panel, `field ${rects.field} > half panel ${0.5 * rects.panel}`);
    return rects;
  },
  async search_debounce_one_request() {
    await navigate('/project-memories');
    await readyMemories();
    const before = await evaluate('window.__requests.length');
    for (const value of ['r', 'ru', 'rus', 'rust', 'rust']) {
      await input(q('memories-search'), value);
      await sleep(50);
    }
    await sleep(700); // settle past the 300 ms timer
    const calls = await searchCalls(before);
    assert.equal(calls.length, 1, `expected exactly 1 search request, got ${calls.length}: ${JSON.stringify(await evaluate(`window.__requests.slice(${before})`))}`);
    return calls;
  },
  async search_clear_affordance_is_reactive() {
    await navigate('/project-memories');
    await readyMemories();
    const count = () => evaluate(`document.querySelectorAll('[data-testid="memories-search-clear"]').length`);
    assert.equal(await count(), 0, 'empty box hides the clear button');
    await input(q('memories-search'), 'rust');
    await sleep(100);
    assert.equal(await count(), 1, 'typed text shows the clear button');
    await click(q('memories-search-clear'));
    await sleep(100);
    assert.equal(await count(), 0, 'cleared box hides the clear button again');
  },
  async search_clear_slot_is_reserved() {
    await navigate('/project-memories');
    await readyMemories();
    const state = () => evaluate(`(()=>{const input=document.querySelector('[data-testid="memories-search"] input');const btn=document.querySelector('[data-testid="memories-search-clear"]');return{inputWidth:input?input.clientWidth:null,btn:btn?{width:btn.getBoundingClientRect().width}:null}})()`);
    const empty = await state();
    await input(q('memories-search'), 'rust');
    await sleep(100);
    const typed = await state();
    assert.ok(typed.btn, 'clear button must exist once text is typed (today it does not)');
    assert.equal(empty.inputWidth, typed.inputWidth, `input clientWidth must not move when the clear button appears (${empty.inputWidth} -> ${typed.inputWidth})`);
    assert.ok(typed.btn.width <= 28, `clear button ${typed.btn.width} must fit the reserved 28px slot (the > button override)`);
    return { empty, typed };
  },
  async search_clear_restores_list() {
    await navigate('/project-memories');
    await readyMemories();
    await input(q('memories-search'), 'rust');
    await sleep(500); // debounce fires once
    const before = await evaluate('window.__requests.length');
    await click(q('memories-search-clear'));
    await sleep(400);
    const added = await evaluate(`window.__requests.slice(${before})`);
    assert.equal(added.length, 1, `one clear must mean one request, got ${added.length}`);
    assert.equal(added[0].path, '/api/memories', 'the clear request is the unfiltered listing');
    assert.ok(!added[0].query.includes('q='), `the clear request carries no q: ${added[0].query}`);
    const range = await evaluate(`document.querySelector('[data-testid="memories-range"]').textContent`);
    assert.equal(range.trim(), '1-8 of 6221', `range restored to the pinned constant, got "${range}"`);
    return added;
  },
  async search_result_reflow_pinned() {
    await navigate('/project-memories');
    await readyMemories();
    const reloadLeft = () => evaluate(`document.querySelector('[data-testid="memories-reload"]').getBoundingClientRect().left`);
    const before = await reloadLeft();
    const preRequests = await evaluate('window.__requests.length');
    await input(q('memories-search'), 'rust');
    await sleep(700);
    const calls = await searchCalls(preRequests);
    assert.equal(calls.length, 1, `exactly one search request, got ${calls.length}`);
    const after = await reloadLeft();
    assert.ok(Math.abs(after - before) <= 1, `Reload must stay pinned: ${before} -> ${after}`);
    return { before, after };
  },
  // --- G4: the stats cards ------------------------------------------------
  async stats_cards_equal_height_top() {
    await navigate('/stats');
    await ready(q('stats-card-total'));
    const rects = await evaluate(`Array.from(document.querySelectorAll('[data-testid^="stats-card-"]'),c=>{const r=c.getBoundingClientRect();return{top:r.top,height:r.height}})`);
    assert.equal(rects.length, 4, 'four overview cards');
    const tops = rects.map(r => r.top);
    const heights = rects.map(r => r.height);
    assert.ok(Math.max(...tops) - Math.min(...tops) <= 1, `tops spread ${Math.max(...tops) - Math.min(...tops)}: ${tops}`);
    assert.ok(Math.max(...heights) - Math.min(...heights) <= 1, `heights spread ${Math.max(...heights) - Math.min(...heights)}: ${heights}`);
    return rects;
  },
  async stats_generated_single_line() {
    await navigate('/stats');
    await ready(q('stats-card-total'));
    // Line count via a Range over the value's text node: distinct rounded
    // rect.top values. getClientRects().length is structurally 1 on a block
    // <p> and could never fail.
    const lines = await evaluate(`(()=>{const p=document.querySelector('[data-testid="stats-card-generated"] .stats-generated');if(!p)return null;const node=p.firstChild;const range=document.createRange();range.selectNodeContents(node);const tops=new Set(Array.from(range.getClientRects(),r=>Math.round(r.top)));return{lines:tops.size,text:p.textContent,width:p.getBoundingClientRect().width}})()`);
    assert.ok(lines, 'generated value exists');
    assert.equal(lines.lines, 1, `generated label must be one line, got ${lines.lines} (${JSON.stringify(lines)})`);
    return lines;
  },
  // --- G5: the errors badge + popover -------------------------------------
  async errors_badge_is_thaw_badge() {
    await navigate('/stats');
    await ready(q('stats-card-total'));
    const badge = await evaluate(`(()=>{const b=document.querySelector('[data-testid="stats-errors-trigger"] .thaw-badge');if(!b)return null;return{bg:getComputedStyle(b).backgroundColor,text:b.textContent}})()`);
    assert.ok(badge, 'the trigger must carry a thaw Badge (a <details> summary has none)');
    assert.notEqual(badge.bg, 'rgba(0, 0, 0, 0)', 'the badge is painted (tint), not transparent');
    return badge;
  },
  async errors_popover_no_reflow() {
    await navigate('/stats');
    await ready(q('stats-card-total'));
    const metrics = () => evaluate(`(()=>{const t=document.querySelector('[data-testid="stats-daily-table"]');const row=t.querySelector('tbody tr');const r=row.getBoundingClientRect();return{tableScroll:t.scrollHeight,rowHeight:r.height}})()`);
    const before = await metrics();
    await openErrorsPopover();
    const after = await metrics();
    assert.equal(after.tableScroll, before.tableScroll, `daily table scrollHeight must not move: ${before.tableScroll} -> ${after.tableScroll}`);
    assert.equal(after.rowHeight, before.rowHeight, `daily row height must not move: ${before.rowHeight} -> ${after.rowHeight}`);
    return { before, after };
  },
  async errors_popover_shows_buckets() {
    await navigate('/stats');
    await ready(q('stats-card-total'));
    await openErrorsPopover();
    const state = await evaluate(`(()=>{const s=document.querySelector('${errorsSurface}');if(!s)return null;return{display:getComputedStyle(s).display,text:s.textContent}})()`);
    assert.ok(state, 'the errors surface must exist');
    assert.notEqual(state.display, 'none', 'the plate must be visible after activation');
    assert.ok(state.text.includes('fixture:timeout'), `bucket name rendered: ${state.text}`);
    assert.ok(state.text.includes('×'), `bucket count rendered: ${state.text}`);
    return state;
  },
  async errors_popover_keyboard_and_escape() {
    await navigate('/stats');
    await ready(q('stats-card-total'));
    await evaluate(`document.querySelector('[data-testid="stats-errors-trigger"]').focus()`);
    assert.equal(await evaluate(`document.activeElement.getAttribute('data-testid')`), 'stats-errors-trigger', 'focus reaches the trigger');
    await realKey('Enter', 'Enter', 13, '\r');
    await sleep(120);
    assert.equal(await evaluate(`document.querySelector('[data-testid="stats-errors-trigger"]').getAttribute('aria-expanded')`), 'true', 'Enter opens the popover');
    assert.notEqual(await evaluate(`getComputedStyle(document.querySelector('${errorsSurface}')).display`), 'none', 'plate displayed after Enter');
    await realKey('Escape', 'Escape', 27);
    await settleFrame(); // the plate's display settles one frame after the keydown
    assert.equal(await evaluate(`document.querySelector('[data-testid="stats-errors-trigger"]').getAttribute('aria-expanded')`), 'false', 'Escape closes the popover');
    assert.equal(await evaluate(`getComputedStyle(document.querySelector('${errorsSurface}')).display`), 'none', 'plate display none after Escape');
    assert.equal(await evaluate(`document.querySelector('${errorsSurface}').getBoundingClientRect().height`), 0, 'plate rect height 0 after Escape');
    // The plate stays mounted (thaw Teleport) — never assert node counts here.
  },
  async errors_popover_placement_scrolled() {
    await navigate('/stats');
    await ready(q('stats-card-total'));
    await openErrorsPopover();
    const dy = () => evaluate(`(()=>{const s=document.querySelector('${errorsSurface}').getBoundingClientRect();const b=document.querySelector('[data-testid="stats-errors-trigger"] .thaw-badge').getBoundingClientRect();return Math.abs(s.top-b.top)})()`);
    const d0 = await dy();
    assert.ok(d0 <= 8, `|dy| at scrollY 0 must be <= 8, got ${d0}`);
    await evaluate('window.scrollTo(0,402)');
    await sleep(120);
    const scrollY = await evaluate('window.scrollY');
    assert.equal(scrollY, 402, `fixture must scroll to exactly 402 (maxScroll >= 402), got ${scrollY}`);
    const d1 = await dy();
    assert.ok(d1 <= 8, `|dy| at scrollY 402 must be <= 8, got ${d1}`);
    return { d0, d1 };
  },
  // --- G6: thaw adoption, /stats (issue 140) ------------------------------
  // Every post-navigate query opens on `ready()`: at the instant `navigate()`
  // resolves the stats landmarks are null 15/15, and an empty page would
  // otherwise satisfy the request assertions vacuously.
  async stats_table_geometry() {
    await navigate('/stats');
    await ready(q('stats-card-total'));
    // The scroll pairs are RETURNED, never asserted: under thaw's
    // `table-layout: fixed` the columns clip instead of widening, so a
    // scrollWidth diff cannot red the mutant, and it would red the wanted
    // `auto` (a long shard cell widens the table). `tableLayout` is the oracle.
    const geom = await evaluate(`(()=>{const pair=el=>({scrollWidth:el.scrollWidth,clientWidth:el.clientWidth});const daily=document.querySelector('.stats-daily');const shards=document.querySelector('.stats-shards');const region=document.querySelector('.stats-shards-region');const ths=el=>Array.from(el.querySelectorAll('th'),t=>Math.round(t.getBoundingClientRect().width));return{layout:{daily:daily?getComputedStyle(daily).tableLayout:null,shards:shards?getComputedStyle(shards).tableLayout:null},daily:daily?pair(daily):null,shards:shards?pair(shards):null,shardRegion:region?pair(region):null,thWidths:{daily:daily?ths(daily):null,shards:shards?ths(shards):null}}})()`);
    assert.ok(geom.daily, 'the .stats-daily root must exist');
    assert.ok(geom.shards, 'the .stats-shards root must exist');
    assert.deepEqual(geom.layout, { daily: 'auto', shards: 'auto' }, `tableLayout ${JSON.stringify(geom.layout)}`);
    return { daily: geom.daily, shards: geom.shards, shardRegion: geom.shardRegion, thWidths: geom.thWidths, tableLayout: geom.layout };
  },
  async stats_tables_are_thaw_tables() {
    await navigate('/stats');
    await ready(q('stats-card-total'));
    const state = await evaluate(`(()=>{const read=el=>({tag:el.tagName,thawTable:el.classList.contains('thaw-table'),cells:el.querySelectorAll('.thaw-table-cell').length,rows:el.querySelectorAll('tbody tr').length});const daily=document.querySelector('[data-testid="stats-daily-table"]');const shards=document.querySelector('[data-testid="stats-shards-table"]');return{daily:daily?read(daily):null,shards:shards?read(shards):null,th:document.querySelectorAll('th').length,headerCells:document.querySelectorAll('.thaw-table-header-cell, .thaw-table-header-cell__button').length}})()`);
    assert.ok(state.daily, 'the daily testid must resolve (to the TABLE root, not a wrapper)');
    assert.ok(state.shards, 'the shards testid must resolve (to the TABLE root, not a wrapper)');
    for (const [label, t] of [['daily', state.daily], ['shards', state.shards]]) {
      assert.equal(t.tag, 'TABLE', `${label} root is ${t.tag}, not TABLE`);
      assert.ok(t.thawTable, `${label} root lacks thaw-table`);
    }
    assert.equal(state.daily.cells, 40, `.thaw-table-cell in the daily table: ${state.daily.cells}`);
    assert.equal(state.shards.cells, 210, `.thaw-table-cell in the shards table: ${state.shards.cells}`);
    assert.equal(state.daily.rows, 8, `daily tbody rows: ${state.daily.rows}`);
    assert.equal(state.shards.rows, 30, `shards tbody rows: ${state.shards.rows}`);
    assert.equal(state.th, 21, `th stays native at 21, got ${state.th}`);
    // TableHeaderCell renders a <th> too, so a th count cannot detect the swap:
    // the zero-count of its wrapper classes is the assertion.
    assert.equal(state.headerCells, 0, `thaw header-cell wrappers: ${state.headerCells}`);
    return state;
  },
  async stats_progress_contracts() {
    await navigate('/stats');
    await ready(q('stats-card-total'));
    await settleFrame(); // the inner bar's width transition (0.3s) must be done
    const state = await evaluate(`(()=>{const row=Array.from(document.querySelectorAll('.stats-day')).find(r=>r.querySelector('th')&&r.querySelector('th').textContent.trim()==='2026-11-08');if(!row)return{error:'no daily row for 2026-11-08'};const bars=Array.from(row.querySelectorAll('.stats-counter [role="progressbar"]'),b=>{const inner=b.querySelector('.thaw-progress-bar__bar');const r=b.getBoundingClientRect();return{label:b.getAttribute('aria-label'),now:b.getAttribute('aria-valuenow'),max:b.getAttribute('aria-valuemax'),height:r.height,width:r.width,innerWidth:inner?inner.getBoundingClientRect().width:null,innerStyle:inner?inner.getAttribute('style'):null}});return{bars,fontSize:parseFloat(getComputedStyle(document.documentElement).fontSize)}})()`);
    assert.ok(!state.error, state.error);
    assert.equal(state.bars.length, 4, `four progress bars in the 2026-11-08 row, got ${state.bars.length}`);
    assert.deepEqual(state.bars.map(b => b.label), ['2026-11-08 Captures', '2026-11-08 Skipped', '2026-11-08 Profile', '2026-11-08 Compaction'], `aria-label: ${JSON.stringify(state.bars.map(b => b.label))}`);
    const now = state.bars.map(b => Number(b.now));
    const max = state.bars.map(b => Number(b.max));
    // raw counts, never percentages: a screen reader announces these.
    assert.deepEqual(now, [4, 0, 2, 0], `aria-valuenow: ${JSON.stringify(state.bars.map(b => b.now))}`);
    assert.deepEqual(max, [11, 11, 11, 11], `aria-valuemax: ${JSON.stringify(state.bars.map(b => b.max))}`);
    const expectedHeight = 0.75 * state.fontSize;
    for (const [i, b] of state.bars.entries()) {
      assert.ok(Math.abs(b.height - expectedHeight) <= 0.5, `bar ${i} height ${b.height} != ${expectedHeight} (the .stats-progress cascade was lost)`);
      assert.ok(Math.abs(b.width - 128) <= 2, `bar ${i} width ${b.width} != 128`);
      assert.ok(Math.abs(b.innerWidth / b.width - now[i] / max[i]) <= 0.02, `bar ${i} inner ratio ${b.innerWidth}/${b.width} != ${now[i]}/${max[i]} (style ${b.innerStyle})`);
    }
    return { bars: state.bars, fontSize: state.fontSize };
  },
  async native_select_contracts() {
    // All three controls exist only because the shared fixture carries one
    // project and two roster rows; as shipped (projects: [], one-row roster)
    // the app mounts zero <select> on all three routes.
    const routes = [['/agents', 'agents-project-select'], ['/sessions', 'sessions-project-select'], ['/user-profile', 'profile-identity-select']];
    const selects = [];
    for (const [route, testid] of routes) {
      await navigate(route);
      await ready(q(testid)); // the testid is its own readiness landmark
      const state = await evaluate(`(()=>{const el=document.querySelector(${JSON.stringify(`select[data-testid="${testid}"]`)});return el?{tag:el.tagName,options:el.options.length}:null})()`);
      assert.ok(state, `no select[data-testid="${testid}"] on ${route}`);
      assert.equal(state.tag, 'SELECT', `${route} ${testid} is ${state.tag}`);
      selects.push({ route, testid, options: state.options });
    }
    return { selects };
  },
  // --- G7: the maintenance section's auto-cleanup status row (issue 141) ---
  async maintenance_status_renders_four_values() {
    // Mutant: the view renders empty cells or collapses the four facts.
    await navigate('/settings', 'normal');
    await ready(q('maintenance-section'));
    await ready(q('maintenance-auto-running'));
    const v = await evaluate(`(()=>{const t=id=>{const e=document.querySelector('[data-testid="'+id+'"]');return e?e.textContent.trim():null};return {enabled:t('maintenance-auto-enabled'),interval:t('maintenance-auto-interval'),last:t('maintenance-auto-last'),running:t('maintenance-auto-running')}})()`);
    assert.equal(v.enabled, 'on', `enabled=${v.enabled}`);
    assert.equal(v.interval, '86400 s', `interval=${v.interval}`);
    assert.notEqual(v.last, 'never', 'last must be a date for a stamped run');
    assert.ok(v.last && v.last.length > 0, 'last is empty');
    assert.equal(v.running, 'no', `running=${v.running}`);
    return { values: v };
  },
  async maintenance_status_never_ran() {
    // Mutant: `null` lastRunMs renders as "" or the epoch year.
    await navigate('/settings', 'never');
    await ready(q('maintenance-auto-last'));
    const last = await evaluate(`document.querySelector('[data-testid="maintenance-auto-last"]').textContent.trim()`);
    assert.equal(last, 'never', `last=${last}`);
    assert.ok(!/1970/.test(last), 'an absent stamp must not read as 1970');
    return { last };
  },
  async maintenance_status_unavailable_not_off() {
    // Mutant: the 501 renders as a silent `off`, or disables the action.
    await navigate('/settings', 'degraded');
    await ready(q('maintenance-auto-unavailable'));
    const state = await evaluate(`(()=>{const note=document.querySelector('[data-testid="maintenance-auto-unavailable"]');const enabled=document.querySelector('[data-testid="maintenance-auto-enabled"]');const action=document.querySelector('[data-testid="maintenance-action"]');return {note:note?note.textContent.trim():null,enabledPresent:!!enabled,actionDisabled:action?action.disabled:null}})()`);
    assert.ok(state.note && state.note.includes('not available in this build'), `note=${state.note}`);
    assert.equal(state.enabledPresent, false, 'no enabled value may render on 501');
    assert.equal(state.actionDisabled, false, 'an unavailable status must not disable the action');
    return state;
  },
  async maintenance_server_running_disables_action() {
    // Mutant: relabel-only — the control stays enabled while the server runs.
    await navigate('/settings', 'auto-running');
    await ready(q('maintenance-auto-running'));
    const state = await evaluate(`(()=>{const r=document.querySelector('[data-testid="maintenance-auto-running"]').textContent.trim();const action=document.querySelector('[data-testid="maintenance-action"]');return {running:r,disabled:action?action.disabled:null}})()`);
    assert.equal(state.running, 'yes', `running=${state.running}`);
    assert.equal(state.disabled, true, 'the action must be disabled while the server reports running');
    return state;
  },
  async cleanup_run_refreshes_status() {
    // Mutant: refetch unwired — status GET stays at one and `last` never flips.
    await navigate('/settings', 'never');
    await ready(q('maintenance-auto-last'));
    await click(q('maintenance-action'));
    await ready(q('maintenance-dialog-confirm'));
    await click(q('maintenance-dialog-confirm'));
    // Non-empty discriminator: the report row exists only after a Ran outcome.
    await ready(q('maintenance-report'));
    await sleep(150);
    const reqs = await evaluate(`window.__requests.map(r=>({path:r.path,method:r.method}))`);
    const posts = reqs.map((r,i)=>[r,i]).filter(([r])=>r.path==='/api/cleanup'&&r.method==='POST').map(([,i])=>i);
    const gets = reqs.map((r,i)=>[r,i]).filter(([r])=>r.path==='/api/cleanup/status'&&r.method==='GET').map(([,i])=>i);
    assert.ok(posts.length >= 1, `no POST /api/cleanup: ${JSON.stringify(reqs)}`);
    assert.ok(gets.some(i => i > posts[0]), `no status GET after the POST (stale status after a run): ${JSON.stringify(reqs)}`);
    const last = await evaluate(`document.querySelector('[data-testid="maintenance-auto-last"]').textContent.trim()`);
    assert.notEqual(last, 'never', 'last must flip from never after the run');
    return { posts, gets, last };
  },
  async settings_renders_clean() {
    // Mutant: a render panic or console error once the status row mounts. The
    // readiness gate waits for the new row, so an unwired bundle reds here too.
    await navigate('/settings', 'normal');
    await ready(q('maintenance-section'));
    await ready(q('maintenance-auto-running'));
    assert.equal(pageErrors.length, 0, `console errors/panics: ${pageErrors.join(' | ')}`);
    return { pageErrors: pageErrors.length };
  },
  async stats_renders_clean() {
    await navigate('/stats');
    await ready(q('stats-card-total'));
    const state = await evaluate(`({requests:window.__requests.slice(),rows:document.querySelectorAll('.stats-day').length})`);
    assert.equal(pageErrors.length, 0, `console errors/panics: ${pageErrors.join(' | ')}`);
    // The positive record and the row count are what stop a blank page from
    // passing: `every` over an empty array is true.
    assert.ok(state.requests.some(r => r.path === '/api/stats'), `no /api/stats request: ${JSON.stringify(state.requests)}`);
    assert.ok(state.requests.every(r => r.status < 400), `non-2xx request(s): ${JSON.stringify(state.requests.filter(r => !(r.status < 400)))}`);
    assert.ok(state.rows > 0, `no .stats-day rows rendered (rows=${state.rows})`);
    return { n: state.requests.length, stats: state.requests.filter(r => r.path === '/api/stats').map(r => r.status), rows: state.rows, pageErrors: pageErrors.length };
  },
};
(async () => {
  assert.ok(selection === 'all' || Object.hasOwn(cases, selection), 'unknown W6_TEST ' + selection);
  assert.ok(process.env.LANE_PAGE_ID, 'LANE_PAGE_ID must be the dedicated CDP target id');
  const pages = await (await fetch('http://127.0.0.1:9222/json')).json();
  const page = pages.find(p => p.id === process.env.LANE_PAGE_ID && p.type === 'page');
  assert.ok(page, 'dedicated target id not found; MCP ordinal is not a CDP target id');
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { ws.onopen = resolve; ws.onerror = reject; });
  let serial = 0;
  const pending = new Map();
  ws.onmessage = e => {
    const m = JSON.parse(e.data);
    if (pending.has(m.id)) { const { resolve, reject } = pending.get(m.id); pending.delete(m.id); m.error ? reject(Error(m.error.message)) : resolve(m.result); }
    // Console errors / uncaught exceptions belong to the case that navigated;
    // without this branch `pageErrors` could never leave zero.
    else if (m.method === 'Runtime.exceptionThrown') pageErrors.push(m.params.exceptionDetails?.exception?.description || m.params.exceptionDetails?.text || 'exception');
    else if (m.method === 'Runtime.consoleAPICalled' && m.params.type === 'error') pageErrors.push(m.params.args.map(a => a.value ?? a.description ?? '').join(' '));
  };
  call = (method, params = {}) => new Promise((resolve, reject) => { const id = ++serial; pending.set(id, { resolve, reject }); ws.send(JSON.stringify({ id, method, params })); });
  evaluate = async expression => { const r = await call('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true }); if (r.exceptionDetails) throw Error(JSON.stringify(r.exceptionDetails)); return r.result.value; };
  fs.mkdirSync(output, { recursive: true });
  await call('Page.enable');
  await call('Runtime.enable');
  let bundle = null;
  try {
    // Bundle provenance, every run: the served bytes must equal the disk
    // dist. Extensionless entries (`.trunk-lock`) are skipped: the server's
    // SPA fallback answers them with index.html by design.
    if (dist) {
      bundle = {};
      for (const entry of fs.readdirSync(dist)) {
        const full = path.join(dist, entry);
        if (fs.statSync(full).isFile() && path.extname(entry) !== '') bundle[entry] = sha(fs.readFileSync(full));
      }
      for (const entry of Object.keys(bundle)) {
        const response = await fetch(base + '/' + entry);
        assert.ok(response.ok, 'served asset ' + entry);
        assert.equal(sha(Buffer.from(await response.arrayBuffer())), bundle[entry], 'served bytes match disk for ' + entry);
      }
    }
    for (const name of selection === 'all' ? Object.keys(cases) : [selection]) {
      try { const extra = await cases[name](); results.push({ name, status: 'PASS', ...(extra || {}) }); console.log('PASS ' + name); }
      catch (e) { results.push({ name, status: 'FAIL', error: e.stack }); console.error('FAIL ' + name + ': ' + e.message); }
    }
  } finally {
    fs.writeFileSync(path.join(output, 'results.json'), JSON.stringify({
      utc: new Date().toISOString(), page: page.id, base, bundle,
      scriptSha256: sha(fs.readFileSync(__filename)),
      fixtureSha256: sha(fs.readFileSync(fixturePath)),
      results,
    }, null, 2) + '\n');
    if (initId) await call('Page.removeScriptToEvaluateOnNewDocument', { identifier: initId });
    ws.close();
  }
  if (results.some(r => r.status === 'FAIL')) process.exitCode = 1;
})().catch(e => { console.error(e); process.exitCode = 1; });
