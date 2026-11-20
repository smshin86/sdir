import fixtures from './test-fixtures.cjs';
// J09/J10: named panel groups, preferences, and restart persistence on native Windows.
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import crypto from 'node:crypto';
import {spawn,spawnSync} from 'node:child_process';
import assert from 'node:assert/strict';
const exe=path.resolve(process.argv[2]||'dist/SDIR.exe');
const output=path.resolve('docs/task-id/pane-groups');fs.mkdirSync(output,{recursive:true});
const root=fixtures.create('sdir-preferences-');
const folders=['업무','제안','개발','참고'].map(name=>path.join(root,name));for(const p of folders)fs.mkdirSync(p);
const nested=path.join(folders[0],'하위자료');fs.mkdirSync(nested);
for(let i=0;i<150;i++)fs.writeFileSync(path.join(folders[0],`file-${String(i).padStart(4,'0')}.txt`),'fixture');
for(const p of folders) {fs.writeFileSync(path.join(p,'공개.txt'),'visible');fs.writeFileSync(path.join(p,'숨김.txt'),'hidden');const a=spawnSync('attrib.exe',['+H',path.join(p,'숨김.txt')],{encoding:'utf8'});assert.equal(a.status,0);}
const profile=path.join(root,'profile');fs.mkdirSync(profile);
const units=p=>Array.from({length:p.length},(_,i)=>p.charCodeAt(i));
fs.writeFileSync(path.join(profile,'settings.json'),JSON.stringify({version:1,layout:4,panes:folders.map(p=>[units(p)]),favorites:[],show_hidden:false}));
const port=19329,pause=ms=>new Promise(r=>setTimeout(r,ms));let app,ws,id=0;const pending=new Map();
const report={fixture:root,executable:exe,sha256:crypto.createHash('sha256').update(fs.readFileSync(exe)).digest('hex'),checks:[]};
async function until(fn,label,limit=20000){const t=Date.now();while(Date.now()-t<limit){if(await fn())return;await pause(40);}throw Error('Timeout: '+label);}
function command(method,params={}){return new Promise((resolve,reject)=>{const key=++id;const timer=setTimeout(()=>{pending.delete(key);reject(Error('CDP '+method))},15000);pending.set(key,{resolve:r=>{clearTimeout(timer);resolve(r)},reject});ws.send(JSON.stringify({id:key,method,params}));});}
async function evaluate(expression){const result=await command('Runtime.evaluate',{expression,returnByValue:true,awaitPromise:true});if(result.exceptionDetails)throw Error(JSON.stringify(result.exceptionDetails));return result.result.value;}
async function connect(){
 let page;
 await until(async()=>{if(app.exitCode!==null)throw Error('App exited early');try{const pages=await(await fetch(`http://127.0.0.1:${port}/json`)).json();page=pages.find(p=>p.type==='page'&&p.webSocketDebuggerUrl);return !!page;}catch{return false;}},'WebView');
 ws=new WebSocket(page.webSocketDebuggerUrl);await new Promise((resolve,reject)=>{ws.onopen=resolve;ws.onerror=reject;});
 ws.onmessage=e=>{const m=JSON.parse(e.data);if(m.id){const p=pending.get(m.id);if(p){pending.delete(m.id);m.error?p.reject(Error(JSON.stringify(m.error))):p.resolve(m.result)}}};
 await command('Runtime.enable');await command('Page.enable');
}
function launch(){app=spawn(exe,[],{env:{...process.env,SDIR_PROFILE_DIR:profile,SDIR_DEBUG_PORT:String(port)},stdio:'ignore'});}
async function waitJS(expression,label){await until(()=>evaluate(expression),label);}
async function nativeClose(){
 const code=`Add-Type -TypeDefinition 'using System;using System.Runtime.InteropServices;public class SdirClose{[DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h,uint m,IntPtr w,IntPtr l);}';$p=Get-Process -Id ${app.pid};if($p.MainWindowHandle -eq 0){exit 1};if(-not [SdirClose]::PostMessage($p.MainWindowHandle,16,[IntPtr]::Zero,[IntPtr]::Zero)){exit 1}`;
 const result=spawnSync('powershell.exe',['-NoProfile','-EncodedCommand',Buffer.from(code,'utf16le').toString('base64')],{encoding:'utf8',timeout:15000});assert.equal(result.status,0,'native WM_CLOSE posted');
 await until(()=>app.exitCode!==null,'normal app close',10000);ws.close();await pause(400);
}

