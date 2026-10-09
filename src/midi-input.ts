import { uiText, uiOption, tr } from './i18n';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { element } from './dom';
import type { MidiClip } from './midi';
interface Status {device:string|null;received:number;delivered:number;dropped:number;last:number;frame:number;sampleClock:number}
export class MidiKeyboard {
  private port=element<HTMLSelectElement>('midi-input-port');
  private tracks:{trackId:string;kind:string;instrument?:'none'|'basicSynth'|'external';clips:({clipId:string}|MidiClip)[]}[]=[];
  private clipId:string|null=null;
  private route:string|null|undefined;
  private routing=Promise.resolve();
  private active=false;
  constructor(private owns:()=>boolean=()=>true){
    element('midi-input-refresh').onclick=()=>void this.run(()=>this.refresh());
    element('midi-input-connect').onclick=()=>void this.run(async()=>{const option=this.port.selectedOptions[0];await invoke('midi_input_connect',{port:this.port.value===''?null:Number(this.port.value),name:option?.textContent});this.route=undefined;await this.routeToClip();});
    element('midi-input-disconnect').onclick=()=>void this.run(async()=>{await invoke('midi_input_connect',{port:null,name:null});this.route=null;});
    document.addEventListener('project-view',e=>{this.tracks=(e as CustomEvent).detail.document.tracks;void this.run(()=>this.routeToClip());});
    document.addEventListener('piano-active-part',e=>{this.clipId=(e as CustomEvent).detail.clipId;void this.run(()=>this.routeToClip());});
    void this.run(()=>this.refresh());
    window.setInterval(()=>{if(!element('panel-piano').hidden&&!this.active)void this.run(()=>this.status());},500);
  }
  private async run(f:()=>Promise<void>):Promise<void>{if(!isTauri())return;try{await f();}catch(e){uiText(element('midi-input-status'), (e as {message?:string}).message??String(e));}}
  private async refresh():Promise<void>{const ports=await invoke<{id:number;name:string}[]>('midi_input_ports');const before=this.port.value;this.port.replaceChildren(uiOption('연결 안 함',''),...ports.map(p=>new Option(p.name,String(p.id))));if(ports.some(p=>String(p.id)===before))this.port.value=before;await this.status();}
  private routeToClip():Promise<void>{this.routing=this.routing.catch(()=>{}).then(async()=>{if(!this.owns())return;const id=this.tracks.find(t=>t.kind==='midi'&&t.clips.some(c=>c.clipId===this.clipId))?.trackId??null;if(id===this.route)return;await invoke('midi_input_route',{trackId:id});this.route=id;});return this.routing;}
  private async status():Promise<void>{this.active=true;try{const s=await invoke<Status>('midi_input_status');uiText(element('midi-input-device-state'), s.device??'연결 안 됨');const status=s.last>>>16,a=(s.last>>>8)&127,b=s.last&127,kind=status&240;const last=kind===0x90&&b?'Note On':kind===0x80||kind===0x90?'Note Off':kind===0xb0?`CC ${a}`:kind===0xe0?'Pitch Bend':'—';uiText(element('midi-input-status'), `${tr(this.route?'선택 Clip Track으로 전달':'MIDI Clip을 선택하세요')} · 수신 ${s.received} / 처리 ${s.delivered} / 누락 ${s.dropped} · ${last} Ch ${(status&15)+1} ${kind===0xe0?b*128+a-8192:b} · sample ${s.frame} · ${this.tracks.find(t=>t.trackId===this.route)?.instrument==='basicSynth'?'MiniDAW Synth → Master':this.tracks.find(t=>t.trackId===this.route)?.instrument==='external'?'External Instrument → Master':tr('악기 없음')}`);}finally{this.active=false;}}
}
