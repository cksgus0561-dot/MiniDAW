// Native release WebView + real output device. No browser transport mocks.
import {chromium} from 'playwright-core';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {readFile,writeFile,mkdir,unlink} from 'node:fs/promises';
import {randomUUID,createHash} from 'node:crypto';
import {resolve,join} from 'node:path';
import assert from 'node:assert/strict';
const exe=resolve('src-tauri/target/release/minidaw.exe'),folder=resolve('tests/local/spectrum-tracks',randomUUID());await mkdir(folder,{recursive:true});
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
try{
 child=spawn(exe,[],{env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=19262'},stdio:'ignore',windowsHide:true});
 await until(async()=>{try{browser=await chromium.connectOverCDP('http://127.0.0.1:19262');return true;}catch{return false;}},'Release CDP');
 await until(async()=>{page=browser.contexts()[0].pages().find(p=>p.url().includes('tauri.localhost'));return !!page;},'Page');
 page.on('pageerror',e=>errors.push(e.message));await page.setViewportSize({width:1440,height:960});await idle();
 prefs=await page.evaluate(()=>Object.fromEntries(['minidaw.ui.panels.v1','minidaw.ui.fontScale','minidaw.ui.spectrum.v1'].map(k=>[k,localStorage.getItem(k)])));
 for(const [id,open]of [['arrangement',true],['media',false],['performance',true],['spectrum',true],['piano',false]])await panel(id,open);
 await page.locator('#font-scale').selectOption('110');
 const a=await add('audio'),b=await add('audio');
 await invoke('load_audio',{path:resolve('tests/generated/long-400s-48000-2ch.wav'),trackId:a});await idle();
 await invoke('load_audio',{path:resolve('tests/generated/long-720s-44100-2ch.flac'),trackId:b});await idle();
 const m=await add('midi'),n=await add('midi');
 for(const[id,pitch]of [[m,76],[n,81]]){
  await edit({command:'midi.track.instrument',trackIds:[id],instrument:'basicSynth'});
  await edit({command:'midi.clip.add',trackIds:[id],targetTick:'0',lengthTick:'230400000'});
  const c=(await project()).document.tracks.find(t=>t.trackId===id).clips[0];
  await edit({command:'midi.note.add',clipIds:[c.clipId],targetTick:'0',lengthTick:'230400000',pitch,velocity:100});
 }
 assert.equal(await page.locator('#spectrum-source option').count(),5);
 await page.locator('#spectrum-fft').selectOption('4096');await page.locator('#spectrum-smoothing').selectOption('0');await page.locator('#spectrum-channel').selectOption('0');
 const spec=()=>invoke('spectrum_snapshot',{after:0});
 async function ready(ids){await until(async()=>{const s=await spec();return JSON.stringify(s.sources)===JSON.stringify(ids)&&s.frame&&(!ids[1]||s.comparison)&&!s.error;},'Spectrum '+ids.join(','));await sleep(250);return spec();}
 async function source(id){if(await page.locator('#spectrum-compare').isChecked())await page.locator('#spectrum-compare').uncheck();await page.locator('#spectrum-source').selectOption(id);return ready(id==='master'?[]:[id]);}
 async function compare(x,y){await source(x);await page.locator('#spectrum-compare').check();await page.locator('#spectrum-source-b').selectOption(y);return ready([x,y]);}
 await invoke('transport_command',{action:'seek',seconds:20});await invoke('transport_command',{action:'play'});
 let s=await source('master');report.output=(await audio()).output;assert.ok(peak(s.frame,227)>-55);assert.ok(peak(s.frame,659.255)>-35);assert.ok(peak(s.frame,880)>-35);
 const authority=await audio(),revision=(await project()).revision;
 s=await source(a);assert.ok(peak(s.frame,227)>-55);assert.ok(peak(s.frame,659.255)<-80);assert.ok(peak(s.frame,443,1)>-55);
 for(const channel of ['1','2','0'])await page.locator('#spectrum-channel').selectOption(channel);
 s=await source(m);assert.ok(peak(s.frame,659.255)>-35);assert.ok(peak(s.frame,227)<-80);assert.ok(peak(s.frame,880)<-75);
 s=await compare(a,m);assert.ok(peak(s.frame,227)>-55);assert.ok(peak(s.frame,659.255)<-80);assert.ok(peak(s.comparison,659.255)>-35);assert.ok(peak(s.comparison,227)<-80);assert.equal(s.frame.sequence,s.comparison.sequence);
 await until(async()=>await page.locator('#spectrum-canvas').getAttribute('data-traces')==='2','Both Canvas traces');
 assert.match(await page.locator('#spectrum-legend').textContent(),/Audio 1/);assert.match(await page.locator('#spectrum-legend').textContent(),/MIDI 1/);
 const box=await page.locator('#spectrum-canvas').boundingBox();await page.mouse.move(box.x+48+(box.width-62)*.35,box.y+box.height*.45);await sleep(80);const hover=await page.locator('#spectrum-hover').textContent();assert.match(hover,/Hz/);assert.match(hover,/Audio 1:.*dBFS.*MIDI 1:.*dBFS/);report.hover=hover;
 assert.ok(s.frame.maxima[0].every((db,i)=>db>=s.frame.curves[0][i]-.01));
 await page.locator('#spectrum-hold').uncheck();await page.locator('#spectrum-hold').check();
 await page.locator('#spectrum-fft').selectOption('8192');await ready([a,m]);await page.locator('#spectrum-smoothing').selectOption('200');await ready([a,m]);await page.locator('#spectrum-fft').selectOption('4096');await page.locator('#spectrum-smoothing').selectOption('0');await ready([a,m]);
 assert.equal((await audio()).transport.appliedCommand,authority.transport.appliedCommand);assert.equal((await project()).revision,revision);assert.equal((await audio()).analysisBuilds,authority.analysisBuilds);
 report.checks.push('Master, isolated Audio, actual Synth and Audio/Synth comparison; independent 2-trace Peak Hold/Hz-Note-dB hover, L/R/Combined, FFT and smoothing. Source changes issue no transport command, project edit or waveform rebuild.');
 await page.mouse.move(5,5);await page.evaluate(()=>{window.__paints=0;const original=CanvasRenderingContext2D.prototype.fillRect;CanvasRenderingContext2D.prototype.fillRect=function(...args){if(this.canvas.id==='spectrum-canvas')window.__paints++;return original.apply(this,args);};});
 report.performance=[];
 for(const mode of ['off','master','audio-midi','two-audio','two-synth']){
  if(mode==='off')await panel('spectrum',false);
  else {await panel('spectrum',true);if(mode==='master')await source('master');else await compare(...(mode==='audio-midi'?[a,m]:mode==='two-audio'?[a,b]:[m,n]));}
  await sleep(700);const before=await audio(),sb=await spec(),at=await page.evaluate(()=>({t:performance.now(),p:window.__paints}));await sleep(2100);const after=await audio(),sa=await spec();
  const fps=await page.evaluate(v=>({actual:(window.__paints-v.p)*1000/(performance.now()-v.t),shown:Number(document.querySelector('#metric-spectrum-fps').textContent)}),at);
  report.performance.push({mode,fps,callbackAvgMs:(after.metrics.callbackAvgMs*after.metrics.callbacks-before.metrics.callbackAvgMs*before.metrics.callbacks)/(after.metrics.callbacks-before.metrics.callbacks),callbackMaxMs:after.metrics.callbackMaxMs,deviceMaxMs:after.metrics.deviceCallbackMaxMs,fftMs:sa.workerMs,trackReadMs:sa.trackReadMs,dropped:sa.droppedBlocks-sb.droppedBlocks,skipped:sa.skippedFrames-sb.skippedFrames,starvation:after.source.starvation,overruns:after.metrics.overruns-before.metrics.overruns,deviceOverruns:after.metrics.deviceCallbackOverruns-before.metrics.deviceCallbackOverruns});
  assert.equal(after.source.starvation,0);assert.equal(after.streamErrors,before.streamErrors);assert.equal(after.metrics.overruns,before.metrics.overruns);assert.equal(after.metrics.deviceCallbackOverruns,before.metrics.deviceCallbackOverruns);
  if(mode==='off'){assert.equal(fps.actual,0);assert.equal(fps.shown,0);assert.equal(sa.processed,sb.processed);}else{assert.ok(fps.actual>54&&fps.actual<65);assert.ok(Math.abs(fps.shown-fps.actual)<5);assert.equal(sa.droppedBlocks,sb.droppedBlocks);}
 }
 s=await compare(m,n);assert.ok(peak(s.frame,659.255)>-35);assert.ok(peak(s.comparison,880)>-35);
 // Muting or removing an Instrument must affect the captured real Synth bus.
 await setMix(m,{mute:true});s=await ready([m,n]);assert.ok(Math.max(...s.frame.curves[0])<-100);assert.ok(peak(s.comparison,880)>-35);await setMix(m,{mute:false});
 await edit({command:'midi.track.instrument',trackIds:[m],instrument:'none'});s=await ready([m,n]);assert.ok(Math.max(...s.frame.curves[0])<-100);await edit({command:'midi.track.instrument',trackIds:[m],instrument:'basicSynth'});await ready([m,n]);
 await compare(a,b);await setMix(b,{volumeDb:-12,pan:1});s=await ready([a,b]);assert.ok(Math.max(...s.comparison.curves[0])<-100);assert.ok(peak(s.comparison,443,1)>-70);
 await compare(a,m);
 // Reordering MIDI relative to Audio AND another MIDI Track must not redirect a trace.
 for(let i=0;i<2;i++)await edit({command:'track.move',trackIds:[n],direction:-1});
 s=await ready([a,m]);assert.ok(peak(s.comparison,659.255)>-35);assert.ok(peak(s.comparison,880)<-75);
 await edit({command:'edit.undo'});await edit({command:'edit.undo'});await ready([a,m]);
 await source(a);const commandBefore=(await audio()).transport.appliedCommand;
 for(const id of [m,b,n,a,m])await page.locator('#spectrum-source').selectOption(id);
 s=await ready([m]);assert.ok(peak(s.frame,659.255)>-35);assert.equal((await audio()).transport.appliedCommand,commandBefore);
 await compare(a,m);await page.screenshot({path:'docs/validation/spectrum-tracks-ui.png'});
 report.checks.push('Two Audio / two Synth / Audio+Synth compare; real Synth None/Mute, Audio fader/pan; Track reorder/Undo and rapid source switches preserve IDs; 60Hz actual Canvas FPS and closed=0; steady comparison without dropped blocks or playback starvation/overruns.');
 await invoke('transport_command',{action:'stop'});await sleep(200);
 // Existing selection analysis remains independent of the live source selectors.
 await page.locator('.audio-clip').first().click({position:{x:45,y:45}});
 await page.evaluate(()=>{window.__section=null;const f=window.fetch;window.fetch=async function(u,o){const r=await f.call(this,u,o);if(decodeURIComponent(String(u)).split('/').at(-1)==='spectrum_analyze')window.__section=await r.clone().json();return r;};});
 await page.locator('#spectrum-analyze').click();await until(async()=>!await page.locator('#spectrum-analyze').isDisabled(),'Long streaming selection',120000);const section=await page.evaluate(()=>window.__section);assert.ok(section.windows>100);assert.ok(section.duration>300);assert.equal(await page.locator('#spectrum-source').isDisabled(),true);
 await page.locator('#spectrum-mode').selectOption('live');await ready([a,m]);
 // Rename by opening this test's saved file (no new Track rename feature).
 let p=await project();const path=join(folder,'sources.minidaw');await invoke('save_project',{path,revision:p.revision});await idle();const file=JSON.parse(await readFile(path,'utf8'));file.tracks.find(t=>t.trackId===a).name='Kick Reference';await writeFile(path,JSON.stringify(file,null,2));
 await invoke('open_project',{path,revision:(await project()).revision,discard:true});await idle();await ready([a,m]);assert.match(await page.locator('#spectrum-legend').textContent(),/Kick Reference/);assert.match(await page.locator(`#spectrum-source option[value="${a}"]`).textContent(),/Kick Reference/);
 // Removing B disables comparison, removing A returns to Master. Undo safely updates options.
 await edit({command:'track.delete',trackIds:[m]});await ready([a]);assert.equal(await page.locator('#spectrum-compare').isChecked(),false);assert.equal(await page.locator(`#spectrum-source option[value="${m}"]`).count(),0);
 await edit({command:'edit.undo'});assert.equal(await page.locator(`#spectrum-source option[value="${m}"]`).count(),1);
 await edit({command:'track.delete',trackIds:[a]});await ready([]);assert.equal(await page.locator('#spectrum-source').inputValue(),'master');await edit({command:'edit.undo'});
 await invoke('new_project',{revision:(await project()).revision,discard:true});await idle();await ready([]);assert.equal(await page.locator('#spectrum-source option').count(),1);assert.equal(await page.locator('#spectrum-compare').isDisabled(),true);
 report.checks.push('Whole 400s streaming Clip selection averaging retained; save/reopen name update, Track deletion and Undo, new project reset without stale source IDs.');
 assert.deepEqual(errors,[]);assert.equal(await page.locator('#error').isVisible(),false);report.passed=true;
}catch(e){report.error=String(e.stack??e);report.pageErrors=errors;process.exitCode=1;if(page){report.uiError=await page.locator('#spectrum-status').textContent().catch(()=>null);report.lastAudio=await audio().catch(()=>null);report.spectrum=await invoke('spectrum_snapshot',{after:0}).catch(()=>null);await page.screenshot({path:'docs/validation/spectrum-tracks-failure.png'}).catch(()=>{});}}
finally{
 if(page&&prefs)await page.evaluate(p=>{for(const[k,v]of Object.entries(p)){if(v===null)localStorage.removeItem(k);else localStorage.setItem(k,v);}},prefs).catch(()=>{});
 if(browser)await browser.close();if(child?.exitCode===null){const exited=once(child,'exit');child.kill();await exited;}
 if(savedRecent)await writeFile(recent,savedRecent);else await unlink(recent).catch(()=>{});
 await writeFile('docs/validation/spectrum-tracks-ui.json',JSON.stringify(report,null,2));console.log(JSON.stringify({passed:report.passed,error:report.error,checks:report.checks,performance:report.performance}));
}
