// Native release WebView, actual output device and post-Master Spectrum tap.
import {chromium} from 'playwright-core';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {readFile,writeFile,mkdir,unlink} from 'node:fs/promises';
import {randomUUID,createHash} from 'node:crypto';
import {resolve,join} from 'node:path';
import assert from 'node:assert/strict';
const exe=resolve('src-tauri/target/release/minidaw.exe'),folder=resolve('tests/local/synth',randomUUID());await mkdir(folder,{recursive:true});
const recent=join(process.env.APPDATA,'local.minidaw.desktop/recent-projects.json');let savedRecent;try{savedRecent=await readFile(recent);}catch{}
const report={checks:[],executableSha256:createHash('sha256').update(await readFile(exe)).digest('hex')};
let child,browser,page,prefs;const errors=[],sleep=ms=>new Promise(r=>setTimeout(r,ms));
async function until(fn,label,limit=20000){const at=performance.now();while(performance.now()-at<limit){if(await fn())return;await sleep(25);}throw Error(label);}
const invoke=(name,args={})=>page.evaluate(({name,args})=>window.__TAURI_INTERNALS__.invoke(name,args),{name,args});
const project=()=>invoke('project_snapshot'),audio=()=>invoke('engine_snapshot'),spec=()=>invoke('spectrum_snapshot',{after:0});
async function idle(){await until(async()=>!await page.locator('#project-save').isDisabled(),'Project idle');await sleep(160);}
async function edit(request){const p=await project();await invoke('edit_project',{revision:p.revision,request});await idle();}
async function panel(id,open){const b=page.locator('#show-'+id);if((await b.getAttribute('aria-pressed')==='true')!==open)await b.click();await sleep(100);}
async function position(seconds){await until(async()=>{const s=await audio();return s.transport.state==='playing'&&s.position>=seconds;},'Transport at '+seconds,6000);}
function peak(f,hz,ch=0){return f.frequencies.map((v,i)=>({hz:v,db:f.curves[ch][i]})).filter(p=>Math.abs(Math.log(p.hz/hz))<.08).sort((a,b)=>b.db-a.db)[0];}
async function frame(){await until(async()=>!!(await spec()).frame,'Spectrum');return (await spec()).frame;}
try{
 child=spawn(exe,[],{env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=19256'},stdio:'ignore',windowsHide:true});
 await until(async()=>{try{browser=await chromium.connectOverCDP('http://127.0.0.1:19256');return true;}catch{return false;}},'Release CDP');
 await until(async()=>{page=browser.contexts()[0].pages().find(p=>p.url().includes('tauri.localhost'));return !!page;},'Release page');
 page.on('pageerror',e=>errors.push(e.message));await page.setViewportSize({width:1560,height:1000});await idle();
 prefs=await page.evaluate(()=>Object.fromEntries(['minidaw.ui.panels.v1','minidaw.ui.fontScale','minidaw.ui.spectrum.v1'].map(k=>[k,localStorage.getItem(k)])));
 await panel('arrangement',true);await panel('media',false);await panel('performance',false);await panel('spectrum',false);await panel('piano',false);
 await page.locator('#midi-track-add').click();await idle();const track=(await project()).document.tracks[0].trackId;
 const selector=page.locator(`[data-track-id="${track}"] .midi-instrument`);assert.equal(await selector.inputValue(),'none');
 await selector.selectOption('basicSynth');await idle();assert.equal((await project()).document.tracks[0].instrument,'basicSynth');
 await edit({command:'edit.undo'});assert.equal(await selector.inputValue(),'none');await edit({command:'edit.redo'});assert.equal(await selector.inputValue(),'basicSynth');
 await page.locator('.midi-clip-add').click();await idle();assert.equal(await selector.isDisabled(),false);
 const clip=(await project()).document.tracks[0].clips[0].clipId;
 await edit({command:'midi.note.add',clipIds:[clip],targetTick:'0',lengthTick:'1920000',pitch:69,velocity:100});
 for(const [tick,data]of [['0',{kind:'cc',controller:64,value:127}],['3840000',{kind:'pitchBend',value:8191}],['5760000',{kind:'cc',controller:64,value:0}]]){
   await edit({command:'midi.control.put',clipIds:[clip],event:{eventId:'',tick,channel:0,data}});
 }
 // Short loop: note-off at 1s, pedal holds to 3s, bend +2 semitones at 2s.
 await edit({command:'project.cycle',cycle:{enabled:true,startTick:'0',endTick:'7680000'}});
 const saved=await project(),path=join(folder,'synth.minidaw');await invoke('save_project',{path,revision:saved.revision});await idle();
 await invoke('open_project',{path,revision:(await project()).revision,discard:true});await idle();assert.deepEqual((await project()).document.tracks,saved.document.tracks);
 await page.locator('.midi-clip').first().dblclick();await idle();assert.ok(await page.locator('#piano-canvas').isVisible());
 report.checks.push('Native Track instrument selector, Undo/Redo, project save/open; selected Piano Roll Part retains notes/controllers.');
 await panel('spectrum',true);await page.locator('#spectrum-fft').selectOption('8192');await page.locator('#spectrum-smoothing').selectOption('0');
 const before=await audio();report.output=before.output;assert.ok(report.output);assert.equal(before.outputError,null);
 await page.locator('#play-pause').click();await position(.45);const initial=await frame();assert.ok(peak(initial,440).db>-35);report.initialPeak=peak(initial,440);
 await position(1.5);const held=await frame();assert.ok(peak(held,440).db>-35);report.heldPeak=peak(held,440);
 await position(2.5);const bent=await frame();assert.ok(peak(bent,493.883).db>-35);report.bentPeak=peak(bent,493.883);
 await page.screenshot({path:'docs/validation/synth-ui.png'});
 await position(3.65);const released=await frame();assert.ok(Math.max(...released.curves[0])<-90);report.releasedMaxDb=Math.max(...released.curves[0]);
 await until(async()=>(await audio()).position<.6,'Cycle wrap',2000);await position(.45);assert.ok(peak(await frame(),440).db>-35);
 await page.locator('#play-pause').click();await sleep(400);assert.equal((await audio()).transport.state,'paused');assert.ok(Math.max(...(await frame()).curves[0])<-90);
 await page.locator('#play-pause').click();await sleep(400);assert.equal((await audio()).transport.state,'playing');assert.ok(peak(await frame(),440).db>-35);
 await invoke('transport_command',{action:'seek',seconds:2.5});await sleep(400);assert.ok(peak(await frame(),493.883).db>-35,'Seek chases pedal-held Note + Bend');
 await page.locator('#stop').click();await sleep(400);assert.equal((await audio()).position,0);assert.ok(Math.max(...(await frame()).curves[0])<-90);
 report.checks.push('Real device Master PCM: 440Hz note, Note Off held by Sustain, 493.883Hz bend, pedal release silence; Cycle, Pause/Resume, Seek chase and Stop.');
 const after=await audio();report.interval={before:before.metrics,after:after.metrics,streamErrors:after.streamErrors-before.streamErrors};
 assert.equal(after.streamErrors,before.streamErrors);assert.equal(after.metrics.overruns,before.metrics.overruns);assert.equal(after.metrics.deviceCallbackOverruns,before.metrics.deviceCallbackOverruns);
 await selector.selectOption('none');await idle();await page.locator('#play-pause').click();await sleep(450);assert.ok(Math.max(...(await frame()).curves[0])<-90);await page.locator('#stop').click();
 await selector.selectOption('basicSynth');await idle();
 // Existing Audio is mixed into the same Master; fixture contains its own tones.
 await invoke('load_audio',{path:resolve('tests/fixtures/stereo-44100.wav')});await idle();assert.ok((await project()).document.tracks.some(t=>t.kind==='audio'));
 await page.locator('#play-pause').click();await sleep(450);assert.equal((await audio()).transport.state,'playing');assert.ok((await audio()).metrics.nonSilentFrames>after.metrics.nonSilentFrames);await page.locator('#stop').click();
 report.checks.push('Instrument None silences the MIDI Track; mixed Audio+Synth playback advances with non-silent Master output. No callback/device overruns or stream errors during the Synth transport check.');
 for(const scale of ['90','110','125']){await page.locator('#font-scale').selectOption(scale);await sleep(150);assert.ok(await selector.isVisible());const layout=await selector.evaluate(e=>{const p=e.closest('.track-header'),b=e.getBoundingClientRect(),r=p.getBoundingClientRect();return{inside:b.bottom<=r.bottom&&b.right<=r.right,overflow:document.documentElement.scrollWidth>innerWidth+1};});assert.equal(layout.inside,true);assert.equal(layout.overflow,false);}
 assert.deepEqual(errors,[]);assert.equal(await page.locator('#error').isVisible(),false);report.passed=true;
}catch(e){report.error=String(e.stack??e);report.pageErrors=errors;process.exitCode=1;if(page){report.uiError=await page.locator('#error-text').textContent().catch(()=>null);report.lastAudio=await audio().catch(()=>null);await page.screenshot({path:'docs/validation/synth-failure.png'}).catch(()=>{});}}
finally{
 if(page&&prefs)await page.evaluate(p=>{for(const[k,v]of Object.entries(p)){if(v===null)localStorage.removeItem(k);else localStorage.setItem(k,v);}},prefs).catch(()=>{});
 if(browser)await browser.close();if(child?.exitCode===null){const exited=once(child,'exit');child.kill();await exited;}
 if(savedRecent)await writeFile(recent,savedRecent);else await unlink(recent).catch(()=>{});
 await writeFile('docs/validation/synth-ui.json',JSON.stringify(report,null,2));console.log(JSON.stringify(report));
}
