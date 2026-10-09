// Actual release WebView key events -> Tauri -> Rust LiveReader -> ASIO Master.
import {chromium} from 'playwright-core';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {readFile,writeFile,unlink} from 'node:fs/promises';
import {resolve,join} from 'node:path';
import assert from 'node:assert/strict';
const sleep=ms=>new Promise(r=>setTimeout(r,ms)),report={checks:[],instruments:[]};
const recent=join(process.env.APPDATA,'local.minidaw.desktop/recent-projects.json');let savedRecent;try{savedRecent=await readFile(recent);}catch{}
let child,browser,page,prefs;const errors=[];
const invoke=(name,args={})=>page.evaluate(({name,args})=>window.__TAURI_INTERNALS__.invoke(name,args),{name,args});
const project=()=>invoke('project_snapshot'),audio=()=>invoke('engine_snapshot'),status=()=>invoke('computer_midi_status');
async function until(fn,label,ms=15000){const at=Date.now();while(Date.now()-at<ms){if(await fn())return;await sleep(30);}throw Error(label);}
async function idle(){await until(async()=>!await page.locator('#project-save').isDisabled(),'Project idle');await sleep(180);}
async function edit(request){await invoke('edit_project',{revision:(await project()).revision,request});await idle();}
async function panel(id,show){if((await page.locator('#show-'+id).getAttribute('aria-pressed')==='true')!==show)await page.locator('#show-'+id).click();}
async function select(id){await page.locator(`.track-header[data-track-id="${id}"] .track-label strong`).click({position:{x:8,y:8}});}
async function mode(on){if((await page.locator('#computer-midi-toggle').getAttribute('aria-pressed')==='true')!==on)await page.locator('#computer-midi-toggle').click();}
async function delivered(count){await until(async()=>(await status()).delivered>=count,'MIDI delivery '+count);}
async function silent(){await until(async()=>Math.max(...(await audio()).master.peakDb)<-75,'Release reached silence',20000);}
async function assign(id,key){await page.locator('#open-app-settings').click();await page.locator('#shortcut-search').fill(id);await page.locator(`[data-shortcut="${id}"]`).click();await page.locator('#shortcut-capture').focus();await page.keyboard.press(key);await page.locator('#shortcut-assign').click();await page.locator('#app-settings-close').click();}
try {
 child=spawn(resolve('src-tauri/target/release/minidaw.exe'),[],{env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=19276'},stdio:'ignore',windowsHide:true});
 await until(async()=>{try{browser=await chromium.connectOverCDP('http://127.0.0.1:19276');return true;}catch{return false;}},'CDP');
 await until(async()=>{page=browser.contexts()[0].pages().find(p=>p.url().includes('tauri.localhost'));return !!page;},'WebView');page.on('pageerror',e=>errors.push(e.message));await page.setViewportSize({width:1440,height:960});await idle();
 prefs=await page.evaluate(()=>Object.fromEntries(Object.keys(localStorage).filter(k=>k.startsWith('minidaw.ui.')).map(k=>[k,localStorage.getItem(k)])));
 await page.locator('#open-app-settings').click();await page.locator('#shortcut-reset').click();await page.locator('#ui-language').selectOption('en');await page.locator('#app-settings-close').click();
 await invoke('new_project',{revision:(await project()).revision,discard:true});await idle();
 for(const id of ['media','performance','piano','mixer','spectrum'])await panel(id,false);await panel('arrangement',true);
 await page.locator('#midi-track-add').click();await idle();const t=(await project()).document.tracks.at(-1).trackId;
 await page.locator(`.track-header[data-track-id="${t}"] .midi-instrument`).selectOption('basicSynth');await idle();
 await edit({command:'track.mix',trackIds:[t],volumeDb:-18});
 assert.match(await page.locator('#computer-midi-track').textContent(),/MIDI/);
 await mode(true);const p0=(await project()).revision,s0=await status();
 for(const key of ['a','d','g'])await page.keyboard.down(key);await delivered(s0.delivered+3);await sleep(180);
 assert.equal((await status()).received,s0.received+3);assert.equal((await status()).last&127,100);
 assert.ok(Math.max(...(await audio()).master.peakDb)>-50);assert.equal((await audio()).transport.state,'stopped');assert.match(await page.locator('#computer-midi-state').textContent(),/3 notes/);
 await page.keyboard.down('a');await sleep(150);assert.equal((await status()).received,s0.received+3);
 for(const key of ['a','d','g'])await page.keyboard.up(key);await delivered(s0.delivered+6);await silent();assert.equal((await project()).revision,p0);
 report.checks.push('Stopped empty MIDI Track plays Synth chord through Master; 3 Ons/3 Offs; repeat ignored; no project recording/history.');
 const beforeOct=await status();await page.keyboard.down('a');await delivered(beforeOct.delivered+1);await page.locator('#computer-midi-up').click();assert.equal(await page.locator('#computer-midi-octave').textContent(),'C4–C5');await page.keyboard.up('a');await delivered(beforeOct.delivered+2);assert.equal(((await status()).last>>>8)&127,60);
 await page.keyboard.press('w');await delivered(beforeOct.delivered+4);assert.equal(((await status()).last>>>8)&127,73);await page.locator('#computer-midi-down').click();
 // Every mapped white/black key emits one On/Off. These otherwise overlap DAW J etc.
 const all=await status();for(const key of ['a','w','s','e','d','f','t','g','y','h','u','j','k'])await page.keyboard.press(key);await delivered(all.delivered+26);assert.equal((await status()).received,all.received+26);
 // Mode off / focus loss / input focus release held keys.
 const off=await status();await page.keyboard.down('a');await delivered(off.delivered+1);await mode(false);await sleep(150);await page.keyboard.up('a');await silent();
 await mode(true);await page.keyboard.down('a');await sleep(120);await page.evaluate(()=>window.dispatchEvent(new Event('blur')));await page.keyboard.up('a');await silent();
 await page.keyboard.down('a');await sleep(100);await page.locator('#project-bpm').focus();await page.keyboard.up('a');const blocked=(await status()).received;await page.keyboard.press('a');await sleep(120);assert.equal((await status()).received,blocked);await silent();await page.locator('#project-bpm').press('Escape');
 report.checks.push('Octave changes preserve held pitch; all 13 keys; mode OFF/window blur/input focus release; numeric input does not play.');
 // Korean layout is physical-key based; composing text remains excluded.
 await page.locator('#computer-midi-toggle').focus();const kr=await status();await page.evaluate(()=>{document.activeElement.dispatchEvent(new KeyboardEvent('keydown',{code:'KeyA',key:'ㅁ',bubbles:true}));document.activeElement.dispatchEvent(new KeyboardEvent('keyup',{code:'KeyA',key:'ㅁ',bubbles:true}));});await delivered(kr.delivered+2);
 const im=await status();await page.evaluate(()=>document.activeElement.dispatchEvent(new KeyboardEvent('keydown',{code:'KeyA',key:'Process',isComposing:true,bubbles:true})));await sleep(100);assert.equal((await status()).received,im.received);
 // Track switching while a key is down flushes only the computer input route.
 await page.locator('#midi-track-add').click();await idle();const t2=(await project()).document.tracks.at(-1).trackId;await page.locator(`.track-header[data-track-id="${t2}"] .midi-instrument`).selectOption('basicSynth');await idle();
 await select(t);await page.keyboard.down('a');await sleep(100);await select(t2);await page.keyboard.up('a');await silent();assert.match(await page.locator('#computer-midi-track').textContent(),/MIDI/);
 await edit({command:'midi.clip.add',trackIds:[t2],targetTick:'0',lengthTick:'15360000'});
 // Key remapping stays active off-mode; same plain piano key is reserved on-mode.
 await assign('transport.toggle','a');await mode(false);await page.locator('#stop').focus();await page.keyboard.press('a');await until(async()=>(await audio()).transport.state==='playing','Remapped A plays');await page.keyboard.press('a');await until(async()=>(await audio()).transport.state==='paused','Remapped A pauses');
 await mode(true);const commands=(await audio()).transport.appliedCommand;await page.keyboard.press('a');await sleep(150);assert.equal((await audio()).transport.appliedCommand,commands);
 await assign('transport.toggle','Space');await page.locator('#stop').focus();await page.keyboard.press('Space');await until(async()=>(await audio()).transport.state==='playing','Space while keyboard ON');await page.keyboard.press('Space');await until(async()=>(await audio()).transport.state==='paused','Space pauses while ON');
 await assign('midi.computerToggle','Control+Alt+k');await page.locator('#stop').focus();await page.keyboard.press('Control+Alt+k');assert.equal(await page.locator('#computer-midi-toggle').getAttribute('aria-pressed'),'false');await page.keyboard.press('Control+Alt+k');assert.equal(await page.locator('#computer-midi-toggle').getAttribute('aria-pressed'),'true');
 report.checks.push('Track change release; Korean physical keys and IME protection; remapped A works OFF/reserved ON; Space and remapped mode toggle work ON.');
 // Basic Piano Roll/editing commands still work when performance mode is off.
 await mode(false);const clip=(await project()).document.tracks.find(t=>t.trackId===t2).clips[0].clipId;
 await edit({command:'midi.note.add',clipIds:[clip],targetTick:'12345',lengthTick:'960000',pitch:64,velocity:80});await page.locator(`[data-clip-id="${clip}"]`).dblclick();await page.locator('#piano-canvas').focus();await page.keyboard.press('Control+a');await page.keyboard.press('ArrowUp');await idle();assert.equal((await project()).document.tracks.find(t=>t.trackId===t2).clips[0].notes[0].pitch,65);await page.keyboard.press('Control+z');await idle();assert.equal((await project()).document.tracks.find(t=>t.trackId===t2).clips[0].notes[0].pitch,64);await panel('piano',false);
 // Use existing portable VST3/CLAP instruments; no install or path preference changes.
 const catalog=(await invoke('plugin_catalog')).catalog;await select(t);await mode(true);
 for(const format of ['vst3','clap']){
   const descriptor=catalog.plugins.find(p=>p.instrument&&p.format===format&&p.name==='Surge XT');assert.ok(descriptor,format+' Surge XT installed');
   await page.locator(`.track-header[data-track-id="${t}"] .midi-instrument`).selectOption('choose');
   await page.locator('#plugin-filter').fill('Surge XT');await page.locator(`#plugin-list .plugin-row[data-format="${format}"] button`).click();
   await until(async()=>!await page.locator('#plugin-manager').isVisible(),'Load '+format);await idle();await select(t);await silent();
   const before=await status(),metrics=await audio();await page.keyboard.down('a');await page.keyboard.down('d');await page.keyboard.down('g');await delivered(before.delivered+3);await sleep(600);const played=await audio();assert.ok(Math.max(...played.master.peakDb)>-60,format+' actual Master output');
   for(const key of ['a','d','g'])await page.keyboard.up(key);await delivered(before.delivered+6);await silent();
   const native=(await invoke('plugin_status')).find(s=>s.instanceId===t);assert.ok(native&&!native.error&&!native.info.faulted);
   if(format==='vst3'){
     await page.keyboard.down('a');await sleep(200);await invoke('plugin_editor',{instanceId:t,show:true});
     await until(async()=>/0 notes/.test(await page.locator('#computer-midi-state').textContent()),'Native editor focus releases keys');await silent();
     await invoke('plugin_editor',{instanceId:t,show:false});await page.keyboard.up('a');
   }
   report.instruments.push({format,name:descriptor.name,peakDb:played.master.peakDb,processCalls:native.info.processCalls,backend:played.output.backend,device:played.output.device,overruns:played.metrics.overruns-metrics.metrics.overruns,starvation:played.source.starvation-metrics.source.starvation});
 }
 await mode(false);await page.locator('.computer-midi summary').click();await page.screenshot({path:'docs/validation/computer-midi-ui.png'});
 assert.deepEqual(errors,[]);assert.equal(await page.locator('#error').isVisible(),false,await page.locator('#error-text').textContent());report.finalStatus=await status();assert.equal(report.finalStatus.dropped,0);report.checks.push('Piano Roll transpose/Undo preserved; actual Surge XT VST3 and CLAP live chords reach ASIO Master and release to silence; native plug-in editor focus releases held notes.');report.passed=true;
}catch(e){report.error=String(e.stack??e);report.errors=errors;if(page){report.status=await status().catch(()=>null);report.audio=await audio().catch(()=>null);await page.screenshot({path:'docs/validation/computer-midi-failure.png'}).catch(()=>{});}process.exitCode=1;}
finally{if(page){await invoke('computer_midi_notes',{trackId:null,notes:[]}).catch(()=>{});if(prefs)await page.evaluate(p=>{for(const k of Object.keys(localStorage).filter(k=>k.startsWith('minidaw.ui.')))localStorage.removeItem(k);for(const[k,v]of Object.entries(p))localStorage.setItem(k,v);},prefs).catch(()=>{});}await browser?.close();if(child?.exitCode===null){const done=once(child,'exit');child.kill();await done;}if(savedRecent)await writeFile(recent,savedRecent);else await unlink(recent).catch(()=>{});await writeFile('docs/validation/computer-midi-ui.json',JSON.stringify(report,null,2));console.log(JSON.stringify(report));}


