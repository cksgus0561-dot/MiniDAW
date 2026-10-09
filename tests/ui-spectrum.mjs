// Actual release app: Spectrum UI, selection commands and a short ON/OFF playback comparison.
import {chromium} from 'playwright-core';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import {readFile,writeFile,mkdir,unlink} from 'node:fs/promises';
import {createHash,randomUUID} from 'node:crypto';
import {resolve,join} from 'node:path';
import assert from 'node:assert/strict';
const exe=resolve('src-tauri/target/release/minidaw.exe'),folder=resolve('tests/local/spectrum',randomUUID());
await mkdir(folder,{recursive:true});
const report={executableSha256:createHash('sha256').update(await readFile(exe)).digest('hex'),checks:[]};
const recent=join(process.env.APPDATA,'local.minidaw.desktop/recent-projects.json');
let savedRecent;try{savedRecent=await readFile(recent);}catch{}
let child,browser,page,prefs;const errors=[],sleep=ms=>new Promise(r=>setTimeout(r,ms));
async function until(fn,label,ms=20000){const at=performance.now();while(performance.now()-at<ms){if(await fn())return;await sleep(35);}throw Error(label);}
const invoke=(name,args={})=>page.evaluate(({name,args})=>window.__TAURI_INTERNALS__.invoke(name,args),{name,args});
const project=()=>invoke('project_snapshot'),audio=()=>invoke('engine_snapshot'),spectrum=()=>invoke('spectrum_snapshot',{after:0});
async function idle(){await until(async()=>!(await page.locator('#project-save').isDisabled()),'Project idle');await sleep(150);}
async function panel(id,open){const b=page.locator('#show-'+id);if((await b.getAttribute('aria-pressed')==='true')!==open)await b.click();await sleep(150);}
function peak(frame,ch,hz){return frame.frequencies.map((f,i)=>({f,db:frame.curves[ch][i]})).filter(p=>Math.abs(Math.log(p.f/hz))<.1).sort((a,b)=>b.db-a.db)[0];}
async function analyzed(){await page.evaluate(()=>window.__selection=null);await page.locator('#spectrum-analyze').click();await until(async()=>!(await page.locator('#spectrum-analyze').isDisabled()),'Selection completes');const f=await page.evaluate(()=>window.__selection);assert.ok(f,await page.locator('#spectrum-status').textContent());return f;}
async function edit(request){const p=await project();await invoke('edit_project',{revision:p.revision,request});await idle();}
async function selectClip(){const n=page.locator('.audio-clip').first(),b=await n.boundingBox();await page.mouse.click(b.x+Math.min(45,b.width/3),b.y+b.height*.65);}
// Quiet native-rate stereo fixture (L 440 Hz, R 1 kHz; -26.02 dBFS peak).
const rate=48000,frames=rate*20,pcm=Buffer.alloc(44+frames*4);pcm.write('RIFF');pcm.writeUInt32LE(pcm.length-8,4);pcm.write('WAVEfmt ',8);pcm.writeUInt32LE(16,16);pcm.writeUInt16LE(1,20);pcm.writeUInt16LE(2,22);pcm.writeUInt32LE(rate,24);pcm.writeUInt32LE(rate*4,28);pcm.writeUInt16LE(4,32);pcm.writeUInt16LE(16,34);pcm.write('data',36);pcm.writeUInt32LE(frames*4,40);
for(let i=0;i<frames;i++)for(let ch=0;ch<2;ch++)pcm.writeInt16LE(Math.round(.05*32767*Math.sin(2*Math.PI*[440,1000][ch]*i/rate)),44+i*4+ch*2);
const file=join(folder,'spectrum-stereo.wav');await writeFile(file,pcm);
try{
 child=spawn(exe,[],{env:{...process.env,WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS:'--remote-debugging-port=19239'},stdio:'ignore',windowsHide:true});
 await until(async()=>{try{browser=await chromium.connectOverCDP('http://127.0.0.1:19239');return true;}catch{return false;}},'Release CDP');
 await until(async()=>{page=browser.contexts()[0].pages().find(p=>p.url().includes('tauri.localhost'));return !!page;},'Release page');
 page.on('pageerror',e=>errors.push(e.message));await page.setViewportSize({width:1440,height:960});await idle();
 prefs=await page.evaluate(()=>Object.fromEntries(['minidaw.ui.panels.v1','minidaw.ui.fontScale','minidaw.ui.spectrum.v1'].map(k=>[k,localStorage.getItem(k)])));
 await page.evaluate(()=>{
   window.__specCalls={};window.__specPaints=0;
   const original=window.fetch;
   window.fetch=async function(url,options){const name=decodeURIComponent(String(url)).split('/').at(-1);if(name.startsWith('spectrum_'))window.__specCalls[name]=(window.__specCalls[name]??0)+1;const response=await original.call(this,url,options);if(name==='spectrum_analyze'){if(window.__delaySelection)await new Promise(r=>setTimeout(r,500));if(response.headers.get('Tauri-Response')==='ok'){window.__selection=await response.clone().json();window.__selectionRequest=JSON.parse(options.body).request;}}return response;};
   const fill=CanvasRenderingContext2D.prototype.fillRect;
   CanvasRenderingContext2D.prototype.fillRect=function(...args){if(this.canvas.id==='spectrum-canvas')window.__specPaints++;return fill.apply(this,args);};
 });
 await panel('arrangement',true);await panel('media',false);await panel('performance',false);await panel('spectrum',false);
 await invoke('load_audio',{path:file});await idle();await page.locator('#zoom-fit').click();
 const p=await project(),id=p.document.tracks[0].clips[0].clipId;report.backend=(await audio()).output;
 const fullWidth=(await page.locator('#panel-arrangement').boundingBox()).width;
 await panel('spectrum',true);assert.ok((await page.locator('#panel-arrangement').boundingBox()).width<fullWidth-200);
 await page.locator('#spectrum-fft').selectOption('4096');await page.locator('#spectrum-smoothing').selectOption('200');
 const playAt=performance.now();await page.locator('#play-pause').click();await until(async()=>{const s=await spectrum();return s.frame&&peak(s.frame,0,440).db>-35;},'Live FFT follows playback');report.liveResponseMs=performance.now()-playAt;
 await until(async()=>{const s=await spectrum();return s.frame&&Math.abs(peak(s.frame,0,440).db+26.02)<.2;},'Smoothing settles');
 let f=(await spectrum()).frame;report.livePeaks=[peak(f,0,440),peak(f,1,1000),peak(f,2,440),peak(f,2,1000)];
 assert.ok(Math.abs(report.livePeaks[0].db+26.02)<.25);assert.ok(Math.abs(report.livePeaks[1].db+26.02)<.25);
 assert.ok(Math.abs(report.livePeaks[2].db+29.03)<.3);assert.ok(Math.abs(report.livePeaks[3].db+29.03)<.3);
 assert.ok(peak(f,0,1000).db<-75);assert.ok(peak(f,1,440).db<-75);
 const hover=async(ch,hz,note)=>{await page.locator('#spectrum-channel').selectOption(String(ch));const b=await page.locator('#spectrum-canvas').boundingBox();await page.mouse.move(b.x+48+Math.log(hz/20)/Math.log(1000)*(b.width-62),b.y+b.height*.5);await sleep(80);const text=await page.locator('#spectrum-hover').textContent();assert.ok(text.includes('Hz')&&text.includes(note)&&text.includes('dBFS'),text);return text;};
 report.hover=[await hover(0,440,'A4'),await hover(1,1000,'B5'),await hover(2,440,'A4')];
 await page.locator('#spectrum-fft').selectOption('8192');await until(async()=>(await spectrum()).frame?.fftSize===8192,'FFT size update');
 f=(await spectrum()).frame;assert.ok(Math.abs(peak(f,0,440).db+26.02)<.25);
 await page.locator('#spectrum-hold').uncheck();await page.locator('#spectrum-hold').check();await page.locator('#spectrum-reset').click();
 await until(async()=>!!(await spectrum()).frame,'Reset live peaks');await page.screenshot({path:'docs/validation/spectrum-live.png'});
 report.checks.push('Live FFT, 20 Hz–20 kHz log Canvas, L/R/Combined levels, Hz/Note/dB hover, FFT size change and Peak controls.');
 await page.locator('#play-pause').click();await until(async()=>(await audio()).transport.state==='paused','Pause');
 await selectClip();const transport=(await audio()).transport.appliedCommand;
 f=await analyzed();assert.ok(Math.abs(f.duration-20)<1e-6);assert.ok(f.windows>100);assert.ok(Math.abs(peak(f,0,440).db+26.02)<.25);
 assert.equal((await audio()).transport.appliedCommand,transport);assert.deepEqual((await project()).document,p.document);
 await edit({command:'audio.gain',clipIds:[id],gainDb:-6});
 await edit({command:'audio.trim',clipIds:[id],sourceStart:'48000',sourceEnd:'720000'});
 f=await analyzed();assert.equal(f.duration,14);assert.ok(Math.abs(peak(f,0,440).db+32.02)<.25);
 await edit({command:'audio.fade',clipIds:[id],fadeIn:'336000',fadeOut:'336000',curve:'linear'});
 f=await analyzed();assert.ok(Math.abs(peak(f,0,440).db+36.79)<.35);
 await edit({command:'audio.muteEvents',clipIds:[id]});f=await analyzed();assert.ok(f.curves.flat().every(v=>v===-120));
 await edit({command:'audio.unmuteEvents',clipIds:[id]});
 await page.locator('[data-command="tool.rangeSelection"]').click();
 const layer=await page.locator('#clip-layer').boundingBox(),step=Number(await page.locator('#scroll').getAttribute('step'));
 await page.mouse.move(layer.x+2/step,layer.y+65);await page.mouse.down();await page.mouse.move(layer.x+5/step,layer.y+65,{steps:30});await page.mouse.up();
 f=await analyzed();const request=await page.evaluate(()=>window.__selectionRequest);assert.ok(request.trackIds.length===1&&request.start!==null);assert.ok(Math.abs(f.duration-(request.end-request.start))<1/rate);assert.ok(f.windows>1);
 report.selection={duration:f.duration,windows:f.windows,processingMs:f.processingMs,request};
 await page.screenshot({path:'docs/validation/spectrum-selection.png'});
 report.checks.push('Selected Clip full-section average, Trim duration, Gain/Fade/Mute, actual Range mouse selection; no transport seek or project data mutation from analysis.');
 // Delay delivery of a real response to prove that cancelling also rejects late results.
 await page.evaluate(()=>window.__delaySelection=true);await page.locator('#spectrum-analyze').click();await page.locator('#spectrum-cancel').click();await sleep(650);
 assert.ok((await page.locator('#spectrum-status').textContent()).includes('취소'));assert.equal(await page.locator('#spectrum-analyze').isDisabled(),false);await page.evaluate(()=>window.__delaySelection=false);
 report.checks.push('Cancel ends the busy state and a delayed real selection result cannot replace the cancelled view.');
 await page.locator('#spectrum-mode').selectOption('live');await page.locator('#spectrum-fft').selectOption('4096');await page.locator('#spectrum-smoothing').selectOption('200');
 await invoke('transport_command',{action:'seek',seconds:3});await invoke('transport_command',{action:'play'});
 report.intervals=[];
 for(const enabled of [false,true,false]){
   await panel('spectrum',enabled);await sleep(180);const before=await audio(),sp=await spectrum();
   const counter=await page.evaluate(()=>({paint:window.__specPaints,polls:window.__specCalls.spectrum_snapshot??0}));
   await sleep(1400);const after=await audio();
   const count=after.metrics.callbacks-before.metrics.callbacks;
   const callbackMs=(after.metrics.callbackAvgMs*after.metrics.callbacks-before.metrics.callbackAvgMs*before.metrics.callbacks)/count;
   const end=await page.evaluate(()=>({paint:window.__specPaints,polls:window.__specCalls.spectrum_snapshot??0}));
   assert.equal(after.transport.state,'playing');assert.ok(after.position>before.position+1);
   assert.equal(after.metrics.overruns,before.metrics.overruns);assert.equal(after.metrics.deviceCallbackOverruns,before.metrics.deviceCallbackOverruns);assert.equal(after.streamErrors,before.streamErrors);
   if(!enabled){assert.deepEqual(end,counter,'Closed panel has no Canvas render or Spectrum polling');assert.equal(sp.enabled,false);assert.ok(Math.abs((await page.locator('#panel-arrangement').boundingBox()).width-fullWidth)<2);}
   report.intervals.push({enabled,callbackMs,callbacks:count,paints:end.paint-counter.paint,polls:end.polls-counter.polls,spectrum:await spectrum()});
 }
 await invoke('transport_command',{action:'stop'});report.checks.push('Spectrum OFF/ON/OFF short playback comparison: position advances, no new callback/device overruns or stream errors; closed panel returns space and stops Canvas/IPC polling.');
 await panel('spectrum',true);await page.locator('#spectrum-fft').selectOption('8192');await page.locator('#spectrum-channel').selectOption('1');await page.locator('#spectrum-hold').uncheck();
 await page.reload();await idle();assert.equal(await page.locator('#spectrum-fft').inputValue(),'8192');assert.equal(await page.locator('#spectrum-channel').inputValue(),'1');assert.equal(await page.locator('#spectrum-hold').isChecked(),false);
 for(const font of ['90','125']){await page.locator('#font-scale').selectOption(font);await sleep(120);assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth+1),false);assert.ok((await page.locator('#spectrum-canvas').boundingBox()).height>80);}
 await page.setViewportSize({width:760,height:520});await sleep(150);await page.locator('#spectrum-status').scrollIntoViewIfNeeded();assert.equal(await page.locator('#spectrum-status').isVisible(),true);assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth+1),false);
 report.checks.push('Spectrum UI preferences survive WebView reload independently of project data; 90/125% font layout.');
 assert.deepEqual(errors,[]);report.passed=true;
}catch(e){report.error=String(e.stack??e);process.exitCode=1;if(page){await page.screenshot({path:'docs/validation/spectrum-failure.png'}).catch(()=>{});report.lastAudio=await audio().catch(()=>null);report.lastSpectrum=await spectrum().catch(()=>null);}}
finally{
 if(page&&prefs)await page.evaluate(p=>{for(const[k,v]of Object.entries(p)){if(v===null)localStorage.removeItem(k);else localStorage.setItem(k,v);}},prefs).catch(()=>{});
 if(browser)await browser.close();if(child?.exitCode===null){const exit=once(child,'exit');child.kill();await exit;}
 if(savedRecent)await writeFile(recent,savedRecent);else await unlink(recent).catch(()=>{});
 await writeFile('docs/validation/spectrum-ui.json',JSON.stringify(report,null,2));console.log(JSON.stringify({passed:report.passed,error:report.error,checks:report.checks,intervals:report.intervals?.map(({spectrum:s,...rest})=>({...rest,workerMs:s.workerMs,workerMaxMs:s.workerMaxMs,dropped:s.droppedBlocks,skipped:s.skippedFrames}))}));
}
