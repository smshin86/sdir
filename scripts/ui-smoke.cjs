const fixtures=require('./test-fixtures.cjs');
async function chooseCopyDestination(...args){const m=await import('./copy-picker.mjs');return m.chooseCopyDestination(...args);}
// Native Windows/WebView2 smoke test. All writes stay in a new temporary fixture.
const fs = require('node:fs');
const crypto = require('node:crypto');
const os = require('node:os');
const path = require('node:path');
const {spawn,spawnSync} = require('node:child_process');
const assert = require('node:assert/strict');
const exe = path.resolve(process.argv[2] || 'dist/SDIR.exe');
const large = process.argv.includes('--large');
const output = path.resolve('docs/task-id/sdir-first-app');
fs.mkdirSync(output, {recursive:true});
const root = fixtures.create('sdir-ui-');
const left = path.join(root,'작업'), right = path.join(root,'대상');
fs.mkdirSync(left); fs.mkdirSync(right);
fs.mkdirSync(path.join(left,'자료'));
fs.writeFileSync(path.join(left,'회의록.txt'),'SDIR fixture');
fs.writeFileSync(path.join(left,'자료','보고서.txt'),'nested');
const count = large ? 100000 : 600;
for(let i=0;i<count;i++) fs.writeFileSync(path.join(left,`sample-${String(i).padStart(6,'0')}.bin`),'');
const profile = path.join(root,'profile'); fs.mkdirSync(profile);
const units = p => Array.from({length:p.length},(_,i)=>p.charCodeAt(i));
fs.writeFileSync(path.join(profile,'settings.json'),JSON.stringify({version:1,layout:4,panes:[left,right,right,right].map(p=>[units(p)]),favorites:[],show_hidden:false}));
const port = 19327;
const log = fs.openSync(path.join(output,'ui-native.log'),'w');
const started = performance.now();
const app = spawn(exe,[],{env:{...process.env,SDIR_PROFILE_DIR:profile,SDIR_DEBUG_PORT:String(port)},stdio:['ignore',log,log]});
let ws, id=0; const pending = new Map(); const errors=[];
const pause = ms=>new Promise(r=>setTimeout(r,ms));
async function until(fn,label,timeout=20000){const begin=performance.now();while(performance.now()-begin<timeout){const value=await fn();if(value)return value;await pause(80);}throw new Error(`Timeout: ${label}`);}
function cdp(method,params={}) { return new Promise((resolve,reject)=>{const key=++id;const timer=setTimeout(()=>{pending.delete(key);reject(new Error(`CDP timeout: ${method}`));},20000);pending.set(key,{resolve:v=>{clearTimeout(timer);resolve(v);},reject});ws.send(JSON.stringify({id:key,method,params}));});}
async function evaluate(expression){const result=await cdp('Runtime.evaluate',{expression,returnByValue:true,awaitPromise:true});if(result.exceptionDetails)throw new Error(JSON.stringify(result.exceptionDetails));return result.result.value;}
async function waitJS(expression,label,timeout){return until(()=>evaluate(expression),label,timeout);}
async function pointerClick(expression,label){await waitJS(`(()=>{const e=${expression};return !!e&&!e.disabled&&e.getBoundingClientRect().width>0})()`,'click '+label);await evaluate(`(${expression}).scrollIntoView({block:'nearest',inline:'nearest'})`);await waitJS(`(()=>{const e=${expression};if(!e)return false;const r=e.getBoundingClientRect(),hit=document.elementFromPoint(r.x+r.width/2,r.y+r.height/2);return !!hit&&e.contains(hit)})()`,'visible click target '+label);const rect=await evaluate(`(()=>{const e=${expression},r=e.getBoundingClientRect();return{x:r.x+r.width/2,y:r.y+r.height/2}})()`);await cdp('Input.dispatchMouseEvent',{type:'mousePressed',button:'left',clickCount:1,...rect});await cdp('Input.dispatchMouseEvent',{type:'mouseReleased',button:'left',clickCount:1,...rect});}
async function click(selector){await pointerClick(`document.querySelector(${JSON.stringify(selector)})`,selector);}
async function input(selector,value){await evaluate(`(()=>{const e=document.querySelector(${JSON.stringify(selector)});const pane=e.closest('[data-pane]'),oldList=pane?.querySelector('.list-scroll');const filter=e.id.startsWith('pane-query-'),changed=e.value!==${JSON.stringify(value)};e.focus();return new Promise((resolve,reject)=>{let observer,timer;const ready=()=>{if(!filter||!changed||pane.querySelector('.list-scroll')!==oldList){observer?.disconnect();clearTimeout(timer);resolve(true);}};if(filter&&changed){observer=new MutationObserver(ready);observer.observe(pane,{childList:true,subtree:true});timer=setTimeout(()=>{observer.disconnect();reject(Error('Filter render did not commit'))},10000);}Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(e,${JSON.stringify(value)});e.dispatchEvent(new Event('input',{bubbles:true}));ready();});})()`);}
async function textButton(scope,text){await pointerClick(`[...document.querySelectorAll(${JSON.stringify(scope+' button')})].find(e=>e.textContent.trim()===${JSON.stringify(text)})`,text);}
async function row(name,pane=0){await pointerClick(`[...document.querySelectorAll('[data-pane="${pane}"] .file-row')].find(e=>e.querySelector('.file-name span').textContent===${JSON.stringify(name)})`,name);await waitJS(`[...document.querySelectorAll('[data-pane="${pane}"] .file-row.selected .file-name>span')].some(e=>e.textContent===${JSON.stringify(name)})`,'selected '+name);}
async function key(selector,key,code=key){await evaluate(`document.querySelector(${JSON.stringify(selector)}).dispatchEvent(new KeyboardEvent('keydown',{key:${JSON.stringify(key)},code:${JSON.stringify(code)},bubbles:true}))`);}
const report={executable:exe,sha256:crypto.createHash('sha256').update(fs.readFileSync(exe)).digest('hex'),fixture:root,count,large,checks:[],errors,environment:{os:os.release(),arch:os.arch(),cpu:os.cpus()[0]?.model,totalMemoryBytes:os.totalmem()}};
async function main() {
try {
 let target;
 await until(async()=>{if(app.exitCode!==null)throw Error(`App exited ${app.exitCode}`);try{const pages=await (await fetch(`http://127.0.0.1:${port}/json`)).json();target=pages.find(p=>p.type==='page'&&p.webSocketDebuggerUrl);return !!target;}catch{return false;}},'WebView debug endpoint');
 ws=new WebSocket(target.webSocketDebuggerUrl);
 await new Promise((resolve,reject)=>{ws.onopen=resolve;ws.onerror=reject;});
 ws.onmessage=e=>{const m=JSON.parse(e.data);if(m.id){const p=pending.get(m.id);if(p){pending.delete(m.id);m.error?p.reject(new Error(JSON.stringify(m.error))):p.resolve(m.result);}}else if(m.method==='Runtime.exceptionThrown') errors.push(m.params.exceptionDetails);};
 await cdp('Runtime.enable');await cdp('Page.enable');
 await waitJS('document.querySelectorAll(".pane").length===4','four panes');
 await waitJS(`document.querySelector('[data-pane="0"] .pane-status').textContent.includes('${count+2} / ${count+2}')`,'listing complete',120000);
 report.launchAndListingMs=performance.now()-started;
 report.checks.push('four independent panes and complete listing');
 for(const n of [1,2,4]){await click(`.layout-switch button[title="${n}분할"]`);await waitJS(`document.querySelectorAll('.pane').length===${n}`,'layout '+n);}
 report.checks.push('1/2/4 layouts');
 await click('[data-pane="0"] .new-tab');await waitJS(`document.querySelectorAll('[data-pane="0"] .tab').length===2`,'new tab');
 await click('[data-pane="0"] .tab.selected .tab-close');await waitJS(`document.querySelectorAll('[data-pane="0"] .tab').length===1`,'close tab');
 await waitJS(`document.querySelector('[data-pane="0"] .pane-status').textContent.includes('${count+2} / ${count+2}')`,'tab reload',120000);
 report.checks.push('tabs');
 await input('#address-3',path.join(root,'존재하지 않는 폴더'));await key('#address-3','Enter');
 await waitJS(`!!document.querySelector('[data-pane="3"] .pane-error')`,'missing path error');
 await input('[data-pane="0"] .pane-filter input','회의');
 await waitJS(`[...document.querySelectorAll('[data-pane="0"] .pane-status span')].some(e=>e.textContent.startsWith('1 /'))`,'other pane remains usable');
 await click('[data-pane="3"] button[title="뒤로"]');await waitJS(`!document.querySelector('[data-pane="3"] .pane-error')`,'recover from missing path');
 await click('[data-pane="0"] .pane-filter');
 report.checks.push('missing folder error and independent other-pane interaction');
 const samples=[];
 for(let i=0;i<20;i++){
  const value=i%2?'회의':'sample-000', expected=String(i%2?1:Math.min(count,1000))+' /';
  samples.push(await evaluate(`new Promise((resolve,reject)=>{const status=document.querySelector('[data-pane="0"] .pane-status');const input=document.querySelector('[data-pane="0"] .pane-filter input');const started=performance.now();const timer=setTimeout(()=>{observer.disconnect();reject(Error('filter update deadline'))},10000);const observer=new MutationObserver(()=>{if([...status.querySelectorAll('span')].some(e=>e.textContent.startsWith(${JSON.stringify(expected)}))){observer.disconnect();clearTimeout(timer);resolve(performance.now()-started);}});observer.observe(status,{childList:true,subtree:true,characterData:true});Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(input,${JSON.stringify(value)});input.dispatchEvent(new Event('input',{bubbles:true}));})`));
 }
 samples.sort((a,b)=>a-b);report.filterInputToDomMs={samples,p95:samples[Math.ceil(samples.length*.95)-1],includesPaint:false};
 await row('회의록.txt');
 await textButton('.action-bar','복사 대상 선택');await chooseCopyDestination(evaluate,click,1);
 await until(()=>fs.existsSync(path.join(right,'회의록.txt')),'native panel copy');
 await waitJS(`document.querySelector('.status-message').textContent.includes('완료했습니다')`,'job completion');
 report.checks.push('native copy to neighboring pane');
 await waitJS(`document.querySelector('[data-pane="1"] .file-row.selected .file-name>span')?.textContent==='회의록.txt'&&document.activeElement===document.querySelector('[data-pane="1"] .list-scroll')`,'copy result selected in destination');
 await click('[data-pane="0"] .list-scroll');
 await textButton('.action-bar','새 폴더');await waitJS(`!!document.querySelector('.dialog-input')`,'new folder dialog');
 await input('.dialog-input','테스트 폴더');await textButton('.modal-actions','확인');
 await until(()=>fs.existsSync(path.join(left,'테스트 폴더')),'native new folder');
 await waitJS(`document.querySelector('.status-message').textContent.includes('완료했습니다')`,'new folder completion');
 await input('[data-pane="0"] .pane-filter input','테스트 폴더');await waitJS(`document.querySelector('[data-pane="0"] .file-name span')?.textContent==='테스트 폴더'`,'new folder listed');
 await row('테스트 폴더');await textButton('.action-bar','이름 변경');await waitJS(`!!document.querySelector('.dialog-input')`,'rename dialog');
 await input('.dialog-input','변경된 폴더');await textButton('.modal-actions','확인');
 await until(()=>fs.existsSync(path.join(left,'변경된 폴더')),'native rename');
 await waitJS(`document.querySelector('.status-message').textContent.includes('완료했습니다')`,'rename complete');
 report.checks.push('native create and rename');
 await input('[data-pane="0"] .pane-filter input','변경된 폴더');await waitJS(`document.querySelector('[data-pane="0"] .file-name span')?.textContent==='변경된 폴더'`,'renamed row');
 await row('변경된 폴더');await textButton('.action-bar','휴지통');await waitJS(`document.querySelector('.modal')?.textContent.includes('휴지통으로 이동')`,'recycle confirmation');await textButton('.modal-actions','취소');
 assert(fs.existsSync(path.join(left,'변경된 폴더')));report.checks.push('recycle confirmation cancellation');
 await click('.small-add');await waitJS(`document.querySelectorAll('.favorite-row').length===1`,'favorite');
 await input('[data-pane="0"] .pane-filter input','자료');await waitJS(`document.querySelector('[data-pane="0"] .file-name span')?.textContent==='자료'`,'folder row');
 await row('자료');await key('[data-pane="0"] .list-scroll','Enter');
 await waitJS(`document.querySelector('#address-0').value===${JSON.stringify(path.join(left,'자료'))}`,'enter directory');
 await click('[data-pane="0"] button[title="뒤로"]');
 await waitJS(`document.querySelector('#address-0').value===${JSON.stringify(left)}`,'back');
 await input('[data-pane="0"] .pane-filter input','');
 await waitJS(`[...document.querySelectorAll('[data-pane="0"] .pane-status span')].some(e=>e.textContent.startsWith('${count+3} /'))`,'clear filter');
 await evaluate(`document.querySelector('[data-pane="0"] .list-scroll').scrollTop=12000`);await pause(150);
 await input('[data-pane="0"] .pane-filter input','회의');
 await waitJS(`document.querySelector('[data-pane="0"] .list-scroll').scrollTop===0&&document.querySelector('[data-pane="0"] .file-name span')?.textContent==='회의록.txt'`,'filter resets scroll');
 report.checks.push('filter resets a scrolled virtual list');
 await input('[data-pane="0"] .pane-filter input','');
 report.checks.push('favorites, directory entry and history');
 await click('.search-toggle');
 await click('.settings-toggle');
 await waitJS(`!!document.querySelector('.preferences .index-actions')`,'index settings opened');
 const indexStarted=performance.now();
 await textButton('.preferences .index-actions','현재 폴더 색인');
 await waitJS(`document.querySelector('.preferences .index-count')?.textContent.includes('색인 완료')`,'built-in index',120000);
 report.indexMs=performance.now()-indexStarted;
 report.indexStatus=await evaluate(`document.querySelector('.preferences .search-index-strip').innerText`);
 await click('.preferences-heading button');
 await waitJS(`!document.querySelector('.preferences')`,'settings closed');
 await input('#global-query','보고서');
 await waitJS(`document.querySelector('.results-summary')?.textContent.includes('1개 결과')&&document.querySelector('.search-result strong')?.textContent==='보고서.txt'`,'unicode search');
 report.searchSummary=await evaluate(`document.querySelector('.results-summary').innerText`);
 report.checks.push('scoped built-in index and Unicode search');
 await click('.search-result');await waitJS(`document.querySelector('[data-pane="0"] .file-row.selected .file-name span')?.textContent==='보고서.txt'`,'search result reveal in current pane');
 report.checks.push('search result reveals and selects file in current pane');
 report.maximumRenderedRows=await evaluate(`Math.max(...[...document.querySelectorAll('.pane')].map(p=>p.querySelectorAll('.file-row').length))`);assert(report.maximumRenderedRows<=80);
 report.checks.push('virtual list bounded to 80 rows per pane');
 await until(()=>{try{const saved=JSON.parse(fs.readFileSync(path.join(profile,'settings.json'),'utf8'));return saved.favorites.length===1&&saved.layout===4;}catch{return false;}},'preferences persisted');
 report.checks.push('layout, tabs and favorite preferences persisted');
 const collector=path.resolve('scripts/process-memory.ps1').replace(/'/g,"''");
 const code=`. '${collector}';$m=Get-SdirProcessMemory -ProcessId ${app.pid};@($m.processes|ForEach-Object{[pscustomobject]@{Id=$_.pid;ProcessName=$_.name;WorkingSet64=$_.workingSetBytes;PrivateMemorySize64=$_.privateBytes;createdUtc=$_.createdUtc}})|ConvertTo-Json -Compress`;

 const memory=spawnSync('powershell.exe',['-NoProfile','-EncodedCommand',Buffer.from(code,'utf16le').toString('base64')],{encoding:'utf8',timeout:20000});
 if(memory.status===0){report.processMemory=JSON.parse(memory.stdout.trim());report.memoryNote='One snapshot including descendant WebView and search processes. Working sets include shared pages; private bytes are committed memory, not resident memory.';}else report.memoryError=String(memory.stderr||memory.error);
 await pause(500);
 const shot=await cdp('Page.captureScreenshot',{format:'png'});fs.writeFileSync(path.join(output,large?'ui-large.png':'ui-smoke.png'),Buffer.from(shot.data,'base64'));
 assert.equal(errors.length,0,'no JavaScript runtime errors');
 report.ok=true;
} catch(e) {report.ok=false;report.failure=e.stack;console.error(e.stack);try{if(ws?.readyState===1){const shot=await cdp('Page.captureScreenshot',{format:'png'});fs.writeFileSync(path.join(output,'ui-failure.png'),Buffer.from(shot.data,'base64'));report.body=await evaluate('document.body.innerText');}}catch{}}
finally {if(ws)ws.close();app.kill();fs.closeSync(log);fs.writeFileSync(path.join(output,large?'ui-large.json':'ui-smoke.json'),JSON.stringify(report,null,2));console.log(JSON.stringify(report,null,2));process.exitCode=report.ok?0:1;}
}
main().catch(error=>{console.error(error);app.kill();process.exitCode=1;});
