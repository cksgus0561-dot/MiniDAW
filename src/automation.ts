import { uiText, uiAttr } from './i18n';
import { shortcutCommand, shortcutLabel } from './shortcuts';
import type {PluginSelection} from './plugins';
import { AutomationPointInfo } from './automation-point-info';
import { keyboardBlocked } from './keyboard';
import type { Waveform } from './waveform';
import type { Insert } from './effects';
import { MusicalClock, type MusicalTime } from './musical-time';
export interface Parameter {effectId?:string;name:string}
interface Point {pointId:string;tick:string;value:number;shape:'linear'|'step'}
interface Lane {laneId:string;parameter:Parameter;points:Point[]}
export interface AutomationChannel {trackId:string;read:boolean;write:boolean;lanes:Lane[]}
export interface AutomationDocument {projectId:string;automation?:AutomationChannel[];musicalTime:MusicalTime;master?:{volumeDb:number;inserts?:Insert[]};tracks:{trackId:string;name:string;kind:string;instrument?:string;extensions?:Record<string,unknown>;mix?:{volumeDb:number;pan:number};synth?:{levelDb:number;attackMs:number;releaseMs:number};inserts?:Insert[]}[]}
export interface ParameterSpec {parameter:Parameter;label:string;min:number;max:number;base:number;step:number;discrete?:boolean;choices?:[number,string][]}
const key=(p:Parameter)=>`${p.effectId??''}:${p.name}`;
const writing=new WeakMap<AutomationDocument,{trackId:string;parameter:Parameter;value:number}[]>();
document.addEventListener('project-view',e=>{const v=(e as CustomEvent<{document:AutomationDocument;automationWriting?:{trackId:string;parameter:Parameter;value:number}[]}>).detail;writing.set(v.document,v.automationWriting??[]);});
export const automationChannel=(p:AutomationDocument,id:string)=>p.automation?.find(a=>a.trackId===id);
export function parameters(p:AutomationDocument,id:string):ParameterSpec[]{
 const t=p.tracks.find(t=>t.trackId===id),out:ParameterSpec[]=[];
 const field=(name:string,label:string,min:number,max:number,base:number,step:number,effectId?:string,choices?:[number,string][])=>out.push({parameter:{name,...(effectId?{effectId}:{})},label,min,max,base,step,discrete:!!choices,choices});
 field('volumeDb','Volume · dB',-96,12,id==='master'?p.master?.volumeDb??0:t?.mix?.volumeDb??0,.1);
 if(t)field('pan','Pan · L −1 / C 0 / R 1',-1,1,t.mix?.pan??0,.01);
 if(t?.kind==='midi'){field('synth.levelDb','Synth · Level dB',-60,12,t.synth?.levelDb??0,.1);field('synth.attackMs','Synth · Attack ms',.1,2000,t.synth?.attackMs??5,.1);field('synth.releaseMs','Synth · Release ms',1,5000,t.synth?.releaseMs??40,1);}
 const instrument=t?.extensions?.['minidaw.plugin.v1'] as PluginSelection|undefined;if(t?.instrument==='external'&&instrument)for(const q of instrument.parameters.filter(q=>!q.readonly&&q.max>q.min)){field('plugin.'+q.id,instrument.descriptor.name+' · '+q.name,q.min,q.max,q.value,q.step||(q.max-q.min)/1000);if(q.stepped)out.at(-1)!.discrete=true;}
 (id==='master'?p.master?.inserts:t?.inserts)?.forEach((e,index)=>{
  const add=(name:string,label:string,min:number,max:number,step:number,choices?:[number,string][])=>field(name,`${index+1} ${e.kind} · ${label}`,min,max,Number(e[name]??0),step,e.effectId,choices);
  field('bypass',`${index+1} ${e.kind} · Bypass`,0,1,e.enabled?0:1,1,e.effectId,[[0,'Off (처리)'],[1,'On (우회)']]);
  if(e.kind==='external')for(const q of (e.plugin as PluginSelection).parameters.filter(q=>!q.readonly&&q.max>q.min)){field('plugin.'+q.id,(index+1)+' '+(e.plugin as PluginSelection).descriptor.name+' · '+q.name,q.min,q.max,q.value,q.step||(q.max-q.min)/1000,e.effectId);if(q.stepped)out.at(-1)!.discrete=true;}
  if(e.kind==='eq')(e.bands as {frequency:number;gainDb:number;q:number}[]).forEach((b,i)=>{for(const [name,min,max,step]of [['frequency',20,20000,1],['gainDb',-24,24,.1],['q',.1,20,.01]] as const)field(`band${i}.${name}`,`${index+1} EQ · Band ${i+1} ${name}`,min,max,b[name],step,e.effectId);});
  if(e.kind==='compressor'){add('thresholdDb','Threshold dB',-60,0,.1);add('ratio','Ratio',1,20,.1);add('attackMs','Attack ms',.1,200,.1);add('releaseMs','Release ms',10,2000,1);add('makeupDb','Makeup dB',-12,24,.1);}
  if(e.kind==='limiter'){add('ceilingDb','Ceiling dBFS',-24,0,.1);add('inputDb','Input dB',-24,24,.1);}
  if(e.kind==='reverb'){add('decay','Decay s',.2,8,.1);add('wet','Wet 0–1',0,1,.01);}
  if(e.kind==='delay'){add('timeMs','Time ms',1,2000,1);add('feedback','Feedback',0,.85,.01);add('wet','Wet 0–1',0,1,.01);add('syncBeats','Tempo Sync',0,4,.125,[[0,'Off'],[.125,'1/32'],[.25,'1/16'],[.5,'1/8'],[1,'1/4'],[2,'1/2'],[4,'1/1']]);}
 });return out;
}
function at(points:Point[],tick:number,base:number,discrete=false):number{let i=0;while(i<points.length&&Number(points[i].tick)<=tick)i++;if(!i)return points[0]?.value??base;const a=points[i-1],b=points[i];return !b||discrete||a.shape==='step'?a.value:a.value+(b.value-a.value)*(tick-Number(a.tick))/(Number(b.tick)-Number(a.tick));}
// Display only. Rust sample-clock evaluators drive actual audio and Write timestamps.
export function automatedValue(p:AutomationDocument,id:string,param:Parameter,seconds:number,base:number,playing:boolean):number{
 const c=automationChannel(p,id);if(!playing||!c?.read)return base;const held=writing.get(p)?.find(w=>w.trackId===id&&key(w.parameter)===key(param));if(held)return held.value;const l=c.lanes.find(l=>key(l.parameter)===key(param));if(!l)return base;const discrete=param.name.startsWith('plugin.')?parameters(p,id).find(s=>key(s.parameter)===key(param))?.discrete: param.name==='bypass'||param.name==='syncBeats';return at(l.points,new MusicalClock(p.musicalTime).ticks(seconds),base,discrete);
}
export class AutomationLanes {
 private doc:AutomationDocument|null=null;
 private pointEditor:AutomationPointInfo;private editingPointTrack:string|null=null;
 private open=new Set<string>();private choice=new Map<string,string>();private selected=new Map<string,string>();
 private canvases=new Map<string,HTMLCanvasElement>();private headers=new Map<string,HTMLElement>();
 private preview:{id:string;point:Point;pointer:number;canvas:HTMLCanvasElement;x:number;y:number;moved:boolean}|null=null;
 constructor(private wave:Waveform,private edit:(r:Record<string,unknown>)=>Promise<void>,private layout:()=>void){
  this.pointEditor=new AutomationPointInfo({current:()=>{const id=this.editingPointTrack;if(!id||!this.open.has(id))return null;const point=this.lane(id)?.points.find(p=>p.pointId===this.selected.get(id)),spec=this.spec(id);return point&&spec?{point,spec,clock:this.wave.musical}:null;},save:async p=>{if(this.editingPointTrack)await this.save(this.editingPointTrack,p);},remove:async()=>{if(this.editingPointTrack)await this.remove(this.editingPointTrack);this.pointEditor.refresh();}});
  document.addEventListener('project-view',e=>{const p=(e as CustomEvent<{document:AutomationDocument}>).detail.document;if(this.doc?.projectId!==p.projectId){this.open.clear();this.choice.clear();this.selected.clear();}this.doc=p;});
  document.addEventListener('automation-toggle',e=>{const id=(e as CustomEvent<string>).detail;if(this.open.has(id))this.open.delete(id);else this.open.add(id);this.layout();if(this.open.has(id))this.canvases.get(id)?.scrollIntoView({block:'nearest',inline:'nearest'});});
  document.addEventListener('workspace-changing',()=>{this.preview=null;});
 }
 height(id:string):number{return this.open.has(id)?Math.round(parseFloat(getComputedStyle(document.documentElement).fontSize)*9.5):0;}
 display(seconds:number,playing:boolean):void{if(!this.doc)return;for(const[id,h]of this.headers){const input=h.querySelector<HTMLInputElement|HTMLSelectElement>('.automation-value'),s=this.spec(id);if(input&&s&&document.activeElement!==input){const v=automatedValue(this.doc,id,s.parameter,seconds,s.base,playing);input.value=String(s.discrete?v:Math.round(v*1000)/1000);}}}
 clear():void{this.preview=null;this.canvases.forEach(c=>c.remove());this.canvases.clear();this.headers.clear();}
 private spec(id:string):ParameterSpec|null{const list=this.doc?parameters(this.doc,id):[];return list.find(s=>key(s.parameter)===this.choice.get(id))??list[0]??null;}
 private lane(id:string):Lane|undefined {const s=this.spec(id);return s?this.doc?.automation?.find(c=>c.trackId===id)?.lanes.find(l=>key(l.parameter)===key(s.parameter)):undefined;}
 mount(id:string,top:number,list:HTMLElement,layer:HTMLElement):void{
  const height=this.height(id),s=this.spec(id);if(!height||!this.doc||!s)return;
  const header=document.createElement('div');header.className='automation-header';header.dataset.trackId=id;header.style.height=`${height}px`;this.headers.set(id,header);
  const title=document.createElement('strong');uiText(title, id==='master'?'Master · Automation':'Automation');
  const close=document.createElement('button');uiText(close, '×');uiAttr(close, 'title', 'Automation Lane 닫기');close.onclick=()=>{this.open.delete(id);this.layout();};
  const flags=document.createElement('div');flags.className='automation-flags';flags.append(title,close);
  const c=automationChannel(this.doc,id);
  for(const [field,label]of [['read','R'],['write','W']]as const){const b=document.createElement('button');uiText(b, label);b.className=`automation-${field}`;b.setAttribute('aria-pressed',String(c?.[field]??field==='read'));uiAttr(b, 'title', field==='read'?'Read · Automation 재생':'Write · Auto-Latch · 조작 이후 Stop/Pause/W 해제까지 기록');b.onclick=()=>void this.edit({command:'automation.channel',trackIds:id==='master'?[]:[id],[field]:!(c?.[field]??field==='read')});flags.append(b);}
  const select=document.createElement('select');select.className='automation-parameter';uiAttr(select, 'title', 'Automation Parameter');uiAttr(select, 'aria-label', 'Automation Parameter');
  for(const x of parameters(this.doc,id)){const o=document.createElement('option');o.value=key(x.parameter);uiText(o, x.label);select.append(o);}select.value=key(s.parameter);
  select.onchange=()=>{this.choice.set(id,select.value);this.selected.delete(id);this.layout();};
  const controls=document.createElement('div');controls.className='automation-point-controls';
  const shape=document.createElement('select');shape.className='automation-shape';uiAttr(shape, 'aria-label', 'Point Curve');for(const [value,label]of [['linear','Linear'],['step','Step']]){const o=document.createElement('option');o.value=value;uiText(o, label);shape.append(o);}shape.disabled=!!s.discrete;shape.value=s.discrete?'step':this.lane(id)?.points.find(p=>p.pointId===this.selected.get(id))?.shape??'linear';shape.onchange=()=>{const p=this.lane(id)?.points.find(p=>p.pointId===this.selected.get(id));if(p)void this.save(id,{...p,shape:shape.value as Point['shape']});};
  const remove=document.createElement('button');uiText(remove, '삭제');remove.className='automation-delete';uiAttr(remove, 'title', '선택 Point 삭제');remove.onclick=()=>void this.remove(id);const info=document.createElement('button');uiText(info, 'Point…');info.className='automation-point-open';uiAttr(info, 'title', '선택 Point의 Position / Value 숫자 편집');info.onclick=()=>{this.editingPointTrack=id;this.pointEditor.open(info);};controls.append(shape,info,remove);
  const value=s.choices?document.createElement('select'):document.createElement('input');value.className='automation-value';if(value instanceof HTMLInputElement){value.type='number';value.min=String(s.min);value.max=String(s.max);value.step=String(s.step);}else for(const [v,label]of s.choices!){const o=document.createElement('option');o.value=String(v);uiText(o, label);value.append(o);}value.value=String(s.base);uiAttr(value, 'title', '현재 Parameter 값 · W + 재생 중 변경하면 기록');uiAttr(value, 'aria-label', s.label);value.onchange=()=>{if(value.reportValidity())void this.parameter(id,s,Number(value.value));};
  const hint=document.createElement('small');hint.dataset.shortcutDeleteHelp='';uiText(hint, `더블 클릭 추가 · 끌기 이동 · ${shortcutLabel('edit.delete')||'—'}: 삭제`);
  header.append(flags,select,value,controls,hint);list.append(header);
  header.onkeydown=e=>{if(keyboardBlocked(e))return;if(shortcutCommand(e)==='edit.delete'){e.preventDefault();e.stopPropagation();void this.remove(id);}};
  const canvas=document.createElement('canvas');canvas.className='automation-canvas';canvas.dataset.trackId=id;canvas.style.top=`${top}px`;canvas.style.height=`${height}px`;canvas.tabIndex=0;uiAttr(canvas, 'aria-label', `${id==='master'?'Master':this.doc.tracks.find(t=>t.trackId===id)?.name} ${s.label} Automation`);layer.append(canvas);this.canvases.set(id,canvas);
  canvas.onpointerdown=e=>{e.stopPropagation();if(e.button!==0)return;canvas.focus();const point=this.hit(id,e);if(point){this.selected.set(id,point.pointId);const shape=this.headers.get(id)?.querySelector<HTMLSelectElement>('.automation-shape');if(shape)shape.value=point.shape;this.preview={id,point:{...point},pointer:e.pointerId,canvas,x:e.clientX,y:e.clientY,moved:false};canvas.setPointerCapture(e.pointerId);e.preventDefault();this.draw();}};
  canvas.onpointermove=e=>{e.stopPropagation();if(this.preview?.canvas===canvas&&this.preview.pointer===e.pointerId){const g=this.preview;g.moved ||= Math.hypot(e.clientX-g.x,e.clientY-g.y)>=3;if(g.moved)g.point={...g.point,...this.position(id,e)};this.draw();}};
  canvas.onpointerup=e=>{e.stopPropagation();const g=this.preview;if(!g||g.canvas!==canvas)return;g.moved ||= Math.hypot(e.clientX-g.x,e.clientY-g.y)>=3;if(g.moved)g.point={...g.point,...this.position(id,e)};this.preview=null;if(canvas.hasPointerCapture(e.pointerId))canvas.releasePointerCapture(e.pointerId);if(g.moved)void this.save(id,g.point);else this.draw();};
  canvas.onpointercancel=e=>{e.stopPropagation();this.preview=null;this.draw();};
  canvas.ondblclick=e=>{e.stopPropagation();const pos=this.position(id,e);void this.save(id,{pointId:crypto.randomUUID(),...pos,shape:s.discrete?'step':'linear'});};
  canvas.onkeydown=e=>{if(shortcutCommand(e)==='edit.delete'){e.stopPropagation();e.preventDefault();void this.remove(id);}else if(e.key==='Escape'&&this.preview){e.stopPropagation();const g=this.preview;this.preview=null;if(g.canvas.hasPointerCapture(g.pointer))g.canvas.releasePointerCapture(g.pointer);this.draw();}else if(['edit.cut','edit.copy','edit.paste','edit.duplicate','audio.crossfade'].includes(shortcutCommand(e)??'')){e.stopPropagation();e.preventDefault();}};
  canvas.oncontextmenu=e=>{e.preventDefault();e.stopPropagation();const p=this.hit(id,e);if(p){this.selected.set(id,p.pointId);void this.remove(id);}};
 }
 private position(id:string,e:MouseEvent):{tick:string;value:number}{const c=this.canvases.get(id)!,s=this.spec(id)!,b=c.getBoundingClientRect(),v=this.wave.viewport;const tick=this.wave.editTick(v.start+Math.max(0,Math.min(1,(e.clientX-b.left)/b.width))*v.span);let value=s.max-Math.max(0,Math.min(1,(e.clientY-b.top-15)/Math.max(1,b.height-40)))*(s.max-s.min);if(s.choices)value=s.choices.reduce((a,b)=>Math.abs(a[0]-value)<Math.abs(b[0]-value)?a:b)[0];else value=Math.round(value/s.step)*s.step;return {tick:String(tick),value:Math.max(s.min,Math.min(s.max,value))};}
 private hit(id:string,e:MouseEvent):Point|undefined{const c=this.canvases.get(id)!,s=this.spec(id)!,b=c.getBoundingClientRect(),v=this.wave.viewport;return this.lane(id)?.points.find(p=>Math.hypot(b.left+(this.wave.musical.seconds(Number(p.tick))-v.start)/v.span*b.width-e.clientX,b.top+15+(s.max-p.value)/(s.max-s.min)*(b.height-40)-e.clientY)<9);}
 private async save(id:string,p:Point):Promise<void>{const s=this.spec(id);if(!s)return;let l=this.lane(id);if(!l){await this.edit({command:'automation.lane.add',trackIds:id==='master'?[]:[id],parameter:s.parameter});l=this.lane(id);}if(!l)return;this.selected.set(id,p.pointId);await this.edit({command:'automation.point.set',trackIds:id==='master'?[]:[id],laneId:l.laneId,parameter:s.parameter,pointId:p.pointId,targetTick:p.tick,value:p.value,shape:p.shape});}
 private async remove(id:string):Promise<void>{const l=this.lane(id),pointId=this.selected.get(id);if(l&&pointId)await this.edit({command:'automation.point.delete',trackIds:id==='master'?[]:[id],laneId:l.laneId,pointId});this.selected.delete(id);}
 private async parameter(id:string,s:ParameterSpec,value:number):Promise<void>{
  if(!this.doc)return;const t=this.doc.tracks.find(t=>t.trackId===id),common={trackIds:id==='master'?[]:[id],parameter:s.parameter,value};
  if(s.parameter.effectId){const e=structuredClone((id==='master'?this.doc.master?.inserts:t?.inserts)?.find(e=>e.effectId===s.parameter.effectId));if(!e)return;const name=s.parameter.name;if(name==='bypass')e.enabled=value<.5;else if(name.startsWith('plugin.')){const p=(e.plugin as PluginSelection).parameters.find(p=>p.id===Number(name.slice(7)));if(p)p.value=value;}else if(name.startsWith('band')){const [band,k]=name.split('.');(e.bands as Record<string,number>[])[Number(band.slice(4))][k]=value;}else e[name]=name==='syncBeats'?(value||null):value;await this.edit({command:'effect.set',effectId:e.effectId,effect:e,...common});
  }else if(s.parameter.name.startsWith('plugin.')){await this.edit({command:'plugin.parameter',...common});}
  else if(s.parameter.name.startsWith('synth.')){await this.edit({command:'synth.set',synth:{levelDb:0,attackMs:5,releaseMs:40,...t?.synth,[s.parameter.name.slice(6)]:value},...common});}
  else await this.edit({command:id==='master'?'master.volume':'track.mix',[s.parameter.name]:value,...common});
 }
 draw():void{
  if(!this.doc)return;this.pointEditor.refresh();const v=this.wave.viewport;
  for(const[id,c]of this.canvases){const hasSelection=this.lane(id)?.points.some(p=>p.pointId===this.selected.get(id))??false;const button=this.headers.get(id)?.querySelector<HTMLButtonElement>('.automation-point-open');if(button)button.disabled=!hasSelection;const s=this.spec(id)!;const width=c.clientWidth,height=c.clientHeight,d=devicePixelRatio;if(!width||!height)continue;c.width=Math.round(width*d);c.height=Math.round(height*d);const ctx=c.getContext('2d')!;ctx.scale(d,d);ctx.fillStyle='#20262a';ctx.fillRect(0,0,width,height);
   for(const l of this.wave.musical.lines(v.start,v.span,width,this.wave.grid)){ctx.strokeStyle=l.bar?'#465058':'#30393e';ctx.beginPath();ctx.moveTo(l.x,0);ctx.lineTo(l.x,height);ctx.stroke();}
   const points=(this.lane(id)?.points??[]).map(p=>this.preview?.id===id&&p.pointId===this.preview.point.pointId?this.preview.point:p).sort((a,b)=>Number(a.tick)-Number(b.tick));
   const x=(tick:number)=>(this.wave.musical.seconds(tick)-v.start)/v.span*width,y=(value:number)=>15+(s.max-value)/(s.max-s.min)*(height-40);
   ctx.strokeStyle=automationChannel(this.doc,id)?.read===false?'#737c80':'#a4c9b2';ctx.lineWidth=1.5;ctx.beginPath();ctx.moveTo(0,y(at(points,this.wave.musical.ticks(v.start),s.base,s.discrete)));
   let previous:Point|undefined;for(const p of points){const px=x(Number(p.tick));if(px<0){previous=p;continue}if(px>width)break;if(s.discrete||previous?.shape==='step')ctx.lineTo(px,y(previous?.value??p.value));ctx.lineTo(px,y(p.value));previous=p;}ctx.lineTo(width,y(at(points,this.wave.musical.ticks(v.start+v.span),s.base,s.discrete)));ctx.stroke();
   for(const p of points){const px=x(Number(p.tick));if(px<0||px>width)continue;ctx.fillStyle=p.pointId===this.selected.get(id)?'#e1c56c':'#bfd8c8';ctx.fillRect(px-4,y(p.value)-4,8,8);}
   ctx.font=`${Math.round(parseFloat(getComputedStyle(document.documentElement).fontSize)*.75)}px Consolas`;ctx.fillStyle='#aeb9bf';const selected=points.find(p=>p.pointId===this.selected.get(id));ctx.fillText(selected?`${this.wave.musical.preciseLabel(Number(selected.tick))} · ${selected.value.toFixed(3)}`:`${s.label} · ${s.min} … ${s.max}`,8,height-7);
  }
 }
}
