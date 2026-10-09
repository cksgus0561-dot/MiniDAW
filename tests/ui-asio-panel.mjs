// Leaves the driver's real panel open for native Computer Use inspection/close.
import { chromium } from 'playwright-core';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { writeFile,stat,readFile } from 'node:fs/promises';
import { createHash } from 'node:crypto';
import { resolve } from 'node:path';
import assert from 'node:assert/strict';
const child=spawn(resolve('src-tauri/target/release/minidaw.exe'),[],{env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=19230'},stdio:'ignore',windowsHide:true});
const sleep=ms=>new Promise(r=>setTimeout(r,ms));let browser;
const report={started:new Date().toISOString(),executableSha256:createHash('sha256').update(await readFile('src-tauri/target/release/minidaw.exe')).digest('hex')};
try {
 for(let i=0;i<100;i++){try{browser=await chromium.connectOverCDP('http://127.0.0.1:19230');break;}catch{await sleep(100);}}
 assert.ok(browser);let page;
 for(let i=0;i<100;i++){page=browser.contexts()[0].pages().find(p=>p.url().includes('tauri.localhost'));if(page)break;await sleep(50);}assert.ok(page);
 const invoke=(name,args={})=>page.evaluate(({name,args})=>window.__TAURI_INTERNALS__.invoke(name,args),{name,args});
 const original=(await invoke('engine_snapshot')).preferences;
 await invoke('apply_audio_settings',{settings:{...original,driverType:'asio',asioDriver:'Topping USB Audio Device'}});
 await page.locator('#open-audio-settings').click();await sleep(300);
 report.before=await invoke('engine_snapshot');
 await page.locator('#asio-panel').click();
 console.log('Real ASIO Control Panel requested; inspect and close the driver window.');
 let observedClose=false;
 for(let i=0;i<1800;i++) {try{if((await stat('tests/local/asio-panel-closed.marker')).mtimeMs>Date.parse(report.started)){observedClose=true;break;}}catch{}await sleep(100);}
 assert.ok(observedClose,'Native panel close was not confirmed');
 for(let i=0;i<500;i++){if(!(await page.locator('#asio-panel').isDisabled()))break;await sleep(20);}
 report.after=await invoke('engine_snapshot');assert.equal(report.after.outputError,null);assert.equal(report.after.output.driverType,'asio');
 assert.equal((await page.locator('#audio-settings-error').textContent()).trim(),'');
 await invoke('apply_audio_settings',{settings:original});report.passed=true;
}catch(e){report.error=String(e.stack??e);throw e;}
finally{if(browser)await browser.close();if(child.exitCode===null){const done=once(child,'exit');child.kill();await done;}await writeFile('docs/validation/asio-panel.json',JSON.stringify(report,null,2));console.log(JSON.stringify({passed:report.passed,error:report.error}));}
