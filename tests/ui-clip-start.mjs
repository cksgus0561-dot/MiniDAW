// Real release WebView + TOPPING/WASAPI output. PCM measurements use the existing
// post-Master Spectrum tap, not mocked renderer data or UI playback timers.
import {chromium} from 'playwright-core';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {readFile,writeFile,mkdir,unlink} from 'node:fs/promises';
import {randomUUID,createHash} from 'node:crypto';
import {resolve,join} from 'node:path';
import assert from 'node:assert/strict';
const exe=resolve('src-tauri/target/release/minidaw.exe'),folder=resolve('tests/local/clip-start',randomUUID());await mkdir(folder,{recursive:true});
const recent=join(process.env.APPDATA,'local.minidaw.desktop/recent-projects.json');let savedRecent;try{savedRecent=await readFile(recent);}catch{}
const report={checks:[],executableSha256:createHash('sha256').update(await readFile(exe)).digest('hex')};
let child,browser,page,prefs;const errors=[],sleep=ms=>new Promise(r=>setTimeout(r,ms));
async function until(fn,label,limit=12000){const at=performance.now();while(performance.now()-at<limit){if(await fn())return;await sleep(40);}throw Error(label);}
const invoke=(name,args={})=>page.evaluate(({name,args})=>window.__TAURI_INTERNALS__.invoke(name,args),{name,args});
const project=()=>invoke('project_snapshot'),audio=()=>invoke('engine_snapshot');
async function idle(){await until(async()=>!await page.locator('#project-save').isDisabled(),'Project idle');await sleep(180);}
async function edit(request){const p=await project();await invoke('edit_project',{revision:p.revision,request});await idle();}
async function panel(id,open){const b=page.locator('#show-'+id);if((await b.getAttribute('aria-pressed')==='true')!==open)await b.click();await sleep(80);}
const strip=id=>page.locator(`.mixer-channel[data-track-id="${id}"]`),slot=id=>page.locator(`.effect-slot[data-effect-id="${id}"]`);
try{
 child=spawn(exe,[],{env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=19268'},stdio:'ignore',windowsHide:true});
 await until(async()=>{try{browser=await chromium.connectOverCDP('http://127.0.0.1:19268');return true;}catch{return false;}},'Release CDP');
 await until(async()=>{page=browser.contexts()[0].pages().find(p=>p.url().includes('tauri.localhost'));return !!page;},'Page');
 page.on('pageerror',e=>errors.push(e.message));await page.setViewportSize({width:1440,height:960});await idle();
 prefs=await page.evaluate(()=>Object.fromEntries(['minidaw.ui.panels.v1','minidaw.ui.fontScale','minidaw.ui.spectrum.v1'].map(k=>[k,localStorage.getItem(k)])));
 for(const[id,show]of [['arrangement',true],['media',false],['performance',false],['spectrum',false],['piano',false],['mixer',false]])await panel(id,show);
 await invoke('new_project',{revision:(await project()).revision,discard:true});await idle();


 await edit({command:'track.add',trackKind:'audio'});const a=(await project()).document.tracks.at(-1).trackId;
 await invoke('load_audio',{path:resolve('tests/generated/long-400s-48000-2ch.wav'),trackId:a});await idle();
 const audioId=(await project()).document.tracks[0].clips[0].clipId;
 await edit({command:'midi.track.add'});const m=(await project()).document.tracks.at(-1).trackId;
 await edit({command:'midi.clip.add',trackIds:[m],targetTick:'0',lengthTick:'7680000'});const midiId=(await project()).document.tracks.at(-1).clips[0].clipId;
 await edit({command:'midi.note.add',clipIds:[midiId],targetTick:'123',lengthTick:'480000',pitch:60,velocity:80});
 const clip=async id=>(await project()).document.tracks.flatMap(t=>t.clips).find(c=>c.clipId===id);
 const select=async ids=>{await page.evaluate(ids=>document.dispatchEvent(new CustomEvent('select-clips',{detail:ids})),ids);await sleep(80);};
 const input=page.locator('#clip-start'),fmt=page.locator('#ruler-format');
 const enter=async text=>{await input.fill(text);await input.press('Enter');await idle();};
 const seconds=c=>c.kind==='midi'?Number(c.startTick)/1920000:c.position.unit==='ticks'?Number(c.position.ticks)/1920000:Number(c.position.numerator)/c.position.denominator;
 const content=c=>c.kind==='midi'?{notes:c.notes,controls:c.controls,lengthTick:c.lengthTick,contentOffsetTick:c.contentOffsetTick}:{sourceStart:c.sourceStart,sourceEnd:c.sourceEnd,fadeIn:c.fadeIn,fadeOut:c.fadeOut,gain:c.gain};
 const audioContent=content(await clip(audioId)),midiContent=content(await clip(midiId));
 await fmt.selectOption('bars');await page.locator('#grid-type').selectOption('bar');if(await page.locator('#snap-toggle').getAttribute('aria-pressed')!=='true')await page.locator('#snap-toggle').click();
 for(const id of [audioId,midiId]){
   await select([id]);const node=page.locator(`[data-clip-id="${id}"]`).first();const box=await node.boundingBox();const layer=await page.locator('#clip-layer').boundingBox();await page.mouse.click(Math.max(layer.x+20,box.x+20),box.y+box.height*.5);await idle();
   assert.equal(await input.inputValue(),'1.1.1.000000');const h=(await project()).history.undo;
   await enter('2.3.2.000123');const moved=await clip(id);assert.equal(Math.round(seconds(moved)*1920000),6000123);assert.equal(await input.inputValue(),'2.3.2.000123');assert.equal((await project()).history.undo,h+1);
   await page.keyboard.press('Control+z');await idle();assert.equal(seconds(await clip(id)),0);await page.keyboard.press('Control+Shift+z');await idle();assert.deepEqual(await clip(id),moved);
   const rev=(await project()).revision;await fmt.selectOption('seconds');assert.equal((await project()).revision,rev);await fmt.selectOption('bars');assert.deepEqual(await clip(id),moved);
 }
 await select([audioId]);await fmt.selectOption('seconds');await enter('00:03.123456789');assert.equal(seconds(await clip(audioId)),3.123456789);assert.equal(await input.inputValue(),'3.123456789 s');
 // Absolute rational destinations remain identical after many repeated moves.
 const exact=structuredClone((await clip(audioId)).position);for(let i=0;i<5;i++){await enter('7.987654321 s');await enter('3.123456789 s');}assert.deepEqual((await clip(audioId)).position,exact);
 await page.locator('#snap-toggle').click();await enter('4.000000001');assert.equal(seconds(await clip(audioId)),4.000000001);
 await select([midiId]);await enter('00:03.125');assert.equal((await clip(midiId)).startTick,'6000000');assert.equal(await input.inputValue(),'3.125000000 s');
 await enter('3.125000001');assert.equal(await input.getAttribute('aria-invalid'),'true');assert.equal((await clip(midiId)).startTick,'6000000');await input.press('Escape');assert.equal(await input.inputValue(),'3.125000000 s');
 for(const bad of ['','-1','abc','00:99.1']){const h=(await project()).history.undo;await enter(bad);assert.equal(await input.getAttribute('aria-invalid'),'true');assert.equal((await project()).history.undo,h);await input.press('Escape');}
 await fmt.selectOption('bars');await enter('1.5.1.000000');assert.equal(await input.getAttribute('aria-invalid'),'true');await input.press('Escape');
 assert.deepEqual(content(await clip(audioId)),audioContent);assert.deepEqual(content(await clip(midiId)),midiContent);
 // Existing mouse Move and Snap on/off still apply to both kinds.
 for(const id of [audioId,midiId]){
   await select([id]);await enter('1.2.1.000000');
   for(const on of [true,false]){
     if((await page.locator('#snap-toggle').getAttribute('aria-pressed')==='true')!==on)await page.locator('#snap-toggle').click();
     await page.locator('#grid-type').selectOption('16');const box=await page.locator(`[data-clip-id="${id}"]`).first().boundingBox(),layer=await page.locator('#clip-layer').boundingBox(),x=Math.max(layer.x+30,box.x+30),y=box.y+box.height*.5;
     const before=await clip(id);await page.mouse.move(x,y);await page.mouse.down();await page.mouse.move(x+57,y,{steps:20});await page.mouse.up();await idle();const after=await clip(id);assert.notEqual(seconds(after),seconds(before));const tick=Math.round(seconds(after)*1920000);assert.equal(tick%240000===0,on);
     await page.keyboard.press('Control+z');await idle();assert.deepEqual(await clip(id),before);
   }
 }
 // Same-kind multi-selection follows the first selected event, preserving gaps.
 await select([audioId]);await page.locator('#clip-layer').focus();await page.keyboard.press('Control+d');await idle();const other=(await project()).document.tracks[0].clips.find(c=>c.clipId!==audioId).clipId;
 const gap=seconds(await clip(other))-seconds(await clip(audioId));await select([audioId,other]);await fmt.selectOption('seconds');await enter('00:02.250');assert.equal(seconds(await clip(audioId)),2.25);assert.equal(seconds(await clip(other))-seconds(await clip(audioId)),gap);
 await select([midiId]);await fmt.selectOption('bars');await enter('3.2.4.000007');assert.equal((await clip(midiId)).startTick,'9360007');
 const saved=(await project()).document,file=join(folder,'start.minidaw');await invoke('save_project',{path:file,revision:(await project()).revision});await invoke('open_project',{path:file,revision:(await project()).revision,discard:true});await idle();assert.deepEqual((await project()).document.tracks,saved.tracks);
 await select([midiId]);assert.equal(await input.inputValue(),'3.2.4.000007');
 for(const scale of ['90','125','110']){await page.locator('#font-scale').selectOption(scale);await sleep(100);assert.ok(await input.evaluate(e=>{const r=e.getBoundingClientRect();return r.width>100&&r.right<innerWidth;}));}
 await page.screenshot({path:'docs/validation/clip-start-ui.png'});
 report.checks.push('Audio/MIDI Start: Bars+Beats exact ticks; decimal seconds and mm:ss; Snap ignored; one Undo/Redo; format switch no edit; repeated rational moves no drift; invalid/cancel; content preserved; mouse Move Snap ON/OFF; same-kind relative multi-selection; save/open; 90–125% Font Scale.');
 assert.deepEqual(errors,[]);assert.equal(await page.locator('#error').isVisible(),false);report.passed=true;
}catch(e){report.error=String(e.stack??e);report.pageErrors=errors;process.exitCode=1;if(page){report.uiError=await page.locator('#error-text').textContent().catch(()=>null);report.lastAudio=await audio().catch(()=>null);report.lastProject=await project().catch(()=>null);await page.screenshot({path:'docs/validation/clip-start-failure.png'}).catch(()=>{});}}
finally{
 if(page&&prefs)await page.evaluate(p=>{for(const[k,v]of Object.entries(p)){if(v===null)localStorage.removeItem(k);else localStorage.setItem(k,v);}},prefs).catch(()=>{});
 if(browser)await browser.close();if(child?.exitCode===null){const exited=once(child,'exit');child.kill();await exited;}
 if(savedRecent)await writeFile(recent,savedRecent);else await unlink(recent).catch(()=>{});
 await writeFile('docs/validation/clip-start-ui.json',JSON.stringify(report,null,2));console.log(JSON.stringify({passed:report.passed,error:report.error,checks:report.checks,trackDeltaDb:report.trackDeltaDb,masterDeltaDb:report.masterDeltaDb,eqBoostDb:report.eqBoostDb}));
}


