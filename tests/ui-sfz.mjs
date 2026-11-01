// Built-in SFZ instrument through the release WebView -> Tauri -> sample loading.
// SFZ_PATH selects the instrument (default: Salamander Grand Piano V3 on D:).
// The native file dialog is not driven; the test sends the same edit request.
import {chromium} from 'playwright-core';
import {spawn} from 'node:child_process';
import {readFile,writeFile,unlink} from 'node:fs/promises';
import {existsSync} from 'node:fs';
import {resolve,join} from 'node:path';
import assert from 'node:assert/strict';
const sfz=process.env.SFZ_PATH??'D:\\Salamander Grand Piano V3\\SalamanderGrandPianoV3_44.1khz16bit\\SalamanderGrandPianoV3.sfz';
if(!existsSync(sfz)){console.log(JSON.stringify({skipped:'SFZ not found: '+sfz}));process.exit(0);}
const sleep=ms=>new Promise(r=>setTimeout(r,ms)),report={checks:[]};
const recent=join(process.env.APPDATA,'local.minidaw.desktop/recent-projects.json');let savedRecent;try{savedRecent=await readFile(recent);}catch{}
let child,browser,page,prefs;const errors=[];
const invoke=(name,args={})=>page.evaluate(({name,args})=>window.__TAURI_INTERNALS__.invoke(name,args),{name,args});
const project=()=>invoke('project_snapshot');
async function until(fn,label,ms=15000){const at=Date.now();while(Date.now()-at<ms){if(await fn())return;await sleep(30);}throw Error(label);}
async function idle(){await until(async()=>!await page.locator('#project-save').isDisabled(),'Project idle',60000);await sleep(180);}
async function edit(request){await invoke('edit_project',{revision:(await project()).revision,request});await idle();}
async function panel(id,show){if((await page.locator('#show-'+id).getAttribute('aria-pressed')==='true')!==show)await page.locator('#show-'+id).click();}
try {
 child=spawn(resolve(process.env.MINIDAW_EXE??'src-tauri/target/release/minidaw.exe'),[],{env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=19282'},stdio:'ignore',windowsHide:true});
 // Attach only after the app page has loaded: attaching to the initial blank
 // WebView misses its navigation to the dev server.
 await until(async()=>{try{return (await (await fetch('http://127.0.0.1:19282/json/list')).json()).some(t=>t.type==='page'&&t.title==='MiniDAW');}catch{return false;}},'App page',90000);
 await until(async()=>{try{browser=await chromium.connectOverCDP('http://127.0.0.1:19282');return true;}catch{return false;}},'CDP',60000);
 await until(async()=>{page=browser.contexts()[0]?.pages().find(p=>/tauri\.localhost|127\.0\.0\.1:1420/.test(p.url()));return !!page;},'WebView',60000);
 page.on('pageerror',e=>errors.push(e.message));await page.setViewportSize({width:1440,height:960});await idle();
 prefs=await page.evaluate(()=>Object.fromEntries(Object.keys(localStorage).filter(k=>k.startsWith('minidaw.ui.')).map(k=>[k,localStorage.getItem(k)])));
 await page.evaluate(()=>localStorage.setItem('minidaw.ui.language.v1','ko'));await page.reload();await idle();
 await invoke('new_project',{revision:(await project()).revision,discard:true});await idle();
 for(const id of ['media','performance','mixer','spectrum','piano'])await panel(id,false);await panel('arrangement',true);
 await page.locator('#midi-track-add').click();await idle();const t=(await project()).document.tracks.at(-1).trackId;
 const select=page.locator(`.track-header[data-track-id="${t}"] .midi-instrument`);
 const options=await select.locator('option').evaluateAll(o=>o.map(x=>[x.value,x.textContent]));
 assert.deepEqual(options.map(o=>o[0]),['none','basicSynth','chooseSfz','choose']);
 assert.equal(options[2][1],'SFZ 파일…');
 report.checks.push('Instrument menu: 악기 없음 / MiniDAW Synth / SFZ 파일… / VST3 / CLAP 선택…');

 const started=Date.now();
 await edit({command:'midi.track.instrument',trackIds:[t],instrument:'sfz',path:sfz});
 const seconds=(Date.now()-started)/1000;
 const doc=(await project()).document.tracks.find(x=>x.trackId===t);
 assert.equal(doc.instrument,'sfz');assert.equal(doc.sfz,sfz);
 assert.equal(await select.inputValue(),'sfz');
 assert.match(await select.locator('option[value="sfz"]').textContent(),/^SFZ · SalamanderGrandPianoV3$/);
 await until(async()=>/Regions/.test(await select.getAttribute('title')??''),'status title');
 const title=await select.getAttribute('title');
 assert.ok(await page.locator(`.track-header[data-track-id="${t}"] .sfz-warning`).isHidden());
 const status=await invoke('sfz_status',{path:sfz});
 assert.ok(status.loaded&&status.regions>600&&!status.error&&!status.unsupported.length&&!status.missing.length);
 report.load={seconds,title};
 report.checks.push(`Choosing the .sfz loads it (${seconds.toFixed(1)} s incl. UI round trip); menu shows "SFZ · SalamanderGrandPianoV3"; no warnings.`);
 await page.screenshot({path:'docs/validation/sfz-ui.png'});

 // Switching away and back keeps the file; a second load uses the cache.
 await select.selectOption('basicSynth');await idle();
 assert.equal((await project()).document.tracks.find(x=>x.trackId===t).sfz,sfz);
 const again=Date.now();await select.selectOption('sfz');await idle();
 assert.equal(await select.inputValue(),'sfz');report.load.cachedSeconds=(Date.now()-again)/1000;
 assert.ok(report.load.cachedSeconds<3,String(report.load.cachedSeconds));
 report.checks.push(`MiniDAW Synth -> SFZ again keeps the path; cached reselect ${report.load.cachedSeconds.toFixed(1)} s.`);

 // A missing file never breaks the project: silent Track, visible warning.
 const gone=sfz.replace(/[^\\]+$/,'missing-instrument.sfz');
 await edit({command:'midi.track.instrument',trackIds:[t],instrument:'sfz',path:gone});
 const warn=page.locator(`.track-header[data-track-id="${t}"] .sfz-warning`);
 await until(async()=>await warn.isVisible(),'missing warning');
 assert.equal(await warn.textContent(),'⚠ SFZ');assert.match(await warn.getAttribute('title'),/SFZ/);
 report.checks.push('Missing .sfz: project stays usable, header shows ⚠ SFZ with the reason.');
 assert.deepEqual(errors,[]);report.passed=true;
} catch(e){report.passed=false;report.error=String(e?.stack??e);try{await page?.screenshot({path:'docs/validation/sfz-failure.png'});}catch{}}
finally {
 try{if(prefs)await page.evaluate(p=>{for(const k of Object.keys(localStorage))if(k.startsWith('minidaw.ui.'))localStorage.removeItem(k);for(const[k,v]of Object.entries(p))localStorage.setItem(k,v);},prefs);}catch{}
 try{await invoke('new_project',{revision:(await project()).revision,discard:true});}catch{}
 try{await browser?.close();}catch{}child?.kill();await sleep(500);
 if(savedRecent)await writeFile(recent,savedRecent);else await unlink(recent).catch(()=>{});
}
await writeFile('docs/validation/sfz-ui.json',JSON.stringify(report,null,1));console.log(JSON.stringify(report,null,1));process.exit(report.passed?0:1);
