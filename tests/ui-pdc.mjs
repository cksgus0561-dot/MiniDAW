// Real release/Tauri/ASIO integration; temporary project and portable plugins only.
import {chromium} from 'playwright-core';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {readFile,writeFile,mkdir,unlink} from 'node:fs/promises';
import {randomUUID} from 'node:crypto';
import {resolve,join} from 'node:path';
import assert from 'node:assert/strict';
const folder=resolve('tests/local/pdc-validation',randomUUID());await mkdir(folder,{recursive:true});
const report={checks:[],folder},sleep=ms=>new Promise(r=>setTimeout(r,ms));let child,browser,page;
const recent=join(process.env.APPDATA,'local.minidaw.desktop/recent-projects.json');let oldRecent;try{oldRecent=await readFile(recent);}catch{}
const prefs=join(process.env.APPDATA,"local.minidaw.desktop/audio-preferences.json");let oldPrefs;try{oldPrefs=await readFile(prefs);}catch{}
let initialPreferences;
const invoke=(name,args={})=>page.evaluate(({name,args})=>window.__TAURI_INTERNALS__.invoke(name,args).catch(e=>{throw Error(JSON.stringify(e));}),{name,args});
const project=()=>invoke('project_snapshot'),audio=()=>invoke('engine_snapshot');
async function until(f,label,timeout=60000){const start=Date.now();while(Date.now()-start<timeout){if(await f())return;await sleep(80);}throw Error(label);}
async function idle(){await until(async()=>!await page.locator('#project-save').isDisabled(),'idle');await sleep(200);}
async function revisionCall(name,args={}){for(let i=0;;i++){try{const revision=(await project()).revision;return await invoke(name,name==='export_audio'?{request:{...args,revision}}:{...args,revision});}catch(e){if(i>=5||!/project_changed|project_busy/.test(String(e)))throw e;await sleep(400);}}}
async function edit(request){for(let i=0;;i++){try{await invoke('edit_project',{revision:(await project()).revision,request});break;}catch(e){if(i>=3||!/revision|busy|변경|진행/.test(String(e)))throw e;await sleep(500);}}await idle();}
async function start(){child=spawn(resolve('src-tauri/target/release/minidaw.exe'),[],{env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=19273'},stdio:'ignore',windowsHide:true});await until(async()=>{try{browser=await chromium.connectOverCDP('http://127.0.0.1:19273');return true;}catch{return false;}},'CDP');await until(async()=>{page=browser.contexts()[0].pages().find(p=>p.url().includes('tauri.localhost'));return !!page;},'WebView');await page.setViewportSize({width:1440,height:960});await idle();}
async function quit(){await browser?.close();if(child?.exitCode===null){const done=once(child,'exit');child.kill();await done;}}
try{
 await start();initialPreferences=(await audio()).preferences;await invoke('new_project',{revision:(await project()).revision,discard:true});await idle();
 await page.locator('#file-menu > summary').click();await page.locator('#plugin-manager-open').click();await page.locator('#plugin-paths').fill(resolve('tests/local/plugins/surge-xt-1.3.4'));await page.locator('#plugin-scan').click();await until(async()=>!await page.locator('#plugin-scan').isDisabled(),'scan',120000);
 const catalog=(await invoke('plugin_catalog')).catalog;report.scan=catalog.plugins.map(d=>({name:d.name,format:d.format,instrument:d.instrument}));report.scanErrors=catalog.errors;assert.equal(catalog.plugins.filter(d=>d.name.startsWith('Surge XT')).length,4);await page.locator('#plugin-manager-close').click();
 const tracks=[];for(const format of ['vst3','clap']){
  await edit({command:'midi.track.add'});const t=(await project()).document.tracks.at(-1).trackId;
  await page.locator('.track-header[data-track-id="'+t+'"] .midi-instrument').selectOption('choose');await page.locator('#plugin-filter').fill('Surge XT');await page.locator('#plugin-list .plugin-row[data-format="'+format+'"] button').click();await until(async()=>!await page.locator('#plugin-manager').isVisible(),'load instrument');await idle();
  assert.equal((await project()).document.tracks.at(-1).instrument,'external');tracks.push(t);
  await edit({command:'midi.clip.add',trackIds:[t],targetTick:'0',lengthTick:'15360000'});const c=(await project()).document.tracks.at(-1).clips[0].clipId;
  for(const[tick,pitch]of [[12345,60],[1920000,64],[3840000,67],[5760000,72],[7680000,60],[9600000,67]])await edit({command:'midi.note.add',clipIds:[c],targetTick:String(tick),lengthTick:'1400000',pitch,velocity:72});
  await edit({command:'track.mix',trackIds:[t],volumeDb:-18,pan:format==='vst3'?-.4:.4});
 }
 await invoke('load_audio',{path:resolve('tests/fixtures/stereo-44100.wav'),trackId:null});await idle();const at=(await project()).document.tracks.find(t=>t.kind==='audio').trackId;
 for(const[format,track]of [['vst3',at],['clap','master']]){const d=catalog.plugins.find(d=>!d.instrument&&d.format===format&&d.name==='Surge XT Effects');const plugin=await invoke('plugin_prepare',{descriptor:d});await edit({command:'effect.add',trackIds:track==='master'?[]:[track],plugin});}
 await edit({command:'effect.add',effectKind:'limiter'});await edit({command:'master.volume',volumeDb:-6});
 report.ids={tracks,audioTrack:at};await writeFile('tests/local/plugins-ready.json',JSON.stringify(report.ids));
 await edit({command:'project.cycle',cycle:{enabled:true,startTick:'0',endTick:'15360000'}});await invoke('transport_command',{action:'play'});await sleep(2200);
 const a=await audio();await sleep(6000);const b=await audio();const n=b.metrics.callbacks-a.metrics.callbacks;report.playback={backend:b.output.backend,device:b.output.device,sampleRate:b.output.sampleRate,bufferFrames:b.metrics.bufferFrames,callbackAvgMs:(b.metrics.callbackAvgMs*b.metrics.callbacks-a.metrics.callbackAvgMs*a.metrics.callbacks)/n,callbacks:n,overruns:b.metrics.overruns-a.metrics.overruns,deviceOverruns:b.metrics.deviceCallbackOverruns-a.metrics.deviceCallbackOverruns,starvation:b.source.starvation-a.source.starvation,streamErrors:b.streamErrors-a.streamErrors,peakDb:b.master.peakDb};report.pdc=b.pdc;assert.equal(b.pdc.audioLookaheadSamples,32);assert.equal(b.pdc.trackDelaySamples,0);assert.equal(b.pdc.masterLatencySamples,32);assert.equal(b.pdc.outputLatencySamples,32);assert.equal(b.pdc.error,null);report.status=await invoke('plugin_status');assert.equal(b.output.backend,'ASIO');assert.match(b.output.device,/topping/i);assert.equal(report.playback.overruns,0);assert.equal(report.playback.starvation,0);assert.ok(report.status.every(s=>!s.error&&!s.info.faulted&&!s.info.restartRequired&&s.info.processCalls>1000));
 report.checks.push('Both VST3 and CLAP instruments plus Audio/Master external Inserts process together on TOPPING ASIO.');
 // The same Insert transaction/history and Automation Write paths are used.
 const mid=(await project()).document.master.inserts[0].effectId;
 await edit({command:'effect.move',effectId:mid,direction:1});assert.equal((await project()).document.master.inserts.at(-1).effectId,mid);
 await edit({command:'edit.undo'});assert.equal((await project()).document.master.inserts[0].effectId,mid);
 await edit({command:'edit.redo'});assert.equal((await project()).document.master.inserts.at(-1).effectId,mid);
 await edit({command:'effect.move',effectId:mid,direction:-1});
 const fx=structuredClone((await project()).document.master.inserts[0]);fx.enabled=false;await edit({command:'effect.set',effectId:mid,effect:fx});assert.equal((await project()).document.master.inserts[0].enabled,false);await edit({command:'edit.undo'});assert.equal((await project()).document.master.inserts[0].enabled,true);
 const ct=(await project()).document.tracks.find(t=>t.trackId===tracks[1]);const gp=ct.extensions['minidaw.plugin.v1'].parameters.find(p=>p.name==='Global Volume');
 await edit({command:'automation.channel',trackIds:[ct.trackId],read:true,write:true});
 await edit({command:'plugin.parameter',trackIds:[ct.trackId],parameter:{name:'plugin.'+gp.id},value:.65});await sleep(500);
 await edit({command:'automation.channel',trackIds:[ct.trackId],write:false});
 const lane=(await project()).document.automation.find(a=>a.trackId===ct.trackId)?.lanes.find(l=>l.parameter.name==='plugin.'+gp.id);assert.ok(lane?.points.some(p=>Math.abs(p.value-.65)<1e-8));report.automation={parameter:gp.id,points:lane.points.map(p=>({tick:p.tick,value:p.value}))};
 report.checks.push('External Insert reorder/bypass Undo/Redo and native sample-clock Instrument Automation Write/Read.');

 // Editors use real Win32 windows; closing only the editor must preserve processing.
 for(const s of report.status){await invoke('plugin_editor',{instanceId:s.instanceId,show:true});await sleep(500);assert.equal((await invoke('plugin_status')).find(x=>x.instanceId===s.instanceId).info.editorOpen,true);await invoke('plugin_editor',{instanceId:s.instanceId,show:false});await sleep(120);assert.equal((await invoke('plugin_status')).find(x=>x.instanceId===s.instanceId).info.editorOpen,false);}
 await invoke('transport_command',{action:'stop'});await idle();report.checks.push('All four native editors opened and closed while playback continued.');
 const file=join(folder,'plugins.minidaw');await revisionCall('save_project',{path:file});report.saved=file;await idle();
 const savedDoc=(await project()).document;await quit();await start();await revisionCall('open_project',{path:file,discard:true});await idle();const normalize=d=>JSON.parse(JSON.stringify(d,(k,v)=>k==="state"&&typeof v==="string"&&v.length>64?"<opaque plugin state>":v));assert.deepEqual(normalize((await project()).document),normalize(savedDoc));report.checks.push('Project state restored after a complete app restart.');
 const result=await revisionCall('export_audio',{jobId:randomUUID(),path:join(folder,'plugins.wav'),format:'float32',sampleRate:48000,overwrite:false});report.export=result;assert.ok(result.peak>.001&&result.realtimeFactor>1);report.checks.push('Offline Export renders independent external Instrument and Effect instances.');
 await invoke('transport_command',{action:'play'});await sleep(3000);report.afterRestart=await invoke('plugin_status');assert.ok(report.afterRestart.every(s=>!s.error&&!s.info.faulted));
 // Optional native-window manual automation phase, controlled by a test sentinel.
 if(process.argv.includes('--native')){await invoke('plugin_editor',{instanceId:tracks[0],show:true});await writeFile('tests/local/plugins-native-ready.json',JSON.stringify({folder,tracks}));await until(async()=>{try{await readFile('tests/local/plugins-native-done');return true;}catch{return false;}},'Native editor verification',600000);}
 await invoke('transport_command',{action:'stop'});
 // The same project/rack and PDC on the Windows shared output path.
 const devices=await invoke('output_devices',{driverType:'wasapi'});report.wasapiDevices=devices;
 const settings={...initialPreferences,driverType:'wasapi',outputDevice:devices.find(d=>/topping/i.test(d))??null};
 await invoke('apply_audio_settings',{settings});await idle();
 await invoke('transport_command',{action:'play'});await sleep(2000);const wa=await audio();await sleep(4000);const wb=await audio();
 report.wasapi={backend:wb.output?.backend,device:wb.output?.device,sampleRate:wb.output?.sampleRate,bufferFrames:wb.metrics.bufferFrames,callbackAvgMs:wb.metrics.callbackAvgMs,overruns:wb.metrics.overruns-wa.metrics.overruns,deviceOverruns:wb.metrics.deviceCallbackOverruns-wa.metrics.deviceCallbackOverruns,starvation:wb.source.starvation-wa.source.starvation,streamErrors:wb.streamErrors-wa.streamErrors,pdc:wb.pdc,peakDb:wb.master.peakDb};
 assert.equal(wb.output.driverType,'wasapi');assert.equal(wb.pdc.outputLatencySamples,32);assert.equal(wb.pdc.audioLookaheadSamples,32);assert.equal(report.wasapi.starvation,0);assert.equal(report.wasapi.overruns,0);assert.equal(report.wasapi.streamErrors,0);assert.ok(wb.master.peakDb.some(v=>v>-90));report.checks.push('WASAPI Shared runs the same PDC graph without starvation or overruns.');
 await invoke('transport_command',{action:'stop'});await invoke('apply_audio_settings',{settings:initialPreferences});await sleep(250);await page.screenshot({path:'docs/validation/pdc-ui.png'});report.passed=true;
}catch(e){report.error=String(e.stack??e);report.status=await invoke('plugin_status').catch(()=>null);report.audio=await audio().catch(()=>null);if(page)await page.screenshot({path:'docs/validation/pdc-failure.png'}).catch(()=>{});process.exitCode=1;}
finally{if(initialPreferences&&page)await invoke("apply_audio_settings",{settings:initialPreferences}).catch(()=>{});await quit();if(oldPrefs)await writeFile(prefs,oldPrefs);else await unlink(prefs).catch(()=>{});if(oldRecent)await writeFile(recent,oldRecent);else await unlink(recent).catch(()=>{});await writeFile('docs/validation/pdc-ui.json',JSON.stringify(report,null,2));console.log(JSON.stringify({passed:report.passed,error:report.error?.slice(0,1500),playback:report.playback,pdc:report.pdc,wasapi:report.wasapi,export:report.export,checks:report.checks}));}
