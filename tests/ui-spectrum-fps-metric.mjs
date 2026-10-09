// UI-only release check: actual Canvas paints versus the independent global rAF metric.
import {chromium} from 'playwright-core';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {writeFile} from 'node:fs/promises';
import assert from 'node:assert/strict';
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
let browser,page,child,prefs;const report={samples:[]},errors=[];
async function until(fn){const start=performance.now();while(performance.now()-start<20000){if(await fn())return;await sleep(50);}throw Error('Timed out');}
async function panel(id,open){const b=page.locator('#show-'+id);if((await b.getAttribute('aria-pressed')==='true')!==open)await b.click();}
try{
 child=spawn('src-tauri/target/release/minidaw.exe',[],{env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=19241'},stdio:'ignore',windowsHide:true});
 await until(async()=>{try{browser=await chromium.connectOverCDP('http://127.0.0.1:19241');return true;}catch{return false;}});
 await until(async()=>{page=browser.contexts()[0].pages().find(p=>p.url().includes('tauri.localhost'));return !!page;});
 page.on('pageerror',e=>errors.push(e.message));await page.setViewportSize({width:1440,height:960});
 await until(async()=>!await page.locator('#project-save').isDisabled());
 prefs=await page.evaluate(()=>Object.fromEntries(['minidaw.ui.panels.v1','minidaw.ui.spectrum.v1'].map(k=>[k,localStorage.getItem(k)])));
 await panel('media',false);await panel('performance',true);await panel('spectrum',true);
 await page.locator('#spectrum-mode').selectOption('live');await page.mouse.move(5,5);
 await page.evaluate(()=>{
   window.__paints=0;const fill=CanvasRenderingContext2D.prototype.fillRect;
   CanvasRenderingContext2D.prototype.fillRect=function(...args){if(this.canvas.id==='spectrum-canvas')window.__paints++;return fill.apply(this,args);};
 });
 for(const state of ['live','closed','reopened','selection']){
   if(state==='closed')await panel('spectrum',false);
   if(state==='reopened')await panel('spectrum',true);
   if(state==='selection')await page.locator('#spectrum-mode').selectOption('selection');
   await sleep(1100);
   const start=await page.evaluate(()=>({t:performance.now(),count:window.__paints}));await sleep(1500);
   const result=await page.evaluate(start=>({actualFps:(window.__paints-start.count)*1000/(performance.now()-start.t),spectrumFps:Number(document.getElementById('metric-spectrum-fps').textContent),globalFps:Number(document.getElementById('metric-fps').textContent)}),start);
   report.samples.push({state,...result});assert.ok(result.globalFps>45,'Global UI metric remains active');
   if(state==='live'||state==='reopened'){
     assert.ok(result.actualFps>55&&result.actualFps<65);assert.ok(Math.abs(result.spectrumFps-result.actualFps)<4);
   }else{assert.equal(result.actualFps,0);assert.equal(result.spectrumFps,0);}
 }
 assert.deepEqual(errors,[]);report.passed=true;
}catch(e){report.error=String(e.stack??e);process.exitCode=1;}
finally{
 if(page&&prefs)await page.evaluate(p=>{for(const[k,v]of Object.entries(p)){if(v===null)localStorage.removeItem(k);else localStorage.setItem(k,v);}},prefs).catch(()=>{});
 if(browser)await browser.close();if(child?.exitCode===null){const exited=once(child,'exit');child.kill();await exited;}
 await writeFile('docs/validation/spectrum-fps-metric.json',JSON.stringify(report,null,2));console.log(JSON.stringify(report));
}
