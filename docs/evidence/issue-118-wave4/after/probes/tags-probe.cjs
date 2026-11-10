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
const SELECTOR = '[data-testid="tags-panel"] a';
const install = `(()=>{const f=${JSON.stringify(fixture)};window.__requests=[];const o=window.fetch.bind(window);window.fetch=async(i,opts={})=>{const u=new URL(typeof i==='string'?i:i.url,location.href);if(!u.pathname.startsWith('/api/'))return o(i,opts);let data,status=200;if(u.pathname==='/api/memories'){await new Promise(r=>setTimeout(r,600));data=f.memories}else if(u.pathname==='/api/health'){data=f.health}else if(u.pathname==='/api/stats'){data=f.stats}else if(u.pathname==='/api/tags'){data=f.tags}else if(u.pathname==='/api/user-profiles'){data=f.roster}else if(u.pathname==='/api/user-profile'){data=f.profile}else if(u.pathname==='/api/skills'){data=f.skills}else if(u.pathname==='/api/projects'){data=[{projectId:'fixture-project',sessionCount:1,milestoneCount:1,lastActivityAt:1730462400000}]}else if(u.pathname.endsWith('/milestones')){data=[{id:'fixture-ms-1',projectId:'fixture-project',agentId:'fixture-agent',summary:'Synthetic milestone summary',sourceCount:1,windowStart:1730462400000,windowEnd:1730462400000,createdAt:1730462400000,promotedFacts:[],contributors:[{sessionRowId:'fixture-session-row-1',sessionId:'fixture-session-1',parentSessionId:null,agentId:'fixture-agent',agentName:'Fixture Agent',createdAt:1730462400000,summary:'Synthetic session summary'}]}]}else if(u.pathname.includes('/sessions')){data=[{id:'fixture-session-1',agentName:'Fixture Agent',summary:'Synthetic session summary',createdAt:1730462400000,orphaned:false}]}else{status=599;data={error:'Unknown fixture path'}}return new Response(JSON.stringify(status===200?{success:true,data}:data),{status,headers:{'Content-Type':'application/json'}})}})();`;
(async () => {
  const pages = await (await fetch('http://127.0.0.1:9222/json')).json();
  const page = pages.find(p => p.id === targetId && p.type === 'page');
  if (!page) throw Error('lane target not found');
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((res, rej) => { ws.onopen = res; ws.onerror = rej; });
  let serial = 0; const pending = new Map();
  ws.onmessage = e => { const m = JSON.parse(e.data); const p = pending.get(m.id); if (p) { pending.delete(m.id); m.error ? p.reject(Error(m.error.message)) : p.resolve(m.result); } };
  const call = (method, params = {}) => new Promise((resolve, reject) => { const id = ++serial; console.log(method); const timer = setTimeout(() => { pending.delete(id); reject(Error('CDP timeout: ' + method)); ws.close(); }, 25000); pending.set(id, { resolve: value => { clearTimeout(timer); resolve(value); }, reject: error => { clearTimeout(timer); reject(error); } }); ws.send(JSON.stringify({ id, method, params })); });
  const evaluate = async expression => { const r = await call('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true }); if (r.exceptionDetails) throw Error(JSON.stringify(r.exceptionDetails)); return r.result.value; };
  const ready = sel => evaluate(`new Promise((resolve,reject)=>{let clean=0;const samples=[];const start=Date.now();const t=setInterval(()=>{const ok=!!document.querySelector(${JSON.stringify(sel)})&&!document.querySelector('.thaw-spinner,[data-testid$="-loading"]');samples.push({elapsed:Date.now()-start,clean});clean=ok?clean+1:0;if(clean===3){clearInterval(t);resolve(samples)}else if(Date.now()-start>20000){clearInterval(t);reject(Error('timeout'))}},100)})`);
  const realKey = async (key, code, vk) => { await call('Input.dispatchKeyEvent', { type: 'keyDown', key, code, windowsVirtualKeyCode: vk }); await call('Input.dispatchKeyEvent', { type: 'keyUp', key, code, windowsVirtualKeyCode: vk }); };
  await call('Page.enable'); await call('Runtime.enable');
  const rows = []; let initId;
  for (const theme of ['light', 'dark']) {
    if (initId) await call('Page.removeScriptToEvaluateOnNewDocument', { identifier: initId });
    ({ identifier: initId } = await call('Page.addScriptToEvaluateOnNewDocument', { source: install + `;localStorage.setItem('opencode-mem-theme',${JSON.stringify(theme)});` }));
    await call('Emulation.setDeviceMetricsOverride', { width: 390, height: 844, deviceScaleFactor: 1, mobile: false });
    await call('Page.navigate', { url: base + '/tags' });
    await evaluate(`new Promise(r=>{if(document.readyState==='complete')r();else addEventListener('load',r,{once:true})})`);
    const samples = await ready(SELECTOR);
    await call('Page.bringToFront');
    let firstTabHref = null;
    let tabSteps = 0;
    for (let i = 1; i <= 25; i++) {
      await realKey('Tab', 'Tab', 9);
      const info = await evaluate(`(()=>{const a=document.activeElement;return{tag:a.tagName,href:a.getAttribute('href'),inTagsPanel:!!a.closest('[data-testid="tags-panel"]')}})()`);
      if (i === 1) firstTabHref = info.href;
      if (info.inTagsPanel && info.tag === 'A') { tabSteps = i; break; }
    }
    const g = await evaluate(`(()=>{const links=document.querySelectorAll(${JSON.stringify(SELECTOR)});const a=links[0];const parse=s=>s.match(/[\\d.]+/g).map(Number);const lum=rgb=>{const f=v=>{v/=255;return v<=0.03928?v/12.92:Math.pow((v+0.055)/1.055,2.4)};return 0.2126*f(rgb[0])+0.7152*f(rgb[1])+0.0722*f(rgb[2])};const fg=parse(getComputedStyle(a).color).slice(0,3);let node=a,bg=null;while(node){const b=getComputedStyle(node).backgroundColor;const m=b.match(/[\\d.]+/g);if(m&&(m.length<4||Number(m[3])>0)){bg=parse(b).slice(0,3);break}node=node.parentElement}const L1=lum(fg),L2=lum(bg);const active=document.activeElement;return{links:links.length,href:a.href,rects:a.getClientRects().length,contrast:(Math.max(L1,L2)+0.05)/(Math.min(L1,L2)+0.05),activeTag:active.tagName,activeInTagsPanel:!!active.closest('[data-testid="tags-panel"]'),activeHref:active.getAttribute('href'),activeOutline:getComputedStyle(active).outlineStyle,activeOutlineWidth:getComputedStyle(active).outlineWidth,activeOutlineColor:getComputedStyle(active).outlineColor,pageOverflow:document.documentElement.scrollWidth>document.documentElement.clientWidth}})()`);
    const name = `after-tags-390-${theme}.png`;
    const { data } = await call('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(path.join(out, name), Buffer.from(data, 'base64'));
    rows.push({ theme, readySamples: samples.length, firstTabHref, tabSteps, ...g, spaceEncoded: g.href.includes('%20'), unicodeRoundTrip: decodeURIComponent(g.href).includes('空間'), screenshot: name, screenshotSha256: sha(fs.readFileSync(path.join(out, name))) });
  }
  fs.writeFileSync(path.join(out, 'tags-probe.json'), JSON.stringify({ utc: new Date().toISOString(), base, targetId, servedBundle: 'dist-after-r3 web-ui-ca2b5df9fdce9bc9', selector: SELECTOR, rows }, null, 2) + '\n');
  console.log(JSON.stringify(rows, null, 1));
  if (initId) await call('Page.removeScriptToEvaluateOnNewDocument', { identifier: initId });
  ws.close();
})().catch(e => { console.error(e); process.exitCode = 1; });
