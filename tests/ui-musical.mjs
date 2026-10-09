import {chromium} from 'playwright-core';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {readFile,writeFile,mkdir,unlink} from 'node:fs/promises';
import {randomUUID} from 'node:crypto';
import {resolve,join} from 'node:path';
import assert from 'node:assert/strict';
const folder=resolve('tests/local/musical',randomUUID());await mkdir(folder,{recursive:true});
const recent=join(process.env.APPDATA,'local.minidaw.desktop/recent-projects.json');let savedRecent;try{savedRecent=await readFile(recent);}catch{}
let child,browser,page,prefs;const report={checks:[]},errors=[],sleep=ms=>new Promise(r=>setTimeout(r,ms));
async function until(fn,label){const at=performance.now();while(performance.now()-at<20000){if(await fn())return;await sleep(40);}throw Error(label);}
const invoke=(name,args={})=>page.evaluate(({name,args})=>window.__TAURI_INTERNALS__.invoke(name,args),{name,args});
const project=()=>invoke('project_snapshot'),audio=()=>invoke('engine_snapshot');
async function idle(){await until(async()=>!await page.locator('#project-save').isDisabled(),'Idle');await sleep(180);}
async function fit(){await page.locator('#zoom-fit').click();await sleep(100);}
const seconds=c=>Number(c.position.numerator)/c.position.denominator;
async function dragClip(id,delta,handle){
 await fit();const old=await project(),node=page.locator(`[data-clip-id="${id}"]`),target=handle?node.locator(`[data-handle="${handle}"]`):node,b=await target.boundingBox();
 const step=Number(await page.locator('#scroll').getAttribute('step')),x=b.x+(handle?b.width/2:40),y=b.y+b.height*.65;
 await page.mouse.move(x,y);await page.mouse.down();await page.mouse.move(x+delta/step,y,{steps:25});assert.equal((await project()).revision,old.revision);await page.mouse.up();
 await until(async()=>(await project()).revision>old.revision,'Drag commits');await idle();
}
const rate=48000,frames=rate*8,pcm=Buffer.alloc(44+frames*4);pcm.write('RIFF');pcm.writeUInt32LE(pcm.length-8,4);pcm.write('WAVEfmt ',8);pcm.writeUInt32LE(16,16);pcm.writeUInt16LE(1,20);pcm.writeUInt16LE(2,22);pcm.writeUInt32LE(rate,24);pcm.writeUInt32LE(rate*4,28);pcm.writeUInt16LE(4,32);pcm.writeUInt16LE(16,34);pcm.write('data',36);pcm.writeUInt32LE(frames*4,40);
for(let i=0;i<frames;i++)for(let ch=0;ch<2;ch++)pcm.writeInt16LE(Math.round(.03*32767*Math.sin(2*Math.PI*440*i/rate)),44+i*4+ch*2);
const file=join(folder,'quiet.wav');await writeFile(file,pcm);
try{
 child=spawn(resolve('src-tauri/target/release/minidaw.exe'),[],{env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=19242'},stdio:'ignore',windowsHide:true});
 await until(async()=>{try{browser=await chromium.connectOverCDP('http://127.0.0.1:19242');return true;}catch{return false;}},'CDP');
 await until(async()=>{page=browser.contexts()[0].pages().find(p=>p.url().includes('tauri.localhost'));return !!page;},'Page');
 page.on('pageerror',e=>errors.push(e.message));await page.setViewportSize({width:1440,height:960});await idle();
 prefs=await page.evaluate(()=>Object.fromEntries(['minidaw.ui.panels.v1','minidaw.ui.fontScale'].map(k=>[k,localStorage.getItem(k)])));
 for(const [id,open]of [['arrangement',true],['media',false],['performance',false],['spectrum',false]]){const b=page.locator('#show-'+id);if((await b.getAttribute('aria-pressed')==='true')!==open)await b.click();}
 await invoke('load_audio',{path:file});await idle();await fit();
 let p=await project();const clip=p.document.tracks[0].clips[0],id=clip.clipId;
 const beforeGrid=await page.locator('#clip-layer').evaluate(n=>n.style.backgroundImage);
 await page.locator('#project-bpm').fill('150');await page.locator('#project-bpm').press('Tab');await idle();
 await until(async()=>(await project()).document.musicalTime.tempoMap[0].bpm===150,'BPM persisted');
 await page.locator('#signature-numerator').fill('3');await page.locator('#signature-numerator').press('Tab');await idle();
 assert.equal((await project()).document.musicalTime.timeSignatures[0].numerator,3);
 assert.deepEqual((await project()).document.tracks[0].clips[0].position,clip.position);
 assert.notEqual(await page.locator('#clip-layer').evaluate(n=>n.style.backgroundImage),beforeGrid);
 report.checks.push('150 BPM / 3/4 update project and musical Grid; seconds-anchored audio remains unchanged.');
 await page.locator('#grid-type').selectOption('8');await page.locator('#clip-layer').focus();await page.keyboard.press('j');
 assert.equal(await page.locator('#snap-toggle').getAttribute('aria-pressed'),'true');
 await dragClip(id,.33);p=await project();assert.ok(Math.abs(seconds(p.document.tracks[0].clips[0])-.4)<1e-8);
 await page.keyboard.press('Control+z');await idle();assert.equal(seconds((await project()).document.tracks[0].clips[0]),0);
 await page.keyboard.press('Control+Shift+z');await idle();assert.ok(Math.abs(seconds((await project()).document.tracks[0].clips[0])-.4)<1e-8);
 await dragClip(id,.31,'trim-left');p=await project();assert.ok(Math.abs(seconds(p.document.tracks[0].clips[0])-.8)<1/rate);
 await dragClip(id,-.31,'trim-right');p=await project();const trimmed=p.document.tracks[0].clips[0];assert.ok(Math.abs((seconds(trimmed)+(Number(trimmed.sourceEnd)-Number(trimmed.sourceStart))/rate)-8)<1/rate);
 await page.locator('[data-command="tool.split"]').click();await fit();const layer=await page.locator('#clip-layer').boundingBox(),step=Number(await page.locator('#scroll').getAttribute('step'));
 await page.mouse.click(layer.x+1.73/step,layer.y+60);await idle();p=await project();assert.equal(p.document.tracks[0].clips.length,2);assert.ok(p.document.tracks[0].clips.some(c=>Math.abs(seconds(c)-1.8)<1/rate));
 report.checks.push('J Snap, 1/8 Move/Trim/Split, pointer-up single commit, Undo/Redo.');
 await page.locator('[data-command="tool.objectSelection"]').click();await page.locator('#clip-layer').focus();await page.keyboard.press('j');
 assert.equal(await page.locator('#snap-toggle').getAttribute('aria-pressed'),'false');
 const moved=p.document.tracks[0].clips[0];await dragClip(moved.clipId,.137);assert.ok(Math.abs(seconds((await project()).document.tracks[0].clips[0])-(seconds(moved)+.137))<.015);
 // Existing zoom and horizontal scroll still operate with the musical ruler.
 const canvas=await page.locator('#waveform').boundingBox();await page.mouse.move(canvas.x+canvas.width*.6,canvas.y+canvas.height-3);
 await page.keyboard.down('Control');await page.mouse.wheel(0,-300);await page.keyboard.up('Control');await sleep(120);
 assert.ok(Number(await page.locator('#scroll').getAttribute('max'))>0);
 const scroll=Number(await page.locator('#scroll').inputValue());await page.keyboard.down('Shift');await page.mouse.wheel(0,100);await page.keyboard.up('Shift');await sleep(120);assert.ok(Number(await page.locator('#scroll').inputValue())>scroll);
 await fit();await page.locator('#snap-toggle').click();
 const zone=await page.locator('#cycle-ruler').boundingBox(),spp=Number(await page.locator('#scroll').getAttribute('step')),revision=(await project()).revision;
 await page.mouse.move(zone.x+1.01/spp,zone.y+zone.height/2);await page.mouse.down();await page.mouse.move(zone.x+1.81/spp,zone.y+zone.height/2,{steps:30});assert.equal((await project()).revision,revision);await page.mouse.up();await idle();
 p=await project();assert.equal(p.document.cycle.startTick,'2400000');assert.equal(p.document.cycle.endTick,'4320000');
 await page.locator('#cycle-toggle').click();await idle();assert.equal((await project()).document.cycle.enabled,true);
 const path=join(folder,'music.minidaw');p=await project();await invoke('save_project',{path,revision:p.revision});await idle();
 p=await project();const saved=p.document;await invoke('new_project',{revision:p.revision,discard:true});await idle();p=await project();await invoke('open_project',{path,revision:p.revision,discard:true});await idle();
 p=await project();assert.deepEqual(p.document.musicalTime,saved.musicalTime);assert.deepEqual(p.document.cycle,saved.cycle);assert.equal(await page.locator('#project-bpm').inputValue(),'150');assert.equal(await page.locator('#cycle-toggle').getAttribute('aria-pressed'),'true');
 await invoke('transport_command',{action:'seek',seconds:1});await invoke('transport_command',{action:'play'});await sleep(250);
 const begin=await audio();let last=begin.position,wraps=0;for(let i=0;i<40;i++){await sleep(100);const a=await audio();assert.equal(a.transport.state,'playing');assert.ok(a.position>=1&&a.position<1.8);if(a.position<last)wraps++;last=a.position;}
 const end=await audio();assert.ok(wraps>=4);assert.equal(end.transport.appliedCommand,begin.transport.appliedCommand);assert.equal(end.source.starvation,begin.source.starvation);assert.equal(end.streamErrors,begin.streamErrors);
 report.loop={wraps,start:1,end:1.8,extraTransportCommands:end.transport.appliedCommand-begin.transport.appliedCommand,starvationDelta:end.source.starvation-begin.source.starvation};
 await invoke('transport_command',{action:'pause'});await sleep(80);assert.equal((await audio()).transport.state,'paused');await invoke('transport_command',{action:'play'});await sleep(100);assert.equal((await audio()).transport.state,'playing');
 await page.locator('#cycle-toggle').click();await idle();await sleep(1000);assert.ok((await audio()).position>1.8,'Cycle OFF continues beyond the right locator');
 await invoke('transport_command',{action:'stop'});await sleep(80);assert.equal((await audio()).position,0);
 const ruler=await page.locator('#waveform').boundingBox(),scale=Number(await page.locator('#scroll').getAttribute('step'));
 const command=(await audio()).transport.appliedCommand;
 await page.mouse.move(ruler.x+2.2/scale,ruler.y+ruler.height-5);await page.mouse.down();await page.mouse.move(ruler.x+2.6/scale,ruler.y+ruler.height-5,{steps:25});
 assert.equal((await audio()).transport.appliedCommand,command);await page.mouse.up();await sleep(150);assert.equal((await audio()).transport.appliedCommand,command+1);
 await invoke('transport_command',{action:'stop'});await page.locator('#cycle-toggle').click();await idle();
 report.checks.push('Free Move, Ctrl+Wheel zoom / Shift+Wheel scroll, ruler Cycle drag, save/open Tempo/Signature/Cycle, repeated Rust-timed Cycle with zero UI seek and no starvation; Pause/Resume/Stop.');
 await page.screenshot({path:'docs/validation/musical-ui.png'});assert.deepEqual(errors,[]);report.passed=true;
}catch(e){report.error=String(e.stack??e);process.exitCode=1;if(page)await page.screenshot({path:'docs/validation/musical-failure.png'}).catch(()=>{});}
finally{
 if(page&&prefs)await page.evaluate(p=>{for(const[k,v]of Object.entries(p)){if(v===null)localStorage.removeItem(k);else localStorage.setItem(k,v);}},prefs).catch(()=>{});
 if(browser)await browser.close();if(child?.exitCode===null){const exited=once(child,'exit');child.kill();await exited;}
 if(savedRecent)await writeFile(recent,savedRecent);else await unlink(recent).catch(()=>{});
 await writeFile('docs/validation/musical-ui.json',JSON.stringify(report,null,2));console.log(JSON.stringify(report));
}
