import fixtures from './test-fixtures.cjs';
// J01/J02: native close/restart, state restore, and interrupted folder reads.
import fs from 'node:fs';
import crypto from 'node:crypto';
import os from 'node:os';
import path from 'node:path';
import {spawn,spawnSync} from 'node:child_process';
import assert from 'node:assert/strict';
const exe=path.resolve(process.argv[2]||'dist/SDIR.exe');
const source=JSON.parse(fs.readFileSync(process.argv[3]||'docs/task-id/sdir-first-app/ui-smoke.json','utf8'));
const profile=fixtures.create('sdir-recovery-');
fs.copyFileSync(path.join(source.fixture,'profile','settings.json'),path.join(profile,'settings.json'));
const expected=path.join(source.fixture,'작업','자료');
const port=19328, pause=ms=>new Promise(r=>setTimeout(r,ms));
let app,ws,id=0;const pending=new Map();
const report={executable:exe,sha256:crypto.createHash('sha256').update(fs.readFileSync(exe)).digest('hex'),fixture:source.fixture,profile,checks:[],gaps:[]};
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
try{
 launch();await connect();await waitJS(`document.querySelectorAll('.pane').length===4`,'initial workspace');
 if(source.large){
  await waitJS(`!!document.querySelector('[data-pane="0"] button[title="새로고침 · F5"]')&&!!document.querySelector('[data-pane="0"] .file-row')&&!document.querySelector('[data-pane="0"] .pane-status')?.textContent.includes('읽는 중')`,'initial large listing completed',120000);
  // Arm before requesting a fresh read; a warm initial load may finish before CDP attaches.
  report.cancelProbe=await evaluate(`new Promise(resolve=>{const pane=document.querySelector('[data-pane="0"]');let finished=false;const stop=clicked=>{if(finished)return;finished=true;observer.disconnect();clearTimeout(timer);resolve({clicked});};const observer=new MutationObserver(()=>{const button=pane.querySelector('button[title="읽기 취소"]');if(button){button.click();stop(true);}});const timer=setTimeout(()=>stop(false),3000);observer.observe(pane,{attributes:true,attributeFilter:['title'],childList:true,subtree:true});pane.querySelector('button[title="새로고침 · F5"]').click();})`);
  if(report.cancelProbe.clicked){
   await waitJS(`document.querySelector('.status-message').textContent.includes('읽기를 취소')`,'read cancelled');
   const status=await evaluate(`[...document.querySelectorAll('[data-pane="0"] .pane-status span')].find(e=>e.textContent.includes(' / '))?.textContent`);await pause(300);
   assert.equal(await evaluate(`[...document.querySelectorAll('[data-pane="0"] .pane-status span')].find(e=>e.textContent.includes(' / '))?.textContent`),status,'old batches cannot change cancelled list');
   report.checks.push('large folder refresh cancellation and rejection of late batches');
  }else{report.gaps.push('Large listing completed without an observable cancel state; cancellation was not counted as passed.');}
 }

 await evaluate(`(()=>{const e=document.querySelector('#address-0');Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(e,${JSON.stringify(expected)});e.dispatchEvent(new Event('input',{bubbles:true}));})()`);await pause(80);
 await evaluate(`document.querySelector('#address-0').dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',code:'Enter',bubbles:true}))`);
 await waitJS(`document.querySelector('[data-pane="0"] .file-name span')?.textContent==='보고서.txt'`,'recover by navigation');
 report.checks.push(report.cancelProbe?.clicked?'new navigation recovers after cancellation':'new navigation opens the expected folder');
 await evaluate(`document.querySelector('.layout-switch button[title="2분할"]').click()`);await waitJS(`document.querySelectorAll('.pane').length===2`,'two-pane workspace');
 await until(()=>{const s=JSON.parse(fs.readFileSync(path.join(profile,'settings.json'),'utf8'));return s.layout===2&&Buffer.from(new Uint16Array(s.panes[0][0]).buffer).toString('utf16le')===expected;},'persist latest layout and tab path');
 await nativeClose();report.checks.push('normal native window close');
 launch();await connect();
 await waitJS(`document.querySelectorAll('.pane').length===2&&document.querySelector('#address-0').value===${JSON.stringify(expected)}&&document.querySelectorAll('.favorite-row').length===1`,'restored layout, tab path and favorite');
 await waitJS(`document.querySelector('[data-pane="0"] .file-name span')?.textContent==='보고서.txt'`,'restored folder listing');
 report.checks.push('restart restores saved layout, tab paths, favorite, and actual folder content');
 const screenshot=await command('Page.captureScreenshot',{format:'png'});fs.writeFileSync('docs/task-id/sdir-first-app/ui-recovery.png',Buffer.from(screenshot.data,'base64'));
 await nativeClose();report.ok=true;
}catch(error){report.ok=false;report.failure=error.stack;console.error(error.stack);process.exitCode=1;}
finally{ws?.close();if(app&&app.exitCode===null)app.kill();fs.writeFileSync('docs/task-id/sdir-first-app/ui-recovery.json',JSON.stringify(report,null,2));console.log(JSON.stringify(report,null,2));}
