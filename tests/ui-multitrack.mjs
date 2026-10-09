// Native release WebView + real output device. No browser transport mocks.
import {chromium} from 'playwright-core';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {readFile,writeFile,mkdir,unlink} from 'node:fs/promises';
import {randomUUID,createHash} from 'node:crypto';
import {resolve,join} from 'node:path';
import assert from 'node:assert/strict';
const exe=resolve('src-tauri/target/release/minidaw.exe'),folder=resolve('tests/local/multitrack',randomUUID());await mkdir(folder,{recursive:true});
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
async function settings(id){await header(id).locator('.track-settings-open').click();await sleep(60);assert.ok(await page.locator('#track-settings').isVisible());}
async function add(kind){await page.locator(`#${kind}-track-add`).click();await idle();return (await project()).document.tracks.at(-1).trackId;}
async function drag(id,track,{dx=0,ctrl=false}={}){
 const b=await clip(id).boundingBox(),h=await header(track).boundingBox(),rev=(await project()).revision;
 const x=b.x+Math.min(b.width*.6,90),y=b.y+b.height*.7;
 if(ctrl)await page.keyboard.down('Control');await page.mouse.move(x,y);await page.mouse.down();await page.mouse.move(x+dx,h.y+Math.min(h.height*.6,60),{steps:30});
 assert.equal((await project()).revision,rev,'drag preview never commits');await page.mouse.up();if(ctrl)await page.keyboard.up('Control');await idle();
}
function find(p,id){return p.document.tracks.flatMap(t=>t.clips).find(c=>c.clipId===id);}
async function setMix(id,fields){await edit({command:'track.mix',trackIds:[id],...fields});}
function peak(f,hz,ch=0){return f.frequencies.map((v,i)=>({hz:v,db:f.curves[ch][i]})).filter(p=>Math.abs(Math.log(p.hz/hz))<.065).sort((a,b)=>b.db-a.db)[0].db;}
async function frame(){await sleep(550);return (await invoke('spectrum_snapshot',{after:0})).frame;}
async function start(seconds=20){await invoke('transport_command',{action:'seek',seconds});await invoke('transport_command',{action:'play'});return frame();}
try{
 child=spawn(exe,[],{env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=19261'},stdio:'ignore',windowsHide:true});
 await until(async()=>{try{browser=await chromium.connectOverCDP('http://127.0.0.1:19261');return true;}catch{return false;}},'Release CDP');
 await until(async()=>{page=browser.contexts()[0].pages().find(p=>p.url().includes('tauri.localhost'));return !!page;},'Page');
 page.on('pageerror',e=>errors.push(e.message));await page.setViewportSize({width:1560,height:1100});await idle();
 prefs=await page.evaluate(()=>Object.fromEntries(['minidaw.ui.panels.v1','minidaw.ui.fontScale','minidaw.ui.spectrum.v1'].map(k=>[k,localStorage.getItem(k)])));
 for(const [id,open]of [['arrangement',true],['media',false],['performance',false],['spectrum',false],['piano',false]])await panel(id,open);
 const a=await add('audio');await invoke('load_audio',{path:resolve('tests/fixtures/stereo-44100.wav'),trackId:a});await idle();
 const b=await add('audio');await invoke('load_audio',{path:resolve('tests/fixtures/stereo-44100.mp3'),trackId:b});await idle();
 const m=await add('midi');await header(m).locator('.midi-clip-add').click();await idle();await panel('piano',false);
 const n=await add('midi');await header(n).locator('.midi-clip-add').click();await idle();await panel('piano',false);
 let p=await project();assert.deepEqual(p.document.tracks.map(t=>t.kind),['audio','audio','midi','midi']);
 const ac=p.document.tracks[0].clips[0].clipId,mc=p.document.tracks[2].clips[0].clipId;
 await edit({command:'midi.note.add',clipIds:[mc],targetTick:'1234567',lengthTick:'123457',pitch:64});
 await page.locator('#zoom-fit').click();await sleep(100);
 const originalAudio=find(await project(),ac);await drag(ac,b,{dx:12,ctrl:true});p=await project();assert.deepEqual(p.document.tracks[1].clips.find(c=>c.clipId===ac),originalAudio);
 await edit({command:'edit.undo'});assert.deepEqual((await project()).document.tracks[0].clips[0],originalAudio);await edit({command:'edit.redo'});assert.deepEqual((await project()).document.tracks[1].clips.find(c=>c.clipId===ac),originalAudio);await edit({command:'edit.undo'});
 const originalMidi=find(await project(),mc);await drag(mc,n,{ctrl:true,dx:9});assert.deepEqual((await project()).document.tracks[3].clips.find(c=>c.clipId===mc),originalMidi);
 await edit({command:'edit.undo'});await edit({command:'edit.redo'});assert.deepEqual(find(await project(),mc),originalMidi);await edit({command:'edit.undo'});
 // Incompatible Audio -> MIDI drop is cancelled as a whole.
 const rev=(await project()).revision;await drag(ac,m);assert.equal((await project()).revision,rev);
 await page.locator('#grid-type').selectOption('8');if(await page.locator('#snap-toggle').getAttribute('aria-pressed')!=='true')await page.locator('#snap-toggle').click();
 const step=Number(await page.locator('#scroll').getAttribute('step'));await drag(mc,n,{dx:.25/step});let moved=find(await project(),mc);assert.equal(moved.startTick,'480000');assert.deepEqual(moved.notes,originalMidi.notes);
 await edit({command:'edit.undo'});if(await page.locator('#snap-toggle').getAttribute('aria-pressed')==='true')await page.locator('#snap-toggle').click();
 await drag(mc,n,{dx:.113/step});moved=find(await project(),mc);assert.notEqual(Number(moved.startTick)%480000,0);assert.deepEqual(moved.notes,originalMidi.notes);await edit({command:'edit.undo'});
 // Exact vertical transfer is also available without pointer precision.
 await clip(ac).click({position:{x:60,y:30}});await settings(b);await page.locator('#track-transfer').click();await idle();assert.deepEqual((await project()).document.tracks[1].clips.find(c=>c.clipId===ac),originalAudio);await edit({command:'edit.undo'});
 report.checks.push('Native Audio/MIDI Track creation; Ctrl vertical drag preserves all positions; diagonal Grid 1/8 and free-tick move; wrong-kind rejection; popup transfer; Undo/Redo.');
 await settings(b);await page.locator('#track-up').click();await idle();assert.equal((await project()).document.tracks[0].trackId,b);await page.locator('#track-down').click();await idle();assert.equal((await project()).document.tracks[1].trackId,b);
 await page.locator('#track-volume').fill('-6');await page.locator('#track-volume').press('Tab');await idle();await page.locator('#track-pan').press('End');await idle();
 assert.equal((await project()).document.tracks[1].mix.volumeDb,-6);assert.equal((await project()).document.tracks[1].mix.pan,1);await page.keyboard.press('Escape');
 await header(a).locator('.track-mute').click();await idle();assert.equal((await project()).document.tracks[0].mix.mute,true);await header(a).locator('.track-mute').click();await idle();
 await header(m).locator('.track-solo').click();await idle();assert.equal((await project()).document.tracks[2].mix.solo,true);await header(m).locator('.track-solo').click();await idle();
 const temp=await add('audio');await settings(temp);await page.locator('#track-delete').click();await idle();assert.equal((await project()).document.tracks.length,4);await edit({command:'edit.undo'});assert.equal((await project()).document.tracks.at(-1).trackId,temp);await edit({command:'edit.redo'});
 for(const scale of ['90','110','125']){await page.locator('#font-scale').selectOption(scale);await sleep(100);for(const id of [m,n]){const gap=await header(id).evaluate(e=>e.querySelector('.track-resize').getBoundingClientRect().top-e.querySelector('.midi-clip-add').getBoundingClientRect().bottom);assert.ok(gap>=0,'MIDI add button above resize boundary');}}
 await page.screenshot({path:'docs/validation/multitrack-ui.png'});
 p=await project();const path=join(folder,'multitrack.minidaw');await invoke('save_project',{path,revision:p.revision});await idle();const saved=(await project()).document;
 await invoke('open_project',{path,revision:(await project()).revision,discard:true});await idle();assert.deepEqual((await project()).document,saved);
 report.checks.push('Track menu reorder/delete, header M/S, Volume/Pan, history/save/open, and MIDI button/resize separation at 90/110/125% font scale.');
 // Dedicated playback scene: two large independently file-backed WAV sources + two instruments.
 await invoke('new_project',{revision:(await project()).revision,discard:true});await idle();
 const aa=await add('audio'),bb=await add('audio');
 await invoke('load_audio',{path:resolve('tests/generated/long-400s-48000-2ch.wav'),trackId:aa});await idle();
 await invoke('load_audio',{path:resolve('tests/generated/long-1200s-48000-2ch.wav'),trackId:bb});await idle();
 const mm=await add('midi'),nn=await add('midi');
 for(const[id,pitch]of [[mm,69],[nn,76]]){
  await edit({command:'midi.track.instrument',trackIds:[id],instrument:'basicSynth'});
  await edit({command:'midi.clip.add',trackIds:[id],targetTick:'0',lengthTick:'115200000'});
  const c=(await project()).document.tracks.find(t=>t.trackId===id).clips[0];await edit({command:'midi.note.add',clipIds:[c.clipId],targetTick:'0',lengthTick:'115200000',pitch,velocity:85});
 }
 await invoke('spectrum_configure',{settings:{enabled:true,fftSize:8192,smoothingMs:0}});
 let f=await start();let before=await audio();assert.equal(before.source.mode,'Streaming');assert.equal(before.source.pcmResidentBytes,0);assert.equal(before.source.bufferBytes,2097152);assert.ok(peak(f,227)>-50);assert.ok(peak(f,440)>-40);assert.ok(peak(f,659.255)>-40);report.output=before.output;
 await sleep(1500);let after=await audio();assert.equal(after.transport.state,'playing');assert.ok(after.position>before.position+1);assert.equal(after.source.starvation,0);assert.equal(after.streamErrors,0);report.streaming={before:before.source,after:after.source,metrics:after.metrics};
 await setMix(mm,{solo:true});f=await start();const solo=peak(f,440);assert.ok(solo>-40);assert.ok(peak(f,659.255)<-75);assert.ok(peak(f,227)<-75);
 await setMix(nn,{solo:true});f=await start();assert.ok(peak(f,440)>-40);assert.ok(peak(f,659.255)>-40);
 await setMix(nn,{mute:true});f=await start();assert.ok(peak(f,659.255)<-75);
 await setMix(mm,{volumeDb:-6,pan:-1});f=await start();const quiet=peak(f,440);assert.ok(Math.abs(quiet-solo+6)<.25,`Synth -6dB: ${quiet-solo}`);assert.ok(Math.max(...f.curves[1])<-100);report.synthGainDeltaDb=quiet-solo;
 await setMix(aa,{solo:true});await setMix(mm,{solo:false});await setMix(nn,{solo:false});f=await start();const audioLevel=peak(f,227);assert.ok(audioLevel>-50);assert.ok(peak(f,659.255)<-75);
 await setMix(aa,{volumeDb:-6,pan:-1});f=await start();const audioQuiet=peak(f,227);assert.ok(Math.abs(audioQuiet-audioLevel+6)<.25,`Audio -6dB: ${audioQuiet-audioLevel}`);assert.ok(Math.max(...f.curves[1])<-100);report.audioGainDeltaDb=audioQuiet-audioLevel;
 for(const id of [aa,bb,mm,nn])await setMix(id,{mute:false,solo:false,volumeDb:-6,pan:0});
 await edit({command:'project.cycle',cycle:{enabled:true,startTick:'38400000',endTick:'40320000'}});
 await start();before=await audio();await sleep(2600);after=await audio();assert.ok(after.position>=20&&after.position<21);assert.equal(after.source.starvation,0);assert.equal(after.streamErrors,0);assert.equal(after.metrics.overruns,before.metrics.overruns);assert.equal(after.metrics.deviceCallbackOverruns,before.metrics.deviceCallbackOverruns);assert.equal(after.analysisBuilds,before.analysisBuilds);
 report.cycle={before:before.metrics,after:after.metrics,position:after.position,source:after.source};
 await invoke('transport_command',{action:'pause'});assert.equal((await audio()).transport.state,'paused');await invoke('transport_command',{action:'play'});await sleep(150);assert.equal((await audio()).transport.state,'playing');await invoke('transport_command',{action:'stop'});await sleep(150);assert.equal((await audio()).position,0);
 report.checks.push('Real output: two large WAV streams + two Synth tracks simultaneously; 2MiB fixed mixed ring/0 resident PCM; global multi-Solo/Mute; Audio and Synth -6dB and hard-left verified via Master PCM Spectrum; Cycle/Pause/Resume/Stop without starvation/stream error or new callback overruns.');
 assert.deepEqual(errors,[]);assert.equal(await page.locator('#error').isVisible(),false);report.passed=true;
}catch(e){report.error=String(e.stack??e);report.pageErrors=errors;process.exitCode=1;if(page){report.uiError=await page.locator('#error-text').textContent().catch(()=>null);report.lastAudio=await audio().catch(()=>null);report.lastProject=await project().catch(()=>null);await page.screenshot({path:'docs/validation/multitrack-failure.png'}).catch(()=>{});}}
finally{
 if(page&&prefs)await page.evaluate(p=>{for(const[k,v]of Object.entries(p)){if(v===null)localStorage.removeItem(k);else localStorage.setItem(k,v);}},prefs).catch(()=>{});
 if(browser)await browser.close();if(child?.exitCode===null){const exited=once(child,'exit');child.kill();await exited;}
 if(savedRecent)await writeFile(recent,savedRecent);else await unlink(recent).catch(()=>{});
 await writeFile('docs/validation/multitrack-ui.json',JSON.stringify(report,null,2));console.log(JSON.stringify({passed:report.passed,error:report.error,checks:report.checks,output:report.output,synthGainDeltaDb:report.synthGainDeltaDb,audioGainDeltaDb:report.audioGainDeltaDb}));
}
