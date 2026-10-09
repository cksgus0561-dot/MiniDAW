// Real release WebView + TOPPING/WASAPI output. PCM measurements use the existing
// post-Master Spectrum tap, not mocked renderer data or UI playback timers.
import {chromium} from 'playwright-core';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {readFile,writeFile,mkdir,unlink} from 'node:fs/promises';
import {randomUUID,createHash} from 'node:crypto';
import {resolve,join} from 'node:path';
import assert from 'node:assert/strict';
const exe=resolve('src-tauri/target/release/minidaw.exe'),folder=resolve('tests/local/effects',randomUUID());await mkdir(folder,{recursive:true});
const recent=join(process.env.APPDATA,'local.minidaw.desktop/recent-projects.json');let savedRecent;try{savedRecent=await readFile(recent);}catch{}
const report={checks:[],executableSha256:createHash('sha256').update(await readFile(exe)).digest('hex')};
let child,browser,page,prefs;const errors=[],sleep=ms=>new Promise(r=>setTimeout(r,ms));
async function until(fn,label,limit=25000){const at=performance.now();while(performance.now()-at<limit){if(await fn())return;await sleep(40);}throw Error(label);}
const invoke=(name,args={})=>page.evaluate(({name,args})=>window.__TAURI_INTERNALS__.invoke(name,args),{name,args});
const project=()=>invoke('project_snapshot'),audio=()=>invoke('engine_snapshot');
async function idle(){await until(async()=>!await page.locator('#project-save').isDisabled(),'Project idle');await sleep(180);}
async function edit(request){const p=await project();await invoke('edit_project',{revision:p.revision,request});await idle();}
async function panel(id,open){const b=page.locator('#show-'+id);if((await b.getAttribute('aria-pressed')==='true')!==open)await b.click();await sleep(80);}
const strip=id=>page.locator(`.mixer-channel[data-track-id="${id}"]`),slot=id=>page.locator(`.effect-slot[data-effect-id="${id}"]`);
async function chain(id){const p=(await project()).document;return (id==='master'?p.master:p.tracks.find(t=>t.trackId===id))?.inserts??[];}
async function open(id){if(await page.locator('#effect-editor').isVisible())await page.locator('#effect-close').click();await strip(id).locator('.mixer-inserts').click();}
async function add(id,kind){await open(id);const count=(await chain(id)).length;await page.locator('#effect-kind').selectOption(kind);await page.locator('#effect-add').click();await until(async()=>(await chain(id)).length===count+1,'Add '+kind);await idle();const e=(await chain(id)).at(-1);await slot(e.effectId).locator('.effect-select').click();return e.effectId;}
async function param(id,effectId,key,value){await slot(effectId).locator('.effect-select').click();const input=page.locator(`#effect-parameters [data-param="${key}"]`);if(key==='syncBeats')await input.selectOption(String(value));else{await input.fill(String(value));await input.press('Tab');}await until(async()=>{const e=(await chain(id)).find(e=>e.effectId===effectId);if(key.startsWith('band-')){const [,b,k]=key.split('-');return e.bands[Number(b)][k]===value;}return (e[key]??'')===(['wet','feedback'].includes(key)?value/100:key==='syncBeats'?Number(value):value);},'Commit '+key);await idle();}
async function bypass(id,effectId,enabled){const e=(await chain(id)).find(e=>e.effectId===effectId);if(e.enabled!==enabled){await slot(effectId).locator('.effect-bypass').click();await until(async()=>(await chain(id)).find(e=>e.effectId===effectId).enabled===enabled,'Bypass');await idle();}}
async function setEffect(id,effectId,fields){const e=(await chain(id)).find(e=>e.effectId===effectId);await edit({command:'effect.set',trackIds:id==='master'?[]:[id],effectId,effect:{...e,...fields}});}
async function allEnabled(ids,enabled){for(const id of ids)for(const e of await chain(id))await setEffect(id,e.effectId,{enabled});}
async function frame(wait=650){await sleep(wait);const f=(await invoke('spectrum_snapshot',{after:0})).frame;assert.ok(f);return f;}
function peak(f,hz,ch=0){return f.frequencies.map((v,i)=>({hz:v,db:f.curves[ch][i]})).filter(p=>Math.abs(Math.log(p.hz/hz))<.065).sort((a,b)=>b.db-a.db)[0].db;}
async function mix(id,fields){await edit({command:'track.mix',trackIds:[id],...fields});}
async function measure(){await frame(1800);const a=await audio();await sleep(5000);const b=await audio();const n=b.metrics.callbacks-a.metrics.callbacks;return {callbackAvgMs:(b.metrics.callbackAvgMs*b.metrics.callbacks-a.metrics.callbackAvgMs*a.metrics.callbacks)/n,callbackMaxMs:b.metrics.callbackMaxMs,callbacks:n,overruns:b.metrics.overruns-a.metrics.overruns,deviceOverruns:b.metrics.deviceCallbackOverruns-a.metrics.deviceCallbackOverruns,starvation:b.source.starvation-a.source.starvation,streamErrors:b.streamErrors-a.streamErrors,source:b.source,peakDb:b.master.peakDb};}
try{
 child=spawn(exe,[],{env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=19264'},stdio:'ignore',windowsHide:true});
 await until(async()=>{try{browser=await chromium.connectOverCDP('http://127.0.0.1:19264');return true;}catch{return false;}},'Release CDP');
 await until(async()=>{page=browser.contexts()[0].pages().find(p=>p.url().includes('tauri.localhost'));return !!page;},'Page');
 page.on('pageerror',e=>errors.push(e.message));await page.setViewportSize({width:1440,height:960});await idle();
 prefs=await page.evaluate(()=>Object.fromEntries(['minidaw.ui.panels.v1','minidaw.ui.fontScale','minidaw.ui.spectrum.v1'].map(k=>[k,localStorage.getItem(k)])));
 for(const[id,show]of [['arrangement',true],['media',false],['performance',false],['spectrum',false],['piano',false],['mixer',true]])await panel(id,show);
 const ids=[];for(const kind of ['audio','audio','midi','midi']){await page.locator(`#${kind}-track-add`).click();await idle();ids.push((await project()).document.tracks.at(-1).trackId);}
 const [a,b,m,n]=ids;
 await invoke('load_audio',{path:resolve('tests/generated/long-400s-48000-2ch.wav'),trackId:a});await idle();
 await invoke('load_audio',{path:resolve('tests/generated/long-720s-44100-2ch.flac'),trackId:b});await idle();
 for(const[id,pitch]of [[m,76],[n,81]]){await edit({command:'midi.track.instrument',trackIds:[id],instrument:'basicSynth'});await edit({command:'midi.clip.add',trackIds:[id],targetTick:'0',lengthTick:'230400000'});const c=(await project()).document.tracks.find(t=>t.trackId===id).clips[0];await edit({command:'midi.note.add',clipIds:[c.clipId],targetTick:'0',lengthTick:'230400000',pitch,velocity:85});}
 for(const id of ids)await mix(id,{volumeDb:-6});
 await invoke('spectrum_configure',{settings:{enabled:true,fftSize:8192,smoothingMs:0}});
 await mix(a,{solo:true});await invoke('transport_command',{action:'seek',seconds:20});await invoke('transport_command',{action:'play'});
 const dryAudio=peak(await frame(),227);const aq=await add(a,'eq');await param(a,aq,'band-0-frequency',227);await param(a,aq,'band-0-gainDb',6);const wetAudio=peak(await frame(),227);report.audioEqDb=wetAudio-dryAudio;assert.ok(Math.abs(report.audioEqDb-6)<.2);
 await bypass(a,aq,false);assert.ok(Math.abs(peak(await frame(),227)-dryAudio)<.1);await bypass(a,aq,true);
 await page.locator('#effect-close').click();await mix(a,{solo:false});await mix(m,{solo:true});const dryMidi=peak(await frame(),659.255);
 const mq=await add(m,'eq');const before=await audio();await param(m,mq,'band-0-frequency',659);await param(m,mq,'band-0-gainDb',6);report.midiEqDb=peak(await frame(),659.255)-dryMidi;assert.ok(Math.abs(report.midiEqDb-6)<.2);
 const after=await audio();assert.equal(after.transport.appliedCommand,before.transport.appliedCommand);assert.equal(after.transport.clipId,before.transport.clipId);assert.equal(after.source.generation,before.source.generation);assert.equal(after.analysisBuilds,before.analysisBuilds);
 const masterQ=await add('master','eq');await param('master',masterQ,'band-0-frequency',659);const beforeMaster=await audio();const inMaster=peak(await frame(),659.255);await param('master',masterQ,'band-0-gainDb',-6);report.masterEqDb=peak(await frame(),659.255)-inMaster;assert.ok(Math.abs(report.masterEqDb+6)<.2);assert.equal((await audio()).transport.appliedCommand,beforeMaster.transport.appliedCommand);
 await bypass('master',masterQ,false);
 const comp=await add('master','compressor');await param('master',comp,'thresholdDb',-36);await param('master',comp,'ratio',4);await param('master',comp,'attackMs',5);await frame();report.masterGR=(await audio()).effects.find(e=>e.effectId===comp).reductionDb;assert.ok(report.masterGR>5);assert.match(await page.locator('#effect-reduction').textContent(),/Gain Reduction/);
 const compressed=peak(await frame(),659.255);await param('master',comp,'makeupDb',3);report.makeupDb=peak(await frame(),659.255)-compressed;assert.ok(Math.abs(report.makeupDb-3)<.2);await bypass('master',comp,false);
 const limit=await add('master','limiter');await param('master',limit,'ceilingDb',-12);await param('master',limit,'inputDb',24);await frame(1800);report.limiterPeakDb=(await audio()).master.peakDb;assert.ok(report.limiterPeakDb.every(v=>v<=-11.99&&v>-13));
 // A boost AFTER Limiter raises the output; moving Limiter last restores its ceiling.
 await setEffect('master',masterQ,{enabled:true,bands:[{frequency:659,gainDb:12,q:.707},{frequency:1000,gainDb:0,q:.707},{frequency:8000,gainDb:0,q:.707}]});
 await slot(masterQ).locator('.effect-down').click();await idle();await slot(masterQ).locator('.effect-down').click();await idle();assert.equal((await chain('master')).at(-1).effectId,masterQ);await frame(1600);assert.ok((await audio()).master.peakDb[0]>-3);
 await slot(limit).locator('.effect-down').click();await idle();assert.equal((await chain('master')).at(-1).effectId,limit);await frame(1800);assert.ok((await audio()).master.peakDb[0]<=-11.99);
 await edit({command:'edit.undo'});assert.equal((await chain('master')).at(-1).effectId,masterQ);await edit({command:'edit.redo'});assert.equal((await chain('master')).at(-1).effectId,limit);
 report.checks.push('Native Insert controls: Audio EQ +6dB, Synth EQ +6dB, Master EQ -6dB, exact Bypass; Compressor GR/Makeup; last-slot Limiter -12dBFS; audible chain-order change and Undo/Redo. MIDI/Master parameter edits preserve source, sample clock and waveform cache.');
 // UI for every processor, on all channel types; worker timing vs actual audio.
 for(const id of [a,m,'master']){
   for(const kind of ['eq','compressor','reverb','delay','limiter'])if(!(await chain(id)).some(e=>e.kind===kind))await add(id,kind);
   await open(id);const es=await chain(id);const r=es.find(e=>e.kind==='reverb'),d=es.find(e=>e.kind==='delay');
   await param(id,r.effectId,'decay',2.4);await param(id,r.effectId,'wet',25);await param(id,d.effectId,'feedback',40);await param(id,d.effectId,'wet',30);await param(id,d.effectId,'syncBeats','0.5');
   assert.match(await page.locator('#effect-parameters output').textContent(),/250\.0 ms/);
   const l=(await chain(id)).find(e=>e.kind==='limiter');while((await chain(id)).at(-1).effectId!==l.effectId){await slot(l.effectId).locator('.effect-down').click();await idle();}
 }
 await page.locator('#effect-close').click();for(const id of ids)await mix(id,{solo:false,mute:false});
 // Ensure native streaming-backed GR corresponds to the current audible frame.
 const ac=(await chain(a)).find(e=>e.kind==='compressor');await setEffect(a,ac.effectId,{thresholdDb:-48,ratio:4});await frame();report.audioGR=(await audio()).effects.find(e=>e.effectId===ac.effectId).reductionDb;assert.ok(report.audioGR>5);
 await allEnabled([a,m,'master'],false);await invoke('transport_command',{action:'seek',seconds:20});report.performanceOff=await measure();
 await allEnabled([a,m,'master'],true);await invoke('transport_command',{action:'seek',seconds:20});report.performanceOn=await measure();report.output=(await audio()).output;
 for(const x of [report.performanceOff,report.performanceOn]){assert.equal(x.overruns,0);assert.equal(x.deviceOverruns,0);assert.equal(x.starvation,0);assert.equal(x.streamErrors,0);}
 assert.equal((await audio()).transport.state,'playing');
 report.checks.push('EQ/Compressor/Limiter/Reverb/Delay on Audio, MIDI and Master; Tempo-synced Delay UI; Audio GR synchronized with read-ahead timeline; two Streaming sources + two Synths play with 15 enabled Inserts. ON/OFF warmed 5-second callback measurements, no starvation or overrun.');
 await open('master');await slot(comp).locator('.effect-select').click();await frame();
 for(const scale of ['90','110','125']){await page.locator('#effect-close').click();await page.locator('#font-scale').selectOption(scale);await open('master');assert.ok(await page.locator('#effect-editor').evaluate(e=>{const b=e.getBoundingClientRect();return b.left>=0&&b.right<=innerWidth&&b.top>=0&&b.bottom<=innerHeight;}));}
 await page.setViewportSize({width:1100,height:760});await sleep(200);assert.ok((await page.locator('#effect-editor').boundingBox()).height<760);await page.setViewportSize({width:1440,height:960});await page.locator('#effect-close').click();await page.locator('#font-scale').selectOption('110');await open('master');await slot(comp).locator('.effect-select').click();await frame();await page.screenshot({path:'docs/validation/effects-ui.png'});
 await page.locator('#effect-close').click();await panel('mixer',false);assert.equal(await page.locator('#panel-mixer').isVisible(),false);await panel('mixer',true);await open(a);await slot(aq).locator('.effect-select').click();await page.screenshot({path:'docs/validation/effects-eq-ui.png'});
 await page.locator('#effect-close').click();await invoke('transport_command',{action:'stop'});await sleep(200);
 // Delete a processor, undo restores exact id/parameters/order, then persistent round-trip.
 const exact=await chain(a);await open(a);await slot(aq).locator('.effect-remove').click();await idle();assert.equal((await chain(a)).length,exact.length-1);await edit({command:'edit.undo'});assert.deepEqual(await chain(a),exact);
 await page.locator('#effect-close').click();const p=await project(),file=join(folder,'effects.minidaw');await invoke('save_project',{path:file,revision:p.revision});const saved=JSON.parse(await readFile(file,'utf8'));
 await invoke('new_project',{revision:(await project()).revision,discard:true});await idle();assert.equal((await chain('master')).length,0);
 await invoke('open_project',{path:file,revision:(await project()).revision,discard:true});await idle();assert.deepEqual((await project()).document.master,saved.master);assert.deepEqual((await project()).document.tracks,saved.tracks);
 await invoke('transport_command',{action:'play'});await frame();assert.equal((await audio()).transport.state,'playing');await invoke('transport_command',{action:'stop'});
 report.checks.push('Processor delete/undo restores exact values/order; project save/new/open restores all Track and Master Inserts and actual playback; 90–125% Font Scale, window resize and existing Mixer show/hide.');
 // Short source note has ended (including the existing 40ms Synth release).
 // Only insert tails can produce measured output at 0.8s.
 await allEnabled([a,m,'master'],false);await mix(m,{solo:true});
 const part=(await project()).document.tracks.find(t=>t.trackId===m).clips[0],note=part.notes[0];
 await edit({command:'midi.note.change',clipIds:[part.clipId],noteId:note.noteId,targetTick:'0',lengthTick:'192000',pitch:note.pitch});
 async function burst(){await invoke('transport_command',{action:'stop'});await invoke('transport_command',{action:'play'});const f=await frame(840);await invoke('transport_command',{action:'stop'});return Math.max(...f.curves.flat());}
 report.dryAfterNoteDb=await burst();assert.ok(report.dryAfterNoteDb<-100);report.tails=[];
 for(const id of [m,'master'])for(const kind of ['delay','reverb']){const e=(await chain(id)).find(e=>e.kind===kind);await setEffect(id,e.effectId,{enabled:true,wet:1});const db=await burst();report.tails.push({channel:id===m?'Synth':'Master',kind,db});assert.ok(db>-90);await setEffect(id,e.effectId,{enabled:false});}
 report.checks.push('Actual post-Master PCM: 100ms MIDI note is silent at 0.8s with Bypass; separate Synth/Master Reverb and Delay retain measurable tails at that time.');
 assert.deepEqual(errors,[]);assert.equal(await page.locator('#error').isVisible(),false);report.passed=true;
}catch(e){report.error=String(e.stack??e);report.pageErrors=errors;process.exitCode=1;if(page){report.uiError=await page.locator('#error-text').textContent().catch(()=>null);report.lastAudio=await audio().catch(()=>null);report.lastProject=await project().catch(()=>null);await page.screenshot({path:'docs/validation/effects-failure.png'}).catch(()=>{});}}
finally{
 if(page&&prefs)await page.evaluate(p=>{for(const[k,v]of Object.entries(p)){if(v===null)localStorage.removeItem(k);else localStorage.setItem(k,v);}},prefs).catch(()=>{});
 if(browser)await browser.close();if(child?.exitCode===null){const exited=once(child,'exit');child.kill();await exited;}
 if(savedRecent)await writeFile(recent,savedRecent);else await unlink(recent).catch(()=>{});
 await writeFile('docs/validation/effects-ui.json',JSON.stringify(report,null,2));console.log(JSON.stringify({passed:report.passed,error:report.error,checks:report.checks,performanceOff:report.performanceOff,performanceOn:report.performanceOn,audioEqDb:report.audioEqDb,midiEqDb:report.midiEqDb,masterEqDb:report.masterEqDb,limiterPeakDb:report.limiterPeakDb}));
}
