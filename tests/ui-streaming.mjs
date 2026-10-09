// Actual release Tauri / WebView2 / CPAL test. Never mocks the Rust engine.
import { chromium } from "playwright-core";
import { spawn, execFileSync } from "node:child_process";
import { once } from "node:events";
import { readFile, writeFile, mkdir, stat, link, unlink } from "node:fs/promises";
import { createHash } from "node:crypto";
import { resolve } from "node:path";
import assert from "node:assert/strict";
const executable=resolve(process.argv[2]??"src-tauri/target/release/minidaw.exe");
const pause=ms=>new Promise(r=>setTimeout(r,ms));
const directory=resolve("docs/validation"); await mkdir(directory,{recursive:true});
const report={executable,sha256:createHash("sha256").update(await readFile(executable)).digest("hex"),started:new Date().toISOString(),files:[],
 limitations:["Measurements submit PCM to the actual CPAL device; they are not acoustic/loopback latency.","RAM is the MiniDAW Rust host process working set/private bytes; WebView2 is measured separately, OS file cache is not application PCM residency."]};
const requestedFiles=process.argv.slice(3).filter(arg=>arg!=='--asio');
const files=requestedFiles.length?requestedFiles:[
 "tests/fixtures/stereo-44100.wav",
 "tests/generated/long-400s-48000-2ch.wav",
 "tests/generated/long-1200s-48000-2ch.wav",
 "tests/generated/long-720s-44100-2ch.mp3",
 "tests/generated/long-720s-44100-2ch.flac",
 "tests/generated/long-720s-48000-1ch.wav",
];
function memory(pid) {
 const script=`$p=Get-Process -Id ${pid}; $all=Get-CimInstance Win32_Process; $ids=[System.Collections.Generic.List[int]]::new(); $ids.Add(${pid}); do { $before=$ids.Count; foreach($c in $all){ if($ids.Contains([int]$c.ParentProcessId) -and !$ids.Contains([int]$c.ProcessId)){$ids.Add([int]$c.ProcessId)} } } while($ids.Count -gt $before); $web=Get-Process -Id ($ids.ToArray() | Where-Object {$_ -ne ${pid}}) -ErrorAction SilentlyContinue; [pscustomobject]@{workingSet=$p.WorkingSet64;privateBytes=$p.PrivateMemorySize64;peakWorkingSet=$p.PeakWorkingSet64;webviewWorkingSet=($web | Measure-Object WorkingSet64 -Sum).Sum;webviewPrivateBytes=($web | Measure-Object PrivateMemorySize64 -Sum).Sum} | ConvertTo-Json -Compress`;
 return JSON.parse(execFileSync("powershell.exe",["-NoProfile","-Command",script],{encoding:"utf8",windowsHide:true}));
}
let child,browser,currentPage,originalFont,originalAudio;
try {
 for(const file of files) {
  child=spawn(executable,[],{env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:"--remote-debugging-port=19224"},stdio:"ignore",windowsHide:true});
  for(let i=0;i<100;i++){try{browser=await chromium.connectOverCDP("http://127.0.0.1:19224");break;}catch{await pause(100);}}
  assert.ok(browser,"App CDP unavailable");
  let page;
  for(let i=0;i<100;i++){page=browser.contexts()[0].pages().find(p=>p.url().includes("tauri.localhost"));if(page)break;await pause(50);}
  assert.ok(page);
  currentPage=page;
  const errors=[];page.on("pageerror",e=>errors.push(e.message));
  const invoke=(command,args={})=>page.evaluate(({command,args})=>window.__TAURI_INTERNALS__.invoke(command,args),{command,args});
  const snapshot=()=>invoke("engine_snapshot");
  originalAudio=(await snapshot()).preferences;
  if(process.argv.includes('--asio')) { const settings=(await snapshot()).preferences;settings.driverType='asio';settings.transportDeclick=true;settings.asioDriver=(await invoke('output_devices',{driverType:'asio'}))[0];await invoke('apply_audio_settings',{settings}); }
  const until=async(predicate,message,timeout=15000)=>{const t=Date.now();while(Date.now()-t<timeout){if(await predicate())return;await pause(15);}throw Error(message);};
  await until(async()=>Boolean((await snapshot()).output),"Output not ready");
  const record={file,sourceBytes:(await stat(file)).size,memoryBefore:memory(child.pid),seeks:[]};report.files.push(record);
  originalFont=await page.evaluate(()=>localStorage.getItem("minidaw.ui.fontScale"));
  await page.locator("#font-scale").selectOption("110");
  const start=performance.now();
  const loaded=await invoke("load_audio",{path:resolve(file)});
  record.openMs=performance.now()-start;record.loaded=loaded;
  await until(async()=>!(await page.locator("#play-pause").isDisabled()),"Play not ready");
  await page.locator("#play-pause").click();
  await until(async()=>{const s=await snapshot();return s.metrics.playMs!==null&&s.position>0;},"Play never supplied audio");
  record.firstPlay=await snapshot();
  record.playedWhileAnalyzing=!record.firstPlay.waveform.complete;
  const wave=await page.locator("#waveform").boundingBox();
  await page.mouse.move(wave.x+wave.width*.7,wave.y+12);
  await page.keyboard.down("Control");
  record.wheelStartedWhileAnalyzing=!(await snapshot()).waveform.complete;
  for(let i=0;i<20;i++)await page.mouse.wheel(0,-50);
  await page.keyboard.up("Control");
  await page.keyboard.down("Shift");
  for(let i=0;i<10;i++)await page.mouse.wheel(0,i%2?60:-60);
  await page.keyboard.up("Shift");
  record.during=await snapshot();record.memoryDuring=memory(child.pid);
  await page.locator("#play-pause").click();
  await until(async()=>(await snapshot()).transport.state==="paused","Pause failed");
  const paused=(await snapshot()).position;await pause(150);assert.equal((await snapshot()).position,paused);
  await page.locator("#play-pause").click();
  await until(async()=>(await snapshot()).position>paused,"Resume failed");
  for(const fraction of [.95,.02,.65,.1,.85,.005]) {
   const seconds=loaded.duration*fraction,t=performance.now();
   const id=await invoke("transport_command",{action:"seek",seconds});
   await until(async()=>{const s=await snapshot();return s.transport.appliedCommand>=id&&s.metrics.seekMs!==null&&s.position>=seconds;},"Seek did not produce samples");
   const s=await snapshot();assert.ok(s.position<seconds+1);
   record.seeks.push({seconds,observedMs:performance.now()-t,audioMs:s.metrics.seekMs,source:s.source});
  }
  // Old requests may finish while new seeks arrive; only the final generation may play.
  const rapid=[];
  for(let i=0;i<20;i++) rapid.push(invoke("transport_command",{action:"seek",seconds:loaded.duration*(i%2?.1:.9)}));
  await Promise.all(rapid);
  const finalTarget=loaded.duration*.33;
  const finalId=await invoke("transport_command",{action:"seek",seconds:finalTarget});
  await until(async()=>{const s=await snapshot();return s.transport.appliedCommand>=finalId&&s.metrics.seekMs!==null&&s.position>=finalTarget&&s.position<finalTarget+1;},"Rapid final seek incorrect");
  record.rapid=await snapshot();
  await until(async()=>(await snapshot()).waveform.complete,"Waveform did not complete",120000);
  assert.equal((await snapshot()).waveform.error,null);
  await page.locator("#zoom-fit").click();await pause(300);
  await page.evaluate(()=>{window.streamProbe={seeks:0,views:0,strokes:0};const f=window.fetch;window.fetch=function(url,opts){if(String(url).endsWith("/transport_command")&&JSON.parse(opts.body).action==="seek")window.streamProbe.seeks++;if(String(url).endsWith("/asset_waveform"))window.streamProbe.views++;return f.call(this,url,opts);};const stroke=CanvasRenderingContext2D.prototype.stroke;CanvasRenderingContext2D.prototype.stroke=function(...args){window.streamProbe.strokes++;return stroke.apply(this,args);};});
  await page.mouse.move(wave.x+wave.width*.2,wave.y+12);await page.mouse.down();
  for(let i=0;i<100;i++)await page.mouse.move(wave.x+wave.width*(.2+.5*i/99),wave.y+12);
  record.dragDuring=await page.evaluate(()=>window.streamProbe);
  assert.equal(record.dragDuring.seeks,0);assert.equal(record.dragDuring.views,0);assert.equal(record.dragDuring.strokes,0);
  await page.mouse.up();await until(async()=>(await page.evaluate(()=>window.streamProbe.seeks))===1,"Drag release must seek exactly once");
  await until(async()=>{const s=await snapshot();return s.position>=Math.min(loaded.duration,Math.max(10,loaded.duration*1.25)*.695)&&s.position<Math.min(loaded.duration,Math.max(10,loaded.duration*1.25)*.7)+1;} ,"Drag final position incorrect");
  record.dragAfter=await page.evaluate(()=>window.streamProbe);
  // Sustain longer than the read-ahead window: this verifies refill, not only priming.
  if(loaded.duration>20)await pause(7000);
  record.completed=await snapshot();record.memoryAfter=memory(child.pid);
  assert.equal(record.completed.source.error,null);assert.equal(record.completed.streamErrors,0);
  assert.equal(record.completed.source.starvation,0);assert.equal(record.completed.metrics.overruns,0);
  assert.equal(record.completed.source.mode,loaded.duration>20?"Streaming":"Memory");
  if(loaded.duration>20){assert.equal(record.completed.source.bufferBytes,2097152);assert.ok(record.completed.waveform.bytes<=8389120);}
  await page.locator("#stop").click();await until(async()=>{const s=await snapshot();return s.position===0&&s.transport.state==="stopped";},"Stop failed");
  await page.locator("#play-pause").click();await until(async()=>(await snapshot()).position>0,"Play after stop failed");
  await page.locator("#play-pause").click();await until(async()=>(await snapshot()).transport.state==="paused","Pause before stop failed");
  await page.locator("#stop").click();await until(async()=>(await snapshot()).position===0,"Paused stop failed");
  if(loaded.duration>20) {
   await invoke("reconnect_output",{buffer:null});
   await until(async()=>{const s=await snapshot();return s.transport.clipId===loaded.transport.clipId&&s.position===0&&s.transport.state==="stopped";},"Streaming reconnect failed");
   await page.locator("#play-pause").click();await until(async()=>(await snapshot()).position>0,"Reconnected streaming source did not play");
   await page.locator("#stop").click();
   record.reconnected=true;
  }
  if(file.includes("long-400s")) {
   const temporary=resolve("tests/generated",`external-delete-${child.pid}.wav`);
   await link(resolve(file),temporary);
   try {
    await invoke("new_project",{revision:(await invoke("project_snapshot")).revision,discard:true});
    await invoke("load_audio",{path:temporary});
    await unlink(temporary);
    await invoke("transport_command",{action:"seek",seconds:100});
    await until(async()=>(await snapshot()).source.error!==null,"Deleted streaming source error was not reported");
    record.deletedSourceError=(await snapshot()).source.error;
    assert.equal(record.deletedSourceError.code,"stream_read");
    await until(async()=>await page.locator("#error").isVisible(),"Korean source error not shown");
    await invoke("new_project",{revision:(await invoke("project_snapshot")).revision,discard:true});
    await invoke("load_audio",{path:resolve(file)});
    await invoke("transport_command",{action:"play"});
    await until(async()=>(await snapshot()).position>0,"Did not recover after deleted source");
    await invoke("transport_command",{action:"stop"});
    await page.locator("#dismiss-error").click();
   } finally { await unlink(temporary).catch(e=>{if(e.code!=="ENOENT")throw e;}); }
  }
  assert.deepEqual(errors,[]);
  await page.screenshot({path:resolve(directory,`streaming-${file.split(/[\\/]/).at(-1)}.png`)});
  record.finished=new Date().toISOString();console.log(JSON.stringify({file,openMs:record.openMs,playMs:record.firstPlay.metrics.playMs,seekMs:record.seeks.map(s=>s.audioMs),memory:record.memoryAfter,starvation:record.completed.source.starvation,overruns:record.completed.metrics.overruns,concurrent:record.playedWhileAnalyzing}));
  await writeFile(resolve(directory,"streaming.json"),JSON.stringify(report,null,2));
  await page.evaluate(value=>{if(value===null)localStorage.removeItem("minidaw.ui.fontScale");else localStorage.setItem("minidaw.ui.fontScale",value);},originalFont);
  await invoke('apply_audio_settings',{settings:originalAudio});
  currentPage=null;
  await browser.close();browser=null;
  if(child.exitCode===null){const done=once(child,"exit");child.kill();await done;}child=null;
 }
 report.passed=true;
} catch(error) {report.error=String(error.stack??error);throw error;}
finally {if(currentPage&&originalAudio)await currentPage.evaluate(settings=>window.__TAURI_INTERNALS__.invoke("apply_audio_settings",{settings}),originalAudio).catch(()=>{});if(currentPage&&originalFont!==undefined)await currentPage.evaluate(value=>{if(value===null)localStorage.removeItem("minidaw.ui.fontScale");else localStorage.setItem("minidaw.ui.fontScale",value);},originalFont).catch(()=>{});if(browser)await browser.close();if(child?.exitCode===null)child.kill();await writeFile(resolve(directory,"streaming.json"),JSON.stringify(report,null,2));}
