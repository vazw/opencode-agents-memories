'use strict';
const fs = require('node:fs');
const path = require('node:path');
const root = '/home/vaz/Code/Personal/Rust/opencode-agents-memory';
const fixture = JSON.parse(fs.readFileSync(path.join(root, 'web-ui/scripts/fixtures/issue-118-wave4.json')));
const base = process.env.TEST_BASE_URL || 'http://127.0.0.1:49418';
const targetId = process.env.LANE_PAGE_ID || 'F84E3B0582B22A8842B5F70B90E6CDEA';
const out = path.join(root, '.tmp/progress/issue118-w4-green-r3/probes');
fs.mkdirSync(out, { recursive: true });
const q = id => `[data-testid="${id}"]`;
function install(scenario) {
  return `(()=>{const f=${JSON.stringify(fixture)};window.__scenario=${JSON.stringify(scenario)};window.__requests=[];window.__OPENCODE_MEM_TOKEN__='synthetic-wave4-marker';const original=window.fetch.bind(window);window.fetch=async(input,options={})=>{const u=new URL(typeof input==='string'?input:input.url,location.href);if(!u.pathname.startsWith('/api/'))return original(input,options);const method=options.method||'GET';const headers=new Headers(options.headers);window.__requests.push({path:u.pathname,method,tokenPresent:headers.get('x-opencode-mem-token')==='synthetic-wave4-marker'});const s=window.__scenario;let data,status=200;if(s.startsWith('home-')&&['/api/stats','/api/health'].includes(u.pathname))await new Promise(r=>setTimeout(r,700));if(u.pathname==='/api/memories')await new Promise(r=>setTimeout(r,600));if(u.pathname==='/api/health'){status=s==='home-stats-ok'?500:200;return new Response(JSON.stringify(status===200?f.health:{success:false,status:'failed'}),{status,headers:{'Content-Type':'application/json'}})}if(u.pathname==='/api/stats'){data=structuredClone(f.stats);if(s==='home-health-ok'||s==='stats-error')status=500;if(s==='home-zero'){data.total=0;data.byScope={user:0,project:0};data.shards=[]}if(s==='zero')data.daily=[{date:'2026-11-01',captures:0,captureSkipped:0,profileInjections:0,compactionInjections:0}]}else if(u.pathname==='/api/tags')data=f.tags;else if(u.pathname==='/api/user-profiles')data=f.roster;else if(u.pathname==='/api/user-profile')data=f.profile;else if(u.pathname==='/api/skills'){data=s==='empty-skills'?{items:[]}:f.skills}else if(u.pathname==='/api/memories')data=f.memories;else if(u.pathname==='/api/projects')data=[{projectId:'fixture-project',sessionCount:1,milestoneCount:1,lastActivityAt:1730462400000}];else if(u.pathname.endsWith('/milestones'))data=[{id:'fixture-ms-1',projectId:'fixture-project',agentId:'fixture-agent',summary:'Synthetic milestone summary',sourceCount:1,windowStart:1730462400000,windowEnd:1730462400000,createdAt:1730462400000,promotedFacts:[],contributors:[{sessionRowId:'fixture-session-row-1',sessionId:'fixture-session-1',parentSessionId:null,agentId:'fixture-agent',agentName:'Fixture Agent',createdAt:1730462400000,summary:'Synthetic session summary'}]}];else if(u.pathname.includes('/sessions'))data=[{id:'fixture-session-1',agentName:'Fixture Agent',summary:'Synthetic session summary',createdAt:1730462400000,orphaned:false}];else{status=599;data={error:'Unknown fixture path'}}return new Response(JSON.stringify(status===200?{success:true,data}:data),{status,headers:{'Content-Type':'application/json'}})}})();`;
}
(async () => {
  const pages = await (await fetch('http://127.0.0.1:9222/json')).json();
  const page = pages.find(p => p.id === targetId && p.type === 'page');
  if (!page) throw Error('lane target not found');
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((res, rej) => { ws.onopen = res; ws.onerror = rej; });
  let serial = 0; const pending = new Map();
  ws.onmessage = e => { const m = JSON.parse(e.data); const p = pending.get(m.id); if (p) { pending.delete(m.id); m.error ? p.reject(Error(m.error.message)) : p.resolve(m.result); } };
  const call = (method, params = {}) => new Promise((resolve, reject) => { const id = ++serial; pending.set(id, { resolve, reject }); ws.send(JSON.stringify({ id, method, params })); });
  const evaluate = async expression => { const r = await call('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true }); if (r.exceptionDetails) throw Error(JSON.stringify(r.exceptionDetails)); return r.result.value; };
  const ready = sel => evaluate(`new Promise((resolve,reject)=>{let clean=0;const samples=[];const start=Date.now();const t=setInterval(()=>{const ok=!!document.querySelector(${JSON.stringify(sel)})&&!document.querySelector('.thaw-spinner,[data-testid$="-loading"]');samples.push({elapsed:Date.now()-start,clean});clean=ok?clean+1:0;if(clean===3){clearInterval(t);resolve(samples)}else if(Date.now()-start>20000){clearInterval(t);reject(Error('timeout '+${JSON.stringify(sel)}))}},100)})`);
  let initId;
  const nav = async (route, scenario = 'normal', width = 1280, theme = 'light') => {
    if (initId) await call('Page.removeScriptToEvaluateOnNewDocument', { identifier: initId });
    ({ identifier: initId } = await call('Page.addScriptToEvaluateOnNewDocument', { source: install(scenario) + `;localStorage.setItem('opencode-mem-theme',${JSON.stringify(theme)});` }));
    await call('Emulation.setDeviceMetricsOverride', { width, height: width === 390 ? 844 : 800, deviceScaleFactor: 1, mobile: false });
    await call('Page.navigate', { url: base + route });
    await evaluate(`new Promise(r=>{if(document.readyState==='complete')r();else addEventListener('load',r,{once:true})})`);
  };
  await call('Page.enable'); await call('Runtime.enable');

  await nav('/', 'normal', 1280, 'light');
  await ready(q('home-stats-ready'));
  const homeNormal = await evaluate(`(()=>{const cards=Array.from(document.querySelectorAll(${JSON.stringify(q('home-summary-card'))}));const h2=document.querySelector('.home-panel h2');const s=getComputedStyle(h2);return{cardCount:cards.length,cards:cards.map(c=>({dt:c.querySelector('dt')?.textContent,dd:c.querySelector('dd')?.textContent,text:c.textContent,labelCount:c.textContent.split(c.querySelector('dt').textContent).length-1})),labelCounts:cards.map(c=>{const t=c.querySelector('dt').textContent;return c.textContent.split(t).length-1}),heading:{text:h2.textContent,fontSize:s.fontSize,marginTop:s.marginTop,marginBottom:s.marginBottom},healthText:document.querySelector(${JSON.stringify(q('home-health-ready'))})?.textContent}})()`);

  await nav('/', 'home-zero', 1280, 'light');
  await ready(q('home-stats-ready'));
  const homeZero = await evaluate(`(()=>{const cards=Array.from(document.querySelectorAll(${JSON.stringify(q('home-summary-card'))}));const h2=document.querySelector('.home-panel h2');const s=getComputedStyle(h2);return{cardCount:cards.length,dd:cards.map(c=>c.querySelector('dd')?.textContent),labelCounts:cards.map(c=>{const t=c.querySelector('dt').textContent;return c.textContent.split(t).length-1}),heading:[s.fontSize,s.marginTop]}})()`);

  await nav('/sessions', 'normal', 1280, 'light');
  await ready(q('session-list'));
  const sessions = await evaluate(`(()=>{const sel=document.querySelector(${JSON.stringify(q('sessions-project-select'))});return{optionsLength:sel.options.length,value:sel.value,option0Text:sel.options[0]?.textContent,labelsLength:sel.labels.length,sessionRowCount:document.querySelectorAll(${JSON.stringify(q('session-row'))}).length,hasMalformed:document.body.innerText.includes('Malformed')||document.body.innerText.includes('Unrecognized'),rowText:document.querySelector(${JSON.stringify(q('session-row'))})?.textContent}})()`);

  await nav('/agents', 'normal', 1280, 'light');
  await ready(q('agents-roster'));
  const agents = await evaluate(`(()=>{const sel=document.querySelector(${JSON.stringify(q('agents-project-select'))});return{optionsLength:sel.options.length,value:sel.value,option0Text:sel.options[0]?.textContent,labelsLength:sel.labels.length,agentsRowCount:document.querySelectorAll(${JSON.stringify(q('agents-row'))}).length,hasMalformed:document.body.innerText.includes('malformed'),rowText:document.querySelector(${JSON.stringify(q('agents-row'))})?.textContent}})()`);

  const outData = { utc: new Date().toISOString(), base, targetId, servedBundle: 'dist-after-r3 web-ui-ca2b5df9fdce9bc9', homeNormal, homeZero, sessions, agents };
  fs.writeFileSync(path.join(out, 'metrics-probe.json'), JSON.stringify(outData, null, 2) + '\n');
  console.log(JSON.stringify(outData, null, 1));
  if (initId) await call('Page.removeScriptToEvaluateOnNewDocument', { identifier: initId });
  ws.close();
})().catch(e => { console.error(e); process.exitCode = 1; });
