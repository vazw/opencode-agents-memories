// Run against a disposable engine serving this build and Chrome CDP on port 9222.
const assert = require('node:assert/strict');
const http = require('node:http');
const base = process.env.TEST_BASE_URL || 'http://127.0.0.1:49381';
const get = url => new Promise((resolve, reject) => http.get(url, r => { let body = ''; r.on('data', c => body += c); r.on('end', () => resolve(JSON.parse(body))); }).on('error', reject));
(async () => {
  const page = (await get('http://127.0.0.1:9222/json')).find(p => p.type === 'page' && p.url.startsWith(base + '/skill-library'));
  assert(page, 'open skill-library on isolated engine first');
  const ws = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { ws.onopen = resolve; ws.onerror = reject; });
  let id = 0; const pending = new Map();
  ws.onmessage = event => { const m = JSON.parse(event.data); if (!pending.has(m.id)) return; const [resolve, reject] = pending.get(m.id); pending.delete(m.id); m.error ? reject(Error(m.error.message)) : resolve(m.result); };
  const call = (method, params = {}) => new Promise((resolve, reject) => { const n = ++id; pending.set(n, [resolve, reject]); ws.send(JSON.stringify({id:n, method, params})); });
  const evaluate = async expression => { const r = await call('Runtime.evaluate', {expression, awaitPromise:true, returnByValue:true}); if (r.exceptionDetails) throw Error(r.exceptionDetails.text); return r.result.value; };
  try {
    await call('Page.navigate', {url:base + '/skill-library'});
    await evaluate(`new Promise((resolve, reject) => { let n=0; const t=setInterval(() => {if(document.querySelector('[data-testid="skill-create"]')){clearInterval(t);resolve()} else if(++n>100){clearInterval(t);reject(Error('page not ready'))}},100) })`);
    const result = await evaluate(`(async () => {
      const wait = ms => new Promise(r => setTimeout(r, ms));
      const q = id => document.querySelector('[data-testid="'+id+'"]');
      const open = async () => { q('skill-create').click(); for(let i=0;i<100 && !q('skill-form');i++) await wait(50); const form=q('skill-form'); if(!form) throw Error('form missing'); form.addEventListener('submit', e => { window.__submits++; e.preventDefault(); }); return form; };
      const set = (id,value) => { let el=q(id); if(el.tagName!=='INPUT' && el.tagName!=='TEXTAREA') el=el.querySelector('input,textarea'); Object.getOwnPropertyDescriptor(el.tagName==='TEXTAREA'?HTMLTextAreaElement.prototype:HTMLInputElement.prototype,'value').set.call(el,value); el.dispatchEvent(new Event('input',{bubbles:true})); };
      window.__submits=0; const url=location.pathname+location.search; await open();
      set('skill-form-name','Regression browser fixture'); set('skill-form-description','Valid description'); set('skill-form-content','Valid content');
      q('skill-save').click(); for(let i=0;i<100 && ![...document.querySelectorAll('[data-testid="skill-name"]')].some(e=>e.textContent==='Regression browser fixture');i++) await wait(100);
      const saved=[...document.querySelectorAll('[data-testid="skill-name"]')].filter(e=>e.textContent==='Regression browser fixture').length;
      return {submits:window.__submits,saved,url,after:location.pathname+location.search};
    })()`);
    assert.equal(result.saved, 1, 'valid skill saved exactly once');
    assert.equal(result.submits, 0, 'Save must not submit form');
    assert.equal(result.after, result.url, 'Save must not navigate');
    const cancelled = await evaluate(`(async () => { const wait=ms=>new Promise(r=>setTimeout(r,ms)); const q=id=>document.querySelector('[data-testid="'+id+'"]'); q('skill-create').click(); for(let i=0;i<100 && !q('skill-form');i++) await wait(50); const f=q('skill-form'); if(!f) throw Error('form missing'); f.addEventListener('submit',e=>{window.__submits++;e.preventDefault()}); q('skill-cancel').click(); await wait(100); return {submits:window.__submits,open:!!q('skill-form'),url:location.pathname+location.search,rows:document.querySelectorAll('[data-testid="skill-row"]').length}; })()`);
    assert.equal(cancelled.submits, 0, 'Cancel must not submit form');
    assert.equal(cancelled.open, false, 'Cancel closes form');
    assert.equal(cancelled.url, result.url, 'Cancel must not navigate');
    assert.equal(cancelled.rows, 1, 'Cancel must not save');
    await evaluate(`(async () => { const wait=ms=>new Promise(r=>setTimeout(r,ms)); const q=id=>document.querySelector('[data-testid="'+id+'"]'); q('skill-create').click(); for(let i=0;i<100 && !q('skill-form');i++) await wait(50); const f=q('skill-form'); if(!f) throw Error('form missing'); let el=q('skill-form-name'); (el.tagName==='INPUT'?el:el.querySelector('input')).focus(); })()`);
    await call('Input.dispatchKeyEvent',{type:'keyDown',key:'Enter',code:'Enter',windowsVirtualKeyCode:13,nativeVirtualKeyCode:13});
    await call('Input.dispatchKeyEvent',{type:'keyUp',key:'Enter',code:'Enter',windowsVirtualKeyCode:13,nativeVirtualKeyCode:13});
    const enter = await evaluate(`new Promise(r=>setTimeout(()=>r({url:location.pathname+location.search,rows:document.querySelectorAll('[data-testid="skill-row"]').length,form:!!document.querySelector('[data-testid="skill-form"]')}),500))`);
    assert.equal(enter.url, result.url, 'Enter must not navigate'); assert.equal(enter.rows, 1, 'Enter must not save'); assert.equal(enter.form, true, 'Enter keeps the form open');
    console.log('PASS Save, Cancel, Enter');
  } finally { ws.close(); }
})().catch(e => { console.error(e); process.exitCode=1; });
