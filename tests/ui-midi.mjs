import {chromium} from 'playwright-core';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {readFile,writeFile,mkdir,unlink} from 'node:fs/promises';
import {randomUUID} from 'node:crypto';
import {resolve,join} from 'node:path';
import assert from 'node:assert/strict';
const folder=resolve('tests/local/midi',randomUUID());await mkdir(folder,{recursive:true});
const recent=join(process.env.APPDATA,'local.minidaw.desktop/recent-projects.json');let savedRecent;try{savedRecent=await readFile(recent);}catch{}
let child,browser,page,prefs;const report={checks:[],notes:[]},errors=[],sleep=ms=>new Promise(r=>setTimeout(r,ms));
async function until(fn,label){const at=performance.now();while(performance.now()-at<20000){if(await fn())return;await sleep(40);}throw Error(label);}
const invoke=(name,args={})=>page.evaluate(({name,args})=>window.__TAURI_INTERNALS__.invoke(name,args),{name,args});
const project=()=>invoke('project_snapshot');
const part=async()=>(await project()).document.tracks.flatMap(t=>t.clips).find(c=>c.kind==='midi');
async function idle(){await until(async()=>!await page.locator('#project-save').isDisabled(),'Idle');await sleep(100);}
async function edit(request){const p=await project();await invoke('edit_project',{revision:p.revision,request});await idle();await sleep(150);}
async function metrics(){const p=await project(),selected=await page.locator('.midi-clip.selected').first().getAttribute('data-clip-id'),clip=p.document.tracks.flatMap(t=>t.clips).find(c=>c.clipId===selected),scale=60/p.document.musicalTime.tempoMap[0].bpm/p.document.musicalTime.ticksPerQuarter;return page.evaluate(({origin,scale})=>{const c=document.querySelector('#piano-canvas'),r=c.getBoundingClientRect(),scroll=document.querySelector('#piano-scroll');return{x:r.x,y:r.y,width:r.width,height:r.height,start:(origin+Number(scroll.value))*scale,span:Number(scroll.step)*(r.width-56)*scale,scroll:document.querySelector('#piano-vscroll').scrollTop,row:parseFloat(getComputedStyle(document.documentElement).fontSize)*1.45};},{origin:Number(clip?.startTick??0),scale});}
async function point(tick,pitch){const m=await metrics(),p=await project(),bpm=p.document.musicalTime.tempoMap[0].bpm;return{x:m.x+56+(tick*60/bpm/960000-m.start)/m.span*(m.width-56),y:m.y+28+(127-pitch+.5)*m.row-m.scroll};}
async function gesture(a,b){(report.gestures??=[]).push({a,b,metrics:await metrics()});const revision=(await project()).revision;await page.mouse.move(a.x,a.y);await page.mouse.down();if(b)await page.mouse.move(b.x,b.y,{steps:100});assert.equal((await project()).revision,revision,'Preview never commits');await page.mouse.up();await until(async()=>(await project()).revision>revision,'Gesture commit');await idle();report.notes.push((await part()).notes);}
try{
 child=spawn(resolve('src-tauri/target/release/minidaw.exe'),[],{env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=19246'},stdio:'ignore',windowsHide:true});
 await until(async()=>{try{browser=await chromium.connectOverCDP('http://127.0.0.1:19246');return true;}catch{return false;}},'CDP');
 await until(async()=>{page=browser.contexts()[0].pages().find(p=>p.url().includes('tauri.localhost'));return !!page;},'Page');
 page.on('pageerror',e=>errors.push(e.message));await page.evaluate(()=>{window.__pointer=[];for(const type of ['pointerdown','pointerup'])document.addEventListener(type,e=>{if(e.target.id==='piano-canvas')window.__pointer.push({type,x:e.clientX,y:e.clientY,rect:e.target.getBoundingClientRect().toJSON(),scroll:document.querySelector('#piano-vscroll').scrollTop});},true);});await page.setViewportSize({width:1600,height:1000});await idle();
 prefs=await page.evaluate(()=>({panels:localStorage.getItem('minidaw.ui.panels.v1'),font:localStorage.getItem('minidaw.ui.fontScale')}));
 for(const[id,open]of[['arrangement',true],['media',false],['performance',false],['spectrum',false],['piano',false]]){const b=page.locator('#show-'+id);if((await b.getAttribute('aria-pressed')==='true')!==open)await b.click();}
 await page.evaluate(()=>{const f=window.fetch;window.fetch=async function(u,o){if(decodeURIComponent(String(u)).split('/').at(-1)==='edit_project')window.__lastEdit=JSON.parse(o.body).request;return f.call(this,u,o);};});
 await page.locator('#midi-track-add').click();await idle();assert.equal((await project()).document.tracks[0].kind,'midi');
 await page.locator('.midi-clip-add').click();await idle();assert.equal(await page.locator('#show-piano').getAttribute('aria-pressed'),'true');
 assert.equal((await part()).startTick,'0');assert.equal((await part()).lengthTick,'15360000');
 await page.locator('#grid-type').selectOption('16');await page.locator('#piano-canvas').focus();await page.keyboard.press('j');
 assert.equal(await page.locator('#snap-toggle').getAttribute('aria-pressed'),'true');
 report.initial=await page.evaluate(()=>{const v=document.querySelector('#piano-vscroll');return {top:v.scrollTop,height:v.clientHeight,scroll:v.scrollHeight,child:v.firstElementChild.getAttribute('style')};});
 await page.locator('#piano-canvas').focus();await page.keyboard.press('8');await sleep(60);assert.equal(await page.locator('#piano-draw').getAttribute('aria-pressed'),'true');await gesture(await point(240000,60));
 let c=await part(),n=c.notes[0];assert.equal(n.startTick,'240000');assert.equal(n.lengthTick,'240000');assert.equal(n.pitch,60);
 await page.locator('#piano-select').click();await gesture(await point(360000,60),await point(840000,64));
 n=(await part()).notes[0];assert.equal(n.startTick,'720000');assert.equal(n.pitch,64);
 let a=await point(960000,64);a.x-=1;await gesture(a,await point(1440000,64));
 n=(await part()).notes[0];assert.equal(n.lengthTick,'720000');
 a=await point(720000,64);a.x+=1;await gesture(a,await point(960000,64));
 n=(await part()).notes[0];assert.equal(n.startTick,'960000');assert.equal(n.lengthTick,'480000');
 const resized=structuredClone(n);await page.keyboard.press('Control+z');await idle();assert.notDeepEqual((await part()).notes[0],resized);await page.keyboard.press('Control+Shift+z');await idle();assert.deepEqual((await part()).notes[0],resized);
 await page.mouse.click(...Object.values(await point(1200000,64)));await page.keyboard.press('Delete');await idle();assert.equal((await part()).notes.length,0);await page.keyboard.press('Control+z');await idle();assert.equal((await part()).notes.length,1);
 // MIDI-only projects also reopen with a playable silent Rust timeline.
 let only=await project();const midiOnlyPath=join(folder,'midi-only.minidaw');await invoke('save_project',{path:midiOnlyPath,revision:only.revision});await idle();only=await project();await invoke('open_project',{path:midiOnlyPath,revision:only.revision,discard:true});await idle();assert.deepEqual((await project()).document.tracks,only.document.tracks);
 await page.locator('#play-pause').click();await sleep(160);assert.equal((await invoke('engine_snapshot')).transport.state,'playing');await page.locator('#stop').click();await idle();
 report.checks.push('Native pointer draw/move/pitch/left and right resize; 100 moves preview-only; global 1/16 Snap; Delete and Undo/Redo.');
 // Existing Audio import must target an Audio track, even when MIDI was first.
 await invoke('load_audio',{path:resolve('tests/fixtures/stereo-44100.wav')});await idle();
 let p=await project();assert.equal(p.document.tracks[0].kind,'midi');assert.equal(p.document.tracks[1].kind,'audio');assert.equal(p.document.tracks[0].clips[0].notes.length,1);
 await page.locator('#zoom-fit').click();await sleep(120);
 const clip=page.locator(`[data-clip-id="${c.clipId}"]`),box=await clip.boundingBox(),step=Number(await page.locator('#scroll').getAttribute('step'));
 const rev=(await project()).revision;await page.mouse.move(box.x+90,box.y+box.height*.7);await page.mouse.down();await page.mouse.move(box.x+90+.5/step,box.y+box.height*.7,{steps:30});assert.equal((await project()).revision,rev);await page.mouse.up();await idle();
 c=await part();assert.equal(c.startTick,'960000');assert.deepEqual(c.notes[0],resized);
 await page.locator('#piano-open').click();await idle();
 await page.mouse.move(...Object.values(await point(1920000,64)));assert.match(await page.locator('#piano-position').textContent(),/1\.3\.1\./);
 report.checks.push('MIDI-first project imports Audio into separate Audio track; Arrangement Clip Move preserves relative note ticks; Piano absolute musical readout matches.');
 // Fine free editing at tick-scale zoom, without passing through samples.
 await edit({command:'midi.clip.move',clipIds:[c.clipId],anchorClipId:c.clipId,targetTick:'0'});
 await edit({command:'midi.note.change',clipIds:[c.clipId],noteId:resized.noteId,targetTick:'123',lengthTick:'240',pitch:64});
 await page.locator('#piano-canvas').focus();await page.keyboard.press('j');await page.locator('#piano-fit').click();
 let m=await metrics();await page.mouse.move(m.x+56,m.y+150);await page.keyboard.down('Control');for(let i=0;i<4;i++){await page.mouse.wheel(0,-1000);await sleep(60);}await page.keyboard.up('Control');
 for(const tick of [157,130,173,123]){const n=(await part()).notes[0];await gesture(await point(Number(n.startTick)+120,64),await point(tick+120,64));assert.equal((await part()).notes[0].startTick,String(tick));}
 assert.equal((await part()).notes[0].lengthTick,'240');report.checks.push('Snap OFF tick-scale pointer moves 123→157→130→173→123 with zero drift.');
 // Restore a musical phrase and check persistence, loop and actual transport cursor.
 await edit({command:'midi.note.change',clipIds:[c.clipId],noteId:resized.noteId,targetTick:'240000',lengthTick:'480000',pitch:64});
 await page.locator('#piano-fit').click();await page.locator('#project-bpm').fill('137.3');await page.locator('#project-bpm').press('Tab');await idle();
 await page.locator('#midi-track-add').click();await idle();await page.locator('.midi-clip-add').last().click();await idle();
 await page.locator('#piano-draw').click();await gesture(await point(480000,67));
 const midis=(await project()).document.tracks.filter(t=>t.kind==='midi');assert.equal(midis.length,2);assert.equal(midis[1].clips[0].notes.length,1);assert.equal(midis[0].clips[0].notes[0].pitch,64);
 await page.locator(`[data-clip-id="${c.clipId}"]`).dblclick();await idle();
 report.checks.push('Two independent MIDI tracks/parts with separate notes, alongside existing Audio track.');
 await edit({command:'project.cycle',cycle:{enabled:true,startTick:'0',endTick:'960000'}});
 p=await project();const path=join(folder,'midi.minidaw');await invoke('save_project',{path,revision:p.revision});await idle();p=await project();const saved=p.document;
 await invoke('open_project',{path,revision:p.revision,discard:true});await idle();assert.deepEqual((await project()).document,saved);
 await page.locator(`[data-clip-id="${c.clipId}"]`).dblclick();await idle();
 await page.locator('#play-pause').click();let wraps=0,last=0;for(let i=0;i<45;i++){await sleep(40);const s=await invoke('engine_snapshot');if(s.position<last)wraps++;last=s.position;assert.equal(s.transport.state,'playing');}
 assert.ok(wraps>=3);assert.equal(await page.locator('#piano-playhead').isVisible(),true);report.wraps=wraps;
 await page.locator('#stop').click();await idle();report.checks.push('Save/open exact MIDI, Audio and tempo/cycle restoration; actual Rust playback and Piano cursor across multiple wraps.');
 await page.screenshot({path:'docs/validation/midi-piano.png'});
 // Count actual paints, not rAF calls, while closed. Exercise font/layout limits.
 await page.evaluate(()=>{window.__pianoPaints=0;const old=CanvasRenderingContext2D.prototype.fillRect;CanvasRenderingContext2D.prototype.fillRect=function(...a){if(this.canvas.id==='piano-canvas')window.__pianoPaints++;return old.apply(this,a);};});
 const width=(await page.locator('#panel-arrangement').boundingBox()).width;await page.locator('#panel-piano [data-panel-close]').click();await sleep(100);assert.ok((await page.locator('#panel-arrangement').boundingBox()).width>width+100);const paints=await page.evaluate(()=>window.__pianoPaints);await sleep(200);assert.equal(await page.evaluate(()=>window.__pianoPaints),paints);await page.locator('#show-piano').click();
 for(const scale of ['90','110','125']){await page.locator('#font-scale').selectOption(scale);await sleep(100);const c=await page.locator('#piano-canvas').boundingBox();assert.ok(c.width>100&&c.height>100);}
 // All 128 pitches can be reached; pointercancel must never commit a note.
 await page.evaluate(()=>document.querySelector('#piano-vscroll').scrollTop=1e8);await sleep(80);const low=await point(240000,0),bounds=await page.locator('#piano-canvas').boundingBox();assert.ok(low.y<bounds.y+bounds.height);
 await page.locator('#piano-draw').click();const cancelRevision=(await project()).revision;await page.mouse.move(low.x,low.y);await page.mouse.down();
 assert.equal(await page.locator('#piano-canvas').evaluate(c=>c.hasPointerCapture(1)),true);
 await page.locator('#piano-canvas').dispatchEvent('pointercancel',{pointerId:1});await page.mouse.up();assert.equal((await project()).revision,cancelRevision);
 await page.setViewportSize({width:1200,height:820});await sleep(100);assert.ok((await page.locator('#piano-canvas').boundingBox()).width>100);
 report.checks.push('Panel close returns space and zero Canvas paints; Font Scale 90/110/125; window resize; pitch 0; pointer capture/cancel.');
 assert.deepEqual(errors,[]);assert.equal(await page.locator('#error').isVisible(),false);report.passed=true;
}catch(e){report.pointer=await page?.evaluate(()=>window.__pointer);report.error=String(e.stack??e);report.pageErrors=errors;process.exitCode=1;if(page){report.uiError=await page.locator('#error-text').textContent().catch(()=>null);await page.screenshot({path:'docs/validation/midi-failure.png'}).catch(()=>{});}}
finally{
 if(page&&prefs)await page.evaluate(p=>{for(const[k,v]of[['minidaw.ui.panels.v1',p.panels],['minidaw.ui.fontScale',p.font]]){if(v===null)localStorage.removeItem(k);else localStorage.setItem(k,v);}},prefs).catch(()=>{});
 if(browser)await browser.close();if(child?.exitCode===null){const exited=once(child,'exit');child.kill();await exited;}
 if(savedRecent)await writeFile(recent,savedRecent);else await unlink(recent).catch(()=>{});
 await writeFile('docs/validation/midi-ui.json',JSON.stringify(report,null,2));console.log(JSON.stringify(report));
}
