import { uiText, uiAttr } from './i18n';
import pluginLicenses from "./plugin-licenses.txt?raw";
import {invoke,isTauri} from '@tauri-apps/api/core';
import {open} from '@tauri-apps/plugin-dialog';
import {numericInput} from './numeric-input';
export interface PluginDescriptor {path:string;format:'vst3'|'clap';id:string;name:string;vendor:string;instrument:boolean}
export interface PluginParameter {id:number;name:string;min:number;max:number;value:number;stepped:boolean;step?:number;readonly:boolean}
export interface PluginSelection {descriptor:PluginDescriptor;state:string;parameters:PluginParameter[];latency:number;tail:number;sampleRate:number}
interface Catalog {paths:string[];plugins:PluginDescriptor[];errors:string[]}
interface Status {instanceId:string;name:string;error:string|null;info:{latency?:number;editorOpen?:boolean;dirty?:boolean;faulted?:boolean;restartRequired?:boolean;processCalls?:number;droppedEvents?:number}}
export const choosePlugin=(trackId:string,kind:'instrument'|'effect')=>document.dispatchEvent(new CustomEvent('plugin-choose',{detail:{trackId,kind}}));
export const openPlugin=(id:string)=>document.dispatchEvent(new CustomEvent('plugin-editor',{detail:id}));
export function pluginControls(plugin:PluginSelection,id:string,change:(id:number,value:number)=>void):HTMLElement {
 const root=document.createElement('section');root.className='plugin-controls';
 const editor=document.createElement('button');uiText(editor, 'Plugin Editor 열기');editor.onclick=()=>openPlugin(id);editor.dataset.pluginEditor=id;
 const note=document.createElement('p');note.className='metric-note';note.dataset.pluginLatency=id;note.dataset.pluginFormat=plugin.descriptor.format;uiText(note, plugin.descriptor.format.toUpperCase()+' · 지연 '+plugin.latency+' samples · 자동 PDC');
 const select=document.createElement('select');uiAttr(select, 'aria-label', 'Plugin Parameter');const writable=plugin.parameters.filter(p=>!p.readonly&&p.max>p.min);
 for(const p of writable){const o=document.createElement('option');o.value=String(p.id);uiText(o, p.name, false);select.append(o);}
 const input=document.createElement('input');input.type='number';uiAttr(input, 'aria-label', 'Plugin Parameter 값');
 const show=()=>{const p=writable.find(p=>p.id===Number(select.value));if(p){input.min=String(p.min);input.max=String(p.max);input.step=p.step?String(p.step):'any';input.value=String(p.value);}input.disabled=!p;};
 select.onchange=show;show();numericInput(input,value=>{const p=writable.find(p=>p.id===Number(select.value));if(p){value=Math.max(p.min,Math.min(p.max,p.step?Math.round((value-p.min)/p.step)*p.step+p.min:value));p.value=value;input.value=String(value);change(p.id,value);}});
 root.append(editor,note,select,input);return root;
}
export class Plugins {
 private dialog=document.createElement('dialog');private title=document.createElement('h2');private paths=document.createElement('textarea');
 private filter=document.createElement('input');private list=document.createElement('div');private message=document.createElement('p');private scanButton=document.createElement('button');private instances=document.createElement('section');
 private catalog:Catalog={paths:[],plugins:[],errors:[]};private context:{trackId:string;kind:'instrument'|'effect'}|null=null;private working=false;private polling=false;private instanceKey="";
 private view:{revision:number;document:{projectId:string;tracks:{trackId:string;name:string;instrument?:string;extensions?:Record<string,unknown>}[]}}|null=null;
 constructor(private edit:(r:Record<string,unknown>)=>Promise<void>,private ready:()=>boolean,private error:(e:unknown)=>void){
  const menu=document.querySelector('#file-menu .file-menu-content')??document.querySelector('#file-menu');
  const entry=document.createElement('button');entry.id='plugin-manager-open';uiText(entry, 'Plug-in Manager…');entry.onclick=()=>{(document.querySelector('#file-menu') as HTMLDetailsElement).open=false;void this.show(null).catch(this.error);};menu?.append(entry);
  this.dialog.id='plugin-manager';uiAttr(this.dialog, 'aria-label', 'Plug-in Manager');this.paths.id='plugin-paths';this.paths.rows=2;uiAttr(this.paths, 'placeholder', '추가 검색 폴더 (한 줄에 하나)');
  const roots=document.createElement('p');roots.className='metric-note';uiText(roots, 'Windows 공용·사용자 VST3/CLAP 경로와 아래 폴더를 검색합니다. Windows x64 · Mono/Stereo Main 출력.');
  const browse=document.createElement('button');uiText(browse, '폴더 추가…');browse.onclick=()=>void open({directory:true,multiple:true}).then(paths=>{if(paths)this.paths.value+=(this.paths.value?'\n':'')+(Array.isArray(paths)?paths:[paths]).join('\n');}).catch(this.error);
  this.scanButton.id='plugin-scan';uiText(this.scanButton, 'Scan');this.scanButton.onclick=()=>void this.scan();
  this.filter.id='plugin-filter';this.filter.type='search';uiAttr(this.filter, 'placeholder', '플러그인 이름 검색');this.filter.oninput=()=>this.render();
  this.list.id='plugin-list';this.list.className='plugin-list';this.message.id='plugin-message';this.message.setAttribute('role','status');this.message.className='metric-note';
  this.instances.id='plugin-instances';const close=document.createElement('button');uiText(close, '닫기');close.id='plugin-manager-close';close.onclick=()=>this.dialog.close();
  const bar=document.createElement('div');bar.className='plugin-toolbar';bar.append(browse,this.scanButton,this.filter);
  const licenses=document.createElement("details");const summary=document.createElement("summary");uiText(summary, "호스트 SDK 라이선스 (MIT)");const pre=document.createElement("pre");uiText(pre, pluginLicenses, false);licenses.append(summary,pre);
  this.dialog.append(this.title,roots,this.paths,bar,this.list,this.instances,this.message,licenses,close);document.body.append(this.dialog);
  const warning=document.createElement('p');warning.id='plugin-runtime-warning';warning.className='metric-note';warning.hidden=true;document.querySelector('#project-notice')?.after(warning);
  document.addEventListener('plugin-choose',e=>void this.show((e as CustomEvent).detail).catch(this.error));
  document.addEventListener('plugin-editor',e=>{if(isTauri())void invoke('plugin_editor',{instanceId:(e as CustomEvent<string>).detail,show:true}).catch(this.error);});
  document.addEventListener('project-view',e=>{this.view=(e as CustomEvent).detail;});
  if(isTauri()){void invoke<{catalog:Catalog}>('plugin_catalog').then(s=>{this.catalog=s.catalog;this.paths.value=s.catalog.paths.join('\n');}).catch(this.error);setInterval(()=>void this.poll(),700);}
 }
 private async show(context:typeof this.context):Promise<void>{
  if(!isTauri()||!this.ready())return;this.context=context;this.instanceKey="";uiText(this.message, '');
  const state=await invoke<{catalog:Catalog}>('plugin_catalog');this.catalog=state.catalog;this.paths.value=this.catalog.paths.join('\n');this.render();this.dialog.showModal();void this.poll();
 }
 private render():void{
  uiText(this.title, this.context?this.context.kind==='instrument'?'가상악기 선택':'Insert Plug-in 선택':'Plug-in Manager');
  this.list.replaceChildren();const query=this.filter.value.toLocaleLowerCase();
  for(const d of this.catalog.plugins.filter(d=>(!this.context||d.instrument===(this.context.kind==='instrument'))&&(d.name+' '+d.vendor).toLocaleLowerCase().includes(query))){
   const row=document.createElement('div');row.className='plugin-row';row.dataset.pluginId=d.id;row.dataset.format=d.format;
   const label=document.createElement('span');uiText(label, d.name+' · '+(d.instrument?'Instrument':'Effect')+' · '+d.format.toUpperCase());uiAttr(label, 'title', d.path, false);
   const button=document.createElement('button');uiText(button, '로드');button.disabled=!this.context||this.working;button.onclick=()=>void this.load(d);row.append(label,button);this.list.append(row);
  }
  if(!this.list.children.length)uiText(this.list, '해당 플러그인이 없습니다. 설치/추가 경로를 확인하고 Scan하세요.');
  this.scanButton.disabled=this.working;this.paths.disabled=this.working;
  if(!this.working)uiText(this.message, this.catalog.errors.join('\n'));
 }
 private async scan():Promise<void>{
  if(this.working)return;this.working=true;this.render();
  try{this.catalog=await invoke<Catalog>('plugin_scan',{paths:this.paths.value.split(/\r?\n/).map(s=>s.trim()).filter(Boolean)});}
  catch(e){this.error(e);}finally{this.working=false;this.render();}
 }
 private async load(descriptor:PluginDescriptor):Promise<void>{
  if(!this.context||this.working)return;this.working=true;this.render();uiText(this.message, descriptor.name+' 준비 중…');
  try{const plugin=await invoke<PluginSelection>('plugin_prepare',{descriptor});await this.edit(this.context.kind==='instrument'?{command:'plugin.instrument',trackIds:[this.context.trackId],plugin}:{command:'effect.add',trackIds:this.context.trackId==='master'?[]:[this.context.trackId],plugin});this.dialog.close();}
  catch(e){uiText(this.message, (e as {message?:string}).message??String(e));}
  finally{this.working=false;this.scanButton.disabled=false;this.paths.disabled=false;for(const b of this.list.querySelectorAll('button'))b.disabled=false;}
 }
 private async poll():Promise<void>{
  if(this.polling)return;this.polling=true;
  try {
   if(this.working){const s=await invoke<{scanning:boolean;progress:string}>('plugin_catalog');if(s.scanning)uiText(this.message, s.progress);}
   const statuses=await invoke<Status[]>('plugin_status');for(const n of document.querySelectorAll<HTMLElement>('[data-plugin-latency]')){const status=statuses.find(s=>s.instanceId===n.dataset.pluginLatency);if(status?.info?.latency!==undefined)uiText(n, n.dataset.pluginFormat?.toUpperCase()+' · 지연 '+status.info.latency+' samples · 자동 PDC');}const warnings=statuses.filter(s=>s.error||s.info?.faulted||s.info?.restartRequired);
   const warn=document.querySelector<HTMLElement>('#plugin-runtime-warning')!;warn.hidden=!warnings.length;uiText(warn, warnings.map(s=>(s.name||'Plugin')+': '+(s.error??(s.info.faulted?'처리 오류 · 우회/무음':'I/O/지연 변경 · Plugin 재로드 필요'))).join(' · '));
   if(this.ready()&&!this.working&&statuses.some(s=>s.info?.dirty))await this.edit({command:'plugin.capture'});
   const key=JSON.stringify([this.view?.revision,statuses.map(s=>[s.instanceId,s.info?.editorOpen,s.info?.latency,s.error])]);
   if(this.dialog.open&&!this.context&&this.instanceKey!==key&&!this.instances.contains(document.activeElement)){this.instanceKey=key;this.instances.replaceChildren();for(const s of statuses){const row=document.createElement('div');row.className='plugin-row';const label=document.createElement('span');uiText(label, (s.name||s.instanceId)+' · '+(s.info?.latency??'—')+' samples · 자동 PDC');const editor=document.createElement('button');uiText(editor, s.info?.editorOpen?'Editor 닫기':'Editor 열기');editor.onclick=()=>void invoke('plugin_editor',{instanceId:s.instanceId,show:!s.info?.editorOpen}).catch(this.error);row.append(label,editor);const track=this.view?.document.tracks.find(t=>t.trackId===s.instanceId);const plugin=track?.extensions?.['minidaw.plugin.v1'] as PluginSelection|undefined;if(plugin)row.append(pluginControls(structuredClone(plugin),s.instanceId,(id,value)=>void this.edit({command:'plugin.parameter',trackIds:[s.instanceId],parameter:{name:'plugin.'+id},value})));this.instances.append(row);}}
  }catch(e){if(this.dialog.open)uiText(this.message, (e as {message?:string}).message??String(e));}finally{this.polling=false;}
 }
}
