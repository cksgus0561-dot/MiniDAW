// Guided native title-bar close test. Seed a disposable in-memory document using
// the real IPC, then wait for actual OS close / Cancel / close / Discard actions.
import { chromium } from 'playwright-core';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { readFile, writeFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { resolve } from 'node:path';
import assert from 'node:assert/strict';
const exe=resolve('src-tauri/target/release/minidaw.exe');
const report={executableSha256:createHash('sha256').update(await readFile(exe)).digest('hex'),checks:[]};
const child=spawn(exe,[],{env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=19232'},stdio:'ignore',windowsHide:true});
let browser,page;const sleep=ms=>new Promise(r=>setTimeout(r,ms));
async function until(fn,message){const t=Date.now();while(Date.now()-t<240000){if(await fn())return;await sleep(100);}throw Error(message);}
try{
 await until(async()=>{try{browser=await chromium.connectOverCDP('http://127.0.0.1:19232');return true;}catch{return false;}},'WebView connection');
 await until(async()=>{page=browser.contexts()[0].pages().find(p=>p.url().includes('tauri.localhost'));return Boolean(page);},'Tauri page');
 const invoke=(name,args={})=>page.evaluate(({name,args})=>window.__TAURI_INTERNALS__.invoke(name,args),{name,args});
 await invoke('load_audio',{path:resolve('tests/fixtures/stereo-44100.wav')});const before=await invoke('project_snapshot');assert.equal(before.dirty,true);
 console.log('READY: click native title-bar X, then Cancel.');
 await until(()=>page.locator('#project-confirm').isVisible(),'Native close prompt missing');
 report.checks.push('Native OS close raises Save/Discard/Cancel for dirty project.');
 await until(async()=>!(await page.locator('#project-confirm').isVisible()),'Cancel not chosen');
 assert.equal(child.exitCode,null);assert.deepEqual((await invoke('project_snapshot')).document,before.document);
 report.checks.push('Cancel keeps the actual window and exact document.');
 console.log('READY: click native title-bar X again, then Discard.');
 await until(()=>page.locator('#project-confirm').isVisible(),'Second native prompt missing');
 await Promise.race([once(child,'exit'),new Promise((_,reject)=>setTimeout(()=>reject(Error('Discard did not close window')),240000).unref())]);
 report.checks.push('Discard closes the actual process; only disposable unsaved test state was discarded.');report.passed=true;
}catch(e){report.error=String(e.stack??e);process.exitCode=1;}
finally{if(browser)await browser.close();if(child.exitCode===null){const exited=once(child,'exit');child.kill();await exited;}await writeFile('docs/validation/project-native.json',JSON.stringify(report,null,2));console.log(JSON.stringify(report));}
