// Native release WebView + real output device. No browser transport mocks.
import {chromium} from 'playwright-core';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {readFile,writeFile,mkdir,unlink} from 'node:fs/promises';
import {randomUUID,createHash} from 'node:crypto';
import {resolve,join} from 'node:path';
import assert from 'node:assert/strict';
const exe=resolve('src-tauri/target/release/minidaw.exe'),folder=resolve('tests/local/mixer',randomUUID());await mkdir(folder,{recursive:true});
const recent=join(process.env.APPDATA,'local.minidaw.desktop/recent-projects.json');let savedRecent;try{savedRecent=await readFile(recent);}catch{}
const report={checks:[],executableSha256:createHash('sha256').update(await readFile(exe)).digest('hex')};
let child,browser,page,prefs;const errors=[],sleep=ms=>new Promise(r=>setTimeout(r,ms));
async function until(fn,label,limit=25000){const at=performance.now();while(performance.now()-at<limit){if(await fn())return;await sleep(35);}throw Error(label);}
const invoke=(name,args={})=>page.evaluate(({name,args})=>window.__TAURI_INTERNALS__.invoke(name,args),{name,args});
const project=()=>invoke('project_snapshot'),audio=()=>invoke('engine_snapshot');
async function idle(){await until(async()=>!await page.locator('#project-save').isDisabled(),'Project idle');await sleep(200);}
async function edit(request){const p=await project();await invoke('edit_project',{revision:p.revision,request});await idle();}
async function panel(id,open){const b=page.locator('#show-'+id);if((await b.getAttribute('aria-pressed')==='true')!==open)await b.click();await sleep(80);}
const header=id=>page.locator(`.track-header[data-track-id="${id}"]`),clip=id=>page.locator(`.audio-clip[data-clip-id="${id}"]`);
async function add(kind){await page.locator(`#${kind}-track-add`).click();await idle();return (await project()).document.tracks.at(-1).trackId;}
async function setMix(id,fields){await edit({command:'track.mix',trackIds:[id],...fields});}
function peak(f,hz,ch=0){return f.frequencies.map((v,i)=>({hz:v,db:f.curves[ch][i]})).filter(p=>Math.abs(Math.log(p.hz/hz))<.065).sort((a,b)=>b.db-a.db)[0].db;}
const strip=id=>page.locator(`.mixer-channel[data-track-id="${id}"]`);
async function number(id,value){const e=strip(id).locator('.mixer-volume');await e.fill(String(value));await e.press('Tab');await until(async()=>{const p=(await project()).document;return (id==='master'?p.master?.volumeDb??0:p.tracks.find(t=>t.trackId===id)?.mix?.volumeDb??0)===value;},'Fader commit');await idle();}
async function frame(){await sleep(650);return (await invoke('spectrum_snapshot',{after:0})).frame;}
async function order(){await until(async()=>JSON.stringify(await page.locator('#mixer-tracks .mixer-channel').evaluateAll(es=>es.map(e=>e.dataset.trackId)))===JSON.stringify((await project()).document.tracks.map(t=>t.trackId)),'Mixer order');}
try{
 child=spawn(exe,[],{env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=19263'},stdio:'ignore',windowsHide:true});
 await until(async()=>{try{browser=await chromium.connectOverCDP('http://127.0.0.1:19263');return true;}catch{return false;}},'Release CDP');
 await until(async()=>{page=browser.contexts()[0].pages().find(p=>p.url().includes('tauri.localhost'));return !!page;},'Page');
 page.on('pageerror',e=>errors.push(e.message));await page.setViewportSize({width:1440,height:960});await idle();
 prefs=await page.evaluate(()=>Object.fromEntries(['minidaw.ui.panels.v1','minidaw.ui.fontScale','minidaw.ui.spectrum.v1'].map(k=>[k,localStorage.getItem(k)])));
 for(const[id,open]of [['arrangement',true],['media',false],['performance',false],['spectrum',false],['piano',false],['mixer',true]])await panel(id,open);
 const a=await add('audio'),b=await add('audio'),m=await add('midi'),n=await add('midi');await order();
 assert.deepEqual(await page.locator('.mixer-kind').allTextContents(),['1 · Audio','2 · Audio','3 · MIDI','4 · MIDI','Stereo Out']);
 // Bidirectional authoritative document state, including the Arrangement popup.
 await strip(a).locator('.mixer-mute').click();await until(async()=>await header(a).locator('.track-mute').getAttribute('aria-pressed')==='true','Mixer → Arrangement mute');
 await header(a).locator('.track-mute').click();await until(async()=>await strip(a).locator('.mixer-mute').getAttribute('aria-pressed')==='false','Arrangement → Mixer mute');
 await header(m).locator('.track-solo').click();await until(async()=>await strip(m).locator('.mixer-solo').getAttribute('aria-pressed')==='true','Arrangement → Mixer solo');
 await strip(m).locator('.mixer-solo').click();await idle();
 await number(a,-6);await header(a).locator('.track-settings-open').click();assert.equal(await page.locator('#track-volume').inputValue(),'-6');
 await page.locator('#track-volume').fill('-9');await page.locator('#track-volume').press('Tab');await until(async()=>Number(await strip(a).locator('.mixer-volume').inputValue())===-9,'Arrangement → Mixer volume');
 await page.locator('#track-pan').press('End');await idle();assert.equal(await strip(a).locator('.mixer-pan').inputValue(),'100');await page.keyboard.press('Escape');
 await strip(a).locator('.mixer-pan').press('Home');await idle();await header(a).locator('.track-settings-open').click();assert.equal(await page.locator('#track-pan').inputValue(),'-100');await page.keyboard.press('Escape');
 // Actual pointer drag, updates during the gesture, one undo transaction.
 const undo=(await project()).history.undo,box=await strip(a).locator('.mixer-fader').boundingBox();
 await page.mouse.move(box.x+box.width/2,box.y+box.height*.3);await page.mouse.down();
 for(let i=0;i<15;i++){await page.mouse.move(box.x+box.width/2,box.y+box.height*(.3+i*.015));await sleep(45);}
 const during=(await project()).document.tracks[0].mix.volumeDb;assert.notEqual(during,-9);
 await page.mouse.up();await sleep(600);await idle();assert.equal((await project()).history.undo,undo+1);const final=(await project()).document.tracks[0].mix.volumeDb;
 await edit({command:'edit.undo'});assert.equal((await project()).document.tracks[0].mix.volumeDb,-9);await edit({command:'edit.redo'});assert.equal((await project()).document.tracks[0].mix.volumeDb,final);
 const temp=await add('audio');await order();await edit({command:'track.move',trackIds:[n],direction:-1});await order();await edit({command:'track.delete',trackIds:[temp]});await order();assert.equal(await strip(temp).count(),0);await edit({command:'edit.undo'});await order();await edit({command:'edit.redo'});
 report.checks.push('Audio/MIDI channel types/order; Arrangement ↔ Mixer Mute/Solo/Volume/Pan; actual pointer fader changes during drag, one Undo/Redo; Track add/delete/reorder.');
 // Actual two Streaming sources and two synths, same Master path.
 await invoke('load_audio',{path:resolve('tests/generated/long-400s-48000-2ch.wav'),trackId:a});await idle();
 await invoke('load_audio',{path:resolve('tests/generated/long-720s-44100-2ch.flac'),trackId:b});await idle();
 for(const[id,pitch]of [[m,76],[n,81]]){await edit({command:'midi.track.instrument',trackIds:[id],instrument:'basicSynth'});await edit({command:'midi.clip.add',trackIds:[id],targetTick:'0',lengthTick:'230400000'});const c=(await project()).document.tracks.find(t=>t.trackId===id).clips[0];await edit({command:'midi.note.add',clipIds:[c.clipId],targetTick:'0',lengthTick:'230400000',pitch,velocity:85});}
 for(const id of [a,b,m,n])await setMix(id,{mute:false,solo:false,volumeDb:-6,pan:0});
 await invoke('spectrum_configure',{settings:{enabled:true,fftSize:8192,smoothingMs:0}});
 await invoke('transport_command',{action:'seek',seconds:20});await invoke('transport_command',{action:'play'});let f=await frame();
 const hz=[227,443,659.255,880],beforeLevels=hz.map(h=>peak(f,h,2));report.beforeLevels=beforeLevels;assert.ok(beforeLevels.every(db=>db>-65));
 const before=await audio();report.output=before.output;
 await number('master',-6);f=await frame();const quieter=hz.map(h=>peak(f,h,2));report.masterDeltaDb=quieter.map((v,i)=>v-beforeLevels[i]);assert.ok(report.masterDeltaDb.every(v=>Math.abs(v+6)<.3));
 const after=await audio();assert.equal(after.transport.appliedCommand,before.transport.appliedCommand);assert.equal(after.transport.clipId,before.transport.clipId);assert.equal(after.source.generation,before.source.generation);assert.equal(after.analysisBuilds,before.analysisBuilds);
 const peaks=after.master.peakDb;assert.ok(peaks.every(v=>v<0&&v>-70));assert.ok(await page.locator('.master-peak').evaluateAll(es=>es.every(e=>!e.textContent.includes('∞'))));
 // Output meter is post-Master; -inf silences Audio and Synth together.
 await number('master',-96);f=await frame();assert.ok(Math.max(...f.curves.flat())<-100);await sleep(2300);assert.ok((await audio()).master.peakDb.every(v=>v<-60));
 await edit({command:'edit.undo'});assert.equal((await project()).document.master.volumeDb,-6);f=await frame();assert.ok(peak(f,659.255)>-55);
 await number('master',0);
 await strip(m).locator('.mixer-solo').click();await idle();f=await frame();assert.ok(peak(f,659.255)>-45);assert.ok(peak(f,227)<-80);assert.ok(peak(f,880)<-80);
 const synth=peak(f,659.255);await number(m,-12);f=await frame();assert.ok(Math.abs(peak(f,659.255)-synth+6)<.25);
 await strip(m).locator('.mixer-pan').press('Home');await idle();f=await frame();assert.ok(Math.max(...f.curves[1])<-100);
 await strip(m).locator('.mixer-mute').click();await idle();f=await frame();assert.ok(Math.max(...f.curves.flat())<-100);
 for(const id of [a,b,m,n])await setMix(id,{mute:false,solo:false,volumeDb:-6,pan:0});
 await frame();const perfBefore=await audio();await sleep(2200);const perfAfter=await audio();report.performance={callbackAvgMs:(perfAfter.metrics.callbackAvgMs*perfAfter.metrics.callbacks-perfBefore.metrics.callbackAvgMs*perfBefore.metrics.callbacks)/(perfAfter.metrics.callbacks-perfBefore.metrics.callbacks),callbackMaxMs:perfAfter.metrics.callbackMaxMs,overruns:perfAfter.metrics.overruns-perfBefore.metrics.overruns,deviceOverruns:perfAfter.metrics.deviceCallbackOverruns-perfBefore.metrics.deviceCallbackOverruns,starvation:perfAfter.source.starvation,peakDb:perfAfter.master.peakDb};
 assert.equal(report.performance.overruns,0);assert.equal(report.performance.deviceOverruns,0);assert.equal(report.performance.starvation,0);assert.equal(perfAfter.streamErrors,0);
 report.checks.push('Two Streaming sources + two Synth outputs: Master -6dB measured on all four frequencies; -inf silence; post-Master stereo meter; Mixer Solo/Mute/Synth gain/pan affect actual output; Master changes preserve source, cursor, streaming generation and waveform cache.');
 // Panel layout / font / meter hidden lifecycle.
 await panel('mixer',false);const wide=(await page.locator('#panel-arrangement').boundingBox()).width;
 const hiddenValues=await page.locator('.master-peak').allTextContents();await edit({command:'master.volume',volumeDb:-24});await sleep(350);assert.deepEqual(await page.locator('.master-peak').allTextContents(),hiddenValues);
 await panel('mixer',true);assert.ok((await page.locator('#panel-arrangement').boundingBox()).width<wide-200);
 assert.equal(Number(await strip('master').locator('.mixer-volume').inputValue()),-24);
 const divider=page.locator('.panel-divider[data-left="arrangement"][data-right="mixer"]'),d=await divider.boundingBox(),oldWidth=(await page.locator('#panel-mixer').boundingBox()).width;
 await page.mouse.move(d.x+3,d.y+100);await page.mouse.down();await page.mouse.move(d.x-100,d.y+100,{steps:10});await page.mouse.up();assert.ok((await page.locator('#panel-mixer').boundingBox()).width>oldWidth+70);
 for(const scale of ['90','110','125']){await page.locator('#font-scale').selectOption(scale);await sleep(150);assert.ok(await page.locator('#mixer-master').evaluate(e=>{const r=e.getBoundingClientRect();return r.right<=innerWidth&&r.width>100;}));assert.ok(await page.locator('.mixer-channel').evaluateAll(es=>es.every(e=>{const f=e.querySelector('.mixer-fader').getBoundingClientRect(),v=e.querySelector('.mixer-volume').getBoundingClientRect();return f.height>60&&f.bottom<=v.top;})));}
 await page.setViewportSize({width:1100,height:760});await sleep(150);assert.ok((await page.locator('#mixer-master').boundingBox()).width>100);await page.setViewportSize({width:1440,height:960});await page.locator('#font-scale').selectOption('110');
 await number('master',-6);await sleep(300);await page.screenshot({path:'docs/validation/mixer-ui.png'});
 await invoke('transport_command',{action:'stop'});await sleep(250);
 let p=await project();const file=join(folder,'mixer.minidaw');await invoke('save_project',{path:file,revision:p.revision});
 const saved=JSON.parse(await readFile(file,'utf8'));saved.tracks.find(t=>t.trackId===a).name='Kick · renamed';await writeFile(file,JSON.stringify(saved,null,2));
 await invoke('open_project',{path:file,revision:(await project()).revision,discard:true});await idle();await order();assert.equal(await strip(a).locator('.mixer-name').textContent(),'Kick · renamed');assert.equal(Number(await strip('master').locator('.mixer-volume').inputValue()),-6);
 assert.deepEqual((await project()).document.master,saved.master);assert.deepEqual((await project()).document.tracks,saved.tracks);
 await invoke('new_project',{revision:(await project()).revision,discard:true});await idle();assert.equal(await page.locator('#mixer-tracks .mixer-channel').count(),0);assert.equal(Number(await strip('master').locator('.mixer-volume').inputValue()),0);
 report.checks.push('Mixer close returns space and stops meter DOM writes; show/resize/window resize/90–125% Font Scale; save/open restores tracks and Master; renamed Track label refresh; new project returns Master to unity.');
 assert.deepEqual(errors,[]);assert.equal(await page.locator('#error').isVisible(),false);report.passed=true;
}catch(e){report.error=String(e.stack??e);report.pageErrors=errors;process.exitCode=1;if(page){report.uiError=await page.locator('#error-text').textContent().catch(()=>null);report.lastAudio=await audio().catch(()=>null);report.lastProject=await project().catch(()=>null);await page.screenshot({path:'docs/validation/mixer-failure.png'}).catch(()=>{});}}
finally{
 if(page&&prefs)await page.evaluate(p=>{for(const[k,v]of Object.entries(p)){if(v===null)localStorage.removeItem(k);else localStorage.setItem(k,v);}},prefs).catch(()=>{});
 if(browser)await browser.close();if(child?.exitCode===null){const exited=once(child,'exit');child.kill();await exited;}
 if(savedRecent)await writeFile(recent,savedRecent);else await unlink(recent).catch(()=>{});
 await writeFile('docs/validation/mixer-ui.json',JSON.stringify(report,null,2));console.log(JSON.stringify({passed:report.passed,error:report.error,checks:report.checks,performance:report.performance,masterDeltaDb:report.masterDeltaDb}));
}
