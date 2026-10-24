// Focused end-to-end test against the real release app. Uses a separate WebView
// profile and scratch project; it never changes the user's audio preferences.
import {chromium} from 'playwright-core';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {mkdir,writeFile,stat} from 'node:fs/promises';
import {resolve,join} from 'node:path';
import {randomUUID} from 'node:crypto';
import assert from 'node:assert/strict';
const folder=resolve('tests/local/notation',randomUUID());await mkdir(folder,{recursive:true});
const executable=resolve(process.argv[2]??'src-tauri/target/x86_64-pc-windows-msvc/release/minidaw.exe');
const port=19322,report={executable,folder,checks:[]},errors=[],sleep=ms=>new Promise(r=>setTimeout(r,ms));
let child,browser,page,savedPath;
async function until(f,label){const start=Date.now();while(Date.now()-start<25000){if(await f())return;await sleep(60);}throw Error(label);}
const invoke=(name,args={})=>page.evaluate(({name,args})=>window.__TAURI_INTERNALS__.invoke(name,args),{name,args});
const project=()=>invoke('project_snapshot');
async function idle(){await until(async()=>!await page.locator('#project-save').isDisabled(),'Idle');await sleep(200);}
async function edit(request){await invoke('edit_project',{revision:(await project()).revision,request});await idle();}
async function changed(action){const before=(await project()).revision;await action();await until(async()=>(await project()).revision>before,'Edit committed');await idle();}
const notes=async id=>(await project()).document.tracks.flatMap(t=>t.clips).find(c=>c.clipId===id).notes;
async function point(tick,pitch){return page.evaluate(({tick,pitch})=>{const c=document.querySelector('#piano-canvas'),b=c.getBoundingClientRect(),s=document.querySelector('#piano-scroll'),span=Number(s.step)*(b.width-56),row=parseFloat(getComputedStyle(document.documentElement).fontSize)*1.45;return {x:b.x+56+(tick-Number(s.value))/span*(b.width-56),y:b.y+28+(127-pitch+.5)*row-document.querySelector('#piano-vscroll').scrollTop};},{tick,pitch});}
async function clickNote(tick,pitch){const p=await point(tick,pitch);await page.mouse.click(p.x,p.y);await sleep(100);}
try{
 child=spawn(executable,[],{env:{...process.env,WEBVIEW2_USER_DATA_FOLDER:join(folder,'webview'),WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:`--remote-debugging-port=${port}`},windowsHide:true,stdio:['ignore','pipe','pipe']});
 child.stdout.on('data',b=>report.appLog=(report.appLog??'')+b.toString());child.stderr.on('data',b=>report.appLog=(report.appLog??'')+b.toString());
 await until(async()=>{if(child.exitCode!==null)throw Error(`App exited ${child.exitCode}: ${report.appLog}`);try{browser=await chromium.connectOverCDP(`http://127.0.0.1:${port}`);return true;}catch{return false;}},'CDP');
 await until(async()=>{page=browser.contexts()[0].pages().find(p=>p.url().includes('tauri.localhost')&&!p.url().includes('?'));return !!page;},'Main app');
 assert.ok((await stat(join(folder,'webview'))).isDirectory(),'Isolated WebView profile exists');
 page.on('pageerror',e=>errors.push(e.message));await page.setViewportSize({width:1550,height:1000});await idle();
 await invoke('new_project',{revision:(await project()).revision,discard:true});await idle();
 for(const id of ['media','spectrum','performance','mixer'])if(await page.locator('#show-'+id).getAttribute('aria-pressed')==='true')await page.locator('#show-'+id).click();
 const tracks=[],clips=[];
 for(let i=0;i<2;i++){
  await edit({command:'midi.track.add'});const t=(await project()).document.tracks.at(-1);tracks.push(t.trackId);
  await edit({command:'midi.track.instrument',trackIds:[t.trackId],instrument:'basicSynth'});
  await edit({command:'track.mix',trackIds:[t.trackId],volumeDb:-30});
  await edit({command:'midi.clip.add',trackIds:[t.trackId],targetTick:'0',lengthTick:'7680000'});
  const c=(await project()).document.tracks.at(-1).clips[0];clips.push(c.clipId);
  for(const [tick,pitch]of[[240000,65],[1440000,65],[2880000,71]])await edit({command:'midi.note.add',clipIds:[c.clipId],targetTick:String(tick),lengthTick:'720000',pitch});
 }
 await page.locator(`[data-clip-id="${clips[0]}"]`).click();await page.locator('#piano-open').click();await idle();
 // Record rendered marks from the actual Canvas, not just requestAnimationFrame.
 await page.evaluate(()=>{window.__notationMarks=[];const f=CanvasRenderingContext2D.prototype.fillRect;CanvasRenderingContext2D.prototype.fillRect=function(...a){if(this.canvas.id==='piano-canvas'){if(this.fillStyle==='#202123')window.__notationMarks=[];if(['#5edbd3','#ffb86b'].includes(this.fillStyle))window.__notationMarks.push(this.fillStyle);}return f.apply(this,a);};});
 const otherBefore=await notes(clips[1]);
 await changed(()=>page.locator('#piano-key-signature').selectOption('1'));
 assert.deepEqual((await notes(clips[0])).map(n=>n.pitch),[66,66,71]);assert.deepEqual(await notes(clips[1]),otherBefore);
 assert.equal(await page.locator(`[data-track-id="${tracks[0]}"] .track-key-signature`).inputValue(),'1');
 await clickNote(500000,66);assert.equal(await page.locator('#piano-accidental').inputValue(),'key');
 await changed(()=>page.locator('#piano-accidental').selectOption('natural'));
 assert.deepEqual((await notes(clips[0])).map(n=>n.pitch),[65,66,71]);
 assert.ok((await page.locator('#piano-position').textContent()).includes('F♮3'));
 let marks=await page.evaluate(()=>window.__notationMarks);assert.ok(marks.includes('#5edbd3')&&marks.includes('#ffb86b'));
 await page.screenshot({path:join(folder,'notation.png')});
 await changed(()=>page.locator('#piano-accidental').selectOption('sharp'));assert.equal((await notes(clips[0]))[0].pitch,66);
 await changed(()=>page.locator('#piano-accidental').selectOption('flat'));assert.equal((await notes(clips[0]))[0].pitch,64);
 await changed(()=>page.locator('#piano-accidental').selectOption('key'));assert.equal((await notes(clips[0]))[0].pitch,66);
 report.checks.push('Track key alters only its notes; sharp/flat/natural/follow-key commands; teal and orange Canvas marks; note spelling readout.');
 await page.locator('#piano-canvas').focus();await changed(()=>page.keyboard.press('Control+z'));assert.equal((await notes(clips[0]))[0].pitch,64);
 await changed(()=>page.keyboard.press('Control+Shift+z'));assert.equal((await notes(clips[0]))[0].pitch,66);
 const beforeGroup=await notes(clips[0]);await page.locator('#piano-canvas').focus();await page.keyboard.press('Control+a');
 await changed(()=>page.locator('#piano-accidental').selectOption('natural'));assert.deepEqual((await notes(clips[0])).map(n=>n.pitch),[65,65,71]);
 await page.locator('#piano-canvas').focus();await changed(()=>page.keyboard.press('Control+z'));assert.deepEqual(await notes(clips[0]),beforeGroup);
 const hover=await point(500000,66);await page.mouse.move(hover.x,hover.y);assert.ok((await page.locator('#piano-canvas').getAttribute('title')).includes('F♯3'));
 report.checks.push('Multi-note accidental is one undo transaction; hover exposes written pitch and notation source.');
 // Draw on F: key-signature preview and committed pitch both become F sharp.
 await page.locator('#piano-draw').click();const at=await point(4800000,65);const revision=(await project()).revision;
 await page.mouse.move(at.x,at.y);await page.mouse.down();assert.equal((await project()).revision,revision);await page.mouse.up();
 await until(async()=>(await notes(clips[0])).length===4,'New note');await idle();assert.equal((await notes(clips[0])).at(-1).pitch,66);
 await page.locator('#piano-select').click();await clickNote(500000,66);
 // Numeric timing retains spelling; horizontal movement does not apply another sharp.
 await page.locator('#note-time-format').selectOption('ticks');await changed(async()=>{await page.locator('#note-start').fill('240123');await page.locator('#note-start').press('Enter');});
 assert.equal((await notes(clips[0]))[0].pitch,66);assert.equal((await notes(clips[0]))[0].startTick,'240123');
 if(await page.locator('#snap-toggle').getAttribute('aria-pressed')!=='true')await page.locator('#snap-toggle').click();
 const a=await point(500000,66),b=await point(980000,66);
 await changed(async()=>{await page.mouse.move(a.x,a.y);await page.mouse.down();await page.mouse.move(b.x,b.y,{steps:30});await page.mouse.up();});
 assert.equal((await notes(clips[0]))[0].pitch,66);assert.equal(Number((await notes(clips[0]))[0].startTick)%960000,0);
 report.checks.push('Draw preview commits once, written F becomes F sharp; Undo/Redo; tick numeric edit; snapped horizontal Move preserves pitch.');
 // Independent flat signature from the second Arrangement track.
 await changed(()=>page.locator(`[data-track-id="${tracks[1]}"] .track-key-signature`).selectOption('-1'));
 assert.deepEqual((await notes(clips[1])).map(n=>n.pitch),[65,65,70]);
 const p=await project();savedPath=join(folder,'notation.minidaw');await invoke('save_project',{path:savedPath,revision:p.revision});
 const saved=(await project()).document;await invoke('open_project',{path:savedPath,revision:(await project()).revision,discard:true});await idle();assert.deepEqual((await project()).document,saved);
 report.checks.push('Independent flat key on second track; actual save/reopen retains key, accidental, sounding pitch, exact ticks.');
 await page.locator(`[data-clip-id="${clips[0]}"]`).click();await page.locator('#piano-open').click();await idle();
 for(const scale of ['90','110','125']){await page.locator('#font-scale').selectOption(scale);await sleep(120);assert.ok((await page.locator('#piano-canvas').boundingBox()).height>50);assert.ok(await page.locator('#piano-key-signature').isVisible());}
 await page.setViewportSize({width:1100,height:800});await sleep(160);assert.ok((await page.locator('#piano-canvas').boundingBox()).height>50);
 // The detached editor must use the same project and commands.
 await page.locator('#detach-piano').click();let satellite;
 await until(async()=>{satellite=browser.contexts()[0].pages().find(p=>p.url().includes('?panel=piano'));return !!satellite;},'Detached Piano Roll');
 await until(async()=>await satellite.locator('#piano-key-signature').isEnabled(),'Detached key ready');
 await changed(()=>satellite.locator('#piano-key-signature').selectOption('2'));assert.equal((await project()).document.tracks[0].keySignature,2);
 await satellite.locator('#panel-dock').click();await idle();
 report.checks.push('Font Scale 90/110/125, smaller window; detached Piano Roll edits same key signature and docks again.');
 await page.locator('#play-pause').click();await until(async()=>(await invoke('engine_snapshot')).transport.state==='playing','Playback');await sleep(300);report.playback=await invoke('engine_snapshot');await page.locator('#stop').click();await idle();
 assert.deepEqual(errors,[]);assert.equal(await page.locator('#error').isVisible(),false);report.passed=true;
}catch(e){report.error=String(e.stack??e);report.pageErrors=errors;process.exitCode=1;if(page){report.uiError=await page.locator('#error-text').textContent().catch(()=>null);await page.screenshot({path:join(folder,'failure.png')}).catch(()=>{});}}
finally{
 if(page&&savedPath)await invoke('remove_recent_project',{path:savedPath}).catch(()=>{});
 if(browser)await browser.close();if(child?.exitCode===null){const exited=once(child,'exit');child.kill();await exited;}
 await writeFile(join(folder,'report.json'),JSON.stringify(report,null,2));console.log(JSON.stringify(report));
}
