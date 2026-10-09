import { chromium } from 'playwright-core';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { readFile,writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { resolve } from 'node:path';
import assert from 'node:assert/strict';
const exe=resolve('src-tauri/target/release/minidaw.exe'), directory=resolve('docs/validation');
const report={executableSha256:createHash('sha256').update(await readFile(exe)).digest('hex'),checks:[],runs:[]};
let child,browser,page,original;
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
async function launch(){child=spawn(exe,[],{env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=19229'},stdio:'ignore',windowsHide:true});
 for(let i=0;i<100;i++){try{browser=await chromium.connectOverCDP('http://127.0.0.1:19229');break;}catch{await sleep(100);}}
 assert.ok(browser);for(let i=0;i<100;i++){page=browser.contexts()[0].pages().find(p=>p.url().includes('tauri.localhost'));if(page)break;await sleep(50);}assert.ok(page);await until(s=>Boolean(s.preferences));}
async function close(){if(browser){await browser.close();browser=null;}if(child?.exitCode===null){const done=once(child,'exit');child.kill();await done;}page=null;}
const invoke=(name,args={})=>page.evaluate(({name,args})=>window.__TAURI_INTERNALS__.invoke(name,args),{name,args});
const snapshot=()=>invoke('engine_snapshot');
async function until(fn){for(let n=0;n<500;n++){const s=await snapshot();if(await fn(s))return s;await sleep(20);}throw Error('Timeout');}
async function open(){await page.locator('#open-audio-settings').click();await until(async()=>!(await page.locator('#output-device').isDisabled()));}
async function apply(){await page.locator('#reconnect').click();await until(async()=>!(await page.locator('#reconnect').isDisabled()));await sleep(100);}
try{
 await launch();original=(await snapshot()).preferences;
 const names=await invoke('output_devices',{driverType:'asio'});assert.ok(names.includes('Topping USB Audio Device'));report.drivers=names;
 await open();await page.locator('#driver-type').selectOption('asio');await until(async()=>!(await page.locator('#output-device').isDisabled()));
 await page.locator('#output-device').selectOption('Topping USB Audio Device');await page.locator('#transport-declick').check();await apply();
 let s=await until(s=>s.output?.driverType==='asio'&&s.metrics.callbacks>5);assert.equal(s.outputError,null);assert.ok(await page.locator('#buffer-size').isDisabled());assert.ok(!(await page.locator('#asio-panel').isDisabled()));
 for(const scale of ['90','110','125']){await page.locator('#close-audio-settings').click();await page.locator('#font-scale').selectOption(scale);await open();
   assert.ok(await page.evaluate(()=>{const d=document.getElementById('audio-settings');return d.scrollWidth<=d.clientWidth&&d.getBoundingClientRect().right<=innerWidth;}));
   await page.screenshot({path:resolve(directory,`audio-settings-${scale}.png`)});
 }
 await page.locator('#close-audio-settings').click();await page.locator('#font-scale').selectOption('110');
 for(const file of ['stereo-44100.wav','stereo-44100.mp3','stereo-44100.flac','모노-48000.WAV']) {
  await invoke('new_project',{revision:(await invoke('project_snapshot')).revision,discard:true});
  await invoke('load_audio',{path:resolve('tests/fixtures',file)});await until(s=>s.waveform?.complete);
  for(const enabled of [false,true]) {
   const before=await snapshot();const viewArgs={clipId:before.transport.clipId,start:0,end:before.duration,width:512};const peaksBefore=await invoke('waveform_view',viewArgs);
   await open();await page.locator('#transport-declick').setChecked(enabled);await apply();await page.locator('#close-audio-settings').click();
   assert.equal((await snapshot()).preferences.transportDeclick,enabled);
   assert.equal((await snapshot()).waveform.buildMs,before.waveform.buildMs);
   assert.deepEqual(await invoke('waveform_view',viewArgs),peaksBefore,'De-click changed waveform cache values');
   const run={file,enabled,commands:[]};report.runs.push(run);
   for(const [action,seconds] of [['play'],['pause'],['play'],['seek',.123],['stop']]) {
    const id=await invoke('transport_command',{action,seconds:seconds??null});await until(s=>s.transport.appliedCommand>=id);await sleep(45);
    const after=await snapshot();run.commands.push({action,snapshot:after});
    assert.equal(after.streamErrors,0);assert.equal(after.metrics.overruns,0);assert.equal(after.metrics.deviceCallbackOverruns,0);
    if(action==='pause'){const position=after.position;await sleep(50);assert.equal((await snapshot()).position,position);}
    if(action==='stop')assert.equal(after.position,0);
   }
  }
 }
 report.checks.push('Real TOPPING ASIO selected through UI; WAV/MP3/FLAC, 44.1k SRC and 48k bypass, ON/OFF, transport, scaled dialog');
 // Hundreds of move events pass through the real canvas with no transport IPC.
 await invoke('new_project',{revision:(await invoke('project_snapshot')).revision,discard:true});
 await invoke('load_audio',{path:resolve('tests/fixtures/stereo-44100.wav')});await until(s=>s.waveform?.complete);await sleep(150);
 const box=await page.locator('#waveform').boundingBox();await page.mouse.move(box.x+box.width*.1,box.y+12);
 const before=await snapshot();await page.mouse.down();await page.mouse.move(box.x+box.width*.5,box.y+12,{steps:100});
 assert.equal((await snapshot()).transport.appliedCommand,before.transport.appliedCommand);await page.mouse.up();await until(s=>s.transport.appliedCommand===before.transport.appliedCommand+1);
 assert.equal((await snapshot()).waveform.buildMs,before.waveform.buildMs);report.checks.push('ASIO drag: 100 moves, 0 intermediate commands, 1 final seek; same waveform cache');
 const saved={...(await snapshot()).preferences,transportDeclick:false};await invoke('apply_audio_settings',{settings:saved});await close();await launch();
 s=await until(s=>s.output?.driverType==='asio');assert.deepEqual(s.preferences,saved);assert.equal(s.file,null);report.restored=s;report.checks.push('Real app restart restores ASIO driver and De-click OFF without a project');
 const missing={...saved,asioDriver:'MiniDAW deliberate missing driver test'};
 await assert.rejects(invoke('apply_audio_settings',{settings:missing}));s=await snapshot();assert.equal(s.output,null);assert.ok(s.outputError);assert.equal(s.preferences.driverType,'asio');
 await close();await launch();s=await snapshot();assert.equal(s.output,null);assert.equal(s.preferences.asioDriver,missing.asioDriver);assert.ok(s.outputError);report.missing=s;
 await open();await page.locator('#driver-type').selectOption('wasapi');await until(async()=>!(await page.locator('#output-device').isDisabled()));await page.locator('#output-device').selectOption('');await page.locator('#transport-declick').check();await apply();
 s=await until(s=>s.output?.driverType==='wasapi');assert.equal(s.outputError,null);await page.locator('#close-audio-settings').click();
 report.checks.push('Missing driver persisted across restart: explicit Korean error, no fallback; user-selected WASAPI recovers');
 await invoke('apply_audio_settings',{settings:original});
 report.final=await snapshot();report.passed=true;
}catch(e){report.error=String(e.stack??e);throw e;}
finally{if(page&&original)await invoke('apply_audio_settings',{settings:original}).catch(()=>{});await close();await writeFile(resolve(directory,'audio-settings.json'),JSON.stringify(report,null,2));console.log(JSON.stringify({passed:report.passed,error:report.error,checks:report.checks}));}
