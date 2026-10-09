import {chromium} from 'playwright-core';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {readFile,writeFile,mkdir,unlink} from 'node:fs/promises';
import {randomUUID} from 'node:crypto';
import {resolve,join} from 'node:path';
import assert from 'node:assert/strict';
const folder=resolve('tests/local/note-time',randomUUID());await mkdir(folder,{recursive:true});
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
 await page.locator('#font-scale').selectOption('110');
 await edit({command:'midi.track.add'});let p=await project();
 await edit({command:'midi.clip.add',trackIds:[p.document.tracks[0].trackId],targetTick:'0',lengthTick:'3840000'});
 let c=await part();for(const[start,length,pitch]of[[240000,480000,64],[960000,720000,67]])await edit({command:'midi.note.add',clipIds:[c.clipId],targetTick:String(start),lengthTick:String(length),pitch});
 await page.locator(`[data-clip-id="${c.clipId}"]`).click();await page.locator('#piano-open').click();await idle();
 const n=()=>part().then(c=>c.notes[0]);
 async function set(field,value,keys='Enter'){await page.locator('#note-'+field).fill(value);await page.locator('#note-'+field).press(keys);await idle();}
 let pt=await point(480000,64);await page.mouse.click(pt.x,pt.y);await sleep(100);
 assert.equal(await page.locator('#note-start').inputValue(),'1.1.2.000000');assert.equal(await page.locator('#note-end').inputValue(),'1.1.4.000000');assert.equal(await page.locator('#note-length').inputValue(),'0.0.2.000000');
 // Explicit numeric edits ignore Snap, down to one tick.
 if(await page.locator('#snap-toggle').getAttribute('aria-pressed')!=='true')await page.locator('#snap-toggle').click();
 await set('start','1.2.3.000123');assert.equal((await n()).startTick,'1440123');assert.equal((await n()).lengthTick,'480000');
 await set('end','1.3.1.000001');assert.equal((await n()).lengthTick,'479878');
 await set('length','0.1.0.000001');assert.equal((await n()).lengthTick,'960001');assert.equal(await page.locator('#note-end').inputValue(),'1.3.3.000124');
 await page.locator('#note-time-format').selectOption('ticks');assert.equal(await page.locator('#note-start').inputValue(),'1440123');await set('start','1440124');assert.equal((await n()).startTick,'1440124');
 await page.locator('#piano-canvas').focus();await page.keyboard.press('Control+z');await idle();assert.equal((await n()).startTick,'1440123');await page.keyboard.press('Control+Shift+z');await idle();assert.equal((await n()).startTick,'1440124');
 let revision=(await project()).revision;await page.locator('#note-start').fill('1440555');await page.locator('#note-start').press('Escape');await page.locator('#note-end').focus();await idle();assert.equal((await project()).revision,revision);assert.equal(await page.locator('#note-start').inputValue(),'1440124');
 await set('length','0');assert.equal((await project()).revision,revision);assert.equal(await page.locator('#note-length').getAttribute('aria-invalid'),'true');await page.locator('#note-length').press('Escape');
 await page.locator('#note-time-format').selectOption('musical');await set('start','1.1.5.000000');assert.equal((await project()).revision,revision);await page.locator('#note-start').press('Escape');
 report.checks.push('Start/End/Length musical entry exact to 1 tick despite Snap ON; raw ticks toggle; invalid input rejected; Escape cancels; single Undo/Redo.');
 c=await part();await edit({command:'midi.note.change',clipIds:[c.clipId],noteId:c.notes[0].noteId,targetTick:'240000',lengthTick:'480000',pitch:64});
 await page.locator('#piano-canvas').focus();await page.keyboard.press('Control+a');await sleep(100);
 await set('start','1.1.2.000001');assert.deepEqual((await part()).notes.map(n=>n.startTick),['240001','960001']);
 await set('end','1.2.1.000002');assert.deepEqual((await part()).notes.map(n=>n.lengthTick),['720001','960001']);
 await set('length','0.0.1.000001','Control+Enter');assert.deepEqual((await part()).notes.map(n=>n.lengthTick),['240001','240001']);
 await page.locator('#piano-canvas').focus();await page.keyboard.press('Control+z');await idle();assert.deepEqual((await part()).notes.map(n=>n.lengthTick),['720001','960001']);await page.keyboard.press('Control+Shift+z');await idle();assert.deepEqual((await part()).notes.map(n=>n.lengthTick),['240001','240001']);
 // Focus-loss commit occurs once, without requiring Enter.
 revision=(await project()).revision;await page.locator('#note-start').fill('1.1.2.000002');await page.locator('#note-end').focus();await idle();assert.equal((await project()).revision,revision+1);assert.deepEqual((await part()).notes.map(n=>n.startTick),['240002','960002']);
 report.checks.push('Multi-select relative delta for Start/End; Ctrl+Enter absolute Length; one transaction per focus-loss commit; group Undo/Redo.');
 p=await project();const path=join(folder,'note-time.minidaw');await invoke('save_project',{path,revision:p.revision});p=await project();const saved=p.document;await invoke('open_project',{path,revision:p.revision,discard:true});await idle();assert.deepEqual((await project()).document,saved);
 await page.locator(`[data-clip-id="${c.clipId}"]`).click();await page.locator('#piano-open').click();await idle();
 // 7/8 length/position fields use the existing signature, not hard-coded 4/4.
 await edit({command:'project.signature',numerator:7,denominator:8});
 await page.locator('#piano-canvas').focus();await page.keyboard.press('Control+a');await set('start','2.2.2.000123');assert.equal((await n()).startTick,'4080123');assert.equal(await page.locator('#note-start').inputValue(),'2.2.2.000123');
 await set('length','0.1.1.000001','Control+Enter');assert.ok((await part()).notes.every(n=>n.lengthTick==='720001'));
 report.checks.push('Save/reopen exact; 7/8 position/length input and Part extension preserve integer ticks.');
 // Shared real borders remain outside editor content.
 for(const id of ['spectrum','performance'])if(await page.locator('#show-'+id).getAttribute('aria-pressed')!=='true')await page.locator('#show-'+id).click();
 await sleep(120);
 report.borders=await page.evaluate(()=>[...document.querySelectorAll('.workspace-panel:not([hidden])')].map(p=>({id:p.id,widths:['Top','Right','Bottom','Left'].map(s=>getComputedStyle(p)['border'+s+'Width'])})));assert.ok(report.borders.every(p=>p.widths.every(w=>w==='2px')));
 for(const id of ['spectrum','performance'])await page.locator('#show-'+id).click();
 const divider=page.locator('.panel-divider').first(),bounds=await divider.boundingBox(),old=(await page.locator('#panel-piano').boundingBox()).width;await page.mouse.move(bounds.x+3,bounds.y+80);await page.mouse.down();await page.mouse.move(bounds.x-60,bounds.y+80,{steps:10});await page.mouse.up();await sleep(120);assert.ok((await page.locator('#panel-piano').boundingBox()).width>old+30);
 await page.locator('#note-start').scrollIntoViewIfNeeded();await page.screenshot({path:'docs/validation/note-time-ui.png'});
 for(const scale of ['90','110','125']){await page.locator('#font-scale').selectOption(scale);await page.locator('#note-start').scrollIntoViewIfNeeded();await sleep(80);assert.ok((await page.locator('#piano-canvas').boundingBox()).height>=100);assert.ok(await page.locator('#note-start').evaluate(e=>{const s=getComputedStyle(e),c=document.createElement('canvas').getContext('2d');c.font=s.font;return c.measureText(e.value).width<=e.clientWidth-parseFloat(s.paddingLeft)-parseFloat(s.paddingRight);}), 'Complete numeric value fits at each font scale');}
 await page.setViewportSize({width:1200,height:820});await sleep(100);assert.ok((await page.locator('#piano-canvas').boundingBox()).width>100);
 report.checks.push('All panel borders 2px; existing divider resize; Font Scale 90/110/125 and window resize.');
 assert.deepEqual(errors,[]);assert.equal(await page.locator('#error').isVisible(),false);report.passed=true;
}catch(e){report.error=String(e.stack??e);report.pageErrors=errors;process.exitCode=1;if(page){report.uiError=await page.locator('#error-text').textContent().catch(()=>null);await page.screenshot({path:'docs/validation/note-time-failure.png'}).catch(()=>{});}}
finally{
 if(page&&prefs)await page.evaluate(p=>{for(const[k,v]of[['minidaw.ui.panels.v1',p.panels],['minidaw.ui.fontScale',p.font]]){if(v===null)localStorage.removeItem(k);else localStorage.setItem(k,v);}},prefs).catch(()=>{});
 if(browser)await browser.close();if(child?.exitCode===null){const exited=once(child,'exit');child.kill();await exited;}
 if(savedRecent)await writeFile(recent,savedRecent);else await unlink(recent).catch(()=>{});
 await writeFile('docs/validation/note-time-ui.json',JSON.stringify(report,null,2));console.log(JSON.stringify(report));
}

