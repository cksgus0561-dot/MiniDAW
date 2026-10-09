import {chromium} from 'playwright-core';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {writeFile} from 'node:fs/promises';
import {resolve} from 'node:path';
import assert from 'node:assert/strict';
const before=process.argv.includes('--before'),sleep=ms=>new Promise(r=>setTimeout(r,ms));
let child,browser,page,prefs;const report={before},errors=[];
async function until(fn){const at=performance.now();while(performance.now()-at<20000){if(await fn())return;await sleep(50);}throw Error('Timeout');}
const invoke=(name,args={})=>page.evaluate(({name,args})=>window.__TAURI_INTERNALS__.invoke(name,args),{name,args});
async function panel(id,open){const b=page.locator('#show-'+id);if((await b.getAttribute('aria-pressed')==='true')!==open)await b.click();}
try{
 child=spawn(resolve('src-tauri/target/release/minidaw.exe'),[],{env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=19243'},stdio:'ignore',windowsHide:true});
 await until(async()=>{try{browser=await chromium.connectOverCDP('http://127.0.0.1:19243');return true;}catch{return false;}});
 await until(async()=>{page=browser.contexts()[0].pages().find(p=>p.url().includes('tauri.localhost'));return !!page;});
 page.on('pageerror',e=>errors.push(e.message));await page.setViewportSize({width:1440,height:960});await until(async()=>!await page.locator('#project-save').isDisabled());
 prefs=await page.evaluate(()=>localStorage.getItem('minidaw.ui.panels.v1'));
 await panel('arrangement',true);for(const id of ['media','performance','spectrum'])await panel(id,false);
 await page.evaluate(()=>{
   window.__rulerLabels=[];const fill=CanvasRenderingContext2D.prototype.fillRect,text=CanvasRenderingContext2D.prototype.fillText;
   CanvasRenderingContext2D.prototype.fillRect=function(...args){if(this.canvas.id==='waveform')window.__rulerLabels=[];return fill.apply(this,args);};
   CanvasRenderingContext2D.prototype.fillText=function(...args){if(this.canvas.id==='waveform')window.__rulerLabels.push(String(args[0]));return text.apply(this,args);};
 });
 await invoke('load_audio',{path:resolve('tests/fixtures/stereo-44100.wav')});await sleep(400);await page.locator('#zoom-fit').click();await sleep(150);
 report.bars=await page.evaluate(()=>window.__rulerLabels);
 assert.ok(report.bars.length>0&&!report.bars.some(s=>s.includes(':')));
 if(before){report.formatControl=await page.locator('#ruler-format').count();assert.equal(report.formatControl,0);report.selectOptions=await page.locator('#panel-arrangement select option').allTextContents();}
 else{
   const p=await invoke('project_snapshot'),a=await invoke('engine_snapshot'),grid=await page.locator('#clip-layer').evaluate(n=>n.style.backgroundImage);
   await page.locator('#ruler-format').selectOption('seconds');await sleep(150);
   report.seconds=await page.evaluate(()=>window.__rulerLabels);assert.ok(report.seconds.some(s=>/^\d+:\d+/.test(s)));
   assert.equal(await page.locator('#clip-layer').evaluate(n=>n.style.backgroundImage),grid);
   await page.locator('#zoom-in').click();await sleep(100);await page.locator('#zoom-in').click();await sleep(100);
   await page.locator('#scroll').evaluate(n=>{n.value=String(Math.min(1,Number(n.max)));n.dispatchEvent(new Event('input',{bubbles:true}));});await sleep(100);
   report.zoomed=await page.evaluate(()=>window.__rulerLabels);assert.ok(report.zoomed.some(s=>s.includes(':')));
   await page.locator('#ruler-format').selectOption('bars');await sleep(150);assert.ok(!(await page.evaluate(()=>window.__rulerLabels)).some(s=>s.includes(':')));
   assert.deepEqual((await invoke('project_snapshot')).document,p.document);assert.equal((await invoke('engine_snapshot')).transport.appliedCommand,a.transport.appliedCommand);
   await page.locator('#ruler-format').selectOption('seconds');await sleep(100);await page.screenshot({path:'docs/validation/ruler-seconds.png'});
 }
 assert.deepEqual(errors,[]);report.passed=true;
}catch(e){report.error=String(e.stack??e);process.exitCode=1;}
finally{
 if(page&&prefs!==undefined)await page.evaluate(p=>{if(p===null)localStorage.removeItem('minidaw.ui.panels.v1');else localStorage.setItem('minidaw.ui.panels.v1',p);},prefs).catch(()=>{});
 if(browser)await browser.close();if(child?.exitCode===null){const exited=once(child,'exit');child.kill();await exited;}
 await writeFile(`docs/validation/ruler-format-${before?'before':'after'}.json`,JSON.stringify(report,null,2));console.log(JSON.stringify(report));
}
