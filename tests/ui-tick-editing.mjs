import {chromium} from 'playwright-core';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {readFile,writeFile,mkdir,unlink} from 'node:fs/promises';
import {randomUUID} from 'node:crypto';
import {resolve,join} from 'node:path';
import assert from 'node:assert/strict';
const folder=resolve('tests/local/tick-editing',randomUUID());await mkdir(folder,{recursive:true});
const recent=join(process.env.APPDATA,'local.minidaw.desktop/recent-projects.json');let savedRecent;try{savedRecent=await readFile(recent);}catch{}
let child,browser,page,prefs;const report={moves:[],checks:[]},errors=[],sleep=ms=>new Promise(r=>setTimeout(r,ms));
async function until(fn,label){const at=performance.now();while(performance.now()-at<20000){if(await fn())return;await sleep(40);}throw Error(label);}
const invoke=(name,args={})=>page.evaluate(({name,args})=>window.__TAURI_INTERNALS__.invoke(name,args),{name,args});
const project=()=>invoke('project_snapshot'),seconds=c=>Number(c.position.numerator)/c.position.denominator;
const clips=async()=>(await project()).document.tracks[0].clips;
async function idle(){await until(async()=>!await page.locator('#project-save').isDisabled(),'Idle');await sleep(100);}
async function drag(id,delta,handle){
 const p=await project(),node=page.locator(`[data-clip-id="${id}"]`),layer=await page.locator('#clip-layer').boundingBox();
 const target=handle?node.locator(`[data-handle="${handle}"]`):node,b=await target.boundingBox();
 const step=Number(await page.locator('#scroll').getAttribute('step')),x=handle?b.x+b.width/2:Math.max(b.x+40,layer.x+40),y=b.y+b.height*.65;
 await page.mouse.move(x,y);await page.mouse.down();await page.mouse.move(x+delta/step,y,{steps:15});
 assert.equal((await project()).revision,p.revision,'Only preview during drag');
 const preview=await page.locator('#edit-position').textContent();assert.match(preview,/\d+\.\d+\.\d+\.\d{6}/);
 await page.mouse.up();await until(async()=>(await project()).revision>p.revision,'Edit commit');await idle();
 return {request:await page.evaluate(()=>window.__lastEdit),preview};
}
async function fit(){await page.locator('#zoom-fit').click();await sleep(100);}
try{
 child=spawn(resolve('src-tauri/target/release/minidaw.exe'),[],{env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=19244'},stdio:'ignore',windowsHide:true});
 await until(async()=>{try{browser=await chromium.connectOverCDP('http://127.0.0.1:19244');return true;}catch{return false;}},'CDP');
 await until(async()=>{page=browser.contexts()[0].pages().find(p=>p.url().includes('tauri.localhost'));return !!page;},'Page');
 page.on('pageerror',e=>errors.push(e.message));await page.setViewportSize({width:1440,height:960});await idle();
 prefs=await page.evaluate(()=>localStorage.getItem('minidaw.ui.panels.v1'));
 for(const[id,open]of[['arrangement',true],['media',false],['performance',false],['spectrum',false]]){const b=page.locator('#show-'+id);if((await b.getAttribute('aria-pressed')==='true')!==open)await b.click();}
 await page.evaluate(()=>{const fetch=window.fetch;window.fetch=async function(url,options){if(decodeURIComponent(String(url)).split('/').at(-1)==='edit_project')window.__lastEdit=JSON.parse(options.body).request;return fetch.call(this,url,options);};});
 await invoke('load_audio',{path:resolve('tests/fixtures/stereo-44100.wav')});await idle();
 await page.locator('#project-bpm').fill('137.3');await page.locator('#project-bpm').press('Tab');await idle();
 let c=(await clips())[0];const id=c.clipId,ticksPerSecond=137.3*960000/60;
 // Max existing horizontal zoom, anchored at the left, allows sub-sample mouse edits.
 await fit();let b=await page.locator('#waveform').boundingBox();await page.mouse.move(b.x+.1,b.y+b.height-3);
 await page.keyboard.down('Control');for(let i=0;i<3;i++){await page.mouse.wheel(0,-2000);await sleep(80);}await page.keyboard.up('Control');
 const step=Number(await page.locator('#scroll').getAttribute('step'));assert.ok(step<.00001);
 for(const destination of [123,50,124,51]){
   c=(await clips())[0];const result=await drag(id,destination/ticksPerSecond-seconds(c));
   const tick=Number(result.request.targetTick),after=(await clips())[0];
   assert.equal(result.request.anchorClipId,id);assert.ok(Math.abs(seconds(after)-Math.round(tick/ticksPerSecond*1e9)/1e9)<1e-12);
   assert.ok(Math.abs(tick-destination)<=3,'Mouse destination quantizes to one tick');
   report.moves.push({...result,actualSeconds:seconds(after)});
 }
 assert.ok(report.moves.some(m=>Math.abs(m.actualSeconds*44100-Math.round(m.actualSeconds*44100))>.05),'Free Move is finer than a source sample');
 await fit();await page.locator('#grid-type').selectOption('128');await page.locator('#clip-layer').focus();await page.keyboard.press('j');
 const result=await drag(id,.035);assert.equal(Number(result.request.targetTick)%30000,0);
 const moved=(await clips())[0];await page.keyboard.press('Control+z');await idle();await page.keyboard.press('Control+Shift+z');await idle();assert.deepEqual((await clips())[0],moved);
 const left=await drag(id,.024,'trim-left');c=(await clips())[0];assert.equal(Number(left.request.targetTick)%30000,0);
 assert.ok(Math.abs(seconds(c)-Number(left.request.targetTick)/ticksPerSecond)<=.5/44100+1e-9);
 const beforeRight=structuredClone(c),right=await drag(id,-.026,'trim-right');c=(await clips())[0];
 assert.equal(Number(right.request.targetTick)%30000,0);assert.ok(Math.abs(seconds(c)+(Number(c.sourceEnd)-Number(c.sourceStart))/44100-Number(right.request.targetTick)/ticksPerSecond)<=.5/44100+1e-9);
 assert.equal(c.sourceStart,beforeRight.sourceStart);
 // Free Split resolves one integer tick to the nearest source sample in Rust.
 await page.locator('#clip-layer').focus();await page.keyboard.press('j');await page.locator('[data-command="tool.split"]').click();
 await fit();const layer=await page.locator('#clip-layer').boundingBox(),spp=Number(await page.locator('#scroll').getAttribute('step'));
 const at=seconds(c)+.17321;await page.mouse.move(layer.x+at/spp,layer.y+60);report.splitPreview=await page.locator('#edit-position').textContent();
 await page.mouse.click(layer.x+at/spp,layer.y+60);await idle();const split=await page.evaluate(()=>window.__lastEdit),parts=await clips();assert.equal(parts.length,2);assert.equal(split.cursor.unit,'ticks');
 const expected=Number(c.sourceStart)+Math.round((Number(split.cursor.ticks)/ticksPerSecond-seconds(c))*44100);
 assert.equal(Number(parts[0].sourceEnd),expected);assert.equal(Number(parts[1].sourceStart),expected);
 const stable=(await project()).document;
 await page.locator('#ruler-format').selectOption('seconds');await sleep(80);assert.ok((await page.locator('#edit-position').textContent()).endsWith(' s'));
 await page.locator('#ruler-format').selectOption('bars');await sleep(80);assert.match(await page.locator('#edit-position').textContent(),/\d+\.\d+\.\d+\.\d{6}/);
 assert.deepEqual((await project()).document,stable);
 const path=join(folder,'precision.minidaw');let p=await project();await invoke('save_project',{path,revision:p.revision});await idle();p=await project();await invoke('open_project',{path,revision:p.revision,discard:true});await idle();assert.deepEqual((await project()).document.tracks,stable.tracks);
 await page.screenshot({path:'docs/validation/tick-editing.png'});
 report.checks.push('Sub-sample free Move uses absolute integer tick destinations; live four-part readout.','1/128 snapped Move and both Trim edges, Undo/Redo.','Free Split tick request resolves to nearest original sample; source continuity.','Ruler format changes preserve actual positions; exact project save/open.');
 assert.deepEqual(errors,[]);report.passed=true;
}catch(e){report.error=String(e.stack??e);process.exitCode=1;if(page)await page.screenshot({path:'docs/validation/tick-failure.png'}).catch(()=>{});}
finally{
 if(page&&prefs!==undefined)await page.evaluate(p=>{if(p===null)localStorage.removeItem('minidaw.ui.panels.v1');else localStorage.setItem('minidaw.ui.panels.v1',p);},prefs).catch(()=>{});
 if(browser)await browser.close();if(child?.exitCode===null){const exited=once(child,'exit');child.kill();await exited;}
 if(savedRecent)await writeFile(recent,savedRecent);else await unlink(recent).catch(()=>{});
 await writeFile('docs/validation/tick-ui.json',JSON.stringify(report,null,2));console.log(JSON.stringify(report));
}
