import fixtures from './test-fixtures.cjs';
// J28: visible active pane and keyboard/pointer focus agree across themes and restart.
import fs from 'node:fs';import os from 'node:os';import path from 'node:path';import crypto from 'node:crypto';import {spawn,spawnSync} from 'node:child_process';import assert from 'node:assert/strict';
const exe=path.resolve(process.argv[2]||'dist/SDIR.exe'),baseline=process.argv.includes('--baseline'),root=fixtures.create('sdir-audit-');fs.writeFileSync(path.join(root,'.sdir-audit-fixture'),'owned');const names=['업무용','제안용','개발용','참고용'],folders=names.map(n=>path.join(root,n));for(const p of folders){fs.mkdirSync(p);fs.mkdirSync(path.join(p,'child'));fs.writeFileSync(path.join(p,'item.txt'),'owned');fs.writeFileSync(path.join(p,'child','nested.txt'),'nested');}const profile=path.join(root,'profile');fs.mkdirSync(profile);const units=p=>Array.from({length:p.length},(_,i)=>p.charCodeAt(i));fs.writeFileSync(path.join(profile,'settings.json'),JSON.stringify({version:1,layout:4,panes:folders.map(p=>[units(p)]),pane_names:names,theme:'light',favorites:[],show_hidden:false}));const output=path.resolve(process.env.SDIR_ACTIVE_PANE_OUTPUT||'docs/task-id/active-pane');fs.mkdirSync(output,{recursive:true});const port=19343,pause=ms=>new Promise(r=>setTimeout(r,ms));let app,ws,id=0;const pending=new Map();const report={fixture:root,executable:exe,sha256:crypto.createHash('sha256').update(fs.readFileSync(exe)).digest('hex'),checks:[]};
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

async function mouse(selector,bottom=false){await waitJS(`(()=>{const e=document.querySelector(${JSON.stringify(selector)});return !!e&&!e.disabled&&e.getBoundingClientRect().width>0})()`,'click target '+selector);await evaluate(`(()=>{const e=document.querySelector(${JSON.stringify(selector)});if(e&&!e.matches('.file-row,.tree-row')&&!e.closest('.tree-row'))e.scrollIntoView({block:'nearest',inline:'nearest'});})()`);await waitJS(`(()=>{const e=document.querySelector(${JSON.stringify(selector)});if(!e)return false;const r=e.getBoundingClientRect(),hit=document.elementFromPoint(r.x+r.width/2,${bottom?'r.bottom-8':'r.y+r.height/2'});return !!hit&&e.contains(hit)})()`,'click hit target '+selector);const rect=await evaluate(`(()=>{const r=document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect();return{x:r.x+r.width/2,y:${bottom?'r.bottom-8':'r.y+r.height/2'}}})()`);await command('Input.dispatchMouseEvent',{type:'mousePressed',button:'left',clickCount:1,...rect});await command('Input.dispatchMouseEvent',{type:'mouseReleased',button:'left',clickCount:1,...rect});}
async function key(key,modifiers=0){const special={ArrowDown:40,ArrowUp:38,ArrowLeft:37,ArrowRight:39,PageUp:33,PageDown:34,Home:36,End:35,Enter:13,Escape:27,Delete:46,Backspace:8,F3:114,F2:113,F5:116,F6:117,Tab:9};const code=key.length===1?(/^\d$/.test(key)?'Digit'+key:'Key'+key.toUpperCase()):key;const virtual=special[key]||key.toUpperCase().charCodeAt(0);await command('Input.dispatchKeyEvent',{type:'keyDown',key,code,windowsVirtualKeyCode:virtual,modifiers,...(key.length===1&&modifiers===0?{text:key,unmodifiedText:key}:{})});await command('Input.dispatchKeyEvent',{type:'keyUp',key,code,windowsVirtualKeyCode:virtual,modifiers});}
async function focus(pane){await mouse(`[data-pane="${pane}"] .list-scroll`,true);await waitJS(`document.activeElement===document.querySelector('[data-pane="${pane}"] .list-scroll')`,'list focused');}
async function chosen(name,pane=0){await waitJS(`document.querySelector('[data-pane="${pane}"] .file-row.selected .file-name span')?.textContent===${JSON.stringify(name)}`,'selected '+name);}
async function button(text){const selector=await evaluate(`(()=>{const buttons=[...document.querySelectorAll('.action-bar button')];const i=buttons.findIndex(e=>e.textContent.trim()===${JSON.stringify(text)});if(i<0)throw Error('Missing button');return '.action-bar button:nth-of-type('+(i+1)+')'})()`);await mouse(selector);}

