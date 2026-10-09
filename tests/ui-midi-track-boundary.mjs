// Focused native release regression: MIDI header controls never overlap resize hit area.
import {chromium} from 'playwright-core';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {readFile,writeFile,unlink} from 'node:fs/promises';
import {resolve,join} from 'node:path';
import assert from 'node:assert/strict';
const report={checks:[],geometry:[]},sleep=ms=>new Promise(r=>setTimeout(r,ms));
const recent=join(process.env.APPDATA,'local.minidaw.desktop/recent-projects.json');let savedRecent;try{savedRecent=await readFile(recent);}catch{}
let child,browser,page,prefs;
async function until(fn,label){const at=performance.now();while(performance.now()-at<20000){if(await fn())return;await sleep(40);}throw Error(label);}
const invoke=(name,args={})=>page.evaluate(({name,args})=>window.__TAURI_INTERNALS__.invoke(name,args),{name,args});
const project=()=>invoke('project_snapshot');
async function idle(){await until(async()=>!await page.locator('#project-save').isDisabled(),'Idle');await sleep(120);}
async function resize(id,dy){
 const h=page.locator(`.track-header[data-track-id="${id}"] .track-resize`);await h.scrollIntoViewIfNeeded();const b=await h.boundingBox();
 const before=await project();await page.mouse.move(b.x+50,b.y+3);await page.mouse.down();await page.mouse.move(b.x+50,b.y+3+dy,{steps:15});await page.mouse.up();await sleep(140);
 assert.equal((await project()).revision,before.revision,'Resize must not edit the project');
}
async function geometry(scale){
 const rows=await page.evaluate(()=>[...document.querySelectorAll('.midi-track-header')].map(e=>{
  const rect=n=>n.getBoundingClientRect().toJSON(),button=e.querySelector('.midi-clip-add'),handle=e.querySelector('.track-resize');
  const b=rect(button),h=rect(handle),timeline=rect(document.querySelector(`#clip-layer > [data-resize-track="${e.dataset.trackId}"]`));
  return{id:e.dataset.trackId,header:rect(e),button:b,handle:h,gap:h.top-b.bottom,aligned:Math.abs(timeline.bottom-rect(e).bottom)<1.1,
   buttonHit:document.elementFromPoint(b.x+b.width/2,b.bottom-2)===button,resizeHit:document.elementFromPoint(h.x+50,h.y+3)===handle};
 }));report.geometry.push({scale,rows});return rows;
}
try{
 child=spawn(resolve('src-tauri/target/release/minidaw.exe'),[],{env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=19257'},stdio:'ignore',windowsHide:true});
 await until(async()=>{try{browser=await chromium.connectOverCDP('http://127.0.0.1:19257');return true;}catch{return false;}},'CDP');
 await until(async()=>{page=browser.contexts()[0].pages().find(p=>p.url().includes('tauri.localhost'));return !!page;},'Page');await page.setViewportSize({width:1400,height:1000});await idle();
 prefs=await page.evaluate(()=>Object.fromEntries(['minidaw.ui.panels.v1','minidaw.ui.fontScale'].map(k=>[k,localStorage.getItem(k)])));
 for(const id of ['arrangement','media','performance','spectrum','piano']){const b=page.locator('#show-'+id),open=id==='arrangement';if((await b.getAttribute('aria-pressed')==='true')!==open)await b.click();}
 for(let i=0;i<2;i++){await page.locator('#midi-track-add').click();await idle();}
 const ids=(await project()).document.tracks.map(t=>t.trackId);
 for(const scale of ['90','110','125']){
  await page.locator('#font-scale').selectOption(scale);await sleep(160);
  for(const id of ids)await resize(id,-70);
  for(const row of await geometry(scale)){assert.ok(row.gap>=3,`Button/resize gap ${row.gap}px at ${scale}%`);assert.ok(row.aligned);assert.ok(row.buttonHit);assert.ok(row.resizeHit);}
 }
 const height=()=>page.locator(`.track-header[data-track-id="${ids[0]}"]`).evaluate(e=>e.offsetHeight);
 const before=await height();await resize(ids[0],90);assert.ok(await height()>before+70);await resize(ids[0],-200);
 await page.locator(`.track-header[data-track-id="${ids[0]}"] .midi-clip-add`).click();await idle();assert.equal((await project()).document.tracks[0].clips.length,1);
 await page.locator('#show-piano').click();await page.setViewportSize({width:900,height:1000});await sleep(180);
 for(const row of await geometry('125/narrow')){assert.ok(row.gap>=3);assert.ok(row.aligned);assert.ok(row.buttonHit);assert.ok(row.resizeHit);}
 await page.screenshot({path:'docs/validation/midi-track-boundary.png'});
 report.checks.push('90/110/125% fonts, two independently resized MIDI Tracks at minimum height, separate button/resize hit targets, header/timeline alignment, actual Clip creation, narrow window.');report.passed=true;
}catch(e){report.error=String(e.stack??e);process.exitCode=1;if(page)await page.screenshot({path:'docs/validation/midi-track-boundary-failure.png'}).catch(()=>{});}
finally{
 if(page&&prefs)await page.evaluate(p=>{for(const[k,v]of Object.entries(p)){if(v===null)localStorage.removeItem(k);else localStorage.setItem(k,v);}},prefs).catch(()=>{});
 if(browser)await browser.close();if(child?.exitCode===null){const exited=once(child,'exit');child.kill();await exited;}
 if(savedRecent)await writeFile(recent,savedRecent);else await unlink(recent).catch(()=>{});
 await writeFile('docs/validation/midi-track-boundary.json',JSON.stringify(report,null,2));console.log(JSON.stringify(report));
}
