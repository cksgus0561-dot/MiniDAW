import {chromium} from 'playwright-core';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {readFile,writeFile,unlink} from 'node:fs/promises';
import {resolve,join} from 'node:path';
import assert from 'node:assert/strict';
const recent=join(process.env.APPDATA,'local.minidaw.desktop/recent-projects.json');let savedRecent;try{savedRecent=await readFile(recent);}catch{}
let child,browser,page,prefs;const report={checks:[],notes:[]},errors=[],sleep=ms=>new Promise(r=>setTimeout(r,ms));
async function until(fn,label){const at=performance.now();while(performance.now()-at<20000){if(await fn())return;await sleep(40);}throw Error(label);}
const invoke=(name,args={})=>page.evaluate(({name,args})=>window.__TAURI_INTERNALS__.invoke(name,args),{name,args});
const project=()=>invoke('project_snapshot');
const part=async()=>(await project()).document.tracks.flatMap(t=>t.clips).find(c=>c.kind==='midi');
async function idle(){await until(async()=>!await page.locator('#project-save').isDisabled(),'Idle');await sleep(100);}
async function edit(request){const p=await project();await invoke('edit_project',{revision:p.revision,request});await idle();await sleep(150);}
async function metrics(){const p=await project(),selected=await page.locator('.midi-clip.selected').first().getAttribute('data-clip-id'),clip=p.document.tracks.flatMap(t=>t.clips).find(c=>c.clipId===selected),scale=60/p.document.musicalTime.tempoMap[0].bpm/p.document.musicalTime.ticksPerQuarter;return page.evaluate(({origin,scale})=>{const c=document.querySelector('#piano-canvas'),r=c.getBoundingClientRect(),scroll=document.querySelector('#piano-scroll');return{x:r.x,y:r.y,width:r.width,height:r.height,start:(origin+Number(scroll.value))*scale,span:Number(scroll.step)*(r.width-56)*scale,scroll:document.querySelector('#piano-vscroll').scrollTop,row:parseFloat(getComputedStyle(document.documentElement).fontSize)*1.45};},{origin:Number(clip?.startTick??0),scale});}
async function point(tick,pitch){const m=await metrics(),p=await project(),bpm=p.document.musicalTime.tempoMap[0].bpm;return{x:m.x+56+(tick*60/bpm/960000-m.start)/m.span*(m.width-56),y:m.y+28+(127-pitch+.5)*m.row-m.scroll};}
async function gesture(a,b){const revision=(await project()).revision;await page.mouse.move(a.x,a.y);await page.mouse.down();if(b)await page.mouse.move(b.x,b.y,{steps:100});assert.equal((await project()).revision,revision,'Preview never commits');await page.mouse.up();await until(async()=>(await project()).revision>revision,'Gesture commit');await idle();report.notes.push((await part()).notes);}
try{
 child=spawn(resolve('src-tauri/target/release/minidaw.exe'),[],{env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=19246'},stdio:'ignore',windowsHide:true});
 await until(async()=>{try{browser=await chromium.connectOverCDP('http://127.0.0.1:19246');return true;}catch{return false;}},'CDP');
 await until(async()=>{page=browser.contexts()[0].pages().find(p=>p.url().includes('tauri.localhost'));return !!page;},'Page');
 page.on('pageerror',e=>errors.push(e.message));await page.setViewportSize({width:1600,height:1000});await idle();
 prefs=await page.evaluate(()=>({panels:localStorage.getItem('minidaw.ui.panels.v1'),font:localStorage.getItem('minidaw.ui.fontScale')}));
 for(const[id,open]of[['arrangement',true],['media',false],['performance',false],['spectrum',false],['piano',false]]){const b=page.locator('#show-'+id);if((await b.getAttribute('aria-pressed')==='true')!==open)await b.click();}
 await page.evaluate(()=>{const f=window.fetch;window.fetch=async function(u,o){if(decodeURIComponent(String(u)).split('/').at(-1)==='edit_project')window.__lastEdit=JSON.parse(o.body).request;return f.call(this,u,o);};});
 await page.locator('#font-scale').selectOption('110');
 await page.locator('#midi-track-add').click();await idle();await page.locator('.midi-clip-add').click();await idle();let c=await part();
 await edit({command:'midi.note.add',clipIds:[c.clipId],targetTick:'240000',lengthTick:'480000',pitch:64});
 async function measure(){return page.evaluate(()=>{const p=document.querySelector('#panel-piano');return {panel:p.getBoundingClientRect().toJSON(),client:p.clientHeight,scroll:p.scrollHeight,overflow:getComputedStyle(p).overflowY,children:[...p.children].map(c=>({class:c.className,id:c.id,rect:c.getBoundingClientRect().toJSON()})),panels:[...document.querySelectorAll('.workspace-panel:not([hidden])')].map(p=>({id:p.id,rect:p.getBoundingClientRect().toJSON(),border:['Top','Right','Bottom','Left'].map(s=>getComputedStyle(p)['border'+s+'Width'])}))};});}
 report.sizes=[];
 for(const [width,height,scale] of [[1280,800,'110'],[1280,800,'125'],[1600,1000,'110'],[1000,700,'125']]){await page.setViewportSize({width,height});await page.locator('#font-scale').selectOption(scale);await sleep(100);const m=await measure();report.sizes.push({width,height,scale,...m});assert.ok(m.scroll<=m.client+1, `No whole-panel overflow: ${width}x${height}/${scale}: ${m.scroll}/${m.client}`); assert.ok(m.children.find(c=>c.class==='piano-body').rect.height>=80,'Useful note viewport'); for(const c of m.children) assert.ok(c.rect.bottom<=m.panel.bottom-1, 'Visible lower area: '+c.class+'/'+c.id); if(width===1280&&scale==='110')await page.screenshot({path:'docs/validation/panel-layout-after.png'});}
 report.checks.push('No whole-Piano overflow at 1280x800 (110/125%), 1600x1000, 1000x700 (125%); note viewport and all lower editors visible.');
 await page.setViewportSize({width:1280,height:800});await page.locator('#font-scale').selectOption('110');await sleep(150);
 // Core mouse edits still use the same Canvas coordinates and command path.
 await page.locator('#grid-type').selectOption('16');if(await page.locator('#snap-toggle').getAttribute('aria-pressed')!=='true')await page.locator('#snap-toggle').click();
 await page.locator('#piano-vscroll').evaluate(e=>{const row=parseFloat(getComputedStyle(document.documentElement).fontSize)*1.45;e.scrollTop=(127-64)*row-e.clientHeight/2;});await sleep(100);
 await page.locator('#piano-select').click();await gesture(await point(480000,64),await point(960000,64));let n=(await part()).notes[0];assert.equal(n.startTick,'720000');assert.equal(n.lengthTick,'480000');
 let a=await point(1200000,64);a.x-=1;await gesture(a,await point(1440000,64));assert.equal((await part()).notes[0].lengthTick,'720000');
 await page.keyboard.press('Control+z');await idle();assert.equal((await part()).notes[0].lengthTick,'480000');await page.keyboard.press('Control+Shift+z');await idle();assert.equal((await part()).notes[0].lengthTick,'720000');
 await page.locator('#note-start').fill('1.1.2.000123');await page.locator('#note-start').press('Enter');await idle();assert.equal((await part()).notes[0].startTick,'240123');assert.equal(await page.locator('#panel-piano').evaluate(e=>e.scrollTop),0);
 await page.screenshot({path:'docs/validation/panel-layout-after.png'});
 report.checks.push('At default window size: note selection, Snap Move/Resize, preview-only pointer moves, Undo/Redo and exact numeric entry all work without panel scrolling.');
 const before=await measure();
 for(const selector of ['.midi-input summary','.piano-help summary']){await page.locator(selector).click();await sleep(80);const m=await measure();assert.equal(m.scroll,m.client);assert.equal(m.children.find(c=>c.class==='piano-body').rect.height,before.children.find(c=>c.class==='piano-body').rect.height);await page.locator(selector).click();}
 // Actual wheel moves the pitch viewport without scrolling the panel.
 const canvas=await page.locator('#piano-canvas').boundingBox(),oldScroll=await page.locator('#piano-vscroll').evaluate(e=>e.scrollTop);
 await page.mouse.move(canvas.x+100,canvas.y+60);await page.mouse.wheel(0,80);await sleep(150);assert.notEqual(await page.locator('#piano-vscroll').evaluate(e=>e.scrollTop),oldScroll);assert.equal(await page.locator('#panel-piano').evaluate(e=>e.scrollTop),0);
 // Repeated drags must preserve the total outer widths with the new real borders.
 for(const delta of [-60,30,40,-10]){const d=await page.locator('.panel-divider').boundingBox(),old=await measure(),w=old.panels.reduce((s,p)=>s+p.rect.width,0);await page.mouse.move(d.x+3,d.y+100);await page.mouse.down();await page.mouse.move(d.x+3+delta,d.y+100,{steps:10});await page.mouse.up();await sleep(100);const m=await measure();assert.ok(Math.abs(m.panels.reduce((s,p)=>s+p.rect.width,0)-w)<.1);assert.ok(m.scroll<=m.client+1);}
 report.checks.push('Pitch scrolling isolated; keyboard/help popup does not displace notes; repeated divider drags preserve total width.');
 await page.setViewportSize({width:1920,height:1080});for(const id of ['media','performance','spectrum'])await page.locator('#show-'+id).click();await sleep(200);
 report.allPanels=(await measure()).panels;assert.equal(report.allPanels.length,5);assert.ok(report.allPanels.every(p=>p.border.every(w=>w==='2px')));
 await page.screenshot({path:'docs/validation/panel-layout-all.png'});
 const full=await page.locator('#panel-piano').boundingBox();await page.locator('#panel-performance [data-panel-close]').click();await sleep(100);assert.ok((await page.locator('#panel-piano').boundingBox()).width>full.width);assert.equal(await page.locator('.panel-divider').count(),3);await page.locator('#show-performance').click();await sleep(100);
 for(const id of ['media','arrangement','performance','spectrum','piano']){const p=page.locator('#panel-'+id);await p.locator('[data-panel-close]').click();assert.equal(await p.isVisible(),false);await page.locator('#show-'+id).click();}
 report.checks.push('All five panel borders 2px on four sides; each panel closes/reopens; closed panel space returned.');
 assert.deepEqual(errors,[]);report.passed=true;
}catch(e){report.error=String(e.stack??e);report.pageErrors=errors;process.exitCode=1;if(page){report.uiError=await page.locator('#error-text').textContent().catch(()=>null);await page.screenshot({path:'docs/validation/panel-layout-failure.png'}).catch(()=>{});}}
finally{
 if(page&&prefs)await page.evaluate(p=>{for(const[k,v]of[['minidaw.ui.panels.v1',p.panels],['minidaw.ui.fontScale',p.font]]){if(v===null)localStorage.removeItem(k);else localStorage.setItem(k,v);}},prefs).catch(()=>{});
 if(browser)await browser.close();if(child?.exitCode===null){const exited=once(child,'exit');child.kill();await exited;}
 if(savedRecent)await writeFile(recent,savedRecent);else await unlink(recent).catch(()=>{});
 await writeFile('docs/validation/panel-layout-ui.json',JSON.stringify(report,null,2));console.log(JSON.stringify(report));
}


