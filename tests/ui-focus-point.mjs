// Real release WebView + TOPPING/WASAPI output. PCM measurements use the existing
// post-Master Spectrum tap, not mocked renderer data or UI playback timers.
import {chromium} from 'playwright-core';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {readFile,writeFile,mkdir,unlink} from 'node:fs/promises';
import {randomUUID,createHash} from 'node:crypto';
import {resolve,join} from 'node:path';
import assert from 'node:assert/strict';
const exe=resolve('src-tauri/target/release/minidaw.exe'),folder=resolve('tests/local/focus-point',randomUUID());await mkdir(folder,{recursive:true});
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
 child=spawn(exe,[],{env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=19267'},stdio:'ignore',windowsHide:true});
 await until(async()=>{try{browser=await chromium.connectOverCDP('http://127.0.0.1:19267');return true;}catch{return false;}},'Release CDP');
 await until(async()=>{page=browser.contexts()[0].pages().find(p=>p.url().includes('tauri.localhost'));return !!page;},'Page');
 page.on('pageerror',e=>errors.push(e.message));await page.setViewportSize({width:1440,height:960});await idle();
 prefs=await page.evaluate(()=>Object.fromEntries(['minidaw.ui.panels.v1','minidaw.ui.fontScale','minidaw.ui.spectrum.v1'].map(k=>[k,localStorage.getItem(k)])));
 for(const[id,show]of [['arrangement',true],['media',false],['performance',false],['spectrum',false],['piano',false],['mixer',true]])await panel(id,show);
 await invoke('new_project',{revision:(await project()).revision,discard:true});await idle();

 await edit({command:'track.add',trackKind:'audio'});const a=(await project()).document.tracks.at(-1).trackId;
 await invoke('load_audio',{path:resolve('tests/generated/long-400s-48000-2ch.wav'),trackId:a});await idle();
 const state=async()=>(await audio()).transport.state;
 const focusTargets=['#play-pause','#stop',`.mixer-channel[data-track-id="${a}"] .mixer-mute`,`.mixer-channel[data-track-id="${a}"] .mixer-fader`,`.mixer-channel[data-track-id="${a}"] .mixer-pan`,'#grid-type','.track-settings-open','.audio-clip'];
 for(const selector of focusTargets){
   const target=page.locator(selector).first();await target.evaluate(e=>{if(e.tabIndex<0)e.tabIndex=0;e.focus();});
   const before=(await audio()).transport.appliedCommand;
   await page.keyboard.press('Space');await until(async()=>await state()==='playing','Space play '+selector);
   assert.equal((await audio()).transport.appliedCommand,before+1);
   await page.keyboard.press('Space');await until(async()=>await state()==='paused','Space pause '+selector);
   assert.equal((await audio()).transport.appliedCommand,before+2);
 }
 assert.equal((await project()).document.tracks[0].mix?.mute??false,false);
 await page.locator('#stop').focus();const count=(await audio()).transport.appliedCommand;
 await page.keyboard.down('Space');await page.keyboard.down('Space');await page.keyboard.up('Space');await sleep(180);assert.equal((await audio()).transport.appliedCommand,count+1);assert.equal(await state(),'playing');
 await page.keyboard.press('Numpad0');await until(async()=>await state()==='stopped','Num0 stop');assert.equal((await audio()).position,0);
 await page.keyboard.press('Enter');await until(async()=>await state()==='playing','Enter play');await page.keyboard.press('Space');await until(async()=>await state()==='paused','Space pause');
 await invoke('transport_command',{action:'seek',seconds:5});await sleep(150);await page.keyboard.press('Home');await page.keyboard.press('End');assert.ok(Math.abs((await audio()).position-5)<.002);
 await page.keyboard.press('NumpadDecimal');await until(async()=>(await audio()).position===0,'Num decimal start');
 await page.keyboard.press('Numpad2');await until(async()=>Math.abs((await audio()).position-2)<.002,'Right locator');await page.keyboard.press('Numpad1');await until(async()=>(await audio()).position===0,'Left locator');
 const cycle=(await project()).document.cycle?.enabled??false;await page.keyboard.press('NumpadDivide');await idle();assert.equal((await project()).document.cycle.enabled,!cycle);await page.keyboard.press('NumpadDivide');await idle();
 await strip(a).locator('.mixer-fader').focus();const snap=await page.locator('#snap-toggle').getAttribute('aria-pressed');await page.keyboard.press('j');assert.notEqual(await page.locator('#snap-toggle').getAttribute('aria-pressed'),snap);await page.keyboard.press('j');
 await mix(a,{volumeDb:-6});await strip(a).locator('.mixer-fader').focus();await page.keyboard.press('Control+z');await idle();assert.equal((await project()).document.tracks[0].mix?.volumeDb??0,0);await page.keyboard.press('Control+Shift+z');await idle();assert.equal((await project()).document.tracks[0].mix.volumeDb,-6);
 const fx=await add(a,'eq');await page.locator('.effect-bypass').focus();await page.keyboard.press('Space');await until(async()=>await state()==='playing','Effects Space');await page.keyboard.press('Space');await until(async()=>await state()==='paused','Effects pause');
 const enabled=(await chain(a))[0].enabled;assert.equal(enabled,true);
 const num=page.locator('[data-param=band-0-gainDb]');await num.fill('3.2');const beforeInput=(await audio()).transport.appliedCommand;await page.keyboard.press('Space');await page.keyboard.press('j');assert.equal((await audio()).transport.appliedCommand,beforeInput);await num.press('Escape');await page.locator('#effect-close').click();
 await page.evaluate(()=>{const div=document.createElement('div');div.id='test-editor';div.contentEditable='true';div.textContent='edit';document.body.append(div);div.focus();});const beforeText=(await audio()).transport.appliedCommand;await page.keyboard.press('Space');await page.keyboard.press('j');await page.keyboard.press('Home');assert.equal((await audio()).transport.appliedCommand,beforeText);assert.match(await page.locator('#test-editor').textContent(),/j/);await page.locator('#test-editor').evaluate(e=>e.remove());
 report.checks.push('Space exactly once across buttons/sliders/select/Track/Clip/Effects; held key suppressed; text/number/contenteditable typing protected; slider J and Undo/Redo; Enter/Num0/Decimal/1/2/Divide; Home/End no transport mapping.');
 // Select a point by mouse without snapping it, then enter an exact sub-grid tick.
 await edit({command:'automation.lane.add',trackIds:[a],parameter:{name:'volumeDb'}});
 const channel=async()=>(await project()).document.automation.find(c=>c.trackId===a),lane=async()=>(await channel()).lanes[0];
 await edit({command:'automation.point.set',trackIds:[a],laneId:(await lane()).laneId,parameter:{name:'volumeDb'},targetTick:'1920000',pointId:'numeric-point',value:-12,shape:'linear'});
 await page.locator(`.track-header[data-track-id="${a}"] .track-automation`).click();await sleep(150);
 const canvas=page.locator(`.automation-canvas[data-track-id="${a}"]`),header=page.locator(`.automation-header[data-track-id="${a}"]`);
 async function xy(p){const b=await canvas.boundingBox(),text=await page.locator('#view-range').textContent(),ts=text.match(/(\d+):(\d+)\.(\d+)/g).map(t=>{const[m,s]=t.split(':');return Number(m)*60+Number(s);});return{x:b.x+(Number(p.tick)/1920000-ts[0])/(ts[1]-ts[0])*b.width,y:b.y+15+(12-p.value)/108*(b.height-40)};}
 const initial=(await lane()).points[0],at=await xy(initial),history=(await project()).history.undo;
 await page.mouse.click(at.x,at.y);await idle();assert.equal((await project()).history.undo,history);assert.deepEqual((await lane()).points[0],initial);
 await header.locator('.automation-point-open').click();const pos=page.locator('#automation-point-position'),value=page.locator('#automation-point-value');
 assert.equal(await pos.inputValue(),'1.3.1.000000');
 await pos.fill('2.3.2.000123');await pos.press('Enter');await idle();assert.equal((await lane()).points[0].tick,'6000123');assert.equal(await pos.inputValue(),'2.3.2.000123');
 await value.fill('-17.375');await value.press('Enter');await idle();assert.equal((await lane()).points[0].value,-17.375);assert.equal((await project()).document.tracks[0].mix.volumeDb,-6);
 await page.getByLabel('Point Position 형식',{exact:true}).selectOption('ticks');assert.equal(await pos.inputValue(),'6000123');await pos.fill('6000124');await pos.press('Tab');await idle();assert.equal((await lane()).points[0].tick,'6000124');
 const oldHistory=(await project()).history.undo;await pos.fill('-1');await pos.press('Enter');assert.equal(await pos.getAttribute('aria-invalid'),'true');assert.equal((await project()).history.undo,oldHistory);await pos.press('Escape');assert.equal(await pos.inputValue(),'6000124');
 await value.fill('999');await value.press('Enter');assert.equal(await value.getAttribute('aria-invalid'),'true');assert.equal((await lane()).points[0].value,-17.375);await value.press('Escape');
 await page.locator('#automation-point-editor strong').click();await page.keyboard.press('Control+z');await idle();assert.equal((await lane()).points[0].tick,'6000123');await page.keyboard.press('Control+Shift+z');await idle();assert.equal((await lane()).points[0].tick,'6000124');
 await page.getByLabel('Point Position 형식',{exact:true}).selectOption('musical');assert.equal(await pos.inputValue(),'2.3.2.000124');
 const beforePointInput=(await audio()).transport.appliedCommand;await pos.focus();await page.keyboard.press('Space');assert.equal((await audio()).transport.appliedCommand,beforePointInput);await pos.press('Escape');
 await page.screenshot({path:'docs/validation/focus-point-ui.png'});
 await page.getByLabel('Point Position 형식',{exact:true}).focus();await page.keyboard.press('Delete');await idle();assert.equal((await lane()).points.length,0);assert.equal((await project()).document.tracks[0].clips.length,1);await page.keyboard.press('Control+z');await idle();assert.equal((await lane()).points[0].tick,'6000124');

 // Existing mouse drag, delete and history retain their point scope.
 const current=(await lane()).points[0],start=await xy(current);await page.mouse.move(start.x,start.y);await page.mouse.down();await page.mouse.move(start.x+36,start.y+8,{steps:12});await page.mouse.up();await idle();assert.notEqual((await lane()).points[0].tick,current.tick);
 await canvas.focus();await page.keyboard.press('Control+z');await idle();assert.equal((await lane()).points[0].tick,current.tick);
 // Registered Arrangement edit commands also work from a Mixer slider.
 await page.locator('.audio-clip').first().click();await strip(a).locator('.mixer-fader').focus();await page.keyboard.press('Control+d');await idle();assert.equal((await project()).document.tracks[0].clips.length,2);await page.keyboard.press('Control+z');await idle();assert.equal((await project()).document.tracks[0].clips.length,1);
 const saved=(await project()).document,file=join(folder,'point.minidaw');await invoke('save_project',{path:file,revision:(await project()).revision});await invoke('open_project',{path:file,revision:(await project()).revision,discard:true});await idle();assert.deepEqual((await project()).document.automation,saved.automation);
 report.checks.push('Mouse selection changes no tick/history; exact musical 2.3.2.000123=6000123 ticks with Snap ON; raw tick edit; exact -17.375 dB; invalid/cancel; Undo/Redo/save/open; existing point drag.');
 assert.deepEqual(errors,[]);assert.equal(await page.locator('#error').isVisible(),false);report.passed=true;
}catch(e){report.error=String(e.stack??e);report.pageErrors=errors;process.exitCode=1;if(page){report.uiError=await page.locator('#error-text').textContent().catch(()=>null);report.lastAudio=await audio().catch(()=>null);report.lastProject=await project().catch(()=>null);await page.screenshot({path:'docs/validation/focus-point-failure.png'}).catch(()=>{});}}
finally{
 if(page&&prefs)await page.evaluate(p=>{for(const[k,v]of Object.entries(p)){if(v===null)localStorage.removeItem(k);else localStorage.setItem(k,v);}},prefs).catch(()=>{});
 if(browser)await browser.close();if(child?.exitCode===null){const exited=once(child,'exit');child.kill();await exited;}
 if(savedRecent)await writeFile(recent,savedRecent);else await unlink(recent).catch(()=>{});
 await writeFile('docs/validation/focus-point-ui.json',JSON.stringify(report,null,2));console.log(JSON.stringify({passed:report.passed,error:report.error,checks:report.checks,trackDeltaDb:report.trackDeltaDb,masterDeltaDb:report.masterDeltaDb,eqBoostDb:report.eqBoostDb}));
}


