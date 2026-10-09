import { invoke } from '@tauri-apps/api/core';
import { listen, emitTo } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { panelEvent, type PanelMessage } from './panel-windows';
import { satellite, panelTitles, type PanelStateAdapter } from './panel-context';
import { Panels } from './panels';
import { PianoRoll } from './piano-roll';
import { SpectrumPanel } from './spectrum';
import { Mixer } from './mixer';
import { Waveform } from './waveform';
import { MusicalControls } from './musical-controls';
import { ComputerMidi } from './computer-midi';
import { Plugins } from './plugins';
import { initializeLocalization, setLanguage, language, uiText } from './i18n';
import { initializeFontScale } from './preferences';
import { refreshShortcutLabels, reloadShortcuts, shortcutCommand } from './shortcuts';
import { globalShortcut } from './keyboard';
import { updateMetrics } from './metrics';
import { element } from './dom';
import type { Snapshot } from './types';

const id=satellite!,win=getCurrentWindow();
initializeLocalization();
const error=(e:unknown)=>{uiText(element('error-text'),(e as {message?:string})?.message??String(e));element('error').hidden=false;};
element('dismiss-error').onclick=()=>{element('error').hidden=true;};
const pending=new Map<string,{resolve:(v:any)=>void;reject:(e:unknown)=>void;timer:number}>();
let token=0,active=false,restoring=false,busy=0;
const send=(kind:string,data?:unknown,t?:string)=>emitTo('main',panelEvent,{kind,panel:id,token:t,data});
const action=(kind:string,value?:unknown):Promise<any>=>new Promise((resolve,reject)=>{
  const t=String(++token),timer=window.setTimeout(()=>{pending.delete(t);reject(new Error('Main window did not respond'));},120000);
  pending.set(t,{resolve,reject,timer});void send('action',{kind,value},t).catch(reject);
});
const edit=async(r:unknown)=>{busy++;try{await action('edit',r);}catch(e){error(e);throw e;}finally{busy--;}};
const panels=new Panels(['media','arrangement','performance','spectrum','piano','mixer'].map(p=>({id:p,title:p,element:`panel-${p}`})));
document.body.append(element('error'));
if(id==='piano')element('panel-piano').querySelector('.workspace-panel-heading')!.after(document.querySelector('.computer-midi')!);
const wave=new Waveform(async seconds=>action('transport',{action:'seek',seconds}),error);
new MusicalControls(wave,edit,error);
const computer=new ComputerMidi(error);
let piano:PianoRoll|undefined,spectrum:SpectrumPanel|undefined,mixer:Mixer|undefined,adapter:PanelStateAdapter|undefined;
let selection:any={revision:0,clipIds:[],trackIds:[],start:null,end:null};
if(id==='piano'){piano=new PianoRoll(panels,wave,edit);adapter={capture:()=>piano!.captureWindowState(),restore:s=>piano!.restoreWindowState(s)};}
if(id==='spectrum'){spectrum=new SpectrumPanel(panels,()=>selection);adapter={capture:()=>spectrum!.captureWindowState(),restore:s=>spectrum!.restoreWindowState(s),suspend:()=>spectrum!.suspendWindow()};}
if(id==='mixer'){mixer=new Mixer(panels,edit,()=>active&&!busy);new Plugins(edit,()=>active&&!busy,error);adapter={capture:()=>({scroll:element('mixer-content').scrollLeft}),restore:s=>{if(s)element('mixer-content').scrollLeft=s.scroll;}};}
initializeFontScale(()=>wave.refreshStyle(),error);refreshShortcutLabels();
let projectRevision=-1;
const dispatch=(name:string,detail:any)=>{
  if(name==='project-view'){if(detail.revision<projectRevision)return;projectRevision=detail.revision;}
  document.dispatchEvent(new CustomEvent(name,{detail}));
};
function context(c:any):void{
  if(!c)return;
  selection=c.selection??selection;
  if(c.language&&c.language!==language())setLanguage(c.language);
  if(c.shortcuts!==undefined)reloadShortcuts(c.shortcuts);
  if(c.font){document.documentElement.style.setProperty('--ui-font-scale',c.font);wave.refreshStyle();}
  if(c.grid){wave.grid=c.grid.grid;wave.snapEnabled=c.grid.snap;dispatch('musical-grid-changed',null);}
  if(c.computer)computer.restoreWindowState(c.computer);
  if(c.project)dispatch('project-view',c.project);
  if(c.track)dispatch('track-selected',c.track);
}
async function receive(m:PanelMessage):Promise<void>{
  if(m.panel!==id)return;
  if(m.kind==='result'){const p=pending.get(m.token!);if(p){clearTimeout(p.timer);pending.delete(m.token!);m.data.error?p.reject(new Error(m.data.error)):p.resolve(m.data.value);}return;}
  if(m.kind==='activate'){
    restoring=true;context(m.data.context);
    for(const[name,value]of Object.entries(m.data.events??{}))if(value!==null)dispatch(name,value);
    if(m.data.state)adapter?.restore(m.data.state);
    active=true;panels.activateSatellite(true);restoring=false;
    element(`panel-${id}`).tabIndex=-1;element(`panel-${id}`).focus();
    await send('reply',null,m.token);return;
  }
  if(m.kind==='deactivate'){
    dispatch('workspace-changing',null);
    while(busy||mixer?.hasPendingChanges())await new Promise(r=>setTimeout(r,20));
    const state=adapter?.capture();await adapter?.suspend?.();active=false;panels.activateSatellite(false);
    if(id==='spectrum')await send('fps',0);
    await send('reply',state,m.token);return;
  }
  if(m.kind==='event'){
    restoring=true;context(m.data.context);
    if(m.data.name==='arrangement-draw-count')element('metric-draws').textContent=String(m.data.value);
    else if(m.data.name==='computer-midi-changed')computer.restoreWindowState(m.data.value);
    else if(m.data.name==='spectrum-window-fps')element('metric-spectrum-fps').textContent=Number(m.data.value).toFixed(1);
    else dispatch(m.data.name,m.data.value);
    restoring=false;
  }
}
for(const name of ['piano-part-activated','piano-active-part','automation-toggle'])document.addEventListener(name,e=>{
  if(active&&!restoring)void send('ui-event',{name,value:(e as CustomEvent).detail}).catch(error);
});
document.addEventListener('panel-host-show',e=>{if(active&&!restoring)void action('panel',(e as CustomEvent).detail).catch(error);});
document.addEventListener('panel-window-action',e=>void send((e as CustomEvent).detail).catch(error));
document.addEventListener('computer-midi-changed',()=>{if(active&&!restoring)void action('computer',computer.captureWindowState()).catch(error);});
document.addEventListener('musical-grid-changed',()=>{if(active&&!restoring)void action('grid',{grid:wave.grid,snap:wave.snapEnabled}).catch(error);});
// Project-wide commands keep their existing bindings and input-focus policy.
globalShortcut(e=>{
  const cmd=shortcutCommand(e);if(!cmd)return;
  if(id==='piano'&&(cmd.startsWith('audio.')||cmd.startsWith('tool.')||(cmd.startsWith('edit.')&&cmd!=='edit.undo'&&cmd!=='edit.redo')))return;
  if(cmd==='grid.snap')return()=>{void action('shortcut',cmd).catch(error);};
  // Piano's own handlers above retain editor priority; all remaining registered
  // global DAW actions use the main registry rather than a second shortcut map.
  return()=>{void action('shortcut',cmd).catch(error);};
});
let snap:Snapshot|null=null,received=0,frames=0,last=performance.now(),revision=-1;
function frame(now:number):void{
  if(active&&!document.hidden){frames++;if(snap)piano?.setPosition(Math.min(snap.duration,snap.position+(snap.transport.state==='playing'?Math.min(50,now-received)/1000:0)));}
  if(now-last>=500){if(id==='performance'){element('metric-fps').textContent=(active?frames*1000/(now-last):0).toFixed(1);element('metric-frame').textContent=frames?`${((now-last)/frames).toFixed(2)} ms`:'—';}if(active&&spectrum)void send('fps',spectrum.sampleRenderFps()).catch(error);frames=0;last=now;}
  requestAnimationFrame(frame);
}
async function poll():Promise<void>{
  try{if(active){snap=await invoke<Snapshot>('engine_snapshot');received=performance.now();mixer?.meter(snap);if(id==='performance'){updateMetrics(snap);uiText(element('metric-rate'),snap.output?`${snap.output.sampleRate.toLocaleString()} Hz`:'—');uiText(element('metric-errors'),snap.streamErrors);}
    if(revision!==snap.projectRevision){revision=snap.projectRevision??-1;const view=await invoke('project_snapshot');dispatch('project-view',view);}
  }}catch(e){error(e);}finally{setTimeout(()=>void poll(),active&&!document.hidden?33:250);}
}
let geometryTimer=0,normal:any;
function geometry():void{clearTimeout(geometryTimer);geometryTimer=window.setTimeout(()=>void(async()=>{
  if(!active||await win.isMinimized())return;const maximized=await win.isMaximized();
  if(!maximized){const scale=await win.scaleFactor(),p=await win.outerPosition(),s=await win.innerSize();normal={x:p.x/scale,y:p.y/scale,width:s.width/scale,height:s.height/scale};}
  await send('geometry',{...normal,maximized});
})().catch(error),200);}
async function initialize():Promise<void>{
  await listen<PanelMessage>(panelEvent,e=>void receive(e.payload).catch(error));
  await win.onCloseRequested(e=>{e.preventDefault();void send('close').catch(error);});
  await win.onMoved(geometry);await win.onResized(geometry);
  await win.setTitle(`${panelTitles[id]} — MiniDAW`);
  await send('ready');requestAnimationFrame(frame);void poll();
}
void initialize().catch(error);
