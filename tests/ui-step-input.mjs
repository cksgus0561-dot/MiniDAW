// Piano Roll step input through actual WebView key events -> Tauri -> project.
// MINIDAW_EXE selects the binary (default: release). A debug binary loads the
// Vite dev server, which must already be running on 127.0.0.1:1420.
import {chromium} from 'playwright-core';
import {spawn} from 'node:child_process';
import {readFile,writeFile,unlink} from 'node:fs/promises';
import {resolve,join} from 'node:path';
import assert from 'node:assert/strict';
const sleep=ms=>new Promise(r=>setTimeout(r,ms)),report={checks:[]};
const recent=join(process.env.APPDATA,'local.minidaw.desktop/recent-projects.json');let savedRecent;try{savedRecent=await readFile(recent);}catch{}
let child,browser,page,prefs;const errors=[];
const invoke=(name,args={})=>page.evaluate(({name,args})=>window.__TAURI_INTERNALS__.invoke(name,args),{name,args});
const project=()=>invoke('project_snapshot');
async function until(fn,label,ms=15000){const at=Date.now();while(Date.now()-at<ms){if(await fn())return;await sleep(30);}throw Error(label);}
async function idle(){await until(async()=>!await page.locator('#project-save').isDisabled(),'Project idle');await sleep(180);}
async function edit(request){await invoke('edit_project',{revision:(await project()).revision,request});await idle();}
async function panel(id,show){if((await page.locator('#show-'+id).getAttribute('aria-pressed')==='true')!==show)await page.locator('#show-'+id).click();}
const pressed=id=>page.locator('#'+id).getAttribute('aria-pressed');
const clip=async()=>(await project()).document.tracks.flatMap(t=>t.clips).find(c=>c.kind==='midi');
const notes=async()=>{const c=await clip();return c.notes.map(n=>({start:Number(n.startTick)-Number(c.contentOffsetTick??0),length:Number(n.lengthTick),pitch:n.pitch,velocity:n.velocity})).sort((a,b)=>a.start-b.start||a.pitch-b.pitch);};
async function count(n){await until(async()=>(await clip()).notes.length===n,'note count '+n);await idle();}
async function play(...keys){for(const k of keys)await page.keyboard.down(k);await sleep(40);for(const k of keys)await page.keyboard.up(k);}
const STEP=240000; // 1/16 at 960000 PPQ
try {
 child=spawn(resolve(process.env.MINIDAW_EXE??'src-tauri/target/release/minidaw.exe'),[],{env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=19281'},stdio:'ignore',windowsHide:true});
 await until(async()=>{try{browser=await chromium.connectOverCDP('http://127.0.0.1:19281');return true;}catch{return false;}},'CDP',60000);
 await until(async()=>{page=browser.contexts()[0]?.pages().find(p=>/tauri\.localhost|127\.0\.0\.1:1420/.test(p.url()));return !!page;},'WebView',60000);
 page.on('pageerror',e=>errors.push(e.message));await page.setViewportSize({width:1440,height:960});await idle();
 prefs=await page.evaluate(()=>Object.fromEntries(Object.keys(localStorage).filter(k=>k.startsWith('minidaw.ui.')).map(k=>[k,localStorage.getItem(k)])));
 // Default shortcuts + Korean UI, then reload so every module reads them.
 await page.evaluate(()=>{localStorage.removeItem('minidaw.ui.shortcuts.v1');localStorage.setItem('minidaw.ui.language.v1','ko');});await page.reload();await idle();
 await invoke('new_project',{revision:(await project()).revision,discard:true});await idle();
 for(const id of ['media','performance','mixer','spectrum'])await panel(id,false);await panel('arrangement',true);
 await page.locator('#midi-track-add').click();await idle();const t=(await project()).document.tracks.at(-1).trackId;
 await edit({command:'midi.clip.add',trackIds:[t],targetTick:'0',lengthTick:'3840000'});const c=(await clip()).clipId;
 await page.locator('#grid-type').selectOption('16');if(await pressed('snap-toggle')!=='true')await page.locator('#snap-toggle').click();
 await page.locator(`[data-clip-id="${c}"]`).click();await page.locator('#piano-open').click();await idle();
 await until(async()=>/MIDI Part/.test(await page.locator('#piano-clip-name').textContent()),'Piano Roll Part');

 // Turning step input ON also enables Computer Keys and targets the Part's Track.
 assert.equal(await pressed('computer-midi-toggle'),'false');
 await page.locator('#piano-step').click();
 assert.equal(await pressed('piano-step'),'true');assert.equal(await pressed('computer-midi-toggle'),'true');
 assert.ok(await page.locator('#piano-step-info').isVisible());
 assert.match(await page.locator('#computer-midi-track').textContent(),/MIDI/);
 await page.locator('#piano-canvas').focus();
 await play('a');await count(1);
 assert.deepEqual(await notes(),[{start:0,length:STEP,pitch:60,velocity:100}]);
 report.checks.push('Step ON enables Computer Keys; A writes C3 (60) at the cursor with Grid length 1/16; silent Track still records.');

 // Keys held together are one chord and one history step.
 const r0=(await project()).revision;await play('a','d','g');await count(4);
 assert.equal((await project()).revision,r0+1);
 assert.deepEqual((await notes()).slice(1).map(n=>[n.start,n.pitch]),[[STEP,60],[STEP,64],[STEP,67]]);
 report.checks.push('A+D+G held together -> one C-E-G chord at step 2 in a single transaction.');

 // Right arrow = rest, Backspace removes the last step and moves back.
 await page.keyboard.press('ArrowRight');await play('k');await count(5);
 assert.deepEqual((await notes()).at(-1),{start:3*STEP,length:STEP,pitch:72,velocity:100});
 await page.keyboard.press('Backspace');await count(4);await play('j');await count(5);
 assert.deepEqual((await notes()).at(-1),{start:3*STEP,length:STEP,pitch:71,velocity:100});
 report.checks.push('→ inserts a rest; Backspace deletes the last step (not the selection command) and re-entry lands on the same step.');

 // Ruler click places the cursor (Snap ON); entering past the Part end extends it.
 const box=await page.locator('#piano-canvas').boundingBox();
 await page.mouse.click(box.x+box.width-6,box.y+12);await play('s');await count(6);
 const tail=(await notes()).find(n=>n.pitch===62);assert.ok(tail.start>=3840000-STEP&&tail.start%STEP===0,JSON.stringify(tail));
 await play('f');await count(7);assert.ok(Number((await clip()).lengthTick)>3840000,'Part extended');
 report.checks.push('Ruler click places the snapped cursor; continuing past the Part end extends the Part.');
 await page.locator('#piano-fit').click();await page.keyboard.down('h');await sleep(250);await page.screenshot({path:'docs/validation/step-input-ui.png'});await page.keyboard.up('h');await count(8);
 await page.keyboard.press('Backspace');await count(7);

 // Undo is one chord/step at a time through the existing History.
 // Backspace removal and step entry are ordinary History entries.
 await page.keyboard.press('Control+z');await count(8);assert.ok((await notes()).some(n=>n.pitch===69),'Backspace undone');
 await page.keyboard.press('Control+z');await count(7);await page.keyboard.press('Control+z');await count(6);assert.ok(!(await notes()).some(n=>n.pitch===65),'F step undone');
 report.checks.push('Ctrl+Z undoes step entries and Backspace removals one at a time through the shared History.');

 // Esc exits; Computer Keys keep playing but write nothing; Backspace is Delete again.
 await page.keyboard.press('Escape');assert.equal(await pressed('piano-step'),'false');assert.ok(!await page.locator('#piano-step-info').isVisible());
 const r1=(await project()).revision;await play('a');await sleep(300);assert.equal((await project()).revision,r1);
 await page.locator('#piano-canvas').focus();await page.keyboard.press('Control+a');await page.keyboard.press('Backspace');await count(0);
 report.checks.push('Esc turns step input OFF: keys only audition; Backspace returns to selection Delete.');

 // Text fields are protected while ON.
 await page.locator('#piano-step').click();await page.locator('#piano-velocity').focus();await page.keyboard.press('Backspace');await play('a');await sleep(300);
 assert.equal((await clip()).notes.length,0);await page.locator('#piano-step').click();
 report.checks.push('Numeric/text fields keep their keys while step input is ON.');
 assert.deepEqual(errors,[]);report.passed=true;
} catch(e){report.passed=false;report.error=String(e?.stack??e);try{report.url=page.url();report.switches=await page.evaluate(()=>[...document.querySelectorAll('#panel-switches button')].map(b=>b.id).join(','));}catch{}try{await page?.screenshot({path:'docs/validation/step-input-failure.png'});}catch{}}
finally {
 try{if(prefs)await page.evaluate(p=>{for(const k of Object.keys(localStorage))if(k.startsWith('minidaw.ui.'))localStorage.removeItem(k);for(const[k,v]of Object.entries(p))localStorage.setItem(k,v);},prefs);}catch{}
 try{await invoke('new_project',{revision:(await project()).revision,discard:true});}catch{}
 try{await browser?.close();}catch{}child?.kill();await sleep(500);
 if(savedRecent)await writeFile(recent,savedRecent);else await unlink(recent).catch(()=>{});
}
await writeFile('docs/validation/step-input-ui.json',JSON.stringify(report,null,1));console.log(JSON.stringify(report,null,1));process.exit(report.passed?0:1);
