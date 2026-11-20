import fs from 'node:fs';import path from 'node:path';import crypto from 'node:crypto';import readline from 'node:readline';import assert from 'node:assert/strict';import {spawn} from 'node:child_process';
const exe=path.resolve(process.argv[2]),output=path.resolve(process.argv[3]);const cfg=JSON.parse(fs.readFileSync('docs/task-id/performance-compare/fixture.json','utf8').replace(/^\uFEFF/,''));
const folder=cfg.large,units=Array.from({length:folder.length},(_,i)=>folder.charCodeAt(i)),expected=fs.readdirSync(folder).length,results=[];
for(let sample=0;sample<4;sample++){
 const started=performance.now(),app=spawn(exe,['--worker'],{stdio:['pipe','pipe','pipe']});let bytes=0,count=0,batches=0,decodeMs=0,firstBatchMs,listedMs,error='';app.stderr.on('data',b=>error+=b.toString().slice(0,2000));app.stdout.on('data',b=>bytes+=b.length);
 readline.createInterface({input:app.stdout}).on('line',line=>{const t=performance.now();const e=JSON.parse(line);decodeMs+=performance.now()-t;if(e.type==='batch'){firstBatchMs??=performance.now()-started;count+=e.entries.length;batches++;}if(e.type==='listed'){assert(!e.error,e.error);listedMs=performance.now()-started;}});
 app.stdin.end(JSON.stringify({type:'list',pane:0,id:1,path:units,hidden:true})+'\n');const code=await new Promise((resolve,reject)=>{const timer=setTimeout(()=>{app.kill();reject(Error('owned listing worker timeout'));},30000);app.on('exit',code=>{clearTimeout(timer);resolve(code)});});assert.equal(code,0,error);assert.equal(count,expected);assert(listedMs!==undefined);
 const r={sample,count,batches,bytes,firstBatchMs,listedMs,exitMs:performance.now()-started,observerDecodeMs:decodeMs};results.push(r);console.log(JSON.stringify(r));
}
fs.writeFileSync(output,JSON.stringify({executable:exe,sha256:crypto.createHash('sha256').update(fs.readFileSync(exe)).digest('hex'),results,note:'One excluded warmup; streaming JSON receiver parses each line immediately; not UI latency.'},null,2));
