import {chromium} from 'playwright-core';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {readFile,writeFile,unlink} from 'node:fs/promises';
import {resolve,join} from 'node:path';
import assert from 'node:assert/strict';
const sleep=ms=>new Promise(r=>setTimeout(r,ms)),report={checks:[]},errors=[];
let child,browser,page,prefs;const recent=join(process.env.APPDATA,'local.minidaw.desktop/recent-projects.json');let savedRecent;try{savedRecent=await readFile(recent);}catch{}
async function until(f,name){for(let n=0;n<400;n++){if(await f())return;await sleep(60);}throw Error(name);}
const call=(p,name,args={})=>p.evaluate(({name,args})=>window.__TAURI_INTERNALS__.invoke(name,args),{name,args});
const invoke=(n,a)=>call(page,n,a),project=()=>invoke('project_snapshot');
async function idle(){await until(async()=>!await page.locator('#project-save').isDisabled(),'main idle');await sleep(120);}
async function edit(request){await invoke('edit_project',{revision:(await project()).revision,request});await idle();}
async function start(){
 child=spawn(resolve('src-tauri/target/release/minidaw.exe'),[],{env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=19279'},windowsHide:true,stdio:'ignore'});
 await until(async()=>{try{browser=await chromium.connectOverCDP('http://127.0.0.1:19279');return true;}catch{return false;}},'CDP');
 browser.contexts()[0].on('page',p=>p.on('pageerror',e=>errors.push(p.url()+': '+e.message)));
 await until(async()=>{page=browser.contexts()[0].pages().find(p=>p.url().includes('tauri.localhost')&&!p.url().includes('?'));return !!page;},'main');
 page.on('pageerror',e=>errors.push(e.message));await idle();
}
async function quit(){await browser?.close();browser=null;if(child?.exitCode===null){const exited=once(child,'exit');child.kill();await exited;}child=null;}
async function open(id){if(await page.locator('#show-'+id).getAttribute('aria-pressed')!=='true')await page.locator('#show-'+id).click();}
async function detached(id){await open(id);await page.locator('#detach-'+id).click();let p;
 await until(async()=>{p=browser.contexts()[0].pages().find(p=>p.url().includes('?panel='+id));return !!p;},'window '+id);
 await until(async()=>await p.locator('.floating-panel').isVisible(),'panel '+id);await sleep(400);
 await until(async()=>await page.locator('#panel-'+id).isHidden(),'main detached '+id);return p;
}
const placement=()=>page.evaluate(()=>JSON.parse(localStorage.getItem('minidaw.ui.windows.v1')));
function sameCamera(a,b,message){assert.equal(a.format,b.format,message);assert.equal(a.start,b.start,message);assert.equal(a.vertical,b.vertical,message);assert.ok(Math.abs(Number(a.value)-Number(b.value))<=Math.max(Number(a.step),Number(b.step)),message);assert.ok(Math.abs(a.span-b.span)<.001,message);}
const camera=p=>p.evaluate(()=>({value:document.getElementById('piano-scroll').value,step:document.getElementById('piano-scroll').step,span:Number(document.getElementById('piano-scroll').step)*(document.getElementById('piano-canvas').clientWidth-56),vertical:document.getElementById('piano-vscroll').scrollTop,format:document.getElementById('note-time-format').value,start:document.getElementById('note-start').value}));
try{
 await start();prefs=await page.evaluate(()=>Object.fromEntries(Object.entries(localStorage).filter(([k])=>k.startsWith('minidaw.ui.'))));
 await page.evaluate(()=>{localStorage.removeItem('minidaw.ui.windows.v1');localStorage.removeItem('minidaw.ui.zones.v1');localStorage.removeItem('minidaw.ui.panels.v1');localStorage.removeItem('minidaw.ui.shortcuts.v1');localStorage.setItem('minidaw.ui.language.v1','ko');});await page.reload();await idle();
 await invoke('new_project',{revision:(await project()).revision,discard:true});await idle();
 await invoke('load_audio',{path:resolve('tests/fixtures/stereo-44100.wav'),trackId:null});await idle();
 await edit({command:'midi.track.add'});let p=await project(),track=p.document.tracks.at(-1).trackId;
 await edit({command:'midi.clip.add',trackIds:[track],targetTick:'1920000',lengthTick:'7680000'});let clip=(await project()).document.tracks.at(-1).clips[0].clipId;
 await edit({command:'midi.note.add',clipIds:[clip],targetTick:'2400000',lengthTick:'480000',pitch:64});
 await page.locator(`[data-clip-id="${clip}"]`).click();await page.locator('#piano-open').click();await idle();
 await page.locator('#piano-canvas').focus();await page.keyboard.press('Control+a');await page.locator('#note-time-format').selectOption('ticks');await page.locator('#piano-zoom-in').click();
 await page.locator('#piano-scroll').evaluate(e=>{e.value='1200000';e.dispatchEvent(new Event('input'));});await sleep(100);
 const original=await camera(page),doc=JSON.stringify((await project()).document);
 const piano=await detached('piano');sameCamera(await camera(piano),original,'Piano state handed to native window');
 await piano.locator('#note-start').fill('4320123');await piano.locator('#note-start').press('Enter');await until(async()=>(await project()).document.tracks.at(-1).clips[0].notes[0].startTick==='2400123','remote edit');
 await piano.locator('#piano-canvas').focus();await piano.keyboard.press('Control+z');await until(async()=>(await project()).document.tracks.at(-1).clips[0].notes[0].startTick==='2400000','remote undo');await piano.keyboard.press('Control+Shift+z');await until(async()=>(await project()).document.tracks.at(-1).clips[0].notes[0].startTick==='2400123','remote redo');
 const changed=await camera(piano);await piano.locator('#panel-dock').click();await until(async()=>await page.locator('#panel-piano').isVisible(),'dock piano');sameCamera(await camera(page),changed,'Piano state returned to Zone');
 await detached('piano');
 const mixer=await detached('mixer'),spectrum=await detached('spectrum'),performance=await detached('performance');
 assert.equal((await placement()).piano.detached,true);assert.equal(await page.locator('#zone-lower').isVisible(),false);assert.equal(await page.locator('#zone-right').isVisible(),false);
 assert.equal(browser.contexts()[0].pages().filter(p=>p.url().includes('?panel=')).length,4);
 report.checks.push('Four native WebViews coexist; right/lower Zones return space. Piano selection/camera survives detach/dock, numeric edit and Undo/Redo change shared Rust project.');
 const volume=mixer.locator('#mixer-tracks .mixer-volume').first();await volume.fill('-7.3');await volume.press('Enter');await until(async()=>(await project()).document.tracks[0].mix?.volumeDb===-7.3,'remote mixer');
 await edit({command:'track.mix',trackIds:[(await project()).document.tracks[0].trackId],volumeDb:-3.2});await until(async()=>await volume.inputValue()==='-3.2','mixer reverse sync');
 await spectrum.locator('#spectrum-fft').selectOption('8192');await spectrum.locator('#spectrum-channel').selectOption('1');
 await piano.locator('#piano-canvas').focus();await piano.keyboard.press('Space');await until(async()=>(await invoke('engine_snapshot')).transport.state==='playing','Space from Piano');
 await until(async()=>(await invoke('spectrum_snapshot',{after:0})).frame?.fftSize===8192,'live spectrum');
 await sleep(1200);report.spectrumFps=await performance.locator('#metric-spectrum-fps').textContent();report.callback=await performance.locator('#metric-avg').textContent().catch(()=>null);
 assert.ok(Number(report.spectrumFps)>30,report.spectrumFps);
 await mixer.locator('.mixer-inserts').first().focus();await mixer.keyboard.press('Space');await until(async()=>(await invoke('engine_snapshot')).transport.state==='paused','Space from Mixer button');
 await spectrum.locator('#panel-dock').click();await until(async()=>await page.locator('#panel-spectrum').isVisible(),'dock spectrum');assert.equal(await page.locator('#spectrum-fft').inputValue(),'8192');assert.equal(await page.locator('#spectrum-channel').inputValue(),'1');
 await until(async()=>(await invoke('spectrum_snapshot',{after:0})).enabled===true,'worker remains enabled after dock');
 await sleep(1000);for(let n=0;n<12;n++){assert.ok(Number(await performance.locator('#metric-spectrum-fps').textContent())>30,'hidden native Spectrum must not overwrite docked FPS');await sleep(100);}
 await detached('spectrum');
 await spectrum.locator('#panel-spectrum [data-panel-close]').click();await until(async()=>!(await placement()).spectrum.open,'close floating');await sleep(150);assert.equal((await invoke('spectrum_snapshot',{after:0})).enabled,false);
 await page.locator('#show-spectrum').click();await until(async()=>(await invoke('spectrum_snapshot',{after:0})).enabled===true,'reopen floating');
 assert.equal(await spectrum.locator('#spectrum-fft').inputValue(),'8192');
 await page.locator('#font-scale').selectOption('125');await until(async()=>await piano.evaluate(()=>document.documentElement.style.getPropertyValue('--ui-font-scale'))==='1.25','font sync');
 report.checks.push('Mixer edits synchronize both ways. Space works from Piano/Mixer buttons. Spectrum live FFT/FPS, settings transfer, close stops worker and reopening resumes. Font preference updates across windows.');
 await page.locator('#open-app-settings').click();await page.locator('#ui-language').selectOption('en');await page.locator('#shortcut-search').fill('transport.toggle');await page.locator('[data-shortcut="transport.toggle"]').click();await page.locator('#shortcut-capture').focus();await page.keyboard.press('Control+Alt+p');await page.locator('#shortcut-assign').click();await page.locator('#app-settings-close').click();
 await until(async()=>await piano.locator('#panel-dock').textContent()==='Dock to Zone','live language');
 await mixer.locator('.mixer-inserts').first().focus();await mixer.keyboard.press('Control+Alt+p');await until(async()=>(await invoke('engine_snapshot')).transport.state==='playing','remap applies remotely');await mixer.keyboard.press('Control+Alt+p');await until(async()=>(await invoke('engine_snapshot')).transport.state==='paused','remap pause');
 await piano.locator('#note-start').focus();await piano.keyboard.press('Control+Alt+p');assert.equal((await invoke('engine_snapshot')).transport.state,'paused','numeric input keeps keyboard priority');await piano.keyboard.press('Escape');
 report.checks.push('Live Korean/English and shortcut remapping reach detached windows; numeric text focus still suppresses global transport.');
 await piano.screenshot({path:'docs/validation/floating-piano.png'});await mixer.screenshot({path:'docs/validation/floating-mixer.png'});await spectrum.screenshot({path:'docs/validation/floating-spectrum.png'});await page.screenshot({path:'docs/validation/floating-main.png'});
 if(process.env.MINIDAW_NATIVE_CHECK==='1'){console.log('READY FOR NATIVE WINDOW CHECK');for(let n=0;n<300;n++){try{await readFile('tmp/native-window-check.done');break;}catch{}await sleep(1000);}}
 const saved=await placement();report.placement=saved;
 await quit();await start();await until(async()=>browser.contexts()[0].pages().filter(p=>p.url().includes('?panel=')).length===4,'restart windows');await sleep(1000);
 const restored=await placement();for(const id of ['piano','mixer','performance','spectrum']){assert.equal(restored[id].detached,true);assert.equal(restored[id].open,true);assert.equal(restored[id].width,saved[id].width);assert.equal(restored[id].height,saved[id].height);}
 report.checks.push('Real process restart restores all four independent windows and dimensions.');
 assert.deepEqual(errors,[]);assert.equal(await page.locator('#error').isVisible(),false);report.passed=true;
}catch(e){report.error=String(e.stack??e);report.errors=errors;process.exitCode=1;if(page){report.uiError=await page.locator('#error-text').textContent().catch(()=>null);report.placement=await placement().catch(()=>null);await page.screenshot({path:'docs/validation/floating-failure.png'}).catch(()=>{});}}
finally{
 if(page&&prefs)await page.evaluate(p=>{for(const k of Object.keys(localStorage).filter(k=>k.startsWith('minidaw.ui.')))localStorage.removeItem(k);for(const[k,v]of Object.entries(p))localStorage.setItem(k,v);},prefs).catch(()=>{});
 await quit();if(savedRecent)await writeFile(recent,savedRecent);else await unlink(recent).catch(()=>{});
 await writeFile('docs/validation/floating-panels-ui.json',JSON.stringify(report,null,2));console.log(JSON.stringify(report));
}



