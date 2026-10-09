// Local user-supplied regression source is read in place, never copied or modified.
import { chromium } from "playwright-core";
import { spawn, execFileSync } from "node:child_process";
import { once } from "node:events";
import { readFile, writeFile, mkdir } from "node:fs/promises";
import { createHash } from "node:crypto";
import { resolve } from "node:path";
import assert from "node:assert/strict";
const source=process.argv[2]; if(!source)throw Error("Pass the local primary MP3 path");
const baseline=process.argv.includes("--expect-error");
const executable=resolve("src-tauri/target/release/minidaw.exe");
const folder=resolve("docs/validation");await mkdir(folder,{recursive:true});
const report={source,sha256:createHash("sha256").update(await readFile(source)).digest("hex"),executableSha256:createHash("sha256").update(await readFile(executable)).digest("hex"),baseline,started:new Date().toISOString(),runs:[]};
const child=spawn(executable,[],{env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:"--remote-debugging-port=19225"},stdio:"ignore",windowsHide:true});
const sleep=ms=>new Promise(r=>setTimeout(r,ms));let browser,page,originalAudio;
function memory(){return JSON.parse(execFileSync("powershell.exe",["-NoProfile","-Command",`Get-Process -Id ${child.pid} | Select-Object WorkingSet64,PrivateMemorySize64,PeakWorkingSet64 | ConvertTo-Json -Compress`],{encoding:"utf8",windowsHide:true}));}
try {
 for(let i=0;i<100;i++){try{browser=await chromium.connectOverCDP("http://127.0.0.1:19225");break;}catch{await sleep(100);}}
 assert.ok(browser);
 for(let i=0;i<100;i++){page=browser.contexts()[0].pages().find(p=>p.url().includes("tauri.localhost"));if(page)break;await sleep(50);}
 assert.ok(page);const errors=[];page.on("pageerror",e=>errors.push(e.message));
 const invoke=(command,args={})=>page.evaluate(({command,args})=>window.__TAURI_INTERNALS__.invoke(command,args),{command,args});
 const snapshot=()=>invoke("engine_snapshot");
 originalAudio=(await snapshot()).preferences;
 if(process.argv.includes('--asio')) { const settings=(await snapshot()).preferences;settings.driverType='asio';settings.transportDeclick=true;settings.asioDriver=(await invoke('output_devices',{driverType:'asio'}))[0];await invoke('apply_audio_settings',{settings}); }
 const until=async(fn,message,ms=120000)=>{const start=Date.now();while(Date.now()-start<ms){if(await fn())return;await sleep(20);}throw Error(message);};
 await until(async()=>Boolean((await snapshot()).output),"Output unavailable");
 report.initial=await snapshot();report.memoryBefore=memory();
 for(let run=0;run<(baseline?1:3);run++) {
  await invoke("new_project",{revision:(await invoke("project_snapshot")).revision,discard:true});
  const t=performance.now();const loaded=await invoke("load_audio",{path:source});
  const record={openMs:performance.now()-t,loaded,progress:[]};report.runs.push(record);
  await invoke("transport_command",{action:"play"});
  await until(async()=>(await snapshot()).position>0,"Playback blocked");
  record.playing=await snapshot();
  await until(async()=>{const s=await snapshot();record.progress.push({atMs:performance.now()-t,waveform:s.waveform,position:s.position});return s.waveform.complete;},"Analysis did not finish");
  record.complete=await snapshot();record.memory=memory();
  await sleep(200);record.errorVisible=await page.locator("#error").isVisible();record.errorText=await page.locator("#error-text").textContent();
  await page.screenshot({path:resolve(folder,`primary-${baseline?'before':'after'}-${run}.png`)});
  if(baseline){assert.ok(record.complete.waveform.error,"Expected original waveform error");break;}
  assert.equal(record.complete.waveform.error,null);assert.equal(record.complete.waveform.progress,1);assert.equal(record.complete.source.error,null);
  const duration=record.complete.duration;
  await page.locator('#play-pause').click();
  await until(async()=>(await snapshot()).transport.state==='paused','Primary pause failed');
  record.paused=await snapshot();await sleep(100);
  assert.equal((await snapshot()).position,record.paused.position);
  await page.locator('#play-pause').click();
  await until(async()=>(await snapshot()).position>record.paused.position,'Primary resume failed');
  record.resumed=await snapshot();
  record.views=[];
  for(const [a,b] of [[0,Math.min(2,duration)],[duration-2,duration],[duration*.5,duration*.5+.03]]) {
   const view=await invoke("waveform_view",{clipId:loaded.transport.clipId,start:a,end:b,width:600});
   assert.equal(view.channels.length,loaded.file.channels);assert.ok(view.channels.flat(2).every(Number.isFinite));
   record.views.push({start:a,end:b,nonzero:view.channels.flat(2).some(s=>s!==0),columns:view.channels[0].length});
  }
  record.seeks=[];
  for(const fraction of [.0001,.1,.25,.5,.75,.9,.999]) {
   const seconds=duration*fraction;const id=await invoke("transport_command",{action:"seek",seconds});
   await until(async()=>{const s=await snapshot();return s.transport.appliedCommand>=id&&s.metrics.seekMs!==null&&s.position>=seconds;},"Primary seek failed",15000);
   record.seeks.push(await snapshot());
  }
  await invoke("transport_command",{action:"seek",seconds:duration-.12});
  await until(async()=>{const s=await snapshot();return s.transport.state==='stopped'&&s.transport.frame===s.file.frames;},'EOF did not stop at exact final frame',15000);
  record.eof=await snapshot();assert.equal(record.eof.source.error,null);assert.equal(record.eof.streamErrors,0);
  assert.equal(record.eof.metrics.overruns,0);assert.equal(record.eof.source.starvation,0);
  const box=await page.locator('canvas').first().boundingBox();assert.ok(box);
  await page.mouse.move(box.x+box.width*.7,box.y+box.height*.5);
  const beforeZoom=await snapshot();await page.keyboard.down('Control');await page.mouse.wheel(0,-240);await page.keyboard.up('Control');await sleep(250);
  assert.equal((await snapshot()).transport.appliedCommand,beforeZoom.transport.appliedCommand);
  assert.equal((await snapshot()).waveform.buildMs,beforeZoom.waveform.buildMs);
  record.zoomLabel=await page.locator('#zoom-value').textContent();assert.notEqual(record.zoomLabel,'1.0×');
  await page.screenshot({path:resolve(folder,`primary-after-${run}-zoom.png`)});
  await page.locator('#zoom-fit').click();
  await invoke("transport_command",{action:"stop"});
 }
 report.jsErrors=errors;assert.deepEqual(errors,[]);report.passed=true;
}catch(error){report.error=String(error.stack??error);throw error;}
finally{if(page&&originalAudio)await page.evaluate(settings=>window.__TAURI_INTERNALS__.invoke("apply_audio_settings",{settings}),originalAudio).catch(()=>{});if(browser)await browser.close();if(child.exitCode===null){const exited=once(child,"exit");child.kill();await exited;}report.sourceSha256After=createHash("sha256").update(await readFile(source)).digest("hex");assert.equal(report.sourceSha256After,report.sha256);await writeFile(resolve(folder,`primary-${baseline?'before':'after'}.json`),JSON.stringify(report,null,2));console.log(JSON.stringify({passed:report.passed,error:report.error,runs:report.runs.map(r=>({openMs:r.openMs,waveform:r.complete?.waveform,metrics:r.complete?.metrics,memory:r.memory}))}));}
