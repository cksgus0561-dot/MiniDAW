import {chromium} from 'playwright-core';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {readFile,writeFile,unlink} from 'node:fs/promises';
import {resolve,join} from 'node:path';
import assert from 'node:assert/strict';
let child,browser,page,prefs;const report={checks:[]},errors=[],sleep=ms=>new Promise(r=>setTimeout(r,ms));
const recent=join(process.env.APPDATA,'local.minidaw.desktop/recent-projects.json');let savedRecent;try{savedRecent=await readFile(recent);}catch{}
async function until(f,label){const at=Date.now();while(Date.now()-at<20000){if(await f())return;await sleep(60);}throw Error(label);}
const invoke=(name,args={})=>page.evaluate(({name,args})=>window.__TAURI_INTERNALS__.invoke(name,args),{name,args});
const project=()=>invoke('project_snapshot');
async function idle(){await until(async()=>!await page.locator('#project-save').isDisabled(),'Idle');await sleep(150);}
async function edit(request){await invoke('edit_project',{revision:(await project()).revision,request});await idle();}
async function start(){
 child=spawn(resolve('src-tauri/target/release/minidaw.exe'),[],{env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=19278'},stdio:'ignore',windowsHide:true});
 await until(async()=>{try{browser=await chromium.connectOverCDP('http://127.0.0.1:19278');return true;}catch{return false;}},'CDP');
 await until(async()=>{page=browser.contexts()[0].pages().find(p=>p.url().includes('tauri.localhost'));return !!page;},'Page');page.on('pageerror',e=>errors.push(e.message));await page.setViewportSize({width:1440,height:1000});await idle();
}
async function quit(){await browser?.close();browser=null;if(child?.exitCode===null){const exited=once(child,'exit');child.kill();await exited;}child=null;}
async function panel(id,open=true){if((await page.locator('#show-'+id).getAttribute('aria-pressed')==='true')!==open)await page.locator('#show-'+id).click();await sleep(100);}
const rect=id=>page.locator('#'+id).boundingBox();
const layout=()=>page.evaluate(()=>({pref:JSON.parse(localStorage.getItem('minidaw.ui.zones.v1')),regions:Object.fromEntries(['workspace','zone-center','zone-left','zone-right','zone-lower'].map(id=>{const e=document.getElementById(id),r=e.getBoundingClientRect();return[id,{x:r.x,y:r.y,w:r.width,h:r.height,hidden:e.hidden,scroll:e.scrollHeight,client:e.clientHeight}];})),visible:[...document.querySelectorAll('.workspace-panel:not([hidden])')].map(p=>p.id)}));
const near=(a,b,label)=>assert.ok(Math.abs(a-b)<1.1,`${label}: ${a} / ${b}`);
async function drag(id,delta){const b=await rect('resize-'+id),x=b.x+b.width/2,y=b.y+b.height/2;await page.mouse.move(x,y);await page.mouse.down();assert.ok(await page.locator('#resize-'+id).evaluate(e=>e.hasPointerCapture(1)),'Pointer capture');await page.mouse.move(x+(id==='lower'?0:delta),y+(id==='lower'?delta:0),{steps:30});await page.mouse.up();await sleep(120);assert.equal(await page.locator('#workspace').evaluate(e=>e.classList.contains('resizing')),false);}
try{
 await start();prefs=await page.evaluate(()=>Object.fromEntries(Object.entries(localStorage).filter(([k])=>k.startsWith('minidaw.ui.'))));
 await page.evaluate(()=>{for(const k of ['minidaw.ui.zones.v1','minidaw.ui.panels.v1','minidaw.ui.shortcuts.v1'])localStorage.removeItem(k);localStorage.setItem('minidaw.ui.language.v1','ko');localStorage.setItem('minidaw.ui.fontScale','110');});await page.reload();await idle();
 let p=await project();await invoke('new_project',{revision:p.revision,discard:true});await idle();
 let initial=await layout();assert.deepEqual(initial.visible,['panel-arrangement','panel-media']);assert.ok(initial.regions['zone-center'].w>1100);report.default=initial;
 await page.screenshot({path:'docs/validation/workspace-zones-default.png'});
 await panel('spectrum');await panel('piano');let l=await layout();near(l.regions['zone-lower'].x,l.regions['zone-center'].x,'Lower and Center left');near(l.regions['zone-lower'].w,l.regions['zone-center'].w,'Lower and Center width');near(l.regions['zone-lower'].y,l.regions['zone-center'].y+l.regions['zone-center'].h+6,'Lower below Center');
 assert.equal(l.visible.length,4);assert.ok(l.regions['zone-center'].w>800);
 await page.evaluate(()=>{window.__zonePaints={};document.addEventListener('panel-visibility',e=>(window.__zoneEvents??=[]).push(e.detail));const f=CanvasRenderingContext2D.prototype.fillRect;CanvasRenderingContext2D.prototype.fillRect=function(...a){window.__zonePaints[this.canvas.id]=(window.__zonePaints[this.canvas.id]??0)+1;return f.apply(this,a);};});
 await page.locator('#zone-tab-performance').click();await sleep(120);assert.equal(await page.locator('#panel-spectrum').isVisible(),false);assert.equal(await page.locator('#panel-performance').isVisible(),true);const paints=await page.evaluate(()=>window.__zonePaints['spectrum-canvas']??0);await sleep(400);assert.equal(await page.evaluate(()=>window.__zonePaints['spectrum-canvas']??0),paints);assert.equal((await invoke('spectrum_snapshot',{after:0})).enabled,false);
 assert.equal(await page.locator('#show-spectrum').getAttribute('aria-pressed'),'false');assert.equal(await page.locator('#show-performance').getAttribute('aria-pressed'),'true');
 await page.locator('#zone-tab-mixer').click();await sleep(100);assert.equal(await page.locator('#panel-piano').isVisible(),false);assert.equal(await page.locator('#panel-mixer').isVisible(),true);
 const events=await page.evaluate(()=>window.__zoneEvents);assert.ok(events.some(e=>e.id==='spectrum'&&!e.open)&&events.some(e=>e.id==='performance'&&e.open)&&events.some(e=>e.id==='piano'&&!e.open)&&events.some(e=>e.id==='mixer'&&e.open));
 report.checks.push('Fixed Center/Left/Right/Lower geometry; one visible panel per tabbed Zone; inactive Spectrum stops Canvas rendering and analysis; both visibility transitions delivered.');
 const documentBefore=(await project()).document;
 await drag('left',37);await drag('right',-43);await drag('lower',-56);l=await layout();report.resized=l;
 for(const[id,active,dimension]of [['left','media','w'],['right','performance','w'],['lower','mixer','h']]){
  const before=await layout();await panel(active,false);const closed=await layout();assert.ok(closed.regions['zone-center'][dimension]>before.regions['zone-center'][dimension]);assert.equal(closed.regions['zone-'+id].hidden,true);
  await panel(active,true);const after=await layout();near(after.regions['zone-'+id][dimension],before.regions['zone-'+id][dimension],id+' reopened size');
 }
 assert.deepEqual((await project()).document,documentBefore,'Layout makes no project edits');
 const saved=await layout();await quit();await start();const restored=await layout();assert.deepEqual(restored.pref,saved.pref);for(const id of ['left','right','lower']){near(restored.regions['zone-'+id].w,saved.regions['zone-'+id].w,'restart width');near(restored.regions['zone-'+id].h,saved.regions['zone-'+id].h,'restart height');}assert.deepEqual(restored.visible,saved.visible);
 report.checks.push('All three pointer-captured divider drags, space return on close, exact reopen dimensions, and real process restart preserve open Zones, active tabs and sizes; no project mutation.');
 // Real panel functions keep their command/data paths inside the new hosts.
 await panel('piano',false);await panel('mixer',false);await panel('performance',false);
 await invoke('load_audio',{path:resolve('tests/fixtures/stereo-44100.wav'),trackId:null});await idle();assert.equal(await page.locator('.project-asset').count(),1);assert.ok((await rect('clip-layer')).width>500);assert.equal(await page.locator('.audio-clip').count(),1);
 
 await panel('mixer');report.channelCount=await page.locator('.mixer-channel').count();
 // Scope by the first Track strip (Master remains the last strip).
 const volume=page.locator('#mixer-tracks .mixer-volume').first();await volume.fill('-8');await volume.press('Enter');await idle();await until(async()=>(await project()).document.tracks[0].mix.volumeDb===-8,'Mixer numeric volume');
 await panel('spectrum');await page.locator('#play-pause').click();await until(async()=>(await invoke('engine_snapshot')).transport.state==='playing','Play');await until(async()=>(await invoke('spectrum_snapshot',{after:0})).frame!==null,'Spectrum live frame');assert.ok((await rect('spectrum-canvas')).width>100);await page.locator('#zone-tab-performance').click();await sleep(450);assert.notEqual(await page.locator('#metric-rate').textContent(),'—');await page.locator('#stop').click();
 report.checks.push('Media import/list and Arrangement waveform, Mixer numeric volume, real Audio transport, Spectrum live data and Performance metrics operate in their Zones.');
 await panel('media',false);await panel('performance',false);
 const ids=[];
 for(const start of [0,1920000]){await edit({command:'midi.track.add'});p=await project();const t=p.document.tracks.at(-1);await edit({command:'midi.clip.add',trackIds:[t.trackId],targetTick:String(start),lengthTick:'7680000'});p=await project();const c=p.document.tracks.at(-1).clips[0];ids.push(c.clipId);await edit({command:'midi.note.add',clipIds:[c.clipId],targetTick:'2400000',lengthTick:'480000',pitch:64});}
 // Arrangement scroll uses its own viewport; reach the new Track without moving the whole workspace.
 await panel('mixer',false);await page.locator(`[data-clip-id="${ids[0]}"]`).click();await page.keyboard.down('Control');await page.locator(`[data-clip-id="${ids[1]}"]`).click();await page.keyboard.up('Control');await page.locator('#piano-open').click();await idle();
 assert.equal(await page.locator('#piano-parts button').count(),2);assert.equal(await page.locator('#zone-tab-piano').getAttribute('aria-selected'),'true');
 await page.locator('#piano-zoom-in').click();await page.locator('#piano-scroll').evaluate(e=>{e.value=String(Number(e.max)*.2);e.dispatchEvent(new Event('input',{bubbles:true}));});await sleep(100);
 const camera=()=>page.evaluate(()=>({start:document.querySelector('#piano-scroll').value,step:document.querySelector('#piano-scroll').step}));const before=await camera();
 await page.locator('#piano-canvas').focus();await page.keyboard.press('Control+a');await page.locator('#note-time-format').selectOption('ticks');await page.locator('#note-start').fill('2400123');await page.locator('#note-start').press('Enter');await idle();assert.equal((await project()).document.tracks[1].clips[0].notes[0].startTick,'2400123');
 await page.locator('#zone-tab-mixer').click();await page.locator('#zone-tab-piano').click();await sleep(100);assert.deepEqual(await camera(),before);assert.equal(await page.locator('#note-start').inputValue(),'2400123');
 await page.locator('#piano-canvas').focus();await page.keyboard.press('Control+z');await idle();assert.equal((await project()).document.tracks[1].clips[0].notes[0].startTick,'2400000');await page.keyboard.press('Control+Shift+z');await idle();assert.equal((await project()).document.tracks[1].clips[0].notes[0].startTick,'2400123');
 report.checks.push('Multi-Part Piano Roll opens in Lower Zone, numeric note editing and Undo/Redo work; Mixer↔Piano switches preserve note selection, horizontal scroll and zoom.');
 await panel('media');await panel('spectrum');await page.screenshot({path:'docs/validation/workspace-zones-all.png'});
 report.sizes=[];
 for(const[w,h,scale]of [[1280,800,'110'],[1440,1000,'125'],[760,520,'125'],[1440,1000,'110']]){
  const pref=(await layout()).pref;await page.setViewportSize({width:w,height:h});await page.locator('#font-scale').selectOption(scale);await sleep(170);const a=await layout();report.sizes.push({w,h,scale,...a});assert.deepEqual(a.pref,pref,'window and font resize do not destroy preferred dimensions');
  assert.ok(a.regions['zone-center'].w>300);assert.ok(a.regions['zone-lower'].h>0);assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth||document.documentElement.scrollHeight>innerHeight),false);
  const borders=await page.locator('.workspace-panel:not([hidden])').evaluateAll(es=>es.map(e=>['Top','Right','Bottom','Left'].map(x=>getComputedStyle(e)['border'+x+'Width'])));assert.ok(borders.every(a=>a.every(w=>w==='2px')));
  if(w===1280)await page.screenshot({path:'docs/validation/workspace-zones-1280.png'});
  // In a short window the zone scrolls its intact content instead of clipping it.
  const footer=page.locator('#panel-piano .piano-footer');await footer.scrollIntoViewIfNeeded();assert.equal(await footer.isVisible(),true);const foot=await footer.boundingBox(),piano=await rect('panel-piano');assert.ok(foot.y+foot.height<=piano.y+piano.height+1,'Piano footer is inside its panel, never clipped');assert.ok((await rect('piano-canvas')).height>=56);await page.locator('#zone-lower').evaluate(e=>e.scrollTop=0);
 }
 // Centre may be explicitly hidden; Lower uses all height and restores on return.
 await panel('arrangement',false);l=await layout();near(l.regions['zone-lower'].h,l.regions.workspace.h,'Lower fills Center column');await panel('arrangement');
 await panel('spectrum',false);await panel('piano',false);assert.equal(await page.locator('#panel-arrangement').isVisible(),true);
 report.checks.push('Font Scale, 1280×800 through minimum-size window, matching four-sided borders, and Arrangement hide/restore; no whole-window overflow or preference loss.');
 assert.deepEqual(errors,[]);assert.equal(await page.locator('#error').isVisible(),false);report.passed=true;
}catch(e){report.error=String(e.stack??e);report.pageErrors=errors;process.exitCode=1;if(page){report.layout=await layout().catch(()=>null);report.uiError=await page.locator('#error-text').textContent().catch(()=>null);await page.screenshot({path:'docs/validation/workspace-zones-failure.png'}).catch(()=>{});}}
finally{
 if(page&&prefs)await page.evaluate(p=>{for(const k of Object.keys(localStorage).filter(k=>k.startsWith('minidaw.ui.')))localStorage.removeItem(k);for(const[k,v]of Object.entries(p))localStorage.setItem(k,v);},prefs).catch(()=>{});
 await quit();if(savedRecent)await writeFile(recent,savedRecent);else await unlink(recent).catch(()=>{});
 await writeFile('docs/validation/workspace-zones-ui.json',JSON.stringify(report,null,2));console.log(JSON.stringify(report));
}
