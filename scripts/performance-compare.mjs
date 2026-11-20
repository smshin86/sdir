// Same-PC cached process-start/list-readiness benchmark. No clipboard or file content reads.
import fs from 'node:fs';import os from 'node:os';import path from 'node:path';import crypto from 'node:crypto';import assert from 'node:assert/strict';import {spawn,spawnSync} from 'node:child_process';import readline from 'node:readline';
const option=name=>{const i=process.argv.indexOf(name);return i<0?undefined:process.argv[i+1];};
const defaults=path.resolve('docs/task-id/performance-compare'),out=path.resolve(option('--output-directory')||defaults);fs.mkdirSync(out,{recursive:true});
const json=p=>JSON.parse(fs.readFileSync(p,'utf8').replace(/^\uFEFF/,''));const hash=p=>crypto.createHash('sha256').update(fs.readFileSync(p)).digest('hex');
const pause=ms=>new Promise(r=>setTimeout(r,ms));const units=p=>Array.from({length:p.length},(_,i)=>p.charCodeAt(i));
let config;const savedFile=fs.existsSync(path.join(out,'fixture.json'))?path.join(out,'fixture.json'):path.join(defaults,'fixture.json');
if(fs.existsSync(savedFile)){const saved=json(savedFile);if(fs.existsSync(path.join(saved.root,'.sdir-benchmark'))&&fs.existsSync(saved.large))config=saved;}
if(!config){
 const env=json(path.join(defaults,'environment.json')),cloud=json(path.join(defaults,'cloud-inventory.json'));
 const root=fs.mkdtempSync(path.join(os.tmpdir(),'sdir-benchmark-'));fs.writeFileSync(path.join(root,'.sdir-benchmark'),'owned');
 const small=path.join(root,'small'),empty=path.join(root,'empty');fs.mkdirSync(small);fs.mkdirSync(empty);for(let i=0;i<600;i++)fs.writeFileSync(path.join(small,`sample-${String(i).padStart(6,'0')}.bin`),'');
 const large=path.join(root,'large');fs.mkdirSync(large);for(let i=0;i<100000;i++)fs.writeFileSync(path.join(large,`sample-${String(i).padStart(6,'0')}.bin`),'');fs.mkdirSync(path.join(large,'자료'));fs.writeFileSync(path.join(large,'회의록.txt'),'fixture');fs.writeFileSync(path.join(large,'tail.txt'),'fixture');
 const source=env.qdirFiles[0].path,qdir=path.join(root,'qdir','Q-Dir.exe'),sdir=path.join(root,'sdir','SDIR.exe');for(const p of [qdir,sdir])fs.mkdirSync(path.dirname(p));fs.copyFileSync(source,qdir);fs.copyFileSync('dist/SDIR.exe',sdir);assert.equal(hash(source),hash(qdir));
 const originalFiles=['Q-Dir.ini','start.qdr'].map(n=>path.join(path.dirname(source),n));
 const originalIni=fs.readFileSync(originalFiles[0],'utf8');const license=originalIni.split(/\r?\n/).find(l=>/^Lizenz=/.test(l));
 fs.writeFileSync(path.join(path.dirname(qdir),'Q-Dir.ini'),'[Start]\r\nm_lang_id=14\r\n[Q-Dir]\r\n'+(license||'')+'\r\nenable_auto_update=0\r\n');
 fs.writeFileSync(path.join(path.dirname(qdir),'start.qdr'),'\uFEFFQ-Dir Version=11.14\r\nViewMode=0\r\n','utf16le');
 const selected=cloud.filter(r=>r.items>0).sort((a,b)=>b.items-a.items)[0];
 config={root,small,empty,large,qdir,sdir,profile:path.join(root,'sdir-profile'),sourceQdir:source,originalFiles:originalFiles.map(p=>({path:p,sha256:hash(p)})),baselineProcesses:env.processes,scenarios:[{id:'local-600',folder:small},{id:'local-100k',folder:large},{id:'onedrive',folder:selected.path,cloudMetadata:selected}],hashes:{qdir:hash(qdir),sdir:hash(sdir)}};
 for(const s of config.scenarios){s.count=fs.readdirSync(s.folder).filter(n=>!n.startsWith('.')).length;}
 fs.mkdirSync(config.profile);fs.writeFileSync(path.join(out,'fixture.json'),JSON.stringify(config,null,2));
}
// Refresh only metadata counts; no file contents or provider state are changed.
const countCode="$paths=$env:SDIR_BENCH_FOLDERS|ConvertFrom-Json;$counts=@(foreach($p in $paths){@(Get-ChildItem -LiteralPath $p -Force -ErrorAction Stop|Where-Object{-not ([int]$_.Attributes -band 2)}).Count});ConvertTo-Json -InputObject $counts -Compress";
const counted=spawnSync('powershell.exe',['-NoProfile','-EncodedCommand',Buffer.from(countCode,'utf16le').toString('base64')],{encoding:'utf8',env:{...process.env,SDIR_BENCH_FOLDERS:JSON.stringify(config.scenarios.map(s=>s.folder))},timeout:60000});assert.equal(counted.status,0,counted.stderr);const counts=JSON.parse(counted.stdout.trim());assert.equal(counts.length,config.scenarios.length);config.scenarios.forEach((s,i)=>s.count=counts[i]);
fs.writeFileSync(path.join(out,'fixture.json'),JSON.stringify(config,null,2));
if(option('--executable')){const requested=path.resolve(option('--executable')),digest=hash(requested),copy=path.join(config.root,'versions',digest.slice(0,12),'SDIR.exe');fs.mkdirSync(path.dirname(copy),{recursive:true});if(!fs.existsSync(copy))fs.copyFileSync(requested,copy);assert.equal(hash(copy),digest);config={...config,sdir:copy,profile:path.join(config.root,'comparison-profile'),hashes:{...config.hashes,sdir:digest}};fs.mkdirSync(config.profile,{recursive:true});}
fs.writeFileSync(path.join(out,'fixture.json'),JSON.stringify(config,null,2));
const kinds=list=>option('--only')?list.filter(k=>k===option('--only')):list;
assert(fs.existsSync(path.join(config.root,'.sdir-benchmark')));assert.equal(hash(config.qdir),config.hashes.qdir);assert.equal(hash(config.sdir),config.hashes.sdir);
const helper=spawn('powershell.exe',['-NoProfile','-ExecutionPolicy','Bypass','-File',path.resolve('scripts/performance-native.ps1'),'-OwnedRoot',config.root],{stdio:['pipe','pipe','pipe']});
let req=0;const waiting=new Map();let helperError='';helper.stderr.on('data',b=>helperError+=b);let readyResolve;const ready=new Promise(r=>readyResolve=r);
readline.createInterface({input:helper.stdout}).on('line',line=>{try{const j=JSON.parse(line);if(j.ready)return readyResolve();const p=waiting.get(j.id);if(p){waiting.delete(j.id);clearTimeout(p.timer);j.ok?p.resolve(j.value):p.reject(Error(j.error));}}catch(e){helperError+=line+'\n';}});
function native(action,pid,extra={}){return new Promise((resolve,reject)=>{const id=++req,timer=setTimeout(()=>{waiting.delete(id);reject(Error('Native helper timeout: '+action+' '+helperError.slice(-1000)));},10000);waiting.set(id,{resolve,reject,timer});helper.stdin.write(JSON.stringify({id,action,pid,...extra})+'\n');});}
async function until(fn,label,ms=30000){const start=performance.now();while(performance.now()-start<ms){const v=await fn();if(v)return v;await pause(30);}throw Error('Timeout: '+label);}
async function cdp(app,port){let target;await until(async()=>{assert(app.exitCode===null&&app.signalCode===null,'Owned app exited before connection');try{const pages=await(await fetch(`http://127.0.0.1:${port}/json`)).json();target=pages.find(p=>p.type==='page'&&p.webSocketDebuggerUrl);return !!target;}catch{return false;}},'WebView');const ws=new WebSocket(target.webSocketDebuggerUrl);await new Promise((r,j)=>{ws.onopen=r;ws.onerror=j;});let serial=0;const pending=new Map();ws.onmessage=e=>{const j=JSON.parse(e.data);const p=pending.get(j.id);if(p){pending.delete(j.id);clearTimeout(p.timer);j.error?p.reject(Error(JSON.stringify(j.error))):p.resolve(j.result);}};const command=(method,params={})=>new Promise((resolve,reject)=>{const id=++serial,timer=setTimeout(()=>{pending.delete(id);reject(Error('CDP timeout '+method));},10000);pending.set(id,{resolve,reject,timer});ws.send(JSON.stringify({id,method,params}));});const evaluate=async expression=>{const j=await command('Runtime.evaluate',{expression,returnByValue:true,awaitPromise:true});if(j.exceptionDetails)throw Error(JSON.stringify(j.exceptionDetails));return j.result.value;};return{ws,command,evaluate};}
let current;const results=[];
async function run(kind,scenario,round){
 const paths=[scenario.folder,config.empty,config.empty,config.empty];
 if(kind==='sdir')fs.writeFileSync(path.join(config.profile,'settings.json'),JSON.stringify({version:1,layout:4,panes:paths.map(p=>[units(p)]),favorites:[],show_hidden:false,tree_follow:false,tree_open:false,window:{x:50,y:50,width:1384,height:861,maximized:false}}));
 const exe=config[kind],port=19417,appEnv={...process.env};delete appEnv.SDIR_DEBUG_PORT;if(kind==='sdir'){appEnv.SDIR_PROFILE_DIR=config.profile;if(!process.argv.includes('--normal-memory'))appEnv.SDIR_DEBUG_PORT=String(port);}
 const start=performance.now();const app=spawn(exe,kind==='qdir'?paths:[],{cwd:path.dirname(exe),env:appEnv,stdio:'ignore'});current=app;let browser;let nativeWindow=0;
 const r={kind,scenario:scenario.id,round,pid:app.pid,paths,expectedCount:scenario.count,sha256:config.hashes[kind],ping:[]};
 try{
  await until(async()=>{if(app.exitCode!==null||app.signalCode!==null)throw Error('Owned process exited; original app will not be controlled');const v=await native('controls',app.pid);nativeWindow=v.main;return nativeWindow;},'owned main window');r.windowReadyMs=performance.now()-start;await native('resize',app.pid);r.windowState=await native('state',app.pid);assert(r.windowState.topmost&&r.windowState.width===1400&&r.windowState.height===900,'owned visible standard window');
  if(process.argv.includes('--normal-memory')){await pause(10000);r.fixedDelayMs=10000;r.remoteDebugging=false;r.ping.push(await native('ping',app.pid));r.memory=await native('memory',app.pid);r.sampleElapsedMs=performance.now()-start;r.ok=true;return r;}
  if(kind==='sdir'){browser=await cdp(app,port);r.observerConnectedMs=performance.now()-start;r.documentVisibility=await browser.evaluate('document.visibilityState');assert.equal(r.documentVisibility,'visible');}
  let stable=0;await until(async()=>{
   const ping=await native('ping',app.pid);r.ping.push(ping);
   let ok;if(kind==='qdir'){
    const v=await native('controls',app.pid),lists=v.controls.filter(c=>c.cls==='SysListView32'&&c.width>20&&c.height>20).sort((a,b)=>a.y-b.y||a.x-b.x);
    r.lastLists=lists.map(c=>({count:c.count,x:c.x,y:c.y,width:c.width,height:c.height,viewStyle:c.viewStyle,view:c.view,hwnd:c.hwnd}));r.lastAddresses=v.controls.filter(c=>c.cls==='Edit'&&c.text).map(c=>({hwnd:c.hwnd,text:c.text,x:c.x,y:c.y}));
    ok=lists.length===4&&lists.every(c=>c.view===1)&&lists.filter(c=>c.count===scenario.count).length===(scenario.count===0?4:1)&&lists.filter(c=>c.count===0).length===(scenario.count===0?4:3);
   }else ok=await browser.evaluate(`document.querySelectorAll('.pane').length===4&&!document.querySelector('button[title="읽기 취소"]')&&[...document.querySelectorAll('[data-pane="0"] .pane-status span')].some(e=>e.textContent.startsWith('${scenario.count} / ${scenario.count}개'))&&!document.querySelector('.pane-error')`);
   stable=ok?stable+1:0;return stable>=2;
  },'four complete listings',90000);r.completeMs=performance.now()-start;
  if(process.argv.includes('--navigation')||process.argv.includes('--navigate-probe')){
   r.navigation=[];
   for(let iteration=0;iteration<(process.argv.includes('--navigate-probe')?1:6);iteration++){
    for(const target of [{path:config.empty,count:0,direction:'away'},{path:scenario.folder,count:scenario.count,direction:'back'}]){
     let edit;if(kind==='qdir'){
      const controls=(await native('controls',app.pid)).controls;const list=controls.filter(c=>c.cls==='SysListView32').sort((a,b)=>a.y-b.y||a.x-b.x)[0];
      edit=controls.filter(c=>c.cls==='Edit'&&c.x>=list.x&&c.x<list.x+list.width&&c.y<list.y).sort((a,b)=>b.y-a.y)[0]?.hwnd;assert(edit,'owned first pane address');await native('prepare',app.pid,{edit,path:target.path});
     }else{
      await browser.evaluate(`document.querySelector('#address-0').focus()`);await browser.command('Input.dispatchKeyEvent',{type:'keyDown',key:'a',code:'KeyA',windowsVirtualKeyCode:65,modifiers:2});await browser.command('Input.dispatchKeyEvent',{type:'keyUp',key:'a',code:'KeyA',windowsVirtualKeyCode:65,modifiers:2});await browser.command('Input.insertText',{text:target.path});
     }
     const began=performance.now();if(kind==='qdir')await native('enter',app.pid,{edit});else{await browser.command('Input.dispatchKeyEvent',{type:'keyDown',key:'Enter',code:'Enter',windowsVirtualKeyCode:13});await browser.command('Input.dispatchKeyEvent',{type:'keyUp',key:'Enter',code:'Enter',windowsVirtualKeyCode:13});}
     const ping=[];let valid=0;await until(async()=>{
      ping.push(await native('ping',app.pid));let ok;
      if(kind==='qdir'){const lists=(await native('controls',app.pid)).controls.filter(c=>c.cls==='SysListView32'&&c.width>20&&c.height>20).sort((a,b)=>a.y-b.y||a.x-b.x);ok=lists.length===4&&lists[0].count===target.count;}
      else ok=await browser.evaluate(`!document.querySelector('[data-pane="0"] button[title="읽기 취소"]')&&document.querySelector('[data-pane="0"] .tab.selected')?.title===${JSON.stringify(target.path)}&&[...document.querySelectorAll('[data-pane="0"] .pane-status span')].some(e=>e.textContent.startsWith('${target.count} / ${target.count}개'))`);
      valid=ok?valid+1:0;return valid>=2;
     },'navigation '+target.direction,90000);
     r.navigation.push({iteration,direction:target.direction,ms:performance.now()-began,ping});
    }
   }
   r.ok=true;return r;
  }
  if(process.argv.includes('--probe')||process.argv.includes('--sdir-probe')){r.controls=await native('controls',app.pid);r.ok=true;return r;}
  await pause(2500);r.memoryBefore=await native('memory',app.pid);const idleStart=performance.now();await pause(2500);r.memory=await native('memory',app.pid);r.idleSampleMs=performance.now()-idleStart;r.idleCpuMs=Math.max(0,r.memory.cpuMs-r.memoryBefore.cpuMs);r.idleCpuOneCorePercent=r.idleCpuMs/r.idleSampleMs*100;
  r.ok=true;return r;
 }catch(e){r.ok=false;r.failure=e.stack;return r;}
 finally{
  if(app.exitCode===null&&app.signalCode===null){try{r.cleanup=await native('close',app.pid,{window:nativeWindow});assert(r.cleanup.posted,'Owned main window accepts close');await until(()=>app.exitCode!==null||app.signalCode!==null,'owned normal close',15000);r.cleanup.normal=true;}catch(error){r.cleanup={...r.cleanup,normal:false,error:String(error),forcedOwnedTest:true};app.kill();await until(()=>app.exitCode!==null||app.signalCode!==null,'owned forced close',10000);}}
  browser?.ws.close();current=null;
 }
}
try{
 await Promise.race([ready,pause(10000).then(()=>{throw Error('Native helper did not start: '+helperError);})]);
 if(process.argv.includes('--normal-memory')){for(let round=0;round<=3;round++){for(const kind of kinds(round%2?['sdir','qdir']:['qdir','sdir'])){const r=await run(kind,config.scenarios[0],round);results.push(r);fs.writeFileSync(path.join(out,'normal-memory-results.json'),JSON.stringify({results},null,2));assert(r.ok,r.failure);console.log(JSON.stringify({kind,round,privateMiB:r.memory.privateBytes/1048576,cleanup:r.cleanup}));}}}
 else if(process.argv.includes('--sdir-probe')){for(let i=0;i<3;i++){const r=await run('sdir',config.scenarios[0],i);fs.writeFileSync(path.join(out,'sdir-probe-'+i+'.json'),JSON.stringify(r,null,2));console.log(JSON.stringify({ok:r.ok,window:r.windowState,cleanup:r.cleanup,failure:r.failure}));assert(r.ok,r.failure);assert(r.cleanup?.normal,'Owned test main window closed normally');}}
 else if(process.argv.includes('--navigate-probe')){const r=await run('qdir',config.scenarios[0],0);fs.writeFileSync(path.join(out,'navigation-probe.json'),JSON.stringify(r,null,2));console.log(JSON.stringify(r,null,2));assert(r.ok,r.failure);}
 else if(process.argv.includes('--navigation')){for(const scenario of config.scenarios){for(const kind of kinds(['qdir','sdir'])){console.log(`Navigate ${scenario.id} ${kind}`);const r=await run(kind,scenario,0);results.push(r);fs.writeFileSync(path.join(out,'navigation-results.json'),JSON.stringify({results},null,2));assert(r.ok,r.failure);console.log(JSON.stringify({kind,scenario:scenario.id,backMs:r.navigation.filter(n=>n.direction==='back').map(n=>n.ms)}));}}}
 else if(process.argv.includes('--probe')){const r=await run('qdir',config.scenarios[0],0);fs.writeFileSync(path.join(out,'qdir-probe.json'),JSON.stringify(r,null,2));console.log(JSON.stringify(r,null,2));assert(r.ok,r.failure);}
 else{for(const scenario of config.scenarios){for(let round=0;round<=(process.argv.includes('--visible')?4:6);round++){for(const kind of kinds(round%2?['sdir','qdir']:['qdir','sdir'])){console.log(`Running ${scenario.id} ${kind} round ${round}${round===0?' warm-up':''}`);const r=await run(kind,scenario,round);results.push(r);fs.writeFileSync(path.join(out,process.argv.includes('--visible')?'visible-results.json':'raw-results.json'),JSON.stringify({results},null,2));assert(r.ok,r.failure);console.log(JSON.stringify({kind,scenario:scenario.id,round,completeMs:r.completeMs,privateMiB:r.memory.privateBytes/1048576,idleCpu:r.idleCpuOneCorePercent}));}}}}
}catch(e){console.error(e.stack);process.exitCode=1;}
finally{helper.stdin.end();if(current&&current.exitCode===null&&current.signalCode===null){current.kill();}const originals=config.originalFiles.map(f=>({...f,afterSha256:hash(f.path),unchanged:hash(f.path)===f.sha256}));fs.writeFileSync(path.join(out,'original-settings-check.json'),JSON.stringify(originals,null,2));}