async function input(selector,value){await mouse(selector);await waitJS(`document.activeElement===document.querySelector(${JSON.stringify(selector)})`,'input focused '+selector);await key('a',2);await command('Input.insertText',{text:value});await waitJS(`document.querySelector(${JSON.stringify(selector)})?.value===${JSON.stringify(value)}`,'input '+selector);await pause(80);}
async function filter(pane,value){await input(`#pane-query-${pane}`,value);await pause(100);}
async function row(name,pane=0){await waitJS(`[...document.querySelectorAll('[data-pane="${pane}"] .file-name span')].some(e=>e.textContent===${JSON.stringify(name)})`,'own row ready');const selector=await evaluate(`(()=>{const rows=[...document.querySelectorAll('[data-pane="${pane}"] .file-row')];const i=rows.findIndex(r=>r.querySelector('.file-name span').textContent===${JSON.stringify(name)});if(i<0)throw Error('Missing own fixture row');return '[data-pane="${pane}"] .file-row:nth-child('+(i+1)+')'})()`);await mouse(selector);return selector;}
async function doubleClick(selector){await mouse(selector);const r=await evaluate(`(()=>{const r=document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect();return{x:r.x+r.width/2,y:r.y+r.height/2}})()`);await command('Input.dispatchMouseEvent',{type:'mousePressed',button:'left',clickCount:2,...r});await command('Input.dispatchMouseEvent',{type:'mouseReleased',button:'left',clickCount:2,...r});}
async function order(expected){await waitJS(`JSON.stringify([...document.querySelectorAll('[data-pane="0"] .file-name span')].map(e=>e.textContent))===${JSON.stringify(JSON.stringify(expected))}`,'sort '+expected.join(','));}
async function done(){await waitJS(`document.querySelector('.status-message')?.textContent.includes('파일 작업을 완료했습니다')`,'job completed');}
async function navigate(pane,destination){await input(`#address-${pane}`,destination);await key('Enter');await waitJS(`document.querySelector('#address-${pane}').value===${JSON.stringify(destination)}&&document.querySelector('[data-pane="${pane}"] .tab.selected')?.title===${JSON.stringify(destination)}&&!document.querySelector('[data-pane="${pane}"] button[title="읽기 취소"]')`,'folder ready');}
function native(action,name){const result=spawnSync('powershell.exe',['-Sta','-NoProfile','-ExecutionPolicy','Bypass','-File',path.resolve('scripts/native-shell-probe.ps1'),'-Action',action,'-Fixture',root,'-FileName',name,'-AppPid',String(app.pid)],{encoding:'utf8',timeout:20000});let value;try{value=JSON.parse(result.stdout.trim())}catch{throw Error('Native helper failed: '+result.stderr.slice(-1200));}if(result.status!==0)throw Error('Native helper did not complete owned action: '+JSON.stringify(value));return value;}
async function searchButton(){await mouse('.index-actions .primary');}
function probe(action,fileName=''){const r=spawnSync('powershell.exe',['-Sta','-NoProfile','-ExecutionPolicy','Bypass','-File',path.resolve('scripts/basics-native-probe.ps1'),'-Action',action,'-AppPid',String(app.pid),'-Fixture',root,'-FileName',fileName],{encoding:'utf8',timeout:20000});assert.equal(r.status,0,r.stderr);return JSON.parse(r.stdout.trim());}
async function focused(pane){await waitJS(`document.activeElement===document.querySelector('[data-pane="${pane}"] .list-scroll')&&document.querySelector('[data-pane="${pane}"]').classList.contains('focused')`,'focus pane '+pane);}
async function selected(pane,name){await waitJS(`document.querySelector('[data-pane="${pane}"] .file-row.selected .file-name>span')?.textContent===${JSON.stringify(name)}`,'selected '+name);await focused(pane);}
async function historyKey(redo){await focused(Number(await evaluate(`document.querySelector('.pane.focused').dataset.pane`)));await key(redo?'y':'z',2);}
async function current(index){await waitJS(`document.querySelectorAll('.pane.focused').length===1&&document.querySelector('[data-pane="${index}"]').classList.contains('focused')`,'active pane '+index);if(!baseline){await waitJS(`document.querySelectorAll('.pane-active-badge').length===1&&!!document.querySelector('[data-pane="${index}"] .pane-active-badge')&&document.querySelector('.status-meta').textContent.includes('현재 패널 ${index+1}')`,'visible active badge '+index);assert.equal(await evaluate(`document.querySelector('[data-pane="${index}"]').getAttribute('aria-current')`),'true');}}
async function visual(label){const result=await evaluate(`(()=>{const panels=[...document.querySelectorAll('.pane')];return panels.map(e=>({pane:Number(e.dataset.pane),active:e.classList.contains('focused'),border:getComputedStyle(e).borderColor,shadow:getComputedStyle(e).boxShadow,header:getComputedStyle(e.querySelector('.group-header')).backgroundColor,badge:!!e.querySelector('.pane-active-badge'),badgeColor:e.querySelector('.pane-active-badge')?getComputedStyle(e.querySelector('.pane-active-badge')).color:null,badgeBackground:e.querySelector('.pane-active-badge')?getComputedStyle(e.querySelector('.pane-active-badge')).backgroundColor:null}))})()`);report[label]=result;if(!baseline){const active=result.find(e=>e.active);for(const other of result.filter(e=>!e.active)){assert.notEqual(active.border,other.border);assert.notEqual(active.header,other.header);}assert(active.badge);const lum=color=>{const c=color.match(/[\d.]+/g).slice(0,3).map(n=>Number(n)/255).map(n=>n<=0.04045?n/12.92:((n+0.055)/1.055)**2.4);return c[0]*0.2126+c[1]*0.7152+c[2]*0.0722;};const a=lum(active.badgeColor),b=lum(active.badgeBackground);const contrast=(Math.max(a,b)+0.05)/(Math.min(a,b)+0.05);assert(contrast>=4.5,'current pane text remains readable in '+label);report[label+'BadgeContrast']=contrast;}}
async function paneTheme(theme){
 const data=await evaluate(`(()=>{
  const app=document.querySelector('.app');
  const swatch=document.createElement('div');
  swatch.style.backgroundColor='var(--soft)';
  app.appendChild(swatch);
  const expected=getComputedStyle(swatch).backgroundColor;
  swatch.remove();
  const panes=[...app.querySelectorAll('.pane')].map(pane=>{
   const read=(surface,text)=>{
    const background=pane.querySelector(surface);
    return {selector:surface,background:getComputedStyle(background).backgroundColor,
      text:getComputedStyle(text?background.querySelector(text):background).color};
   };
   return {active:pane.classList.contains('focused'),
    title:read('.group-header','.group-title'), tabs:read('.tabs'),
    selectedTab:read('.tab.selected'), columns:read('.list-header','button'),
    filter:read('.pane-filter'), status:read('.pane-status')};
  });
  return {theme:app.dataset.theme,expected,panes};
 })()`);
 report[theme+'PaneTheme']=data;
 assert.equal(data.theme,theme);
 assert.equal(data.panes.length,4);
 const luminance=color=>{
  const rgb=color.match(/[0-9.]+/g).slice(0,3).map(n=>Number(n)/255);
  const linear=rgb.map(n=>n<=0.04045?n/12.92:((n+0.055)/1.055)**2.4);
  return linear[0]*0.2126+linear[1]*0.7152+linear[2]*0.0722;
 };
 for(const [index,pane] of data.panes.entries()){
  const surfaces=[pane.tabs,pane.columns,pane.filter,pane.status];
  if(!pane.active)surfaces.push(pane.title);
  for(const row of surfaces){
   assert.equal(row.background,data.expected,theme+' pane '+index+' '+row.selector);
  }
  const textSurfaces=[pane.title,pane.selectedTab,pane.columns];
  if(theme==='dark')textSurfaces.push(pane.status);
  for(const row of textSurfaces){
   const [text,background]=[luminance(row.text),luminance(row.background)];
   row.contrast=(Math.max(text,background)+0.05)/(Math.min(text,background)+0.05);
   assert(row.contrast>=4.5,theme+' pane '+index+' '+row.selector+' text contrast '+row.contrast);
  }
 }
}
async function tabToAnotherPane(){await focus(0);for(let i=0;i<30;i++){await key('Tab');await pause(25);const pane=await evaluate(`document.activeElement.closest('[data-pane]')?.dataset.pane??null`);if(pane!==null&&Number(pane)!==0)return Number(pane);}throw Error('Keyboard Tab did not reach another pane');}
try{
 launch();await connect();await waitJS(`document.querySelectorAll('.pane').length===4&&!document.querySelector('button[title="읽기 취소"]')`,'initial folders');await visual('light');
 if(baseline){report.tabFocusPane=await tabToAnotherPane();report.modelActivePane=await evaluate(`Number(document.querySelector('.pane.focused').dataset.pane)`);report.reproduced=report.light.every(e=>e.border===report.light[0].border)&&report.light.every(e=>!e.badge);assert(report.reproduced);assert.notEqual(report.tabFocusPane,report.modelActivePane);await nativeClose();report.ok=true;}
 else{
 await current(0);await mouse('[data-pane="2"] .group-number');await current(2);await focus(2);await key('F6');await current(3);await key('F6',8);await current(2);report.checks.push('J28 exactly one active badge, accent outline/header and footer target follow mouse activation and F6/ShiftF6');
 const pane=await tabToAnotherPane();await current(pane);report.checks.push('J28 keyboard Tab into another pane changes the actual work target and indicator without requiring a click');
 await mouse('#address-2');await current(2);await key('a',2);await command('Input.insertText',{text:path.join(folders[2],'child')});await key('Enter');await waitJS(`document.querySelector('#address-2').value===${JSON.stringify(path.join(folders[2],'child'))}&&document.querySelector('[data-pane="2"] .file-name>span')?.textContent==='nested.txt'`,'navigation content');await current(2);await mouse('.settings-toggle');await waitJS(`!!document.querySelector('.preferences')`,'settings');await current(2);await mouse('[data-theme-choice="dark"]');await waitJS(`document.querySelector('.app').dataset.theme==='dark'`,'dark theme');await key('Escape');await waitJS(`!document.querySelector('.preferences')`,'close dark settings');await current(2);await visual('dark');await paneTheme('dark');let shot=await command('Page.captureScreenshot',{format:'png'});fs.writeFileSync(path.join(output,'active-dark.png'),Buffer.from(shot.data,'base64'));await mouse('.settings-toggle');await waitJS(`!!document.querySelector('.preferences')`,'reopen settings');await mouse('[data-theme-choice="light"]');await key('Escape');await waitJS(`!document.querySelector('.preferences')`,'close settings');await current(2);await paneTheme('light');report.checks.push('J28 panels follow the chosen surface palette and title and column text stays readable in light and dark themes');report.checks.push('J28 active target remains visible during address navigation and toolbar/settings focus in light and dark themes');
 await command('Emulation.setDeviceMetricsOverride',{width:980,height:580,deviceScaleFactor:1,mobile:false});await pause(80);report.minimum=await evaluate(`(()=>{const e=document.querySelector('.pane-active-badge'),r=e.getBoundingClientRect(),h=e.closest('.group-header').getBoundingClientRect();return {listHeights:[...document.querySelectorAll('.list-scroll')].map(e=>e.clientHeight),badgeFits:r.x>=h.x&&r.right<=h.right&&r.y>=h.y&&r.bottom<=h.bottom,headerHeight:h.height}})()`);assert(report.minimum.badgeFits);assert(report.minimum.listHeights.every(h=>h>=30));shot=await command('Page.captureScreenshot',{format:'png'});fs.writeFileSync(path.join(output,'active-minimum.png'),Buffer.from(shot.data,'base64'));await command('Emulation.clearDeviceMetricsOverride');report.checks.push('J28 minimum 980x580 four-pane layout retains one visible badge and at least one complete file row per pane');
 await mouse('.layout-switch button[title="1분할"]');await waitJS(`document.querySelectorAll('.pane').length===1`,'one pane');await current(0);await mouse('.layout-switch button[title="2분할"]');await waitJS(`document.querySelectorAll('.pane').length===2`,'two panes');await focus(1);await current(1);await mouse('.layout-switch button[title="4분할"]');await waitJS(`document.querySelectorAll('.pane').length===4`,'four panes');await current(1);shot=await command('Page.captureScreenshot',{format:'png'});fs.writeFileSync(path.join(output,'active-light.png'),Buffer.from(shot.data,'base64'));await nativeClose();assert.equal(JSON.parse(fs.readFileSync(path.join(profile,'settings.json'),'utf8')).active_pane,1);launch();await connect();await waitJS(`document.querySelectorAll('.pane').length===4`,'restart');await current(1);await nativeClose();report.checks.push('J28 layout changes keep exactly one visible target and normal restart restores the saved active pane');report.ok=true;
 }
}catch(error){report.ok=false;report.failure=error.stack;process.exitCode=1;try{report.focus=await evaluate('document.activeElement.outerHTML.slice(0,400)');report.body=await evaluate('document.body.innerText');}catch{}}
finally{ws?.close();if(app&&app.exitCode===null)app.kill();fs.writeFileSync(path.join(output,baseline?'baseline.json':'active-journey.json'),JSON.stringify(report,null,2));console.log(JSON.stringify({...report,body:report.body?'captured in report':undefined},null,2));}
