import {chromium} from 'playwright-core';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {readFile,writeFile,mkdir,unlink} from 'node:fs/promises';
import {randomUUID} from 'node:crypto';
import {resolve,join} from 'node:path';
import assert from 'node:assert/strict';
const folder=resolve('tests/local/midi2',randomUUID());await mkdir(folder,{recursive:true});
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
async function gesture(a,b){const revision=(await project()).revision;await page.mouse.move(a.x,a.y);await page.mouse.down();if(b)await page.mouse.move(b.x,b.y,{steps:100});assert.equal((await project()).revision,revision,'Preview never commits');await page.mouse.up();await until(async()=>(await project()).revision>revision,'Gesture commit');await idle();report.notes.push((await part()).notes);}
try{
 child=spawn(resolve('src-tauri/target/release/minidaw.exe'),[],{env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=19246'},stdio:'ignore',windowsHide:true});
 await until(async()=>{try{browser=await chromium.connectOverCDP('http://127.0.0.1:19246');return true;}catch{return false;}},'CDP');
 await until(async()=>{page=browser.contexts()[0].pages().find(p=>p.url().includes('tauri.localhost'));return !!page;},'Page');
 page.on('pageerror',e=>errors.push(e.message));await page.setViewportSize({width:1600,height:1000});await idle();
 prefs=await page.evaluate(()=>({panels:localStorage.getItem('minidaw.ui.panels.v1'),font:localStorage.getItem('minidaw.ui.fontScale')}));
 for(const[id,open]of[['arrangement',true],['media',false],['performance',false],['spectrum',false],['piano',false]]){const b=page.locator('#show-'+id);if((await b.getAttribute('aria-pressed')==='true')!==open)await b.click();}
 await page.evaluate(()=>{const f=window.fetch;window.fetch=async function(u,o){if(decodeURIComponent(String(u)).split('/').at(-1)==='edit_project')window.__lastEdit=JSON.parse(o.body).request;return f.call(this,u,o);};});
 await edit({command:'midi.track.add'});let p=await project();
 await edit({command:'midi.clip.add',trackIds:[p.document.tracks[0].trackId],targetTick:'0',lengthTick:'3840000'});
 let c=await part();
 for(const [start,pitch] of [[251123,64],[731123,67]])await edit({command:'midi.note.add',clipIds:[c.clipId],targetTick:String(start),lengthTick:'360000',pitch});
 await page.locator(`[data-clip-id="${c.clipId}"]`).click();await page.locator('#piano-open').click();await idle();
 await page.locator('#grid-type').selectOption('16');
 let a=await point(400000,64),b=await point(900000,67);
 await page.mouse.click(a.x,a.y);await page.keyboard.down('Shift');await page.mouse.click(b.x,b.y);await page.keyboard.up('Shift');
 assert.match(await page.locator('#piano-selected-count').textContent(),/2개/);
 await page.locator('#piano-velocity').fill('113');await page.locator('#piano-velocity-apply').click();await idle();
 assert.deepEqual((await part()).notes.map(n=>n.velocity),[113,113]);
 await page.locator('#piano-canvas').focus();await page.keyboard.press('q');await idle();assert.deepEqual((await part()).notes.map(n=>n.startTick),['240000','720000']);
 assert.deepEqual((await part()).notes.map(n=>n.lengthTick),['360000','360000']);
 await page.keyboard.press('ArrowUp');await idle();assert.deepEqual((await part()).notes.map(n=>n.pitch),[65,68]);
 await page.keyboard.press('ArrowDown');await idle();assert.deepEqual((await part()).notes.map(n=>n.pitch),[64,67]);
 await page.keyboard.press('Control+z');await idle();assert.deepEqual((await part()).notes.map(n=>n.pitch),[65,68]);await page.keyboard.press('Control+Shift+z');await idle();
 report.checks.push('Actual Shift multi-select, Velocity 113, Q quantize to 1/16 with lengths preserved, semitone Up/Down and Undo/Redo.');
 // Velocity bars edit selected notes in one pointerup transaction.
 async function lanePoint(tick,value,min=0,max=127){const box=await page.locator('#midi-lane-canvas').boundingBox(),pt=await point(tick,64);return{x:pt.x,y:box.y+8+(max-value)/(max-min)*(box.height-16)};}
 let rev=(await project()).revision;await page.mouse.move(...Object.values(await lanePoint(240000,113,1)));await page.mouse.down();await page.mouse.move(...Object.values(await lanePoint(240000,72,1)),{steps:100});assert.equal((await project()).revision,rev);await page.mouse.up();await idle();
 assert.ok((await part()).notes.every(n=>Math.abs(n.velocity-72)<=1));await page.locator('#midi-lane-value').fill('72');await page.locator('#midi-lane-apply').click();await idle();assert.deepEqual((await part()).notes.map(n=>n.velocity),[72,72]);
 // Draw and edit all three controller kinds with shared Grid/Snap.
 if(await page.locator('#snap-toggle').getAttribute('aria-pressed')!=='true')await page.locator('#snap-toggle').click();
 await page.locator('#piano-draw').click();
 for(const[type,tick,value,min,max]of[['cc',480000,93,0,127],['pitchBend',960000,-4096,-8192,8191],['sustain',1200000,127,0,127],['sustain',1920000,0,0,127]]){
  await page.locator('#midi-lane-type').selectOption(type);await sleep(80);const pt=await lanePoint(tick,value,min,max);rev=(await project()).revision;await page.mouse.move(pt.x,pt.y);await page.mouse.down();assert.equal((await project()).revision,rev);await page.mouse.up();await idle();
 }
 c=await part();assert.equal(c.controls.length,4);assert.deepEqual(c.controls.map(e=>e.tick),['480000','960000','1200000','1920000']);assert.deepEqual(c.controls.map(e=>e.data.kind),['cc','pitchBend','cc','cc']);
 assert.deepEqual(c.controls.slice(2).map(e=>e.data.value),[127,0]);
 await page.locator('#midi-lane-type').selectOption('cc');await page.locator('#piano-select').click();await sleep(80);
 let pt=await lanePoint(480000,c.controls[0].data.value);await page.mouse.click(pt.x,pt.y);await page.locator('#midi-lane-value').fill('88');await page.locator('#midi-lane-apply').click();await idle();assert.equal((await part()).controls[0].data.value,88);
 pt=await lanePoint(480000,88);const dest=await lanePoint(720000,40);rev=(await project()).revision;await page.mouse.move(pt.x,pt.y);await page.mouse.down();await page.mouse.move(dest.x,dest.y,{steps:100});assert.equal((await project()).revision,rev);await page.mouse.up();await idle();assert.equal((await part()).controls[0].tick,'720000');assert.ok(Math.abs((await part()).controls[0].data.value-40)<=1);
 await page.locator('#midi-lane-delete').click();await idle();assert.equal((await part()).controls.length,3);await page.locator('#midi-lane-canvas').focus();await page.keyboard.press('Control+z');await idle();assert.equal((await part()).controls.length,4);
 report.checks.push('Velocity bar multi-edit and CC/Pitch Bend/Sustain Canvas draw, numeric edit, move/value drag, delete and Undo; zero commits during 100 pointer moves.');
 // Clipboard-independent persistence and file I/O operate through real IPC.
 p=await project();let path=join(folder,'midi2.minidaw');await invoke('save_project',{path,revision:p.revision});p=await project();const saved=p.document;await invoke('open_project',{path,revision:p.revision,discard:true});await idle();assert.deepEqual((await project()).document,saved);
 const midiPath=join(folder,'midi2.mid');p=await project();await invoke('export_midi',{path:midiPath,revision:p.revision});assert.equal((await readFile(midiPath)).subarray(0,4).toString(),'MThd');
 await edit({command:'midi.import',path:midiPath,importTempo:true});p=await project();assert.equal(p.document.tracks.length,2);
 const core=t=>t.clips.map(c=>({start:c.startTick,length:c.lengthTick,notes:c.notes.map(({noteId,...n})=>n),controls:c.controls.map(({eventId,...e})=>e)}));
 assert.deepEqual(core(p.document.tracks[0]),core(p.document.tracks[1]));await edit({command:'edit.undo'});assert.equal((await project()).document.tracks.length,1);await edit({command:'edit.redo'});assert.equal((await project()).document.tracks.length,2);
 report.checks.push('Release IPC project save/open exact, SMF Type1 export/import preserves all core ticks/velocity/channel/CC/bend/pedal, import Undo/Redo.');
 await page.locator(`[data-clip-id="${c.clipId}"]`).click();await page.locator('#piano-open').click();await idle();
 report.midiInputPorts=await invoke('midi_input_ports');await page.locator('.midi-input summary').click();await page.locator('#midi-input-refresh').click();await sleep(100);assert.equal(await page.locator('#midi-input-device-state').textContent(),'연결 안 됨');
 await page.locator('#midi-input-disconnect').click();await page.locator('.midi-input summary').click();
 await edit({command:'project.cycle',cycle:{enabled:true,startTick:'0',endTick:'960000'}});await page.locator('#play-pause').click();let wraps=0,last=0;for(let i=0;i<35;i++){await sleep(50);let s=await invoke('engine_snapshot');if(s.position<last)wraps++;last=s.position;assert.equal(s.transport.state,'playing');}assert.ok(wraps>=2);await page.locator('#stop').click();await idle();report.wraps=wraps;
 report.checks.push('Rust transport playback/cycle and stop; keyboard device enumeration/disconnect UI. No physical keyboard attached.');
 // Existing note editor keeps move/resize and Snap OFF tick precision.
 await page.locator('#piano-select').click();await page.locator('#piano-fit').click();await sleep(100);c=await part();
 const before=c.notes[0];await gesture(await point(420000,before.pitch),await point(660000,before.pitch));assert.equal((await part()).notes[0].startTick,'480000');
 let edge=await point(840000,before.pitch);edge.x-=1;await gesture(edge,await point(1200000,before.pitch));assert.equal((await part()).notes[0].lengthTick,'720000');
 await page.locator('#piano-canvas').focus();await page.keyboard.press('Control+a');assert.match(await page.locator('#piano-selected-count').textContent(),/2개/);
 await page.locator('#midi-lane-type').selectOption('sustain');await sleep(80);await page.screenshot({path:'docs/validation/midi2-piano.png'});
 for(const scale of ['90','110','125']){await page.locator('#font-scale').selectOption(scale);await sleep(100);let box=await page.locator('#piano-canvas').boundingBox();assert.ok(box.height>=100&&box.width>100);assert.ok((await page.locator('#midi-lane-canvas').boundingBox()).height>70);}
 await page.evaluate(()=>{window.__lanePaints=0;const old=CanvasRenderingContext2D.prototype.fillRect;CanvasRenderingContext2D.prototype.fillRect=function(...a){if(['midi-lane-canvas','piano-canvas'].includes(this.canvas.id))window.__lanePaints++;return old.apply(this,a);};});
 await page.locator('#panel-piano [data-panel-close]').click();await sleep(80);const paints=await page.evaluate(()=>window.__lanePaints);await sleep(300);assert.equal(await page.evaluate(()=>window.__lanePaints),paints);
 report.checks.push('Existing note Move/Resize; Ctrl+A selection; Font Scale 90/110/125; closed Piano and controller canvases stop painting.');
 assert.deepEqual(errors,[]);assert.equal(await page.locator('#error').isVisible(),false);report.passed=true;
}catch(e){report.error=String(e.stack??e);report.pageErrors=errors;process.exitCode=1;if(page){report.uiError=await page.locator('#error-text').textContent().catch(()=>null);await page.screenshot({path:'docs/validation/midi2-failure.png'}).catch(()=>{});}}
finally{
 if(page&&prefs)await page.evaluate(p=>{for(const[k,v]of[['minidaw.ui.panels.v1',p.panels],['minidaw.ui.fontScale',p.font]]){if(v===null)localStorage.removeItem(k);else localStorage.setItem(k,v);}},prefs).catch(()=>{});
 if(browser)await browser.close();if(child?.exitCode===null){const exited=once(child,'exit');child.kill();await exited;}
 if(savedRecent)await writeFile(recent,savedRecent);else await unlink(recent).catch(()=>{});
 await writeFile('docs/validation/midi2-ui.json',JSON.stringify(report,null,2));console.log(JSON.stringify(report));
}
