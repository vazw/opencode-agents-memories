'use strict';
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const fixturePath = path.join(__dirname, 'fixtures/issue-118-wave4.json');
const fixture = JSON.parse(fs.readFileSync(fixturePath));
const base = process.env.TEST_BASE_URL || 'http://127.0.0.1:49418';
const selection = process.env.W4_TEST || 'all';
const output = process.env.W4_OUTPUT_DIR || path.join('.tmp/artifacts/issue-118-wave4', new Date().toISOString().replace(/[:.]/g, '-'));
const dist = process.env.WEBUI_DIST;
const sha = bytes => crypto.createHash('sha256').update(bytes).digest('hex');
const q = id => `[data-testid="${id}"]`;
const results = [];
let call, evaluate, initId;
const sleep = ms => evaluate(`new Promise(r=>setTimeout(r,${ms}))`);

async function ready(selector) {
  return evaluate(`new Promise((resolve,reject)=>{let clean=0;const samples=[];const start=Date.now();const timer=setInterval(()=>{const ok=!!document.querySelector(${JSON.stringify(selector)})&&!document.querySelector('.thaw-spinner,[data-testid$="-loading"]');samples.push({elapsed:Date.now()-start,clean});clean=ok?clean+1:0;if(clean===3){clearInterval(timer);resolve(samples)}else if(Date.now()-start>20000){clearInterval(timer);reject(Error('readiness timeout '+${JSON.stringify(selector)}+' | '+document.body.innerText.slice(0,120)))}},100)})`);
}
function install(scenario) {
  return `(()=>{const f=${JSON.stringify(fixture)};window.__scenario=${JSON.stringify(scenario)};window.__requests=[];window.__submits=[];window.__OPENCODE_MEM_TOKEN__='synthetic-wave4-marker';document.addEventListener('submit',e=>{queueMicrotask(()=>window.__submits.push({prevented:e.defaultPrevented}))},true);const original=window.fetch.bind(window);window.fetch=async(input,options={})=>{const u=new URL(typeof input==='string'?input:input.url,location.href);if(!u.pathname.startsWith('/api/'))return original(input,options);const method=options.method||'GET';const headers=new Headers(options.headers);window.__requests.push({path:u.pathname,method,tokenPresent:headers.get('x-opencode-mem-token')==='synthetic-wave4-marker'});const s=window.__scenario;let data,status=200;if(s.startsWith('home-')&&['/api/stats','/api/health'].includes(u.pathname))await new Promise(r=>setTimeout(r,700));if(u.pathname==='/api/memories')await new Promise(r=>setTimeout(r,600));if(u.pathname==='/api/health'){status=s==='home-stats-ok'?500:200;return new Response(JSON.stringify(status===200?f.health:{success:false,status:'failed'}),{status,headers:{'Content-Type':'application/json'}})}if(u.pathname==='/api/stats'){data=structuredClone(f.stats);if(s==='home-health-ok'||s==='stats-error')status=500;if(s==='home-zero'){data.total=0;data.byScope={user:0,project:0};data.shards=[]}if(s==='zero')data.daily=[{date:'2026-11-01',captures:0,captureSkipped:0,profileInjections:0,compactionInjections:0}]}else if(u.pathname==='/api/tags')data=f.tags;else if(u.pathname==='/api/user-profiles')data=f.roster;else if(u.pathname==='/api/user-profile')data=f.profile;else if(u.pathname==='/api/skills'){data=s==='empty-skills'?{items:[]}:f.skills;if(method!=='GET'){if(s==='save-success'){data={...JSON.parse(options.body),id:'fixture-created',updatedAt:'2026-11-01T12:00:00Z'}}else{status=500;data={success:false,error:'Synthetic failure'}}}}else if(u.pathname.startsWith('/api/skills/')){status=500;data={success:false,error:'Synthetic failure'}}else if(u.pathname==='/api/memories')data=f.memories;else if(u.pathname==='/api/projects')data=[{projectId:'fixture-project',sessionCount:1,milestoneCount:1,lastActivityAt:1730462400000}];else if(u.pathname.endsWith('/milestones'))data=[{id:'fixture-ms-1',projectId:'fixture-project',agentId:'fixture-agent',summary:'Synthetic milestone summary',sourceCount:1,windowStart:1730462400000,windowEnd:1730462400000,createdAt:1730462400000,promotedFacts:[],contributors:[{sessionRowId:'fixture-session-row-1',sessionId:'fixture-session-1',parentSessionId:null,agentId:'fixture-agent',agentName:'Fixture Agent',createdAt:1730462400000,summary:'Synthetic session summary'}]}];else if(u.pathname.includes('/sessions'))data=[{id:'fixture-session-1',agentName:'Fixture Agent',summary:'Synthetic session summary',createdAt:1730462400000,orphaned:false}];else{status=599;data={error:'Unknown fixture path'}}return new Response(JSON.stringify(status===200?{success:true,data}:data),{status,headers:{'Content-Type':'application/json'}})}})();`;
}
async function navigate(route, scenario = 'normal', width = 1280, theme = 'light') {
  if (initId) await call('Page.removeScriptToEvaluateOnNewDocument', { identifier: initId });
  ({ identifier: initId } = await call('Page.addScriptToEvaluateOnNewDocument', { source: install(scenario) + `;localStorage.setItem('opencode-mem-theme',${JSON.stringify(theme)});` }));
  await call('Emulation.setDeviceMetricsOverride', { width, height: width === 390 ? 844 : 800, deviceScaleFactor: 1, mobile: false });
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
async function val(selector) {
  return evaluate(`(()=>{const root=document.querySelector(${JSON.stringify(selector)});const el=root.matches('input,textarea')?root:root.querySelector('input,textarea');return el?el.value:null})()`);
}
async function shot(name) {
  const { data } = await call('Page.captureScreenshot', { format: 'png' });
  fs.writeFileSync(path.join(output, name), Buffer.from(data, 'base64'));
}
const TERMA = { '/': q('home-stats-ready'), '/user-profile': '#profile-identity-select', '/tags': `${q('tags-panel')} a`, '/export-import': `${q('export-download')}:not([disabled])`, '/skill-library': `${q('skill-row')},${q('skills-empty')}`, '/stats': q('stats-card-total') };
async function openSkillEditor() {
  await navigate('/skill-library');
  await ready(TERMA['/skill-library']);
  await click(q('skill-create'));
  await ready(q('skill-form'));
}
async function fillDraft() {
  await input(q('skill-form-name'), 'Wave4 Skill');
  await input(q('skill-form-description'), 'Synthetic draft');
  await input(q('skill-form-content'), '# body');
}
const cases = {
  async home_independent_resources() {
    for (const [scenario, good, bad] of [['home-stats-ok', 'stats', 'health'], ['home-health-ok', 'health', 'stats']]) {
      await navigate('/', scenario);
      await ready(q(`home-${bad}-error`));
      assert.ok(await evaluate(`!!document.querySelector(${JSON.stringify(q(`home-${good}-ready`))})`), 'independent successful resource stays visible');
      const before = await evaluate('window.__requests.length');
      await click(q(`home-${bad}-retry`));
      await sleep(80);
      assert.equal(await evaluate(`document.querySelector(${JSON.stringify(q(`home-${bad}-retry`))}).disabled`), true, 'pending retry disabled');
      await click(q(`home-${bad}-retry`));
      await ready(q(`home-${bad}-error`));
      const added = await evaluate(`window.__requests.slice(${before})`);
      assert.deepEqual(added.map(r => r.path), [`/api/${bad}`], 'only the failing resource re-requested, once');
      assert.ok(added.every(r => r.tokenPresent), 'retry passes synthetic marker through existing seam');
    }
    await navigate('/', 'home-zero');
    assert.equal(await evaluate(`document.querySelectorAll(${JSON.stringify(q('home-summary-card'))}).length`), 0, 'no fake numeric summary while pending');
    assert.ok(await evaluate(`!!document.querySelector(${JSON.stringify(q('home-stats-loading'))})`), 'pending is loading, not zero');
    await ready(q('home-stats-ready'));
    assert.deepEqual(await evaluate(`Array.from(document.querySelectorAll(${JSON.stringify(q('home-summary-card') + ' dd')}),e=>e.textContent)`), ['0', '0', '0', '0'], 'zero payload renders four zero cards');
    const labelCounts = await evaluate(`Array.from(document.querySelectorAll(${JSON.stringify(q('home-summary-card'))}),c=>{const t=c.querySelector('dt').textContent;return c.textContent.split(t).length-1})`);
    assert.deepEqual(labelCounts, [1, 1, 1, 1], 'each card paints its label exactly once');
    const heading = await evaluate(`(()=>{const s=getComputedStyle(document.querySelector('.home-panel h2'));return [s.fontSize,s.marginTop]})()`);
    assert.deepEqual(heading, ['18.4px', '0px'], 'Home heading uses the shared .home-panel h2 rule, not UA defaults');
    assert.ok(await evaluate(`['/project-memories','/user-profile','/skill-library','/stats'].every(p=>document.querySelector('a[href="'+p+'"]'))`), 'existing shell anchors preserved');
  },
  async profile_picker_label() {
    for (const theme of ['light', 'dark']) for (const width of [1280, 390]) {
      await navigate('/user-profile', 'normal', width, theme);
      await ready('#profile-identity-select');
      assert.equal(await evaluate(`document.querySelector('#profile-identity-select').labels.length`), 1, 'native select has exactly one associated label');
      await click('label[for="profile-identity-select"]');
      assert.ok(await evaluate(`document.activeElement===document.querySelector('select#profile-identity-select')`), 'label click focuses the native select');
      assert.deepEqual(await evaluate(`Array.from(document.querySelectorAll('.profile-section h3'),e=>e.textContent)`), ['Preferences (2)', 'Patterns (1)', 'Workflows (0)'], 'three concurrently visible sections with counts');
    }
  },
  async skills_filtered_empty() {
    await navigate('/skill-library');
    await ready(TERMA['/skill-library']);
    await click(q('skill-create'));
    await ready(q('skill-form'));
    await input(q('skill-search'), 'unmatched-wave4');
    await ready(q('skills-filtered-empty'));
    assert.equal(await evaluate(`document.querySelectorAll(${JSON.stringify(q('skill-row'))}).length`), 0, 'unmatched query shows no rows');
    const before = await evaluate('window.__requests.length');
    await click(q('skills-clear-filters'));
    await ready(TERMA['/skill-library']);
    assert.deepEqual(await evaluate('window.__submits'), [], 'clear never submits the form');
    assert.equal(await evaluate(`window.__requests.slice(${before}).filter(r=>r.method!=='GET').length`), 0, 'clear writes nothing');
    assert.ok(await evaluate(`!!document.querySelector(${JSON.stringify(q('skill-form'))})`), 'clear keeps the open editor');
    assert.ok(await evaluate(`!!document.querySelector(${JSON.stringify(q('skill-row'))})`), 'clear restores the existing row');
    await navigate('/skill-library', 'empty-skills');
    await ready(q('skills-empty'));
    assert.match(await evaluate(`document.querySelector(${JSON.stringify(q('skills-empty'))}).textContent`), /No skills yet/, 'genuinely empty library copy is distinct');
    assert.equal(await evaluate(`document.querySelectorAll(${JSON.stringify(q('skills-filtered-empty'))}).length`), 0);
  },
  async stats_accessible_counters_and_shards() {
    const failures = [];
    for (const theme of ['light', 'dark']) {
      await navigate('/stats', 'normal', 390, theme);
      await ready(TERMA['/stats']);
      try {
        assert.deepEqual(
          await evaluate(`Array.from(document.querySelectorAll('.stats-counter progress'),e=>({value:e.value,max:e.max,name:e.getAttribute('aria-label')}))`),
          [0, 2, 7, 1].map((value, i) => ({ value, max: 7, name: '2026-11-01 ' + ['Captures', 'Skipped', 'Profile', 'Compaction'][i] })),
          'exact_counter_semantics');
        assert.ok(await evaluate(`Array.from(document.querySelectorAll('.stats-count')).map(e=>e.textContent).join(',')==='0,2,7,1'`), 'adjacent exact count text retained');
      } catch (e) { failures.push('exact_counter_semantics: ' + e.message); }
      try {
        const region = '[role="region"][aria-label="Shard diagnostics"]';
        const geometry = await evaluate(`(()=>{const r=document.querySelector(${JSON.stringify(region)});if(!r)return null;return{height:r.clientHeight,scroll:r.scrollHeight,max:24*parseFloat(getComputedStyle(document.documentElement).fontSize),tab:r.tabIndex,rows:r.querySelectorAll('tbody tr').length,page:document.documentElement.scrollWidth<=document.documentElement.clientWidth}})()`);
        assert.ok(geometry, 'named region exists');
        assert.equal(geometry.tab, 0, 'region focusable');
        assert.equal(geometry.rows, 40, 'all 40 shard rows retained');
        assert.ok(geometry.height <= geometry.max && geometry.scroll > geometry.height && geometry.page, 'bounded local overflow, no document overflow');
        await call('Page.bringToFront');
        await evaluate(`document.querySelector(${JSON.stringify(region)}).focus()`);
        for (const [key, vk] of [['End', 35], ['PageDown', 34]]) await realKey(key, key, vk);
        assert.ok(await evaluate(`new Promise(resolve=>{let n=0;const t=setInterval(()=>{const r=document.querySelector(${JSON.stringify(region)});if(r.scrollTop+r.clientHeight>=r.scrollHeight-2||++n===30){clearInterval(t);resolve(r.scrollTop+r.clientHeight>=r.scrollHeight-2)}},100)})`), 'real keyboard End/PageDown reaches the last shard');
      } catch (e) { failures.push('local_keyboard_overflow: ' + e.message); }
    }
    await navigate('/stats', 'zero');
    await ready(TERMA['/stats']);
    try {
      assert.deepEqual(
        await evaluate(`Array.from(document.querySelectorAll('.stats-counter progress'),e=>({value:e.value,max:e.max,name:e.getAttribute('aria-label')}))`),
        [0, 0, 0, 0].map((value, i) => ({ value, max: 1, name: '2026-11-01 ' + ['Captures', 'Skipped', 'Profile', 'Compaction'][i] })),
        'all-zero exact_counter_semantics');
    } catch (e) { failures.push('all-zero: ' + e.message); }
    assert.deepEqual(failures, []);
  },
  async skills_form_save_non_submit() {
    await navigate('/skill-library', 'save-error');
    await ready(TERMA['/skill-library']);
    await click(q('skill-create'));
    await ready(q('skill-form'));
    await fillDraft();
    await click(q('skill-save'));
    await ready(q('skill-form-error'));
    assert.deepEqual(await evaluate('window.__submits'), [], 'Save click never submits the form');
    assert.equal(await evaluate(`window.__requests.filter(r=>r.method!=='GET').length`), 1, 'exactly one intended write');
    assert.deepEqual(await evaluate(`window.__requests.find(r=>r.method==='POST')`), { path: '/api/skills', method: 'POST', tokenPresent: true });
    assert.equal(await val(q('skill-form-name')), 'Wave4 Skill', 'rejected Save keeps name draft');
    assert.equal(await val(q('skill-form-description')), 'Synthetic draft', 'rejected Save keeps description draft');
    assert.equal(await val(q('skill-form-content')), '# body', 'rejected Save keeps content draft');
    assert.ok(await evaluate(`!!document.querySelector(${JSON.stringify(q('skill-form'))})`), 'form stays open after failure');
    await navigate('/skill-library', 'save-success');
    await ready(TERMA['/skill-library']);
    await click(q('skill-create'));
    await ready(q('skill-form'));
    await fillDraft();
    await click(q('skill-save'));
    await ready(q('skill-row'));
    assert.ok(await evaluate(`!document.querySelector(${JSON.stringify(q('skill-form'))})`), 'successful Save closes the form');
    const writes = await evaluate(`window.__requests.filter(r=>r.method!=='GET')`);
    assert.equal(writes.length, 1, 'successful Save writes exactly once');
    assert.deepEqual(await evaluate('window.__submits'), [], 'no submit events on success path');
  },
  async skills_form_cancel_non_submit() {
    await navigate('/skill-library');
    await ready(TERMA['/skill-library']);
    await click(q('skill-create'));
    await ready(q('skill-form'));
    await input(q('skill-form-name'), 'Draft X');
    await click(q('skill-cancel'));
    assert.ok(await evaluate(`!document.querySelector(${JSON.stringify(q('skill-form'))})`), 'Cancel closes the form');
    assert.deepEqual(await evaluate('window.__submits'), [], 'Cancel never submits');
    assert.equal(await evaluate(`window.__requests.filter(r=>r.method!=='GET').length`), 0, 'Cancel writes nothing');
    assert.equal(await evaluate('location.pathname'), '/skill-library', 'no navigation');
  },
  async skills_form_enter_prevented() {
    await navigate('/skill-library');
    await ready(TERMA['/skill-library']);
    await click(q('skill-create'));
    await ready(q('skill-form'));
    await input(q('skill-form-name'), 'Enter Draft');
    await evaluate(`document.querySelector(${JSON.stringify(q('skill-form-name'))}).matches('input')?document.querySelector(${JSON.stringify(q('skill-form-name'))}).focus():document.querySelector(${JSON.stringify(q('skill-form-name'))}+' input').focus()`);
    await call('Page.bringToFront');
    await realKey('Enter', 'Enter', 13, '\r');
    await sleep(120);
    assert.equal(await evaluate('location.pathname'), '/skill-library', 'real Enter causes no navigation');
    assert.equal(await evaluate(`window.__requests.filter(r=>r.method!=='GET').length`), 0, 'real Enter causes no write');
    assert.ok(await evaluate(`!!document.querySelector(${JSON.stringify(q('skill-form'))})`), 'form stays open');
    const prevented = await evaluate(`(()=>{const f=document.querySelector(${JSON.stringify(q('skill-form'))});const e=new Event('submit',{cancelable:true,bubbles:true});f.dispatchEvent(e);return e.defaultPrevented})()`);
    assert.equal(prevented, true, 'prevention verification: implicit submit events are prevented');
    const submits = await evaluate('window.__submits');
    assert.ok(submits.length >= 1, 'the submit path was exercised');
    assert.deepEqual(submits.filter(e => e.prevented !== true), [], 'every observed submit event was defaultPrevented');
  },
  async portability_existing_contract() {
    await navigate('/export-import');
    await ready(TERMA['/export-import']);
    assert.ok(await evaluate(`!document.querySelector(${JSON.stringify(q('export-download'))}).disabled`), 'delayed export readiness settles enabled');
    assert.equal(await evaluate(`document.querySelector(${JSON.stringify(q('export-download'))}).labels.length`), 0);
    await click('label[for="import-json"]');
    assert.ok(await evaluate(`document.activeElement===document.querySelector('textarea#import-json')`), 'existing label focuses the textarea');
    const doc = await evaluate(`(async()=>{const orig=URL.createObjectURL.bind(URL);let captured=null;URL.createObjectURL=b=>{captured=b;return orig(b)};document.querySelector(${JSON.stringify(q('export-download'))}).click();await new Promise(r=>setTimeout(r,150));URL.createObjectURL=orig;const text=await captured.text();return{type:captured.type,text}})()`);
    assert.equal(doc.type, 'application/json', 'valid download Blob type');
    const parsed = JSON.parse(doc.text);
    assert.equal(parsed.kind, 'opencode-mem-explorer-export', 'downloaded document is an explorer export');
    assert.equal(parsed.memories.length, 1, 'prompt row excluded from the export document');
    await input(q('import-input'), 'not json');
    await ready(q('export-import-error'));
    await input(q('import-input'), doc.text);
    assert.ok(await evaluate(`!!document.querySelector(${JSON.stringify(q('import-preview'))})&&!document.querySelector(${JSON.stringify(q('export-import-error'))})`), 'valid explorer document clears the error and previews');
    assert.equal(await evaluate(`document.querySelector(${JSON.stringify(q('import-preview'))}).textContent`), '1', 'preview counts non-prompt rows');
    await input(q('import-input'), '{}');
    await ready(q('export-import-error'));
    assert.ok(await evaluate(`!!document.querySelector(${JSON.stringify(q('import-unsupported'))})`), 'unsupported notice stays');
    assert.equal(await evaluate(`window.__requests.filter(r=>r.method!=='GET').length`), 0, 'no import POST');
  },
  async stats_existing_content() {
    await navigate('/stats');
    await ready(TERMA['/stats']);
    assert.equal(await evaluate(`document.querySelectorAll('[data-testid^="stats-card-"]').length`), 4, 'four named overview cards');
    assert.equal(await evaluate(`document.querySelector(${JSON.stringify(q('stats-card-total') + ' .stats-card-value')}).textContent`), '7');
    assert.match(await evaluate(`document.querySelector(${JSON.stringify(q('stats-card-shards') + ' .stats-card-meta')}).textContent`), /40 shards total/);
    assert.ok(await evaluate(`!!document.querySelector('.stats-shard-error')`), 'shard error text retained');
    assert.ok(await evaluate(`!!document.querySelector('.stats-mismatch')`), 'count mismatch text retained');
    await evaluate(`window.__scenario='stats-error'`);
    await click(q('stats-reload'));
    await ready(q('stats-stale'));
    assert.equal(await evaluate(`document.querySelector(${JSON.stringify(q('stats-card-total') + ' .stats-card-value')}).textContent`), '7', 'reload failure retains the last snapshot');
    await navigate('/stats', 'stats-error');
    await ready(q('stats-error'));
    assert.equal(await evaluate(`document.querySelectorAll('[data-testid^="stats-card-"]').length`), 0, 'fresh failure shows no fake cards');
  },
  async tags_unchanged_matrix() {
    for (const theme of ['light', 'dark']) {
      await navigate('/tags', 'normal', 390, theme);
      await ready(TERMA['/tags']);
      const href = await evaluate(`document.querySelector(${JSON.stringify(q('tags-panel') + ' a')}).href`);
      assert.ok(href.includes('%20'), 'space-bearing tag destination encoded');
      assert.ok(decodeURIComponent(href).includes('空間'), 'Unicode tag destination round-trips');
      const rects = await evaluate(`document.querySelectorAll(${JSON.stringify(q('tags-panel') + ' a')})[0].getClientRects().length`);
      assert.ok(rects > 1, 'long tag wraps locally');
      const ratio = await evaluate(`(()=>{const parse=s=>s.match(/[\\d.]+/g).map(Number);const lum=rgb=>{const f=v=>{v/=255;return v<=0.03928?v/12.92:Math.pow((v+0.055)/1.055,2.4)};return 0.2126*f(rgb[0])+0.7152*f(rgb[1])+0.0722*f(rgb[2])};let el=document.querySelector('a[href*="%20"]');let fg=parse(getComputedStyle(el).color).slice(0,3);let node=el,bg=null;while(node){const b=getComputedStyle(node).backgroundColor;const m=b.match(/[\\d.]+/g);if(m&&(m.length<4||Number(m[3])>0)){bg=parse(b).slice(0,3);break}node=node.parentElement}const L1=lum(fg),L2=lum(bg);return (Math.max(L1,L2)+0.05)/(Math.min(L1,L2)+0.05)})()`);
      assert.ok(ratio >= 4.5, `tag link contrast ${ratio} >= 4.5 in ${theme}`);
      await call('Page.bringToFront');
      await realKey('Tab', 'Tab', 9);
      assert.ok(await evaluate(`document.activeElement.tagName==='A'`), 'real keyboard focus reaches a tag link');
      assert.notEqual(await evaluate(`getComputedStyle(document.activeElement).outlineStyle`), 'none', 'focused tag link has a focus outline');
    }
  },
  async native_select_and_route_regressions() {
    await navigate('/user-profile');
    await ready('#profile-identity-select');
    assert.equal(await evaluate(`document.querySelector('#profile-identity-select').options.length`), 3, 'populated profile roster');
    await navigate('/sessions');
    await ready(q('session-list'));
    assert.equal(await evaluate(`document.querySelector(${JSON.stringify(q('sessions-project-select'))}).options.length`), 1, 'sessions select populated with exact fixture project count');
    assert.equal(await evaluate(`document.querySelector(${JSON.stringify(q('sessions-project-select'))}).value`), 'fixture-project', 'sessions select selects fixture-project');
    assert.equal(await evaluate(`document.querySelector(${JSON.stringify(q('sessions-project-select'))}).options[0].textContent`), 'fixture-project — 1 sessions · 1 milestones', 'sessions select option label matches project_option_label');
    assert.equal(await evaluate(`document.querySelector(${JSON.stringify(q('sessions-project-select'))}).labels.length`), 1, 'sessions select keeps its label');
    assert.equal(await evaluate(`document.querySelectorAll(${JSON.stringify(q('session-row'))}).length`), 1, 'sessions Ready timeline renders populated row');
    assert.ok(await evaluate(`!document.body.innerText.includes('Malformed')&&!document.body.innerText.includes('Unrecognized')`), 'valid sessions fixture renders no malformed response');
    await navigate('/agents');
    await ready(q('agents-roster'));
    assert.equal(await evaluate(`document.querySelector(${JSON.stringify(q('agents-project-select'))}).options.length`), 1, 'agents select populated with exact fixture project count');
    assert.equal(await evaluate(`document.querySelector(${JSON.stringify(q('agents-project-select'))}).value`), 'fixture-project', 'agents select selects fixture-project');
    assert.equal(await evaluate(`document.querySelector(${JSON.stringify(q('agents-project-select'))}).options[0].textContent`), 'fixture-project — 1 sessions · 1 milestones', 'agents select option label matches project_option_label');
    assert.equal(await evaluate(`document.querySelectorAll(${JSON.stringify(q('agents-row'))}).length`), 1, 'agents Ready roster renders populated contributor');
    assert.ok(await evaluate(`!document.body.innerText.includes('malformed')`), 'agents milestones envelope valid');
  },
  async matrix_ready_captures() {
    assert.ok(dist, 'WEBUI_DIST must point at the served dist for bundle verification');
    const disk = {};
    for (const entry of fs.readdirSync(dist)) {
      const full = path.join(dist, entry);
      if (fs.statSync(full).isFile()) disk[entry] = sha(fs.readFileSync(full));
    }
    for (const entry of Object.keys(disk)) {
      const response = await fetch(base + '/' + entry);
      assert.ok(response.ok, 'served asset ' + entry);
      assert.equal(sha(Buffer.from(await response.arrayBuffer())), disk[entry], 'served bytes match disk for ' + entry);
    }
    const captures = [];
    for (const route of ['/', '/user-profile', '/tags', '/export-import', '/skill-library', '/stats'])
      for (const theme of ['light', 'dark'])
        for (const width of [1280, 390]) {
          await navigate(route, 'normal', width, theme);
          const samples = await ready(TERMA[route]);
          const name = (route === '/' ? 'home' : route.slice(1)) + '-' + theme + '-' + width + '.png';
          await shot(name);
          captures.push({
            route, theme, width, screenshot: name,
            readySamples: samples.length,
            overflow: await evaluate(`document.documentElement.scrollWidth>document.documentElement.clientWidth`),
          });
          assert.equal(captures.at(-1).overflow, false, 'no document horizontal overflow at ' + route + ' ' + width);
        }
    return { bundle: disk, captures };
  },
};
(async () => {
  assert.ok(selection === 'all' || Object.hasOwn(cases, selection), 'unknown W4_TEST ' + selection);
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
  };
  call = (method, params = {}) => new Promise((resolve, reject) => { const id = ++serial; pending.set(id, { resolve, reject }); ws.send(JSON.stringify({ id, method, params })); });
  evaluate = async expression => { const r = await call('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true }); if (r.exceptionDetails) throw Error(JSON.stringify(r.exceptionDetails)); return r.result.value; };
  fs.mkdirSync(output, { recursive: true });
  await call('Page.enable');
  await call('Runtime.enable');
  try {
    for (const name of selection === 'all' ? Object.keys(cases) : [selection]) {
      try { const extra = await cases[name](); results.push({ name, status: 'PASS', ...(extra || {}) }); console.log('PASS ' + name); }
      catch (e) { results.push({ name, status: 'FAIL', error: e.stack }); console.error('FAIL ' + name + ': ' + e.message); }
    }
  } finally {
    fs.writeFileSync(path.join(output, 'results.json'), JSON.stringify({
      utc: new Date().toISOString(), page: page.id, base,
      scriptSha256: sha(fs.readFileSync(__filename)),
      fixtureSha256: sha(fs.readFileSync(fixturePath)),
      results,
    }, null, 2) + '\n');
    if (initId) await call('Page.removeScriptToEvaluateOnNewDocument', { identifier: initId });
    ws.close();
  }
  if (results.some(r => r.status === 'FAIL')) process.exitCode = 1;
})().catch(e => { console.error(e); process.exitCode = 1; });