async function click(selector){await waitJS(`!!document.querySelector(${JSON.stringify(selector)})`,'control '+selector);await evaluate(`document.querySelector(${JSON.stringify(selector)}).click()`);}
async function input(selector,value,event='input'){await waitJS(`!!document.querySelector(${JSON.stringify(selector)})`,'input '+selector);await evaluate(`(()=>{const e=document.querySelector(${JSON.stringify(selector)});Object.getOwnPropertyDescriptor(${event==='change'?'HTMLSelectElement':'HTMLInputElement'}.prototype,'value').set.call(e,${JSON.stringify(value)});e.dispatchEvent(new Event(${JSON.stringify(event)},{bubbles:true}));})()`);await pause(60);}
async function key(selector,key,extra={}){await evaluate(`document.querySelector(${JSON.stringify(selector)}).dispatchEvent(new KeyboardEvent('keydown',{key:${JSON.stringify(key)},bubbles:true,...${JSON.stringify(extra)}}))`);}
const title=i=>`[data-pane="${i}"] .group-title`;
async function named(i,name){await click(`[data-pane="${i}"] .group-name`);await input(`[data-pane="${i}"] .group-name-input`,name);await key(`[data-pane="${i}"] .group-name-input`,'Enter');await waitJS(`document.querySelector(${JSON.stringify(title(i))})?.textContent===${JSON.stringify(name.trim()||'패널 '+(i+1))}`,'group name saved');}
async function screenshot(name){const r=await command('Page.captureScreenshot',{format:'png'});fs.writeFileSync(path.join(output,name),Buffer.from(r.data,'base64'));}
try{
 launch();await connect();await waitJS(`document.querySelectorAll('.pane').length===4&&[...document.querySelectorAll('[data-pane="0"] .pane-status span')].some(e=>e.textContent.startsWith('152 /'))`,'legacy profile loaded');
 assert.deepEqual(await evaluate(`[...document.querySelectorAll('.group-title')].map(e=>e.textContent)`),['패널 1','패널 2','패널 3','패널 4']);report.checks.push('legacy settings retain paths and get default group names');
 const names=['업무용','제안용','개발용 🛠','참고용'];for(let i=0;i<4;i++)await named(i,i===0?'  업무용  ':names[i]);
 await click('[data-pane="1"] .group-name');await input('[data-pane="1"] .group-name-input','바뀌면 안 됨');await key('[data-pane="1"] .group-name-input','Escape');await waitJS(`document.querySelector('[data-pane="1"] .group-title')?.textContent==='제안용'`,'escape cancellation');
 await click('[data-pane="2"] .group-name');await input('[data-pane="2"] .group-name-input','취소할 이름');await click('[data-pane="2"] .group-cancel');await waitJS(`document.querySelector('[data-pane="2"] .group-title')?.textContent===${JSON.stringify(names[2])}`,'button cancellation');
 await named(3,'');await named(3,names[3]);report.checks.push('four names, trim, Korean/emoji, empty-name default, Escape and cancel');
 await click('[data-pane="0"] .new-tab');await waitJS(`document.querySelectorAll('[data-pane="0"] .tab').length===2`,'tab in group');await input('#address-0',nested);await key('#address-0','Enter');await waitJS(`document.querySelector('#address-0').value===${JSON.stringify(nested)}`,'nested tab path');
 for(const n of [1,2,4]){await click(`.layout-switch button[title="${n}분할"]`);await waitJS(`document.querySelectorAll('.pane').length===${n}`,'layout '+n);assert.deepEqual(await evaluate(`[...document.querySelectorAll('.group-title')].map(e=>e.textContent)`),names.slice(0,n));}
 assert.equal(await evaluate(`document.querySelectorAll('[data-pane="0"] .tab').length`),2);await click('[data-pane="0"] .tab');await waitJS(`[...document.querySelectorAll('[data-pane="0"] .pane-status span')].some(e=>e.textContent.startsWith('152 /'))`,'first tab');report.checks.push('group identity and independent tabs survive 1/2/4 layout changes');
 await click('.settings-toggle');await click('[data-theme-choice="dark"]');await waitJS(`document.querySelector('.app').dataset.theme==='dark'&&getComputedStyle(document.querySelector('.app-header')).backgroundColor==='rgb(32, 39, 53)'`,'dark theme rendered');
 await input('.density-select','comfortable','change');await waitJS(`document.querySelector('.file-row').getBoundingClientRect().height===38`,'comfortable rows');
 await click('.preferences-hidden');await waitJS(`[...document.querySelectorAll('[data-pane="0"] .pane-status span')].some(e=>e.textContent.startsWith('153 /'))`,'hidden files shown');
 await click('.preferences-heading button');
 await evaluate(`document.querySelector('[data-pane="0"] .list-scroll').scrollTop=1140`);await pause(200);
 const virtual=await evaluate(`(()=>{const p=document.querySelector('[data-pane="0"]');return {space:parseFloat(p.querySelector('.virtual-space').style.height),rows:p.querySelectorAll('.file-row').length,offset:new DOMMatrixReadOnly(getComputedStyle(p.querySelector('.virtual-rows')).transform).m42}})()`);
 assert.equal(virtual.space,153*38);assert(virtual.rows<=80);assert.equal(virtual.offset,26*38);report.checks.push('comfortable density changes physical row size and virtual offsets together');
 await click('.layout-switch button[title="1분할"]');await click('.settings-toggle');await click('.preferences-hidden');await click('.preferences-heading button');await click('.layout-switch button[title="4분할"]');
 await waitJS(`[...document.querySelectorAll('[data-pane="3"] .pane-status span')].some(e=>e.textContent.startsWith('1 /'))`,'hidden setting applied to previously hidden panes');report.checks.push('hidden preference also invalidates collapsed panes');
 await click('.settings-toggle');await click('[data-theme-choice="light"]');await waitJS(`getComputedStyle(document.querySelector('.app-header')).backgroundColor==='rgb(255, 255, 255)'`,'light theme');
 await click('[data-theme-choice="system"]');await command('Emulation.setEmulatedMedia',{features:[{name:'prefers-color-scheme',value:'dark'}]});await waitJS(`getComputedStyle(document.querySelector('.app-header')).backgroundColor==='rgb(32, 39, 53)'`,'system follows dark');await command('Emulation.setEmulatedMedia',{features:[{name:'prefers-color-scheme',value:'light'}]});await waitJS(`getComputedStyle(document.querySelector('.app-header')).backgroundColor==='rgb(255, 255, 255)'`,'system follows light');
 await click('.preferences-heading button');await screenshot('preferences-light.png');
 await click('.settings-toggle');await click('[data-theme-choice="dark"]');await click('.preferences-heading button');await screenshot('preferences-dark.png');report.checks.push('light, dark, and live system theme selection');
 await until(()=>{const s=JSON.parse(fs.readFileSync(path.join(profile,'settings.json'),'utf8'));return s.theme==='dark'&&!s.compact&&!s.show_hidden&&s.pane_names.join('|')===names.join('|')&&s.panes[0].length===2;},'preferences on disk');
 assert.deepEqual(fs.readdirSync(nested),[]);for(const p of folders)assert(fs.readFileSync(path.join(p,'공개.txt'),'utf8')==='visible');
 await nativeClose();launch();await connect();await waitJS(`document.querySelectorAll('.pane').length===4&&document.querySelectorAll('[data-pane="0"] .tab').length===2&&document.querySelector('.app').dataset.theme==='dark'&&document.querySelector('.file-row')?.getBoundingClientRect().height===38`,'restored preferences');
 assert.deepEqual(await evaluate(`[...document.querySelectorAll('.group-title')].map(e=>e.textContent)`),names);assert.deepEqual(JSON.parse(fs.readFileSync(path.join(profile,'settings.json'),'utf8')).panes[0],[units(folders[0]),units(nested)]);report.checks.push('normal restart restores all group names, theme, density and unchanged tab paths');
 await nativeClose();report.ok=true;
}catch(error){report.ok=false;report.failure=error.stack;console.error(error.stack);process.exitCode=1;try{if(ws?.readyState===1){await screenshot('preferences-failure.png');report.body=await evaluate('document.body.innerText');}}catch{}}
finally{ws?.close();if(app&&app.exitCode===null)app.kill();fs.writeFileSync(path.join(output,'preferences-journey.json'),JSON.stringify(report,null,2));console.log(JSON.stringify(report,null,2));}
