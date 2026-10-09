// Same real release IPC protocol before/after; acknowledgement includes IPC/poll overhead.
import { chromium } from 'playwright-core';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { readFile, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { resolve } from 'node:path';
import assert from 'node:assert/strict';
const exe=resolve(process.argv[2]), destination=process.argv[3];
const backend=process.argv[4], declick=process.argv[5]!=='off';
const report={exe,executableSha256:createHash('sha256').update(await readFile(exe)).digest('hex'),started:new Date().toISOString(),runs:[]};
const child=spawn(exe,[],{env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=19228'},stdio:'ignore',windowsHide:true});
const sleep=ms=>new Promise(r=>setTimeout(r,ms));let browser,page,originalAudio;
try {
 for(let i=0;i<100;i++){try{browser=await chromium.connectOverCDP('http://127.0.0.1:19228');break;}catch{await sleep(100);}}
 assert.ok(browser);
 for(let i=0;i<100;i++){page=browser.contexts()[0].pages().find(p=>p.url().includes('tauri.localhost'));if(page)break;await sleep(50);}
 assert.ok(page);
 const invoke=(name,args={})=>page.evaluate(({name,args})=>window.__TAURI_INTERNALS__.invoke(name,args),{name,args});
 const snap=()=>invoke('engine_snapshot');
 originalAudio=(await snap()).preferences;
 const until=async fn=>{for(let i=0;i<1000;i++){const s=await snap();if(fn(s))return s;await sleep(2);}throw Error('Timed out');};
 if(backend){const settings=(await snap()).preferences;settings.driverType=backend;settings.transportDeclick=declick;if(backend==='asio'){const names=await invoke('output_devices',{driverType:backend});assert.ok(names.length);settings.asioDriver=names[0];}await invoke('apply_audio_settings',{settings});}
 await until(s=>s.output&&!s.outputError);report.initial=await snap();
 for(const extension of ['wav','mp3','flac']) {
  const current=await invoke('project_snapshot');await invoke('new_project',{revision:current.revision,discard:true});
  await invoke('load_audio',{path:resolve(`tests/fixtures/stereo-44100.${extension}`)});
  for(let round=0;round<3;round++) {
   const run={extension,round,commands:[]};report.runs.push(run);
   for(const [action,seconds] of [['play'],['pause'],['play'],['seek',3.137],['stop']]) {
    const started=performance.now();const id=await invoke('transport_command',{action,seconds:seconds??null});
    const s=await until(s=>s.transport.appliedCommand>=id&&(action!=='seek'||s.metrics.seekMs!==null)&&(action!=='play'||s.metrics.playMs!==null));
    run.commands.push({action,acknowledgedMs:performance.now()-started,snapshot:s});
    if(action==='pause'){const p=s.position;await sleep(80);assert.equal((await snap()).position,p);}
    if(action==='stop')assert.equal(s.position,0);
    await sleep(130);
   }
  }
 }
 report.final=await snap();assert.equal(report.final.streamErrors,0);assert.equal(report.final.metrics.overruns,0);if(backend)assert.equal(report.final.metrics.deviceCallbackOverruns,0);assert.equal(report.final.source.starvation,0);report.passed=true;
} catch(e){report.error=String(e.stack??e);throw e;}
finally {if(page&&originalAudio)await page.evaluate(settings=>window.__TAURI_INTERNALS__.invoke('apply_audio_settings',{settings}),originalAudio).catch(()=>{});if(browser)await browser.close();if(child.exitCode===null){const exited=once(child,'exit');child.kill();await exited;}await writeFile(destination,JSON.stringify(report,null,2));console.log(JSON.stringify({passed:report.passed,error:report.error,output:report.final?.output,metrics:report.final?.metrics}));}
