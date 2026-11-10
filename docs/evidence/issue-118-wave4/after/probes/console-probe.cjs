'use strict';
const fs = require('node:fs');
const path = require('node:path');
const root = '/home/vaz/Code/Personal/Rust/opencode-agents-memory';
const fixture = JSON.parse(fs.readFileSync(path.join(root, 'web-ui/scripts/fixtures/issue-118-wave4.json')));
const base = process.env.TEST_BASE_URL || 'http://127.0.0.1:49418';
const targetId = process.env.LANE_PAGE_ID || 'F84E3B0582B22A8842B5F70B90E6CDEA';
const out = path.join(root, '.tmp/progress/issue118-w4-green-r3/probes');
fs.mkdirSync(out, { recursive: true });
const install = `(()=>{const f=${JSON.stringify(fixture)};const o=window.fetch.bind(window);window.fetch=async(i,opts={})=>{const u=new URL(typeof i==='string'?i:i.url,location.href);if(!u.pathname.startsWith('/api/'))return o(i,opts);let data,status=200;if(u.pathname==='/api/memories'){await new Promise(r=>setTimeout(r,600));data=f.memories}else if(u.pathname==='/api/health'){data=f.health}else if(u.pathname==='/api/stats'){data=f.stats}else if(u.pathname==='/api/tags'){data=f.tags}else if(u.pathname==='/api/user-profiles'){data=f.roster}else if(u.pathname==='/api/user-profile'){data=f.profile}else if(u.pathname==='/api/skills'){data=f.skills}else if(u.pathname==='/api/projects'){data=[{projectId:'fixture-project',sessionCount:1,milestoneCount:1,lastActivityAt:1730462400000}]}else if(u.pathname.endsWith('/milestones')){data=[{id:'fixture-ms-1',projectId:'fixture-project',agentId:'fixture-agent',summary:'Synthetic milestone summary',sourceCount:1,windowStart:1730462400000,windowEnd:1730462400000,createdAt:1730462400000,promotedFacts:[],contributors:[{sessionRowId:'fixture-session-row-1',sessionId:'fixture-session-1',parentSessionId:null,agentId:'fixture-agent',agentName:'Fixture Agent',createdAt:1730462400000,summary:'Synthetic session summary'}]}]}else if(u.pathname.includes('/sessions')){data=[{id:'fixture-session-1',agentName:'Fixture Agent',summary:'Synthetic session summary',createdAt:1730462400000,orphaned:false}]}else{status=599;data={error:'Unknown fixture path'}}return new Response(JSON.stringify(status===200?{success:true,data}:data),{status,headers:{'Content-Type':'application/json'}})}})();`;
const TERMA = { '/': '[data-testid="home-stats-ready"]', '/user-profile': '#profile-identity-select', '/tags': '[data-testid="tags-panel"] a', '/export-import': '[data-testid="export-download"]:not([disabled])', '/skill-library': '[data-testid="skill-row"],[data-testid="skills-empty"]', '/stats': '[data-testid="stats-card-total"]' };
(async () => {
  const pages = await (await fetch('http://127.0.0.1:9222/json')).json();
  const page = pages.find(p => p.id === targetId && p.type === 'page');
  if (!page) throw Error('lane target not found');
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((res, rej) => { ws.onopen = res; ws.onerror = rej; });
  let serial = 0; const pending = new Map(); let sink = null;
  ws.onmessage = e => {
    const m = JSON.parse(e.data);
    if (m.id && pending.has(m.id)) { const p = pending.get(m.id); pending.delete(m.id); m.error ? p.reject(Error(m.error.message)) : p.resolve(m.result); return; }
    if (sink && m.method) sink(m);
  };
  const call = (method, params = {}) => new Promise((resolve, reject) => { const id = ++serial; pending.set(id, { resolve, reject }); ws.send(JSON.stringify({ id, method, params })); });
  const evaluate = async expression => { const r = await call('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true }); if (r.exceptionDetails) throw Error(JSON.stringify(r.exceptionDetails)); return r.result.value; };
  const ready = sel => evaluate(`new Promise((resolve,reject)=>{let clean=0;const samples=[];const start=Date.now();const t=setInterval(()=>{const ok=!!document.querySelector(${JSON.stringify(sel)})&&!document.querySelector('.thaw-spinner,[data-testid$="-loading"]');samples.push({elapsed:Date.now()-start,clean});clean=ok?clean+1:0;if(clean===3){clearInterval(t);resolve(samples)}else if(Date.now()-start>20000){clearInterval(t);reject(Error('timeout '+${JSON.stringify(sel)}))}},100)})`);
  await call('Page.enable'); await call('Runtime.enable'); await call('Network.enable');
  await call('Emulation.setDeviceMetricsOverride', { width: 1280, height: 800, deviceScaleFactor: 1, mobile: false });
  let initId;
  ({ identifier: initId } = await call('Page.addScriptToEvaluateOnNewDocument', { source: install + `;localStorage.setItem('opencode-mem-theme','light');` }));
  const routes = [];
  for (const route of ['/', '/user-profile', '/tags', '/export-import', '/skill-library', '/stats']) {
    const log = { exceptions: [], console: [], failed: [], badResponses: [] };
    sink = m => {
      if (m.method === 'Runtime.exceptionThrown') log.exceptions.push(m.params.exceptionDetails.exception?.description || m.params.exceptionDetails.text);
      if (m.method === 'Runtime.consoleAPICalled' && ['error', 'warning', 'assert'].includes(m.params.type)) log.console.push({ type: m.params.type, text: m.params.args.map(a => a.value ?? a.description).join(' ') });
      if (m.method === 'Network.loadingFailed') log.failed.push({ url: m.params.requestId, error: m.params.errorText, canceled: m.params.canceled });
      if (m.method === 'Network.responseReceived' && m.params.response.status >= 400) log.badResponses.push({ url: m.params.response.url, status: m.params.response.status });
    };
    await call('Page.navigate', { url: base + route });
    await evaluate(`new Promise(r=>{if(document.readyState==='complete')r();else addEventListener('load',r,{once:true})})`);
    const samples = await ready(TERMA[route]);
    await evaluate('new Promise(r=>setTimeout(r,300))');
    routes.push({ route, readySamples: samples.length, ...log });
    sink = null;
  }
  const summary = {
    exceptionsTotal: routes.reduce((n, r) => n + r.exceptions.length, 0),
    consoleTotal: routes.reduce((n, r) => n + r.console.length, 0),
    failedTotal: routes.reduce((n, r) => n + r.failed.length, 0),
    badResponsesTotal: routes.reduce((n, r) => n + r.badResponses.length, 0),
  };
  fs.writeFileSync(path.join(out, 'console-network.json'), JSON.stringify({ utc: new Date().toISOString(), base, targetId, servedBundle: 'dist-after-r3 web-ui-ca2b5df9fdce9bc9', summary, routes }, null, 2) + '\n');
  console.log(JSON.stringify({ summary, routes }, null, 1));
  if (initId) await call('Page.removeScriptToEvaluateOnNewDocument', { identifier: initId });
  ws.close();
})().catch(e => { console.error(e); process.exitCode = 1; });
