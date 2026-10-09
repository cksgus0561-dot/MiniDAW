// Owned release WebView, real mouse edits and TOPPING ASIO sample-clock output.
import {chromium} from 'playwright-core';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {readFile,writeFile,mkdir,unlink,readdir} from 'node:fs/promises';
import {randomUUID,createHash} from 'node:crypto';
import {resolve,join} from 'node:path';
import assert from 'node:assert/strict';
const exe=resolve('src-tauri/target/release/minidaw.exe'),folder=resolve('tests/local/pitch',randomUUID());await mkdir(folder,{recursive:true});
const recent=join(process.env.APPDATA,'local.minidaw.desktop/recent-projects.json');let savedRecent;try{savedRecent=await readFile(recent);}catch{}
const report={checks:[],executableSha256:createHash('sha256').update(await readFile(exe)).digest('hex')};let child,browser,page,prefs;const errors=[],sleep=ms=>new Promise(r=>setTimeout(r,ms));
async function until(fn,label,limit=45000){const at=performance.now();while(performance.now()-at<limit){if(await fn())return;await sleep(40);}throw Error(label);}
const invoke=(name,args={})=>page.evaluate(({name,args})=>window.__TAURI_INTERNALS__.invoke(name,args).catch(e=>{throw Error(JSON.stringify(e));}),{name,args});
const project=()=>invoke('project_snapshot'),audio=()=>invoke('engine_snapshot');
async function idle(){await until(async()=>!await page.locator('#project-save').isDisabled(),'Project idle');await sleep(180);}
async function edit(request){await invoke('edit_project',{revision:(await project()).revision,request});await idle();}
async function panel(id,open){const b=page.locator('#show-'+id);if((await b.getAttribute('aria-pressed')==='true')!==open)await b.click();await sleep(80);}
async function select(id){await page.evaluate(id=>document.dispatchEvent(new CustomEvent('select-clips',{detail:[id]})),id);await sleep(100);}
async function enter(id,value){const input=page.locator(id);await input.fill(String(value));await input.press('Enter');await idle();}
const clip=async id=>(await project()).document.tracks.flatMap(t=>t.clips).find(c=>c.clipId===id);
const length=c=>Number(c.sourceEnd)-Number(c.sourceStart),position=c=>Number(c.position.numerator)/c.position.denominator;
async function measure(){await sleep(1800);const a=await audio();await sleep(5000);const b=await audio();const n=b.metrics.callbacks-a.metrics.callbacks;return {callbackAvgMs:(b.metrics.callbackAvgMs*b.metrics.callbacks-a.metrics.callbackAvgMs*a.metrics.callbacks)/n,callbacks:n,overruns:b.metrics.overruns-a.metrics.overruns,deviceOverruns:b.metrics.deviceCallbackOverruns-a.metrics.deviceCallbackOverruns,starvation:b.source.starvation-a.source.starvation,streamErrors:b.streamErrors-a.streamErrors,source:b.source,peakDb:b.master.peakDb};}
try{
 child=spawn(exe,[],{env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=19270'},stdio:'ignore',windowsHide:true});
 await until(async()=>{try{browser=await chromium.connectOverCDP('http://127.0.0.1:19270');return true;}catch{return false;}},'Release CDP');
 await until(async()=>{page=browser.contexts()[0].pages().find(p=>p.url().includes('tauri.localhost'));return !!page;},'Release page');page.on('pageerror',e=>errors.push(e.message));await page.setViewportSize({width:1440,height:960});await idle();
 prefs=await page.evaluate(()=>Object.fromEntries(['minidaw.ui.panels.v1','minidaw.ui.fontScale','minidaw.ui.spectrum.v1','minidaw.ui.snap','minidaw.ui.grid','minidaw.ui.rulerFormat'].map(k=>[k,localStorage.getItem(k)])));
 for(const[id,show]of [['arrangement',true],['media',false],['performance',false],['spectrum',false],['piano',false],['mixer',false]])await panel(id,show);
 await invoke('new_project',{revision:(await project()).revision,discard:true});await idle();await edit({command:'track.add',trackKind:'audio'});const a=(await project()).document.tracks.at(-1).trackId;
 await invoke('load_audio',{path:resolve('tests/generated/long-400s-48000-2ch.wav'),trackId:a});await idle();const id=(await project()).document.tracks[0].clips[0].clipId;
 // Only a short original window is rendered, even though the source is Streaming.
 await edit({command:'audio.trim',clipIds:[id],sourceStart:'0',sourceEnd:'480000'});await select(id);const original=await clip(id);
 await enter('#clip-pitch',7);await enter('#clip-pitch-cents',37);assert.equal(length(await clip(id)),480000);assert.deepEqual((await clip(id)).extensions['minidaw.pitchShift.v1'],{semitones:7,cents:37});
 await page.locator('#clip-layer').focus();await page.keyboard.press('Control+z');await idle();assert.equal((await clip(id)).extensions['minidaw.pitchShift.v1'].cents,0);await page.keyboard.press('Control+Shift+z');await idle();assert.equal((await clip(id)).extensions['minidaw.pitchShift.v1'].cents,37);
 const pitched=await clip(id);await enter('#clip-pitch',13);assert.equal(await page.locator('#clip-pitch').getAttribute('aria-invalid'),'true');assert.deepEqual(await clip(id),pitched);await page.locator('#clip-pitch').press('Escape');
 await page.locator('#clip-pitch-reset').click();await idle();assert.deepEqual(await clip(id),original);
 report.checks.push('Semitone + Fine cent numeric input, constant length, invalid range, exact Pitch 0 restoration, Undo/Redo.');
 // Same UI command on a resident fixture, on another Audio Track.
 await edit({command:'track.add',trackKind:'audio'});const memoryTrack=(await project()).document.tracks.at(-1).trackId;
 await invoke('load_audio',{path:resolve('tests/fixtures/stereo-44100.wav'),trackId:memoryTrack});await idle();
 const memoryClip=(await project()).document.tracks.at(-1).clips[0].clipId,memoryOriginal=await clip(memoryClip);await select(memoryClip);
 await enter('#clip-pitch',12);await enter('#clip-pitch-cents',-1);assert.equal(length(await clip(memoryClip)),length(memoryOriginal));await page.locator('#clip-pitch-reset').click();await idle();assert.deepEqual(await clip(memoryClip),memoryOriginal);await select(id);


 await enter('#clip-stretch',150);assert.equal(length(await clip(id)),720000);assert.equal(await page.locator('#clip-stretch-length').inputValue(),'15.000000000');
 await page.locator('#clip-layer').focus();await page.keyboard.press('Control+z');await idle();assert.deepEqual(await clip(id),original);await page.keyboard.press('Control+Shift+z');await idle();assert.equal(length(await clip(id)),720000);
 await enter('#clip-stretch-length','7.123456789');assert.equal(length(await clip(id)),341926);assert.ok(Math.abs(Number(await page.locator('#clip-stretch-length').inputValue())-341926/48000)<1e-9);
 const stable=await clip(id);await enter('#clip-stretch',0);assert.equal(await page.locator('#clip-stretch').getAttribute('aria-invalid'),'true');assert.deepEqual(await clip(id),stable);await page.locator('#clip-stretch').press('Escape');
 await enter('#clip-stretch',100);assert.equal(length(await clip(id)),480000);
 await enter('#clip-pitch',-5);await enter('#clip-pitch-cents',-37);const pitchBeforeStretch=(await clip(id)).extensions['minidaw.pitchShift.v1'];
 await edit({command:'audio.move',clipIds:[id],anchorClipId:id,targetTick:'7680000'});await select(id);await page.locator('#zoom-fit').click();await sleep(150);
 const sizing=page.locator('#audio-sizing');await sizing.selectOption('stretch');await page.locator('#grid-type').selectOption('beat');
 async function drag(side,dx,on){if((await page.locator('#snap-toggle').getAttribute('aria-pressed')==='true')!==on)await page.locator('#snap-toggle').click();await select(id);await page.locator('#zoom-fit').click();await sleep(150);const c=await clip(id),node=page.locator(`#clip-layer [data-clip-id="${id}"]`),b=await node.boundingBox();const x=side==='left'?b.x+3:b.x+b.width-3,y=b.y+b.height*.65,revision=(await project()).revision;await page.mouse.move(x,y);await page.mouse.down();await page.mouse.move(x+dx,y,{steps:30});assert.equal((await project()).revision,revision,'preview has no edit IPC');await page.mouse.up();await idle();const changed=await clip(id);assert.notEqual(length(changed),length(c));if(side==='left')assert.ok(Math.abs(position(c)+length(c)/48000-position(changed)-length(changed)/48000)<1e-8);else assert.deepEqual(changed.position,c.position);if(on){const edge=position(changed)+(side==='right'?length(changed)/48000:0);assert.ok(Math.abs(edge*2-Math.round(edge*2))<.00005);}return changed;}
 await drag('right',-73,true);await drag('left',61,false);assert.deepEqual((await clip(id)).extensions['minidaw.pitchShift.v1'],pitchBeforeStretch);await sizing.selectOption('trim');const preTrim=await clip(id);await drag('right',-33,false);assert.deepEqual((await clip(id)).extensions,preTrim.extensions);await edit({command:'edit.undo'});assert.deepEqual(await clip(id),preTrim);
 report.checks.push('Numeric ratio/seconds sample rounding, exact Undo/Redo, invalid value, mouse left/right stretch with Snap ON/OFF, pointerup-only commit, Trim mode retained.');
 // Split/move/fade/crossfade/normalize still use the rendered sample coordinates.
 const c=await clip(id),at=position(c)+length(c)/48000*.5;await edit({command:'audio.splitAtCursor',clipIds:[id],cursor:{unit:'seconds',numerator:String(Math.round(at*48000)),denominator:48000}});const parts=(await project()).document.tracks[0].clips;assert.equal(parts.length,2);const left=parts[0].clipId,right=parts[1].clipId;
 await edit({command:'audio.crossfade',clipIds:[left,right]});await edit({command:'audio.gain',clipIds:[left],gainDb:-3});await edit({command:'audio.fade',clipIds:[right],fadeOut:'2000'});await edit({command:'audio.normalize',clipIds:[right],normalizeTargetDb:-12});await edit({command:'audio.muteEvents',clipIds:[left]});await edit({command:'audio.unmuteEvents',clipIds:[left]});
 assert.deepEqual((await clip(right)).extensions['minidaw.pitchShift.v1'],pitchBeforeStretch);
 const saved=(await project()).document,file=join(folder,'pitch.minidaw');await invoke('save_project',{path:file,revision:(await project()).revision});await invoke('new_project',{revision:(await project()).revision,discard:true});await idle();await invoke('open_project',{path:file,revision:(await project()).revision,discard:true});await idle();assert.deepEqual((await project()).document.tracks,saved.tracks);assert.equal((await readFile(file,'utf8')).includes('.f32'),false);
 report.checks.push('Pitch + Stretch Split/Crossfade/Gain/Fade/Normalize/Mute; save/new/open restores recipes without PCM/cache paths.');
 // A longer stretched Streaming voice and Synth through existing effects and
 // sample-clock automation; compare warmed callbacks to an unstretched plan.
 await invoke('new_project',{revision:(await project()).revision,discard:true});await idle();await edit({command:'track.add',trackKind:'audio'});const ta=(await project()).document.tracks.at(-1).trackId;await invoke('load_audio',{path:resolve('tests/generated/long-400s-48000-2ch.wav'),trackId:ta});await idle();const ca=(await project()).document.tracks[0].clips[0].clipId;
 await edit({command:'audio.trim',clipIds:[ca],sourceStart:'0',sourceEnd:'1440000'});
 // Verify that persisted Pitch reaches actual post-Master audio, not only cache
 // metadata. Fine cent accuracy is independently measured from offline PCM.
 await edit({command:'audio.pitch',clipIds:[ca],pitchShift:{semitones:12,cents:-37}});
 await invoke('spectrum_configure',{settings:{enabled:true,fftSize:16384,smoothingMs:0}});await invoke('transport_command',{action:'play'});await sleep(1200);
 const frame=(await invoke('spectrum_snapshot',{after:0})).frame;assert.ok(frame);report.actualOutputHz=frame.curves.slice(0,2).map(curve=>frame.frequencies[curve.indexOf(Math.max(...curve))]);
 for(const[ch,hz]of [227,443].entries())assert.ok(Math.abs(report.actualOutputHz[ch]/(hz*2**(1163/1200))-1)<.015);
 await invoke('transport_command',{action:'stop'});await invoke('spectrum_configure',{settings:{enabled:false,fftSize:4096,smoothingMs:200}});await edit({command:'audio.pitch',clipIds:[ca],pitchShift:{semitones:0,cents:0}});
 report.checks.push('Actual post-Master L/R PCM Spectrum peaks match +12 semitones / -37 cent; independent offline PCM provides cent accuracy.');

 await edit({command:'midi.track.add'});const tm=(await project()).document.tracks.at(-1).trackId;await edit({command:'midi.track.instrument',trackIds:[tm],instrument:'basicSynth'});await edit({command:'midi.clip.add',trackIds:[tm],targetTick:'0',lengthTick:'76800000'});const midi=(await project()).document.tracks.at(-1).clips[0].clipId;await edit({command:'midi.note.add',clipIds:[midi],targetTick:'0',lengthTick:'76800000',pitch:69,velocity:70});
 await edit({command:'effect.add',trackIds:[ta],effectKind:'eq'});await edit({command:'effect.add',trackIds:[],effectKind:'limiter'});await edit({command:'track.mix',trackIds:[ta],volumeDb:-6,pan:-.1});
 await edit({command:'automation.lane.add',trackIds:[ta],parameter:{name:'volumeDb'}});const lane=(await project()).document.automation.find(c=>c.trackId===ta).lanes[0].laneId;for(const[tick,value]of [['0',-6],['76800000',-12]])await edit({command:'automation.point.set',trackIds:[ta],laneId:lane,parameter:{name:'volumeDb'},targetTick:tick,value});await edit({command:'automation.channel',trackIds:[ta],read:true});
 await invoke('transport_command',{action:'play'});report.before=await measure();await invoke('transport_command',{action:'stop'});
 const began=performance.now();await edit({command:'audio.stretch',clipIds:[ca],stretchFrames:'2160000'});await edit({command:'audio.pitch',clipIds:[ca],pitchShift:{semitones:5,cents:37}});assert.equal(length(await clip(ca)),2160000);report.renderTransactionMs=performance.now()-began;await invoke('transport_command',{action:'play'});report.shifted=await measure();report.output=(await audio()).output;assert.equal(report.output.backend,'ASIO');
 for(const r of [report.before,report.shifted]){assert.equal(r.overruns,0);assert.equal(r.deviceOverruns,0);assert.equal(r.starvation,0);assert.equal(r.streamErrors,0);assert.ok(r.peakDb.some(x=>x>-70));}
 const liveBefore=await audio();await edit({command:'audio.pitch',clipIds:[ca],pitchShift:{semitones:-3,cents:-25}});assert.equal(length(await clip(ca)),2160000);const liveAfter=await audio();assert.equal(liveAfter.transport.state,'playing');assert.ok(liveAfter.position>liveBefore.position);report.livePrepare={starvation:liveAfter.source.starvation,overruns:liveAfter.metrics.overruns-liveBefore.metrics.overruns,deviceOverruns:liveAfter.metrics.deviceCallbackOverruns-liveBefore.metrics.deviceCallbackOverruns};assert.equal(report.livePrepare.starvation,0);assert.equal(report.livePrepare.overruns,0);assert.equal(report.livePrepare.deviceOverruns,0);
 await invoke('transport_command',{action:'stop'});await select(ca);await sizing.selectOption('stretch');await page.locator('#zoom-fit').click();await sleep(400);await page.screenshot({path:'docs/validation/pitch-ui.png'});for(const scale of ['90','125','110']){await page.locator('#font-scale').selectOption(scale);await sleep(80);assert.ok(await page.locator('#audio-sizing').evaluate(e=>{const c=document.createElement('canvas').getContext('2d');c.font=getComputedStyle(e).font;return e.clientWidth>=c.measureText(e.selectedOptions[0].textContent).width+25;}));assert.ok(await page.locator('#clip-pitch-cents').evaluate(e=>e.getBoundingClientRect().right<innerWidth));}assert.ok((await page.evaluate(()=>fetch('/third-party-licenses.txt').then(r=>r.text()))).includes('Copyright (c) 2022 Geraint Luff'));
 report.checks.push('Actual TOPPING ASIO playback of Pitch + Stretch Streaming Audio + MIDI Synth + Track/Master Inserts; 5s warmed callback comparison, zero starvation/overrun/stream errors.');
 assert.deepEqual(errors,[]);assert.equal(await page.locator('#error').isVisible(),false);report.passed=true;
}catch(e){report.error=String(e.stack??e);report.pageErrors=errors;process.exitCode=1;if(page){report.uiError=await page.locator('#error-text').textContent().catch(()=>null);report.lastAudio=await audio().catch(()=>null);report.lastProject=await project().catch(()=>null);await page.screenshot({path:'docs/validation/pitch-failure.png'}).catch(()=>{});}}
finally{
 if(page&&prefs)await page.evaluate(p=>{for(const[k,v]of Object.entries(p)){if(v===null)localStorage.removeItem(k);else localStorage.setItem(k,v);}},prefs).catch(()=>{});
 if(browser)await browser.close();if(child?.exitCode===null){const exited=once(child,'exit');child.kill();await exited;}
 if(child){report.remainingTempCaches=(await readdir(process.env.TEMP)).filter(n=>n.startsWith(`minidaw-stretch-${child.pid}-`));if(report.remainingTempCaches.length){report.passed=false;report.error='Temporary cache not removed on process exit';process.exitCode=1;}}
 if(savedRecent)await writeFile(recent,savedRecent);else await unlink(recent).catch(()=>{});
 await writeFile('docs/validation/pitch-ui.json',JSON.stringify(report,null,2));console.log(JSON.stringify({passed:report.passed,error:report.error,before:report.before,shifted:report.shifted,output:report.output,checks:report.checks}));
}
