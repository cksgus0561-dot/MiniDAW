// Focused real-release display cadence and short playback comparison.
import {chromium} from 'playwright-core';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {readFile,writeFile,mkdir,unlink} from 'node:fs/promises';
import {createHash,randomUUID} from 'node:crypto';
import {resolve,join} from 'node:path';
import assert from 'node:assert/strict';
const baseline=process.argv.includes('--before'),exe=resolve('src-tauri/target/release/minidaw.exe');
const folder=resolve('tests/local/spectrum-fps',randomUUID());await mkdir(folder,{recursive:true});
const recent=join(process.env.APPDATA,'local.minidaw.desktop/recent-projects.json');
let savedRecent;try{savedRecent=await readFile(recent);}catch{}
const report={baseline,executableSha256:createHash('sha256').update(await readFile(exe)).digest('hex'),intervals:[]};
let child,browser,page,prefs;const errors=[],sleep=ms=>new Promise(r=>setTimeout(r,ms));
async function until(fn,label){const start=performance.now();while(performance.now()-start<20000){if(await fn())return;await sleep(40);}throw Error(label);}
const invoke=(name,args={})=>page.evaluate(({name,args})=>window.__TAURI_INTERNALS__.invoke(name,args),{name,args});
async function panel(id,open){const b=page.locator('#show-'+id);if((await b.getAttribute('aria-pressed')==='true')!==open)await b.click();await sleep(180);}
const rate=48000,frames=rate*30,pcm=Buffer.alloc(44+frames*4);pcm.write('RIFF');pcm.writeUInt32LE(pcm.length-8,4);pcm.write('WAVEfmt ',8);pcm.writeUInt32LE(16,16);pcm.writeUInt16LE(1,20);pcm.writeUInt16LE(2,22);pcm.writeUInt32LE(rate,24);pcm.writeUInt32LE(rate*4,28);pcm.writeUInt16LE(4,32);pcm.writeUInt16LE(16,34);pcm.write('data',36);pcm.writeUInt32LE(frames*4,40);
for(let i=0;i<frames;i++)for(let ch=0;ch<2;ch++)pcm.writeInt16LE(Math.round(.05*32767*Math.sin(2*Math.PI*[440,1000][ch]*i/rate)),44+i*4+ch*2);
const file=join(folder,'stereo.wav');await writeFile(file,pcm);
try{
 child=spawn(exe,[],{env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=19240'},stdio:'ignore',windowsHide:true});
 await until(async()=>{try{browser=await chromium.connectOverCDP('http://127.0.0.1:19240');return true;}catch{return false;}},'Release CDP');
 await until(async()=>{page=browser.contexts()[0].pages().find(p=>p.url().includes('tauri.localhost'));return !!page;},'Release page');
 page.on('pageerror',e=>errors.push(e.message));await page.setViewportSize({width:1440,height:960});
 await until(async()=>!await page.locator('#project-save').isDisabled(),'Project ready');
 prefs=await page.evaluate(()=>Object.fromEntries(['minidaw.ui.panels.v1','minidaw.ui.fontScale','minidaw.ui.spectrum.v1'].map(k=>[k,localStorage.getItem(k)])));
 await page.evaluate(()=>{
   window.__measure={paints:0,polls:0,worker:[]};
   const fetch=window.fetch;window.fetch=async function(url,options){
     const result=await fetch.call(this,url,options);
     if(decodeURIComponent(String(url)).split('/').at(-1)==='spectrum_snapshot'){
       const m=window.__measure;m.polls++;const s=await result.clone().json();if(s.frame)m.worker.push(s.frame.processingMs);
     }return result;
   };
   const fill=CanvasRenderingContext2D.prototype.fillRect;
   CanvasRenderingContext2D.prototype.fillRect=function(...args){if(this.canvas.id==='spectrum-canvas')window.__measure.paints++;return fill.apply(this,args);};
 });
 await panel('arrangement',true);await panel('media',false);await panel('performance',false);await panel('spectrum',true);
 await page.locator('#spectrum-mode').selectOption('live');await page.locator('#spectrum-fft').selectOption('4096');await page.locator('#spectrum-smoothing').selectOption('200');
 await invoke('load_audio',{path:file});await until(async()=>!await page.locator('#project-save').isDisabled(),'Load ready');
 await invoke('transport_command',{action:'play'});await sleep(700);await page.mouse.move(5,5);
 report.output=(await invoke('engine_snapshot')).output;
 for(const enabled of [false,true,false]){
   await panel('spectrum',enabled);await sleep(250);
   const before=await invoke('engine_snapshot'),sp=await invoke('spectrum_snapshot',{after:0});
   await page.evaluate(()=>{window.__measure={paints:0,polls:0,worker:[],start:performance.now()};});await sleep(3000);
   const measured=await page.evaluate(()=>({...window.__measure,elapsed:performance.now()-window.__measure.start}));
   const after=await invoke('engine_snapshot'),end=await invoke('spectrum_snapshot',{after:0});
   const callbacks=after.metrics.callbacks-before.metrics.callbacks;
   const result={enabled,uiFps:measured.paints*1000/measured.elapsed,pollHz:measured.polls*1000/measured.elapsed,workerMs:measured.worker.length?measured.worker.reduce((a,b)=>a+b,0)/measured.worker.length:0,workerMaxMs:Math.max(0,...measured.worker),processedDelta:end.processed-sp.processed,droppedDelta:end.droppedBlocks-sp.droppedBlocks,skippedDelta:end.skippedFrames-sp.skippedFrames,callbackMs:(after.metrics.callbackAvgMs*after.metrics.callbacks-before.metrics.callbackAvgMs*before.metrics.callbacks)/callbacks};
   report.intervals.push(result);
   assert.equal(after.transport.state,'playing');assert.ok(after.position>before.position+2.5);
   assert.equal(after.transport.appliedCommand,before.transport.appliedCommand);
   for(const k of ['overruns','deviceCallbackOverruns'])assert.equal(after.metrics[k],before.metrics[k],k);
   assert.equal(after.streamErrors,before.streamErrors);assert.equal(result.droppedDelta,0);assert.equal(result.skippedDelta,0);
   if(enabled){assert.ok(result.uiFps>(baseline?20:55)&&result.uiFps<(baseline?40:65),JSON.stringify(result));assert.ok(result.pollHz>25&&result.pollHz<35);}
   else{assert.equal(measured.paints,0);assert.equal(measured.polls,0);assert.equal(end.enabled,false);}
 }
 await panel('spectrum',true);await page.locator('#spectrum-mode').selectOption('selection');await sleep(250);
 await page.evaluate(()=>{window.__measure={paints:0,polls:0,worker:[]};});await sleep(500);
 assert.equal(await page.evaluate(()=>window.__measure.paints),0,'Static selection has no continuous paint loop');
 await invoke('transport_command',{action:'stop'});assert.deepEqual(errors,[]);report.passed=true;
}catch(e){report.error=String(e.stack??e);process.exitCode=1;}
finally{
 if(page&&prefs)await page.evaluate(p=>{for(const[k,v]of Object.entries(p)){if(v===null)localStorage.removeItem(k);else localStorage.setItem(k,v);}},prefs).catch(()=>{});
 if(browser)await browser.close();if(child?.exitCode===null){const exited=once(child,'exit');child.kill();await exited;}
 if(savedRecent)await writeFile(recent,savedRecent);else await unlink(recent).catch(()=>{});
 await writeFile(`docs/validation/spectrum-fps-${baseline?'before':'after'}.json`,JSON.stringify(report,null,2));console.log(JSON.stringify(report));
}
