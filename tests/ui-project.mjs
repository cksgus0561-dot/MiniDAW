// Real compiled Tauri + Rust + audio output. Only native file-picker answers are
// supplied by the test; all UI handlers, project/engine IPC and filesystem are real.
import {chromium} from 'playwright-core';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {readFile,writeFile,mkdir,copyFile,rename,stat,unlink} from 'node:fs/promises';
import {createReadStream} from 'node:fs';
import {resolve,join} from 'node:path';
import {createHash,randomUUID} from 'node:crypto';
import assert from 'node:assert/strict';
const exe=resolve('src-tauri/target/release/minidaw.exe');
const folder=resolve('tests/local/project-ui',randomUUID());await mkdir(folder,{recursive:true});
const preferences=join(process.env.APPDATA,'local.minidaw.desktop','recent-projects.json');
let recentBefore;try{recentBefore=await readFile(preferences);}catch(e){if(e.code!=='ENOENT')throw e;}
const report={started:new Date().toISOString(),executableSha256:createHash('sha256').update(await readFile(exe)).digest('hex'),folder,checks:[],roundTrips:[],savePerformance:[],limitations:['Native file picker responses supplied by test; project IPC and audio engine are genuine.','Callback PCM delivery measured, not acoustic latency.']};
report.checks.push=function(...items){console.log(items.join('\n'));return Array.prototype.push.apply(this,items);};
let child,browser,page,settingsBefore,fontBefore;const errors=[];const sleep=ms=>new Promise(r=>setTimeout(r,ms));
const invoke=(name,args={})=>page.evaluate(({name,args})=>window.__TAURI_INTERNALS__.invoke(name,args),{name,args});
const snapshot=()=>invoke('engine_snapshot'), project=()=>invoke('project_snapshot');
async function until(fn,message,timeout=15000){const started=Date.now();while(Date.now()-started<timeout){if(await fn())return;await sleep(30);}throw Error(message);}
async function launch(){
 child=spawn(exe,[],{env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=19231'},stdio:'ignore',windowsHide:true});
 for(let i=0;i<150;i++){try{browser=await chromium.connectOverCDP('http://127.0.0.1:19231');break;}catch{await sleep(100);}}
 assert.ok(browser,'WebView connection');
 for(let i=0;i<100;i++){page=browser.contexts()[0].pages().find(p=>p.url().includes('tauri.localhost'));if(page)break;await sleep(100);}assert.ok(page);
 page.on('pageerror',e=>errors.push(e.message));
 await until(async()=>!(await page.locator('#project-save').isDisabled()),'Project UI initialization');
 await page.evaluate(()=>{const original=window.fetch;window.__projectAnswers=[];window.__projectDialogs=[];window.fetch=function(url,options){
  const command=decodeURIComponent(String(url)).split('/').at(-1);
  if(command==='plugin:dialog|save'||command==='plugin:dialog|open'){window.__projectDialogs.push(command);if(!window.__projectAnswers.length)throw Error(`No picker answer for ${command}`);return Promise.resolve(new Response(JSON.stringify(window.__projectAnswers.shift()),{status:200,headers:{'Content-Type':'application/json','Tauri-Response':'ok'}}));}
  return original.call(this,url,options);
 };});
}
async function detach(){if(browser){await browser.close();browser=null;}page=null;}
async function forceClose(){await detach();if(child?.exitCode===null){const exited=once(child,'exit');child.kill();await exited;}}
async function closeClean(){assert.equal((await project()).dirty,false);const exited=once(child,'exit');await menu('project-close');await Promise.race([exited,sleep(8000).then(()=>{throw Error('Clean close blocked');})]);await detach();}
async function answer(value){await page.evaluate(value=>window.__projectAnswers.push(value),value);}
async function menu(id){if(!(await page.locator('#file-menu').getAttribute('open')!==null))await page.locator('#file-menu summary').click();await page.locator(`#${id}`).click();}
async function idle(){await until(async()=>!(await page.locator('#project-save').isDisabled()),'Project operation did not finish');}
async function saveUi(path,as=false){if(path!==undefined)await answer(path);await menu(as?'project-save-as':'project-save');await idle();}
async function newUi(){await menu('project-new');if((await project()).dirty){await page.locator('#project-confirm-discard').click();}await idle();await until(async()=>(await snapshot()).file===null,'New project did not unload');}
async function importUi(path){await answer(path);await page.locator('#open-file').click();await until(async()=>!(await page.locator('#open-file').isDisabled()),'Import unfinished');await until(async()=>(await project()).document.assets.some(a=>a.filename===path.split(/[\\/]/).at(-1)),'Import missing from project');}
async function openUi(path,discard=false){await answer(path);await menu('project-open');if(discard)await page.locator('#project-confirm-discard').click();await idle();}
async function digest(path){const h=createHash('sha256');for await(const chunk of createReadStream(path))h.update(chunk);return h.digest('hex');}
async function clickTransport(action){const stopped=action==='stop'||action==='stopped';await page.locator(stopped?'#stop':'#play-pause').click();await until(async()=>(await snapshot()).transport.state===(stopped?'stopped':action),'Transport '+action);}
async function moveOwned(from,to){assert.ok(resolve(from).startsWith(folder+'\\'));assert.ok(resolve(to).startsWith(folder+'\\'));await rename(from,to);}
try{
 await launch();settingsBefore=(await snapshot()).preferences;fontBefore=await page.locator('#font-scale').inputValue();
 await invoke('apply_audio_settings',{settings:{...settingsBefore,driverType:'wasapi',outputDevice:null,transportDeclick:true}});
 await until(async()=>Boolean((await snapshot()).output),'WASAPI unavailable');
 const initial=await project();assert.equal(initial.dirty,false);assert.equal(initial.path,null);
 await menu('project-new');await idle();assert.notEqual((await project()).document.projectId,initial.document.projectId);
 const media=join(folder,'한글 음악 원본.wav');await copyFile(resolve('tests/fixtures/stereo-44100.wav'),media);const sourceHash=await digest(media);
 await importUi(media);let p=await project();const imported=p;assert.equal(p.dirty,true);assert.equal(p.document.tracks[0].clips.length,1);
 await until(async()=>(await page.locator('#project-name').textContent()).endsWith('*'),'Dirty marker');
 await menu('project-new');await page.locator('#project-confirm-cancel').click();await idle();assert.equal((await project()).document.projectId,p.document.projectId);
 await answer(null);await menu('project-new');await page.locator('#project-confirm-save').click();await idle();assert.equal((await project()).dirty,true);
 const first=join(folder,'첫 노래.minidaw');await answer(first);await menu('project-new');await page.locator('#project-confirm-save').click();await idle();assert.equal((await project()).document.assets.length,0);assert.equal((await snapshot()).file,null);
 const stored=JSON.parse(await readFile(first,'utf8'));assert.equal(stored.projectId,imported.document.projectId);assert.equal(stored.assets[0].path.projectRelativePath,'한글 음악 원본.wav');
 report.checks.push('New Untitled; import creates stable Project/Track/Clip/Asset; dirty *; Cancel and cancelled Save retain project; Save then New unloads audio.');
 await openUi(first);p=await project();assert.equal(p.document.projectId,stored.projectId);assert.deepEqual(p.document.tracks,stored.tracks);await until(async()=>(await snapshot()).waveform?.complete,'Restored waveform');
 await clickTransport('playing');await sleep(140);await clickTransport('paused');const paused=await snapshot();await sleep(80);assert.equal((await snapshot()).position,paused.position);
 await invoke('transport_command',{action:'seek',seconds:2.3});await until(async()=>Math.abs((await snapshot()).position-2.3)<0.001,'Seek after restore');assert.equal((await project()).dirty,false);
 await clickTransport('playing');await clickTransport('stopped');assert.equal((await snapshot()).position,0);
 const sub=join(folder,'다른 위치');await mkdir(sub);const second=join(sub,'Save As.minidaw');await saveUi(second,true);p=await project();assert.equal(p.path,second);assert.equal(p.document.assets[0].path.projectRelativePath,null);assert.equal(p.document.assets[0].path.originalAbsolutePath,media);assert.deepEqual(p.document.tracks,stored.tracks);assert.equal(await digest(media),sourceHash);
 report.checks.push('Open restores IDs/source boundaries/waveform; Play/Pause/Resume/Stop/Seek work; runtime changes stay clean; Save As rebases paths without moving audio.');
 await importUi(resolve('tests/fixtures/stereo-44100.mp3'));p=await project();const oldBytes=await readFile(second);await saveUi(join(folder,'nonexistent','fail.minidaw'),true);
 assert.equal((await project()).path,second);assert.equal((await project()).dirty,true);assert.deepEqual(await readFile(second),oldBytes);
 const broken=join(folder,'broken.minidaw');await writeFile(broken,'{bad');await openUi(broken,true);assert.equal((await project()).document.projectId,p.document.projectId);assert.equal((await project()).dirty,true);assert.ok((await snapshot()).file);
 const newer=join(folder,'newer.minidaw');await writeFile(newer,JSON.stringify({...stored,schemaVersion:999}));await openUi(newer,true);assert.match(await page.locator('#error-text').textContent(),/더 새로운 버전/);
 await menu('project-close');await page.locator('#project-confirm-cancel').click();await idle();assert.equal(child.exitCode,null);
 const exited=once(child,'exit');await menu('project-close');await page.locator('#project-confirm-save').click();await exited;await detach();await launch();
 assert.equal((await snapshot()).file,null);assert.equal((await project()).dirty,false);
 await page.locator('#file-menu summary').click();await page.locator('#recent-projects button').filter({hasText:'Save As.minidaw'}).first().click();await idle();await until(async()=>(await snapshot()).file?.name==='Audio Timeline','Recent restore');
 assert.equal((await project()).document.tracks[0].clips.length,2);report.checks.push('Invalid save/open/newer schema retain dirty project, path and audio; close Cancel/save; real process restart + Recent restores all clips.');
 // Missing asset is exercised by moving only a test-owned copy after closing.
 await newUi();await importUi(media);const missingProject=join(folder,'missing.minidaw');await saveUi(missingProject);const beforeMove=await project();await closeClean();
 const moved=join(folder,'이동한 같은 음악.wav');await moveOwned(media,moved);await launch();await openUi(missingProject);
 p=await project();assert.equal(p.assets[0].status,'missing');assert.equal((await snapshot()).file,null);assert.equal(p.dirty,false);
 await page.locator('#project-assets').click();await until(async()=>await page.locator('#project-media').isVisible(),'Media dialog');await answer(moved);await page.locator('.relink-asset').click();await idle();
 p=await project();assert.equal(p.assets[0].status,'available');assert.equal(p.dirty,true);assert.equal(p.document.assets[0].assetId,beforeMove.document.assets[0].assetId);assert.deepEqual(p.document.tracks,beforeMove.document.tracks);
 const assetId=p.document.assets[0].assetId;await answer(resolve('tests/fixtures/모노-48000.WAV'));await page.locator('.relink-asset').click();await page.locator('#project-confirm-cancel').click();await idle();assert.equal((await project()).assets[0].resolvedPath,moved);
 await answer(resolve('tests/fixtures/모노-48000.WAV'));await page.locator('.relink-asset').click();await page.locator('#project-confirm-replace').click();await idle();assert.deepEqual((await project()).document.tracks,beforeMove.document.tracks);assert.equal((await snapshot()).file,null);
 await answer(moved);await page.locator('.relink-asset').click();await page.locator('#project-confirm-replace').click();await idle();assert.equal((await project()).document.assets[0].assetId,assetId);assert.ok((await snapshot()).file);
 await page.locator('#close-project-media').click();await saveUi();assert.equal(await digest(moved),sourceHash);
 report.checks.push('Closed-app source move → Missing project opens; UI Relink keeps asset/clip IDs and edit fields; mismatch Cancel/explicit replacement; source hash unchanged.');
 // Real save/close/restart/open for all supported codecs.
 for(const extension of ['wav','mp3','flac']){
  await newUi();const source=resolve(`tests/fixtures/stereo-44100.${extension}`);const beforeHash=await digest(source);await importUi(source);const path=join(folder,`roundtrip-${extension}.minidaw`);await saveUi(path);const before=await project();await closeClean();await launch();await openUi(path);
  const after=await project();assert.deepEqual(after.document,before.document);await until(async()=>(await snapshot()).waveform?.complete,'Waveform recreation');await clickTransport('playing');await sleep(100);await clickTransport('paused');
  const engine=await snapshot();assert.ok(engine.metrics.nonSilentFrames>0);assert.equal(engine.source.mode,'Memory');assert.equal(engine.streamErrors,0);assert.equal(await digest(source),beforeHash);
  report.roundTrips.push({extension,projectBytes:(await stat(path)).size,sourceHash:beforeHash,document:after.document,engine});
 }
 report.checks.push('WAV/MP3/FLAC each save → real process exit/restart → open: exact document, source range, derived waveform and Memory playback; original hashes unchanged.');
 await newUi();const large=resolve('tests/generated/long-1200s-48000-2ch.wav');const largeHash=await digest(large);await importUi(large);const largeProject=join(folder,'large-streaming.minidaw');await saveUi(largeProject);await closeClean();await launch();await openUi(largeProject);
 await until(async()=>(await snapshot()).waveform?.complete,'Large waveform',120000);assert.equal((await snapshot()).source.mode,'Streaming');assert.ok((await stat(large)).size>400_000_000);assert.ok((await stat(largeProject)).size<8192);
 for(const backend of ['wasapi','asio']){
  const settings={...(await snapshot()).preferences,driverType:backend,transportDeclick:true};if(backend==='asio'){settings.asioDriver=(await invoke('output_devices',{driverType:'asio'}))[0];assert.ok(settings.asioDriver);}
  await invoke('apply_audio_settings',{settings});await until(async()=>(await snapshot()).output?.driverType===backend,'Backend switch');await clickTransport('playing');await sleep(300);const before=await snapshot();
  const times=[];for(let n=0;n<30;n++){const p=await project();const started=performance.now();await invoke('save_project',{path:null,revision:p.revision});times.push(performance.now()-started);}
  await sleep(300);const after=await snapshot();assert.equal(after.transport.appliedCommand,before.transport.appliedCommand);assert.ok(after.position>before.position);assert.equal(after.waveform.buildMs,before.waveform.buildMs);assert.equal(after.streamErrors,0);assert.equal(after.metrics.overruns,0);assert.equal(after.metrics.deviceCallbackOverruns,0);assert.equal(after.source.starvation,0);assert.equal((await project()).dirty,false);
  report.savePerformance.push({backend,saveAverageMs:times.reduce((a,b)=>a+b,0)/times.length,saveMaxMs:Math.max(...times),before,after});
  await clickTransport('paused');const box=await page.locator('#waveform').boundingBox();const commandBefore=(await snapshot()).transport.appliedCommand;await page.mouse.move(box.x+box.width*.2,box.y+12);await page.mouse.down();await page.mouse.move(box.x+box.width*.7,box.y+12,{steps:100});assert.equal((await snapshot()).transport.appliedCommand,commandBefore);await page.mouse.up();await until(async()=>(await snapshot()).transport.appliedCommand===commandBefore+1,'Drag commit');assert.equal((await project()).dirty,false);
 }
 assert.equal(await digest(large),largeHash);report.large={sourceBytes:(await stat(large)).size,projectBytes:(await stat(largeProject)).size,sourceHash:largeHash};
 report.checks.push('460 MB Streaming asset saves as small metadata project; restart/restore/playback; 30 saves while playing on WASAPI and ASIO: no seek/transport command/cache rebuild/overruns/starvation; 100 drag moves=0 seeks, up=1.');
 await invoke('apply_audio_settings',{settings:settingsBefore});
 for(const font of ['90','110','125']){
  await page.locator('#font-scale').selectOption(font);await page.locator('#project-assets').click();assert.ok(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));await page.screenshot({path:resolve(`docs/validation/project-font-${font}.png`)});await page.locator('#close-project-media').click();
 }
 await page.locator('#font-scale').selectOption(fontBefore);assert.equal((await project()).dirty,false);report.checks.push('Font/Audio preferences remain separate; project UI 90/110/125% layouts; no WebView errors.');
 assert.deepEqual(errors,[]);report.passed=true;
}catch(e){report.error=String(e.stack??e);if(page){await page.screenshot({path:resolve('docs/validation/project-failure.png')}).catch(()=>{});report.lastProject=await project().catch(()=>null);report.lastEngine=await snapshot().catch(()=>null);}throw e;}
finally{
 if(page&&settingsBefore)await invoke('apply_audio_settings',{settings:settingsBefore}).catch(()=>{});
 if(page&&fontBefore)await page.locator('#font-scale').selectOption(fontBefore).catch(()=>{});
 await forceClose();
 if(recentBefore!==undefined)await writeFile(preferences,recentBefore);else await unlink(preferences).catch(e=>{if(e.code!=='ENOENT')throw e;});
 report.jsErrors=errors;await writeFile('docs/validation/project-ui.json',JSON.stringify(report,null,2));console.log(JSON.stringify({passed:report.passed,error:report.error,checks:report.checks,large:report.large,savePerformance:report.savePerformance.map(x=>({backend:x.backend,saveAverageMs:x.saveAverageMs,saveMaxMs:x.saveMaxMs}))}));
}
