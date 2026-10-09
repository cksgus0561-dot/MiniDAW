import {chromium} from 'playwright-core';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {readFile,writeFile,mkdir,unlink} from 'node:fs/promises';
import {randomUUID} from 'node:crypto';
import {resolve,join} from 'node:path';
import assert from 'node:assert/strict';
const folder=resolve('tests/local/piano-viewport',randomUUID());await mkdir(folder,{recursive:true});
const recent=join(process.env.APPDATA,'local.minidaw.desktop/recent-projects.json');let savedRecent;try{savedRecent=await readFile(recent);}catch{}
let child,browser,page,prefs;const report={checks:[],notes:[]},errors=[],sleep=ms=>new Promise(r=>setTimeout(r,ms));
async function until(fn,label){const at=performance.now();while(performance.now()-at<20000){if(await fn())return;await sleep(40);}throw Error(label);}
const invoke=(name,args={})=>page.evaluate(({name,args})=>window.__TAURI_INTERNALS__.invoke(name,args),{name,args});
const project=()=>invoke('project_snapshot');
const part=async()=>(await project()).document.tracks.flatMap(t=>t.clips).find(c=>c.kind==='midi');
async function idle(){await until(async()=>!await page.locator('#project-save').isDisabled(),'Idle');await sleep(100);}
async function edit(request){const p=await project();await invoke('edit_project',{revision:p.revision,request});await idle();await sleep(150);}
async function metrics(){return page.evaluate(()=>{const c=document.querySelector('#piano-canvas'),r=c.getBoundingClientRect(),scroll=document.querySelector('#piano-scroll');return{x:r.x,y:r.y,width:r.width,height:r.height,start:Number(scroll.value),span:Number(scroll.step)*(r.width-56),scroll:document.querySelector('#piano-vscroll').scrollTop,row:parseFloat(getComputedStyle(document.documentElement).fontSize)*1.45};});}
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
 await page.evaluate(()=>{window.__noteRects=[];const fill=CanvasRenderingContext2D.prototype.fillRect;CanvasRenderingContext2D.prototype.fillRect=function(x,y,w,h){if(this.canvas.id==='piano-canvas'){if(this.fillStyle==='#202123')window.__noteRects=[];if(this.fillStyle==='#80aaa0'&&y>=28)window.__noteRects.push({x,y,w,h});}return fill.call(this,x,y,w,h);};});
 await page.locator('#font-scale').selectOption('110');await edit({command:'midi.track.add'});let p=await project();await edit({command:'midi.clip.add',trackIds:[p.document.tracks[0].trackId],targetTick:'0',lengthTick:'15360000'});let c=await part(),id=c.clipId;
 await edit({command:'midi.note.add',clipIds:[id],targetTick:'5000000',lengthTick:'480000',pitch:64});
 await page.locator(`[data-clip-id="${id}"]`).click();await page.locator('#piano-open').click();await idle();await page.locator('#piano-zoom-in').click();await page.locator('#piano-zoom-in').click();await sleep(100);
 await page.locator('#piano-scroll').evaluate(e=>{e.value=String(Number(e.max)*.35);e.dispatchEvent(new Event('input',{bubbles:true}));});await sleep(100);
 async function camera(){await sleep(80);return page.evaluate(()=>({start:Number(document.querySelector('#piano-scroll').value),step:Number(document.querySelector('#piano-scroll').step),rects:window.__noteRects}));}
 const before=await camera(),notes=structuredClone((await part()).notes);assert.equal(before.rects.length,1);
 function assertCamera(a){assert.equal(a.start,before.start);assert.equal(a.step,before.step);assert.deepEqual(a.rects.map(({x,w})=>({x,w})),before.rects.map(({x,w})=>({x,w})),'Visible note horizontal position and width stay fixed');}
 async function unchanged(){const a=await camera();report.notes.push({clipStart:(await part()).startTick,camera:a});assertCamera(a);assert.deepEqual((await part()).notes,notes);}
 await page.locator('#grid-type').selectOption('beat');if(await page.locator('#snap-toggle').getAttribute('aria-pressed')!=='true')await page.locator('#snap-toggle').click();
 // Real pointer drags in both directions, including a destination beyond the old viewport.
 async function movePart(target){const old=Number((await part()).startTick),b=await page.locator(`[data-clip-id="${id}"]`).boundingBox(),step=Number(await page.locator('#scroll').getAttribute('step')),x=b.x+Math.min(40,b.width/2),y=b.y+b.height*.6,revision=(await project()).revision;
 await page.mouse.move(x,y);await page.mouse.down();await page.mouse.move(x+(Number(target)-old)/1920000/step,y,{steps:40});assert.equal((await project()).revision,revision);assertCamera(await camera());await page.mouse.up();await idle();assert.equal((await part()).startTick,target);await unchanged();}
 await movePart('3840000');
 for(const target of ['11520000','0','7680000']){await movePart(target);await page.locator('#piano-canvas').focus();await page.keyboard.press('Control+z');await idle();await unchanged();await page.keyboard.press('Control+Shift+z');await idle();assert.equal((await part()).startTick,target);await unchanged();}
 // Deselect and reselect the same Part without an implicit fit.
 await page.locator('#clip-layer').click({position:{x:15,y:250}});await sleep(100);await page.locator(`[data-clip-id="${id}"]`).click();await idle();await unchanged();
 // Switching to another Part must not discard this Part's saved camera.
 p=await project();await edit({command:'midi.clip.add',trackIds:[p.document.tracks[0].trackId],targetTick:'0',lengthTick:'1920000'});const other=(await project()).document.tracks[0].clips.find(c=>c.clipId!==id);await page.locator(`[data-clip-id="${other.clipId}"]`).click();await idle();await page.locator(`[data-clip-id="${id}"]`).click();await idle();await unchanged();
 report.checks.push('Arbitrary horizontal scroll/zoom preserved through native Arrangement Move, several distant Moves, Undo/Redo, deselect/reselect and switching Parts; actual Canvas note bounds unchanged.');
 // Note fields and hover must advance in project ticks while the note stays on screen.
 c=await part();let canvas=await page.locator('#piano-canvas').boundingBox(),r=(await camera()).rects[0];await page.mouse.click(canvas.x+r.x+r.w/2,canvas.y+r.y+r.h/2);await sleep(100);await page.locator('#note-time-format').selectOption('ticks');assert.equal(await page.locator('#note-start').inputValue(),String(Number(c.startTick)+5000000));
 await page.locator('#note-start').fill(String(Number(c.startTick)+5000123));await page.locator('#note-start').press('Enter');await idle();assert.equal((await part()).notes[0].startTick,'5000123');
 await page.locator('#piano-canvas').focus();await page.keyboard.press('Control+z');await idle();assert.equal((await part()).notes[0].startTick,'5000000');
 // Selected color can change, but the actual geometry must be identical.
 const after=await camera();assertCamera(after);
 await invoke('transport_command',{action:'seek',seconds:(Number((await part()).startTick)+5000000)/1920000});await until(async()=>await page.locator('#piano-playhead').isVisible(),'Moved Part playhead');await sleep(120);const playheadX=await page.locator('#piano-playhead').evaluate(e=>new DOMMatrix(getComputedStyle(e).transform).m41);assert.ok(Math.abs(playheadX-before.rects[0].x)<1,'Project transport tick maps to the same visible note position');report.checks.push('Transport seek on moved Part aligns the playhead with the visible note within one pixel.');
 await page.screenshot({path:'docs/validation/piano-viewport-ui.png'});
 await page.locator('#piano-fit').click();await sleep(100);assert.equal(Number(await page.locator('#piano-scroll').inputValue()),0);report.checks.push('Absolute Note Start updates with moved Clip; exact numeric editing and Undo retain note geometry; explicit Fit works.');
 assert.deepEqual(errors,[]);assert.equal(await page.locator('#error').isVisible(),false);report.passed=true;
}catch(e){report.error=String(e.stack??e);report.pageErrors=errors;process.exitCode=1;if(page){report.uiError=await page.locator('#error-text').textContent().catch(()=>null);await page.screenshot({path:'docs/validation/piano-viewport-failure.png'}).catch(()=>{});}}
finally{
 if(page&&prefs)await page.evaluate(p=>{for(const[k,v]of[['minidaw.ui.panels.v1',p.panels],['minidaw.ui.fontScale',p.font]]){if(v===null)localStorage.removeItem(k);else localStorage.setItem(k,v);}},prefs).catch(()=>{});
 if(browser)await browser.close();if(child?.exitCode===null){const exited=once(child,'exit');child.kill();await exited;}
 if(savedRecent)await writeFile(recent,savedRecent);else await unlink(recent).catch(()=>{});
 await writeFile('docs/validation/piano-viewport-ui.json',JSON.stringify(report,null,2));console.log(JSON.stringify(report));
}

