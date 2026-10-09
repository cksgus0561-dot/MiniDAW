import {chromium} from 'playwright-core';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {readFile,writeFile,mkdir,unlink} from 'node:fs/promises';
import {randomUUID} from 'node:crypto';
import {resolve,join} from 'node:path';
import assert from 'node:assert/strict';
const folder=resolve('tests/local/piano-multipart',randomUUID());await mkdir(folder,{recursive:true});
const recent=join(process.env.APPDATA,'local.minidaw.desktop/recent-projects.json');let savedRecent;try{savedRecent=await readFile(recent);}catch{}
let child,browser,page,prefs;const report={checks:[]},errors=[],sleep=ms=>new Promise(r=>setTimeout(r,ms));
async function until(fn,label){const at=performance.now();while(performance.now()-at<20000){if(await fn())return;await sleep(40);}throw Error(label);}
const invoke=(name,args={})=>page.evaluate(({name,args})=>window.__TAURI_INTERNALS__.invoke(name,args),{name,args});
const project=()=>invoke('project_snapshot');
const part=async id=>(await project()).document.tracks.flatMap(t=>t.clips).find(c=>c.clipId===id);
async function idle(){await until(async()=>!await page.locator('#project-save').isDisabled(),'Idle');await sleep(140);}
async function edit(request){const p=await project();await invoke('edit_project',{revision:p.revision,request});await idle();}
async function camera(){await sleep(80);return page.evaluate(()=>{const c=document.querySelector('#piano-canvas'),r=c.getBoundingClientRect(),s=document.querySelector('#piano-scroll');return{x:r.x,y:r.y,width:r.width,height:r.height,start:Number(s.value),span:Number(s.step)*(r.width-56),scroll:document.querySelector('#piano-vscroll').scrollTop,row:parseFloat(getComputedStyle(document.documentElement).fontSize)*1.45,rects:window.__notes};});}
async function point(tick,pitch){const m=await camera();return{x:m.x+56+(tick-m.start)/m.span*(m.width-56),y:m.y+28+(127-pitch+.5)*m.row-m.scroll};}
async function clickPoint(tick,pitch){const p=await point(tick,pitch);await page.mouse.click(p.x,p.y);await sleep(80);}
async function gesture(a,b){const rev=(await project()).revision;await page.mouse.move(a.x,a.y);await page.mouse.down();if(b)await page.mouse.move(b.x,b.y,{steps:100});assert.equal((await project()).revision,rev,'No preview transaction');await page.mouse.up();await until(async()=>(await project()).revision>rev,'Pointer commit');await idle();}
async function lanePoint(tick,value,min=0,max=127){const b=await page.locator('#midi-lane-canvas').boundingBox(),p=await point(tick,64);return{x:p.x,y:b.y+8+(max-value)/(max-min)*(b.height-16)};}
function assertCamera(a,b){assert.equal(a.start,b.start,'Camera origin stable');assert.equal(a.span,b.span,'Zoom stable');assert.equal(a.scroll,b.scroll,'Pitch scroll stable');}
try{
 child=spawn(resolve('src-tauri/target/release/minidaw.exe'),[],{env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=19277'},stdio:'ignore',windowsHide:true});
 await until(async()=>{try{browser=await chromium.connectOverCDP('http://127.0.0.1:19277');return true;}catch{return false;}},'CDP');
 await until(async()=>{page=browser.contexts()[0].pages().find(p=>p.url().includes('tauri.localhost'));return !!page;},'Page');
 page.on('pageerror',e=>errors.push(e.message));await page.setViewportSize({width:1700,height:1050});await idle();
 prefs=await page.evaluate(()=>Object.fromEntries(Object.entries(localStorage).filter(([k])=>k.startsWith('minidaw.ui.'))));
 await page.evaluate(()=>{localStorage.setItem('minidaw.ui.language.v1','ko');localStorage.removeItem('minidaw.ui.shortcuts.v1');});await page.reload();await idle();
 let p=await project();await invoke('new_project',{revision:p.revision,discard:true});await idle();
 for(const[id,open]of[['arrangement',true],['media',false],['performance',false],['spectrum',false],['piano',false],['mixer',false]]){const b=page.locator('#show-'+id);if((await b.getAttribute('aria-pressed')==='true')!==open)await b.click();}
 await page.locator('#font-scale').selectOption('110');
 await page.evaluate(()=>{window.__notes=[];const f=CanvasRenderingContext2D.prototype.fillRect;CanvasRenderingContext2D.prototype.fillRect=function(x,y,w,h){if(this.canvas.id==='piano-canvas'){if(this.fillStyle==='#202123')window.__notes=[];if(['#80aaa0','#8c9fb6'].includes(this.fillStyle)&&y>=28)window.__notes.push({x,y,w,h,color:this.fillStyle,alpha:this.globalAlpha});}return f.call(this,x,y,w,h);};});
 const ids=[],tracks=[];
 for(const [start,pitch,noteStart] of [[0,64,2880000],[1920000,67,960000]]){
  await edit({command:'midi.track.add'});p=await project();const t=p.document.tracks.at(-1);tracks.push(t.trackId);
  await edit({command:'midi.track.instrument',trackIds:[t.trackId],instrument:'basicSynth'});
  await edit({command:'midi.clip.add',trackIds:[t.trackId],targetTick:String(start),lengthTick:'7680000'});
  p=await project();const c=p.document.tracks.at(-1).clips[0];ids.push(c.clipId);
  await edit({command:'midi.note.add',clipIds:[c.clipId],targetTick:String(noteStart),lengthTick:'480000',pitch,velocity:85});
 }
 const [a,b]=ids;
 // Trimmed Parts retain their source offsets; display must still use project ticks.
 await edit({command:'midi.clip.resize',clipIds:[b],trimSide:'left',targetTick:'2160000'});
 await page.locator(`[data-clip-id="${a}"]`).click();await page.keyboard.down('Control');await page.locator(`[data-clip-id="${b}"]`).click();await page.keyboard.up('Control');
 assert.equal(await page.locator('.midi-clip.selected').count(),2);await page.locator('#piano-open').click();await idle();
 assert.equal(await page.locator('#piano-parts button').count(),2);assert.equal(await page.locator(`[data-part-id="${a}"]`).getAttribute('aria-pressed'),'true');
 let m=await camera();assert.equal(m.rects.length,2);assert.equal(m.rects[0].color,'#8c9fb6');assert.ok(Math.abs(m.rects[0].alpha-.28)<.001);assert.equal(m.rects[1].alpha,1);assert.equal(m.rects[0].x,m.rects[1].x);
 assert.ok(Math.abs(m.rects[0].x-(56+(2880000-m.start)/m.span*(m.width-56)))<.001);report.initial=m;
 report.checks.push('Two Tracks, different Part Starts and contentOffsetTick: same project note tick maps to exactly equal Canvas x; distinct track colors and active/ghost opacity.');
 // Reference hit-testing is read-only. A destructive key cannot target a ghost.
 const originalA=await part(a),originalB=await part(b);await page.locator('#piano-select').click();await clickPoint(3000000,67);assert.match(await page.locator('#piano-canvas').getAttribute('title'),/MIDI 2.*참고 Part/);await page.keyboard.press('Delete');await idle();assert.deepEqual(await part(a),originalA);assert.deepEqual(await part(b),originalB);
 // Active selection switches editor, controller lane and live input route without changing camera.
 await page.locator(`[data-part-id="${b}"]`).click();assertCamera(await camera(),m);assert.equal(await page.locator(`[data-part-id="${b}"]`).getAttribute('aria-pressed'),'true');assert.equal(await page.locator('#computer-midi-track').textContent(),'MIDI 2');
 await page.locator('#piano-canvas').focus();await page.keyboard.press('Control+a');await sleep(80);assert.match(await page.locator('#piano-selected-count').textContent(),/1개/);
 await page.locator('#piano-velocity').fill('113');await page.locator('#piano-velocity-apply').click();await idle();assert.equal((await part(b)).notes[0].velocity,113);assert.deepEqual(await part(a),originalA);
 await page.locator('#note-time-format').selectOption('ticks');assert.equal(await page.locator('#note-start').inputValue(),'2880000');
 await page.locator('#note-start').fill('2880123');await page.locator('#note-start').press('Enter');await idle();let c=await part(b);assert.equal(Number(c.startTick)+Number(c.notes[0].startTick)-Number(c.contentOffsetTick),2880123);
 await page.locator('#grid-type').selectOption('16');if(await page.locator('#snap-toggle').getAttribute('aria-pressed')!=='true')await page.locator('#snap-toggle').click();
 await page.locator('#piano-canvas').focus();await page.keyboard.press('q');await idle();c=await part(b);assert.equal(Number(c.startTick)+Number(c.notes[0].startTick)-Number(c.contentOffsetTick),2880000);
 await page.keyboard.press('ArrowUp');await idle();assert.equal((await part(b)).notes[0].pitch,68);await page.keyboard.press('Control+z');await idle();assert.equal((await part(b)).notes[0].pitch,67);await page.keyboard.press('Control+Shift+z');await idle();assert.equal((await part(b)).notes[0].pitch,68);
 assert.deepEqual(await part(a),originalA);report.checks.push('Reference note click/Delete leaves both Parts unchanged; active Part Ctrl+A, Velocity, absolute numeric tick Start, Q, Transpose and Undo/Redo affect only the original owning Track/Part.');
 // Mouse Move/Resize and draw in a nonzero-start active Part, with 100 preview moves.
 await gesture(await point(3120000,68),await point(3360000,68));c=await part(b);assert.equal(Number(c.startTick)+Number(c.notes[0].startTick)-Number(c.contentOffsetTick),3120000);
 let edge=await point(3600000,68);edge.x-=1;await gesture(edge,await point(3840000,68));assert.equal((await part(b)).notes[0].lengthTick,'720000');
 await page.locator('#piano-draw').click();await gesture(await point(4080000,65),await point(4560000,65));assert.equal((await part(b)).notes.length,2);
 await page.locator('#snap-toggle').click();await page.locator('#piano-select').click();await gesture(await point(4200000,65),await point(4250123,65));c=await part(b);const free=c.notes.find(n=>n.pitch===65);assert.ok(Number.isInteger(Number(free.startTick)));assert.notEqual(Number(free.startTick)%240000,0);
 await page.locator('#note-time-format').selectOption('ticks');await page.locator('#note-start').fill('4080123');await page.locator('#note-start').press('Enter');await idle();c=await part(b);assert.equal(Number(c.startTick)+Number(c.notes.find(n=>n.pitch===65).startTick)-Number(c.contentOffsetTick),4080123);
 await page.locator('#piano-delete').click();await idle();assert.equal((await part(b)).notes.length,1);await page.locator('#piano-canvas').focus();await page.keyboard.press('Control+z');await idle();assert.equal((await part(b)).notes.length,2);
 assert.deepEqual(await part(a),originalA);report.checks.push('Active Part mouse Move/Resize/add/delete and Undo; Snap ON 1/16 exact ticks, Snap OFF integer sub-grid ticks, numeric tick precision; no commits during pointer preview.');
 // CC, bend and sustain lane use the same absolute x but Part-relative command data.
 await page.locator('#snap-toggle').click();await page.locator('#piano-draw').click();
 for(const[type,tick,value,min,max]of[['cc',4800000,93,0,127],['pitchBend',5040000,-4096,-8192,8191],['sustain',5280000,127,0,127],['sustain',5520000,0,0,127]]){
  await page.locator('#midi-lane-type').selectOption(type);await sleep(60);await gesture(await lanePoint(tick,value,min,max));
 }
 c=await part(b);assert.deepEqual(c.controls.map(e=>Number(c.startTick)+Number(e.tick)-Number(c.contentOffsetTick)),[4800000,5040000,5280000,5520000]);assert.deepEqual(c.controls.map(e=>e.data.kind),['cc','pitchBend','cc','cc']);assert.deepEqual(c.controls.slice(2).map(e=>e.data.value),[127,0]);assert.deepEqual(await part(a),originalA);
 await page.locator(`[data-part-id="${a}"]`).click();assert.equal(await page.locator('#midi-lane-delete').isDisabled(),true);assertCamera(await camera(),m);report.checks.push('CC/Pitch Bend/Sustain draw is correctly offset into active Part only; switching Part clears stale controller selection.');
 // Multi-Part camera stays in project coordinates through Move/Undo/Redo.
 await page.locator('#piano-zoom-in').click();await page.locator('#piano-scroll').evaluate(e=>{e.value=String(Number(e.max)*.27);e.dispatchEvent(new Event('input',{bubbles:true}));});m=await camera();
 for(const id of [b,a,b]){await page.locator(`[data-part-id="${id}"]`).click();assertCamera(await camera(),m);}
 const sourceNotes=(await part(b)).notes;
 for(const target of ['3120000','960000','4080000']){
  const old=Number((await part(b)).startTick);await edit({command:'midi.clip.move',clipIds:[b],anchorClipId:b,targetTick:target});assertCamera(await camera(),m);assert.deepEqual((await part(b)).notes,sourceNotes);
  await page.locator('#piano-canvas').focus();await page.keyboard.press('Control+z');await idle();assert.equal(Number((await part(b)).startTick),old);assertCamera(await camera(),m);await page.keyboard.press('Control+Shift+z');await idle();assert.equal((await part(b)).startTick,target);assertCamera(await camera(),m);
 }
 // Real Arrangement multi-selection drag must also keep the camera and membership.
 const box=await page.locator(`[data-clip-id="${a}"]`).boundingBox(),step=Number(await page.locator('#scroll').getAttribute('step')),x=box.x+Math.min(35,box.width/2),y=box.y+box.height*.6;
 const rev=(await project()).revision;await page.mouse.move(x,y);await page.mouse.down();await page.mouse.move(x+.5/step,y,{steps:35});assert.equal((await project()).revision,rev);await page.mouse.up();await idle();assert.equal((await part(a)).startTick,'960000');assert.equal((await part(b)).startTick,'5040000');assertCamera(await camera(),m);assert.equal(await page.locator('#piano-parts button').count(),2);
 await page.locator('#piano-canvas').focus();await page.keyboard.press('Control+z');await idle();assertCamera(await camera(),m);await page.keyboard.press('Control+Shift+z');await idle();assertCamera(await camera(),m);
 report.checks.push('Arbitrary zoom/scroll survives active changes, individual Part Moves in both directions, real Arrangement multi-Part drag and Undo/Redo. Musical positions change; camera does not.');
 // Live transport remains authoritative for both independent Synth Tracks.
 await page.locator('#piano-fit').click();await idle();m=await camera();const targetTick=Number((await part(a)).startTick)+Number((await part(a)).notes[0].startTick);
 await invoke('transport_command',{action:'seek',seconds:targetTick/1920000});await sleep(160);const px=await page.locator('#piano-playhead').evaluate(e=>new DOMMatrix(getComputedStyle(e).transform).m41);assert.ok(Math.abs(px-(56+(targetTick-m.start)/m.span*(m.width-56)))<1);
 await invoke('transport_command',{action:'play'});await sleep(130);let audio=await invoke('engine_snapshot');assert.equal(audio.transport.state,'playing');assert.ok(Math.max(...audio.master.peakDb)>-60);await invoke('transport_command',{action:'stop'});
 c=await part(b);await invoke('transport_command',{action:'seek',seconds:(Number(c.startTick)+Number(c.notes[0].startTick)-Number(c.contentOffsetTick))/1920000});await invoke('transport_command',{action:'play'});await sleep(130);audio=await invoke('engine_snapshot');assert.ok(Math.max(...audio.master.peakDb)>-60);await invoke('transport_command',{action:'stop'});
 report.playback={backend:audio.output.backend,peakDb:audio.master.peakDb};report.checks.push('Both independently routed Synth Tracks produce Master output; Rust transport playhead aligns with project tick within one pixel.');
 const path=join(folder,'multipart.minidaw');p=await project();await invoke('save_project',{path,revision:p.revision});p=await project();const saved=p.document;await invoke('open_project',{path,revision:p.revision,discard:true});await idle();assert.deepEqual((await project()).document,saved);report.checks.push('Project save/reopen exactly preserves separate Track instruments, Part offsets, notes and controller data.');
 // Re-select from the reopened project and inspect the real release layout.
 await page.locator(`[data-clip-id="${a}"]`).click();await page.keyboard.down('Control');await page.locator(`[data-clip-id="${b}"]`).click();await page.keyboard.up('Control');await page.locator('#piano-open').click();await page.locator('#piano-fit').click();await idle();
 await page.screenshot({path:'docs/validation/piano-multipart-ui.png'});
 for(const scale of ['90','110','125']){await page.locator('#font-scale').selectOption(scale);await sleep(100);const box=await page.locator('#piano-canvas').boundingBox();assert.ok(box.height>100&&box.width>100);assert.equal(await page.locator('#panel-piano').evaluate(e=>e.scrollHeight>e.clientHeight),false);}
 await page.setViewportSize({width:1400,height:900});await sleep(150);assert.ok((await page.locator('#piano-canvas').boundingBox()).height>100);assert.equal(await page.locator('#panel-piano').evaluate(e=>e.scrollHeight>e.clientHeight),false);
 await page.locator('#show-piano').click();await page.locator('#show-piano').click();await sleep(100);assert.equal(await page.locator('#piano-parts button').count(),2);report.checks.push('Font Scale 90/110/125, 1400×900 window, panel close/reopen preserve usable note/controller layout without whole-panel vertical scrolling.');
 assert.deepEqual(errors,[]);assert.equal(await page.locator('#error').isVisible(),false);report.passed=true;
}catch(e){report.error=String(e.stack??e);report.pageErrors=errors;process.exitCode=1;if(page){report.uiError=await page.locator('#error-text').textContent().catch(()=>null);await page.screenshot({path:'docs/validation/piano-multipart-failure.png'}).catch(()=>{});}}
finally{
 if(page&&prefs)await page.evaluate(p=>{for(const k of Object.keys(localStorage).filter(k=>k.startsWith('minidaw.ui.')))localStorage.removeItem(k);for(const[k,v]of Object.entries(p))localStorage.setItem(k,v);},prefs).catch(()=>{});
 if(browser)await browser.close();if(child?.exitCode===null){const exited=once(child,'exit');child.kill();await exited;}
 if(savedRecent)await writeFile(recent,savedRecent);else await unlink(recent).catch(()=>{});
 await writeFile('docs/validation/piano-multipart-ui.json',JSON.stringify(report,null,2));console.log(JSON.stringify(report));
}
