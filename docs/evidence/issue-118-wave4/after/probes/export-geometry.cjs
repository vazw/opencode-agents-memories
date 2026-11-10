'use strict';
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const root = '/home/vaz/Code/Personal/Rust/opencode-agents-memory';
const fixture = JSON.parse(fs.readFileSync(path.join(root, 'web-ui/scripts/fixtures/issue-118-wave4.json')));
const base = process.env.TEST_BASE_URL || 'http://127.0.0.1:49418';
const targetId = process.env.LANE_PAGE_ID || 'F84E3B0582B22A8842B5F70B90E6CDEA';
const out = path.join(root, '.tmp/progress/issue118-w4-green-r3/probes');
fs.mkdirSync(out, { recursive: true });
const sha = b => crypto.createHash('sha256').update(b).digest('hex');

const install = `(()=>{const f=${JSON.stringify(fixture)};window.__requests=[];const o=window.fetch.bind(window);window.fetch=async(i,opts={})=>{const u=new URL(typeof i==='string'?i:i.url,location.href);if(!u.pathname.startsWith('/api/'))return o(i,opts);const method=opts.method||'GET';window.__requests.push({path:u.pathname,method});let data,status=200;if(u.pathname==='/api/memories'){await new Promise(r=>setTimeout(r,600));data=f.memories}else if(u.pathname==='/api/health'){data=f.health}else if(u.pathname==='/api/stats'){data=f.stats}else if(u.pathname==='/api/tags'){data=f.tags}else if(u.pathname==='/api/user-profiles'){data=f.roster}else if(u.pathname==='/api/user-profile'){data=f.profile}else if(u.pathname==='/api/skills'){data=f.skills}else if(u.pathname==='/api/projects'){data=[{projectId:'fixture-project',sessionCount:1,milestoneCount:1,lastActivityAt:1730462400000}]}else if(u.pathname.endsWith('/milestones')){data=[{id:'fixture-ms-1',projectId:'fixture-project',agentId:'fixture-agent',summary:'Synthetic milestone summary',sourceCount:1,windowStart:1730462400000,windowEnd:1730462400000,createdAt:1730462400000,promotedFacts:[],contributors:[{sessionRowId:'fixture-session-row-1',sessionId:'fixture-session-1',parentSessionId:null,agentId:'fixture-agent',agentName:'Fixture Agent',createdAt:1730462400000,summary:'Synthetic session summary'}]}]}else if(u.pathname.includes('/sessions')){data=[{id:'fixture-session-1',agentName:'Fixture Agent',summary:'Synthetic session summary',createdAt:1730462400000,orphaned:false}]}else{status=599;data={error:'Unknown fixture path'}}return new Response(JSON.stringify(status===200?{success:true,data}:data),{status,headers:{'Content-Type':'application/json'}})}})();`;

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
  await call('Page.enable'); await call('Runtime.enable');
  const rows = [];
  let initId;
  const nav = async (theme, width) => {
    if (initId) await call('Page.removeScriptToEvaluateOnNewDocument', { identifier: initId });
    ({ identifier: initId } = await call('Page.addScriptToEvaluateOnNewDocument', { source: install + `;localStorage.setItem('opencode-mem-theme',${JSON.stringify(theme)});` }));
    await call('Emulation.setDeviceMetricsOverride', { width, height: width === 390 ? 844 : 800, deviceScaleFactor: 1, mobile: false });
    await call('Page.navigate', { url: base + '/export-import' });
    await evaluate(`new Promise(r=>{if(document.readyState==='complete')r();else addEventListener('load',r,{once:true})})`);
    return ready('[data-testid="export-download"]:not([disabled])');
  };
  for (const theme of ['light', 'dark']) for (const width of [1280, 390]) {
    const samples = await nav(theme, width);
    const g = await evaluate(`(()=>{const d=document.querySelector('[data-testid="export-download"]');const l=document.querySelector('label[for="import-json"]');const dr=d.getBoundingClientRect(),lr=l.getBoundingClientRect();return{download:{left:dr.left,right:dr.right,top:dr.top,bottom:dr.bottom,width:dr.width,height:dr.height},label:{left:lr.left,right:lr.right,top:lr.top,bottom:lr.bottom,width:lr.width,height:lr.height},hGap:lr.left-dr.right,vGap:lr.top-dr.bottom,marginBottom:getComputedStyle(d).marginBottom,display:getComputedStyle(d).display,pageOverflow:document.documentElement.scrollWidth>document.documentElement.clientWidth,requests:window.__requests.length}})()`);
    const name = `after-export-${width}-${theme}.png`;
    const { data } = await call('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(path.join(out, name), Buffer.from(data, 'base64'));
    rows.push({ theme, width, readySamples: samples.length, ...g, screenshot: name, screenshotSha256: sha(fs.readFileSync(path.join(out, name))) });
  }
  fs.writeFileSync(path.join(out, 'export-geometry.json'), JSON.stringify({ utc: new Date().toISOString(), base, targetId, servedBundle: 'dist-after-r3 web-ui-ca2b5df9fdce9bc9', rows }, null, 2) + '\n');
  console.log(JSON.stringify(rows, null, 1));
  if (initId) await call('Page.removeScriptToEvaluateOnNewDocument', { identifier: initId });
  ws.close();
})().catch(e => { console.error(e); process.exitCode = 1; });
