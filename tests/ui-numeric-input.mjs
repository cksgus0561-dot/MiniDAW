// Real release WebView + TOPPING/WASAPI output. PCM measurements use the existing
// post-Master Spectrum tap, not mocked renderer data or UI playback timers.
import {chromium} from 'playwright-core';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {readFile,writeFile,mkdir,unlink} from 'node:fs/promises';
import {randomUUID,createHash} from 'node:crypto';
import {resolve,join} from 'node:path';
import assert from 'node:assert/strict';
const exe=resolve('src-tauri/target/release/minidaw.exe'),folder=resolve('tests/local/numeric-input',randomUUID());await mkdir(folder,{recursive:true});
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
async function chain(id){const p=(await project()).document;return (id==='master'?p.master:p.tracks.find(t=>t.trackId===id))?.inserts??[];}
async function open(id){if(await page.locator('#effect-editor').isVisible())await page.locator('#effect-close').click();await strip(id).locator('.mixer-inserts').click();}
async function add(id,kind){await open(id);const count=(await chain(id)).length;await page.locator('#effect-kind').selectOption(kind);await page.locator('#effect-add').click();await until(async()=>(await chain(id)).length===count+1,'Add '+kind);await idle();const e=(await chain(id)).at(-1);await slot(e.effectId).locator('.effect-select').click();return e.effectId;}
async function bypass(id,effectId,enabled){const e=(await chain(id)).find(e=>e.effectId===effectId);if(e.enabled!==enabled){await slot(effectId).locator('.effect-bypass').click();await until(async()=>(await chain(id)).find(e=>e.effectId===effectId).enabled===enabled,'Bypass');await idle();}}
async function frame(wait=650){await sleep(wait);const f=(await invoke('spectrum_snapshot',{after:0})).frame;assert.ok(f);return f;}
function peak(f,hz,ch=0){return f.frequencies.map((v,i)=>({hz:v,db:f.curves[ch][i]})).filter(p=>Math.abs(Math.log(p.hz/hz))<.065).sort((a,b)=>b.db-a.db)[0].db;}
async function mix(id,fields){await edit({command:'track.mix',trackIds:[id],...fields});}
try{
 child=spawn(exe,[],{env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=19266'},stdio:'ignore',windowsHide:true});
 await until(async()=>{try{browser=await chromium.connectOverCDP('http://127.0.0.1:19266');return true;}catch{return false;}},'Release CDP');
 await until(async()=>{page=browser.contexts()[0].pages().find(p=>p.url().includes('tauri.localhost'));return !!page;},'Page');
 page.on('pageerror',e=>errors.push(e.message));await page.setViewportSize({width:1440,height:960});await idle();
 prefs=await page.evaluate(()=>Object.fromEntries(['minidaw.ui.panels.v1','minidaw.ui.fontScale','minidaw.ui.spectrum.v1'].map(k=>[k,localStorage.getItem(k)])));
 for(const[id,show]of [['arrangement',true],['media',false],['performance',false],['spectrum',false],['piano',false],['mixer',true]])await panel(id,show);
 await invoke('new_project',{revision:(await project()).revision,discard:true});await idle();
 await edit({command:'track.add',trackKind:'audio'});const a=(await project()).document.tracks.at(-1).trackId;
 await edit({command:'midi.track.add'});const m=(await project()).document.tracks.at(-1).trackId;
 const track=async id=>(await project()).document.tracks.find(t=>t.trackId===id);
 const put=async(input,value,key='Enter')=>{await input.fill(String(value));await input.press(key);await idle();};
 const pan=id=>strip(id).locator('.mixer-pan-value');
 for(const id of [a,m]){
   const history=(await project()).history.undo;
   await put(pan(id),-37);await until(async()=>(await track(id)).mix.pan===-.37,'Numeric Pan');
   assert.equal(await strip(id).locator('.mixer-pan').inputValue(),'-37');assert.equal((await project()).history.undo,history+1);
   await edit({command:'edit.undo'});assert.equal((await track(id)).mix?.pan??0,0);
   await edit({command:'edit.redo'});assert.equal((await track(id)).mix.pan,-.37);
   await put(pan(id),22,'Escape');assert.equal((await track(id)).mix.pan,-.37);
   await put(pan(id),'');assert.equal((await track(id)).mix.pan,-.37);assert.equal(await pan(id).inputValue(),'-37');
   await put(pan(id),500);assert.equal((await track(id)).mix.pan,1);assert.equal(await pan(id).inputValue(),'100');
   await strip(id).locator('.mixer-pan').press('Home');await idle();assert.equal(await pan(id).inputValue(),'-100');
   const box=await strip(id).locator('.mixer-pan').boundingBox();await page.mouse.move(box.x+3,box.y+box.height/2);await page.mouse.down();await page.mouse.move(box.x+box.width*.65,box.y+box.height/2,{steps:8});await page.mouse.up();await idle();
   assert.equal(Number(await pan(id).inputValue()),Number(await strip(id).locator('.mixer-pan').inputValue()));assert.ok(Math.abs((await track(id)).mix.pan-Number(await pan(id).inputValue())/100)<1e-12);
   await put(pan(id),0);
 }
 await page.locator(`.track-header[data-track-id="${a}"] .track-settings-open`).click();
 await put(page.locator('#track-pan-value'),64);assert.equal((await track(a)).mix.pan,.64);assert.equal(await pan(a).inputValue(),'64');assert.equal(await page.locator('#track-pan').inputValue(),'64');await page.keyboard.press('Escape');
 for(const id of [a,m,'master']){await put(strip(id).locator('.mixer-volume'),-7.3);assert.equal(await strip(id).locator('.mixer-fader').inputValue(),'-7.3');}
 await put(strip('master').locator('.mixer-volume'),999);assert.equal((await project()).document.master.volumeDb,12);
 await put(strip('master').locator('.mixer-volume'),0);await mix(a,{pan:0,volumeDb:0});await mix(m,{pan:0,volumeDb:0});
 report.checks.push('Audio/MIDI numeric Pan; Enter/Tab/Escape/empty/clamp; native pointer slider synchronization; one Undo/Redo; Arrangement inspector sync; Track/Master numeric Volume.');
 const cases={eq:[[...Array(3)].flatMap((_,i)=>[[`band-${i}-frequency`,[200,800,8000][i]],[`band-${i}-gainDb`,2.3],[`band-${i}-q`,1.23]])].flat(),compressor:[['thresholdDb',-17.3],['ratio',3.2],['attackMs',7.3],['releaseMs',241],['makeupDb',1.7]],limiter:[['ceilingDb',-2.3],['inputDb',3.1]],reverb:[['decay',1.7],['wet',23]],delay:[['timeMs',321],['feedback',31],['wet',27]]};
 const effects={};
 for(const id of [a,m,'master']){
   effects[id]={};
   for(const[kind,fields]of Object.entries(cases)){
     const fx=await add(id,kind);effects[id][kind]=fx;
     assert.equal(await page.locator('#effect-parameters input[type=number]').count(),fields.length);
     for(const[key,value]of fields){const input=page.locator(`#effect-parameters [data-param="${key}"]`);await put(input,value);const e=(await chain(id)).find(e=>e.effectId===fx);const actual=key.startsWith('band-')?e.bands[Number(key.split('-')[1])][key.split('-')[2]]:e[key];assert.equal(actual,['wet','feedback'].includes(key)?value/100:value);}
     if(kind==='delay'){await page.locator('[data-param=syncBeats]').selectOption('0.5');await idle();assert.equal((await chain(id)).at(-1).syncBeats,.5);await page.locator('[data-param=syncBeats]').selectOption('');await idle();}
     await bypass(id,fx,false);
   }
 }
 const fx=effects[m].eq;await open(m);await slot(fx).locator('.effect-select').click();
 const q=page.locator('[data-param=band-0-q]');await put(q,-20);assert.equal((await chain(m))[0].bands[0].q,.1);await put(q,1.23);const before=(await project()).history.undo;await put(q,12,'Escape');assert.equal((await project()).history.undo,before);assert.equal((await chain(m))[0].bands[0].q,1.23);
 await page.locator('#effect-close').click();report.checks.push('All 21 continuous Effect fields typed on Audio/MIDI/Master (63 inputs); exact serialized units; existing Tempo Sync choices retained; range/cancel handling.');
 // Measure typed values in actual post-Master PCM from a stable MIDI tone.
 await edit({command:'midi.track.instrument',trackIds:[m],instrument:'basicSynth'});await edit({command:'midi.clip.add',trackIds:[m],targetTick:'0',lengthTick:'230400000'});const c=(await track(m)).clips[0];await edit({command:'midi.note.add',clipIds:[c.clipId],targetTick:'0',lengthTick:'230400000',pitch:76,velocity:85});
 await invoke('spectrum_configure',{settings:{enabled:true,fftSize:8192,smoothingMs:0}});await invoke('transport_command',{action:'play'});
 const initial=peak(await frame(),659.255,0);await put(strip(m).locator('.mixer-volume'),-6);report.trackDeltaDb=peak(await frame(),659.255,0)-initial;assert.ok(Math.abs(report.trackDeltaDb+6)<.15);
 await put(strip('master').locator('.mixer-volume'),-3);report.masterDeltaDb=peak(await frame(),659.255,0)-initial-report.trackDeltaDb;assert.ok(Math.abs(report.masterDeltaDb+3)<.15);
 await put(pan(m),-100);let f=await frame();assert.ok(peak(f,659.255,0)>-60);assert.ok(Math.max(...f.curves[1])<-100);
 await put(pan(m),100);f=await frame();assert.ok(peak(f,659.255,1)>-60);assert.ok(Math.max(...f.curves[0])<-100);await put(pan(m),0);
 await open(m);await slot(fx).locator('.effect-select').click();await put(page.locator('[data-param=band-1-frequency]'),659);await put(page.locator('[data-param=band-1-gainDb]'),6);await put(page.locator('[data-param=band-1-q]'),1);const dry=peak(await frame(),659.255,0);await bypass(m,fx,true);report.eqBoostDb=peak(await frame(),659.255,0)-dry;assert.ok(Math.abs(report.eqBoostDb-6)<.25);await page.locator('#effect-close').click();
 // Numeric changes still take the original sample-clock Automation Write path.
 await strip(m).locator('.mixer-automation-write').click();await idle();const at=(await audio()).position;await put(pan(m),-42);
 const auto=async()=>(await project()).document.automation.find(x=>x.trackId===m);
 await until(async()=>(await auto())?.lanes.some(l=>l.parameter.name==='pan'&&l.points.some(p=>p.value===-.42)),'Numeric Pan Write');
 const pt=(await auto()).lanes.find(l=>l.parameter.name==='pan').points.find(p=>p.value===-.42);assert.ok(Number(pt.tick)/1920000>=at-.01&&Number(pt.tick)/1920000<=(await audio()).position+.01);
 await open(m);await slot(fx).locator('.effect-select').click();await put(page.locator('[data-param=band-1-gainDb]'),3);
 await until(async()=>(await auto()).lanes.some(l=>l.parameter.name==='band1.gainDb'&&l.points.some(p=>p.value===3)),'Numeric Effect Write');await page.locator('#effect-close').click();
 await invoke('transport_command',{action:'pause'});await idle();await strip(m).locator('.mixer-automation-write').click();await idle();
 const persisted=(await project()).document, file=join(folder,'numeric.minidaw');await invoke('save_project',{path:file,revision:(await project()).revision});await invoke('open_project',{path:file,revision:(await project()).revision,discard:true});await idle();assert.deepEqual((await project()).document.tracks,persisted.tracks);assert.deepEqual((await project()).document.master,persisted.master);assert.deepEqual((await project()).document.automation,persisted.automation);
 for(const scale of ['90','110','125']){await page.locator('#font-scale').selectOption(scale);await sleep(100);assert.ok(await page.locator('.mixer-pan-value').evaluateAll(es=>es.every(e=>{const a=e.getBoundingClientRect(),b=e.closest('.mixer-channel').getBoundingClientRect();return a.left>=b.left&&a.right<=b.right&&a.height>12;})));}
 await page.locator('#font-scale').selectOption('110');await page.screenshot({path:'docs/validation/numeric-input-ui.png'});
 const metrics=await audio();report.output=metrics.output;assert.equal(metrics.streamErrors,0);assert.equal(metrics.metrics.overruns,0);report.checks.push('Real output: numeric Track/Master gain and left/right Pan; typed EQ gain; sample-clock Pan/Effect Automation Write; save/open; 90–125% Font Scale; zero callback overruns.');
 assert.deepEqual(errors,[]);assert.equal(await page.locator('#error').isVisible(),false);report.passed=true;
}catch(e){report.error=String(e.stack??e);report.pageErrors=errors;process.exitCode=1;if(page){report.uiError=await page.locator('#error-text').textContent().catch(()=>null);report.lastAudio=await audio().catch(()=>null);report.lastProject=await project().catch(()=>null);await page.screenshot({path:'docs/validation/numeric-input-failure.png'}).catch(()=>{});}}
finally{
 if(page&&prefs)await page.evaluate(p=>{for(const[k,v]of Object.entries(p)){if(v===null)localStorage.removeItem(k);else localStorage.setItem(k,v);}},prefs).catch(()=>{});
 if(browser)await browser.close();if(child?.exitCode===null){const exited=once(child,'exit');child.kill();await exited;}
 if(savedRecent)await writeFile(recent,savedRecent);else await unlink(recent).catch(()=>{});
 await writeFile('docs/validation/numeric-input-ui.json',JSON.stringify(report,null,2));console.log(JSON.stringify({passed:report.passed,error:report.error,checks:report.checks,trackDeltaDb:report.trackDeltaDb,masterDeltaDb:report.masterDeltaDb,eqBoostDb:report.eqBoostDb}));
}


