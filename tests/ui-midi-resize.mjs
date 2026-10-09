import {chromium} from 'playwright-core';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {readFile,writeFile,mkdir,unlink} from 'node:fs/promises';
import {randomUUID} from 'node:crypto';
import {resolve,join} from 'node:path';
import assert from 'node:assert/strict';
const folder=resolve('tests/local/midi-resize',randomUUID());await mkdir(folder,{recursive:true});
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
 await edit({command:'midi.track.add'});let p=await project();await edit({command:'midi.clip.add',trackIds:[p.document.tracks[0].trackId],targetTick:'960000',lengthTick:'3840000'});
 let c=await part(),id=c.clipId;for(const[start,len,pitch]of[[123,240001,60],[1200123,480001,64],[3000123,480001,67]])await edit({command:'midi.note.add',clipIds:[id],targetTick:String(start),lengthTick:String(len),pitch});
 await edit({command:'midi.control.put',clipIds:[id],event:{eventId:'',tick:'3000123',channel:0,data:{kind:'cc',controller:1,value:80}}});
 c=await part();const original=structuredClone(c);
 async function fit(){await page.locator('#zoom-fit').click();await sleep(120);}
 async function drag(side,delta,cancel=false){const before=await project(),old=await part(),b=await page.locator(`[data-clip-id="${id}"] [data-handle="trim-${side}"]`).boundingBox(),step=Number(await page.locator('#scroll').getAttribute('step')),x=b.x+b.width/2,y=b.y+b.height*.6;
 await page.mouse.move(x,y);await page.mouse.down();assert.equal(await page.locator('#clip-layer').evaluate(e=>e.hasPointerCapture(1)),true);await page.mouse.move(x+delta/1920000/step,y,{steps:100});assert.equal((await project()).revision,before.revision);assert.deepEqual(await part(),old);
 const preview=JSON.parse(await page.locator(`[data-clip-id="${id}"]`).getAttribute('data-preview'));
 if(cancel){await page.locator('#clip-layer').dispatchEvent('pointercancel',{pointerId:1});await page.mouse.up();await sleep(100);assert.equal((await project()).revision,before.revision);return;}
 await page.mouse.up();await until(async()=>(await project()).revision>before.revision,'Resize committed');await idle();const after=await part();assert.equal((await project()).revision,before.revision+1);assert.equal(after.startTick,preview.startTick);assert.equal(after.lengthTick,preview.lengthTick);assert.deepEqual(after.notes,original.notes);assert.deepEqual(after.controls,original.controls);assert.equal(Number(after.startTick)-Number(after.contentOffsetTick),Number(original.startTick));report.notes.push({side,start:after.startTick,length:after.lengthTick,offset:after.contentOffsetTick});return after;}
 await fit();await page.locator('#grid-type').selectOption('16');if(await page.locator('#snap-toggle').getAttribute('aria-pressed')!=='true')await page.locator('#snap-toggle').click();
 c=await drag('left',960000);assert.equal(c.startTick,'1920000');assert.equal(c.lengthTick,'2880000');
 c=await drag('right',-1920000);assert.equal(c.lengthTick,'960000');
 const trimmed=structuredClone(c);await page.keyboard.press('Control+z');await idle();assert.equal((await part()).lengthTick,'2880000');await page.keyboard.press('Control+Shift+z');await idle();assert.deepEqual(await part(),trimmed);
 await drag('left',240000,true);assert.deepEqual(await part(),trimmed);
 await fit();await drag('left',-960000);await drag('right',1920000);assert.deepEqual(await part(),original);
 report.checks.push('Both edges Snap 1/16; 100 pointer moves preview-only; exactly one commit; pointer capture/cancel; shrink/reveal preserves all Notes and CC; Undo/Redo.');
 // At maximum timeline zoom, non-grid edits are still integer ticks, never samples.
 await page.locator('#snap-toggle').click();await fit();let layer=await page.locator('#clip-layer').boundingBox(),step=Number(await page.locator('#scroll').getAttribute('step')),ruler=await page.locator('#waveform').boundingBox();
 await page.mouse.move(layer.x+.5/step,ruler.y+ruler.height*.8);await page.keyboard.down('Control');for(let i=0;i<4;i++){await page.mouse.wheel(0,-1000);await sleep(70);}await page.keyboard.up('Control');
 await page.locator('#scroll').evaluate(e=>{e.value='0.499';e.dispatchEvent(new Event('input',{bubbles:true}));});await sleep(120);
 let previous=960000;for(const delta of [123,-37,81,-167]){c=await drag('left',delta);assert.equal(Number(c.startTick),previous+delta);previous+=delta;}
 assert.equal(previous,960000);assert.deepEqual(c,original);report.checks.push('Snap OFF maximum-zoom left Resize +123/-37/+81/-167 ticks returns exactly to origin without drift.');
 await fit();layer=await page.locator('#clip-layer').boundingBox();step=Number(await page.locator('#scroll').getAttribute('step'));ruler=await page.locator('#waveform').boundingBox();await page.mouse.move(layer.x+2.5/step,ruler.y+ruler.height*.8);await page.keyboard.down('Control');for(let i=0;i<4;i++){await page.mouse.wheel(0,-1000);await sleep(70);}await page.keyboard.up('Control');
 await page.locator('#scroll').evaluate(e=>{e.value='2.499';e.dispatchEvent(new Event('input',{bubbles:true}));});await sleep(120);
 let length=3840000;for(const delta of [83,-41,-42]){c=await drag('right',delta);assert.equal(Number(c.lengthTick),length+delta);length+=delta;}assert.deepEqual(c,original);report.checks.push('Snap OFF right Resize is also exact to individual ticks and reversible.');
 await fit();await page.locator('#snap-toggle').click();c=await drag('right',-5000000);assert.equal(c.lengthTick,'1');await page.keyboard.press('Control+z');await idle();await fit();c=await drag('left',-1920000);assert.equal(c.startTick,'0');await page.keyboard.press('Control+z');await idle();
 await fit();c=await drag('left',960000);c=await drag('right',-1920000);
 p=await project();const path=join(folder,'part-resize.minidaw');await invoke('save_project',{path,revision:p.revision});p=await project();const saved=p.document;await invoke('open_project',{path,revision:p.revision,discard:true});await idle();assert.deepEqual((await project()).document,saved);
 // Piano Roll projects the offset without moving the underlying event.
 await page.locator(`[data-clip-id="${id}"]`).click();await page.locator("#piano-open").click();await idle();await page.locator('#piano-vscroll').evaluate(e=>{const row=parseFloat(getComputedStyle(document.documentElement).fontSize)*1.45;e.scrollTop=(127-64)*row-e.clientHeight/2;});await sleep(100);
 let pt=await point(2400123,64);await page.mouse.click(pt.x,pt.y);await sleep(100);assert.equal(await page.locator('#note-start').inputValue(),'1.3.2.000123');assert.equal(await page.locator('#note-length').inputValue(),'0.0.2.000001');
 await page.locator('#note-start').fill('1.3.2.000124');await page.locator('#note-start').press('Enter');await idle();c=await part();assert.equal(Number(c.startTick)-Number(c.contentOffsetTick)+Number(c.notes[1].startTick),2160124);assert.equal(Number(c.startTick)-Number(c.contentOffsetTick)+Number(c.notes[0].startTick),960123);
 await page.locator('#piano-canvas').focus();await page.keyboard.press('Control+z');await idle();assert.deepEqual(await part(),trimmed);
 report.checks.push('Project save/reopen exact; resized Part opens in Piano Roll at unchanged absolute note positions; precise note editing and Undo preserve hidden events.');
 await page.screenshot({path:'docs/validation/midi-resize-ui.png'});
 assert.deepEqual(errors,[]);assert.equal(await page.locator('#error').isVisible(),false);report.passed=true;
}catch(e){report.error=String(e.stack??e);report.pageErrors=errors;process.exitCode=1;if(page){report.uiError=await page.locator('#error-text').textContent().catch(()=>null);await page.screenshot({path:'docs/validation/midi-resize-failure.png'}).catch(()=>{});}}
finally{
 if(page&&prefs)await page.evaluate(p=>{for(const[k,v]of[['minidaw.ui.panels.v1',p.panels],['minidaw.ui.fontScale',p.font]]){if(v===null)localStorage.removeItem(k);else localStorage.setItem(k,v);}},prefs).catch(()=>{});
 if(browser)await browser.close();if(child?.exitCode===null){const exited=once(child,'exit');child.kill();await exited;}
 if(savedRecent)await writeFile(recent,savedRecent);else await unlink(recent).catch(()=>{});
 await writeFile('docs/validation/midi-resize-ui.json',JSON.stringify(report,null,2));console.log(JSON.stringify(report));
}

