import { invoke, isTauri } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { element } from './dom';
import { editingText, keyboardBlocked, globalShortcut } from './keyboard';
import { shortcutCommand, shortcutLabel } from './shortcuts';
import { uiText, uiAttr, tr } from './i18n';
import { noteName } from './midi';

// Physical keys work in either Korean or English layout. MIDI 60 = C3, as in
// the existing Piano Roll. Modifier combinations remain DAW commands.
export const pianoKeys: Readonly<Record<string, number>> = {
  KeyA:0, KeyW:1, KeyS:2, KeyE:3, KeyD:4, KeyF:5, KeyT:6,
  KeyG:7, KeyY:8, KeyH:9, KeyU:10, KeyJ:11, KeyK:12,
};
interface Track { trackId:string; name:string; kind:string; instrument?:string; extensions?:Record<string,unknown> }
export class ComputerMidi {
  private enabled=false;
  private root=60;
  private trackId:string|null=null;
  private tracks:Track[]=[];
  private project='';
  private targetSignature='';
  private held=new Map<string,number>();
  private swallowed=new Set<string>();
  private pending:Promise<void>=Promise.resolve();
  private failureEpoch=0;
  private toggle=element<HTMLButtonElement>('computer-midi-toggle');
  constructor(private error:(e:unknown)=>void) {
    this.toggle.onclick=()=>this.setEnabled(!this.enabled);
    element('computer-midi-down').onclick=()=>this.octave(-1);
    element('computer-midi-up').onclick=()=>this.octave(1);
    // Register before global DAW handlers. Only actual performance keys are
    // consumed; Space and remapped non-piano keys keep their normal dispatch.
    document.addEventListener('keydown',e=>this.down(e),true);
    document.addEventListener('keyup',e=>this.up(e),true);
    globalShortcut(e=>{
      const id=shortcutCommand(e);
      if(id==='midi.computerToggle')return()=>this.setEnabled(!this.enabled);
      if(id==='midi.computerOctaveDown')return()=>this.octave(-1);
      if(id==='midi.computerOctaveUp')return()=>this.octave(1);
    });
    window.addEventListener('blur',()=>{this.release();this.swallowed.clear();});
    // Owned native plug-in windows can leave WebView's document focused. Also
    // observe the Tauri top-level window, not only DOM blur.
    if(isTauri())void getCurrentWindow().onFocusChanged(({payload})=>{
      if(!payload){this.release();this.swallowed.clear();}
    }).catch(this.error);
    document.addEventListener('visibilitychange',()=>{if(document.hidden){this.release();this.swallowed.clear();}});
    document.addEventListener('focusin',e=>{if(editingText(e.target)||(e.target instanceof Element&&e.target.closest('dialog')))this.release();});
    document.addEventListener('workspace-changing',()=>this.release());
    document.addEventListener('track-selected',e=>{
      const id=(e as CustomEvent<{trackId:string|null}>).detail.trackId;
      if(id!==this.trackId){this.release();this.trackId=id;}
      this.targetChanged();
    });
    document.addEventListener('project-view',e=>{
      const doc=(e as CustomEvent<{document:{projectId:string;tracks:Track[]}}>).detail.document;
      if(this.project!==doc.projectId){this.release();this.trackId=null;this.project=doc.projectId;}
      this.tracks=doc.tracks;this.targetChanged();
    });
    document.addEventListener('language-changed',()=>this.render());
    document.addEventListener('shortcuts-changed',()=>this.render());
    this.render();
  }
  private target():Track|undefined { return this.tracks.find(t=>t.trackId===this.trackId&&t.kind==='midi'); }
  private targetChanged():void {
    const t=this.target(),signature=t?`${t.trackId}:${t.instrument}:${JSON.stringify((t.extensions?.['minidaw.plugin.v1'] as {descriptor?:unknown}|undefined)?.descriptor)}`:'';
    if(signature!==this.targetSignature){this.release();this.targetSignature=signature;}
    this.render();
  }
  captureWindowState(){return {enabled:this.enabled,root:this.root};}
  restoreWindowState(s:ReturnType<ComputerMidi['captureWindowState']>):void {if(!s)return;this.release();this.enabled=s.enabled;this.root=s.root;this.render();}
  private setEnabled(on:boolean):void { this.release();this.enabled=on;this.render();document.dispatchEvent(new Event('computer-midi-changed')); }
  private octave(direction:number):void { this.root=Math.max(0,Math.min(108,this.root+direction*12));this.render();document.dispatchEvent(new Event('computer-midi-changed')); }
  private send(trackId:string|null,notes:number[]):void {
    if(!isTauri())return;
    const epoch=this.failureEpoch;
    // Preserve On/Off order, including rapid taps and a release during an IPC.
    // No UI timer stamps MIDI events; LiveReader assigns the Rust sample frame.
    this.pending=this.pending.then(()=>epoch===this.failureEpoch?invoke<void>('computer_midi_notes',{trackId,notes}):undefined).catch(async e=>{
      this.failureEpoch++;
      this.held.clear();this.enabled=false;this.render();this.error(e);
      await invoke('computer_midi_notes',{trackId:null,notes:[]}).catch(()=>{});
    });
  }
  private release():void {
    if(this.held.size){this.held.clear();this.send(null,[]);this.render();}
    // Keep consumed key codes until keyup: a focus/mode change must not turn an
    // already-held piano key's auto-repeat into a DAW command or button click.
  }
  private down(e:KeyboardEvent):void {
    if(this.swallowed.has(e.code)){e.preventDefault();e.stopImmediatePropagation();return;}
    const offset=pianoKeys[e.code];
    if(!this.enabled||offset===undefined||e.ctrlKey||e.altKey||e.metaKey||e.shiftKey||keyboardBlocked(e))return;
    e.preventDefault();e.stopImmediatePropagation();this.swallowed.add(e.code);
    if(e.repeat)return;
    const t=this.target();if(!t||!t.instrument||t.instrument==='none')return;
    this.held.set(e.code,this.root+offset);
    this.send(t.trackId,[...new Set(this.held.values())]);this.render();
  }
  private up(e:KeyboardEvent):void {
    if(!this.swallowed.delete(e.code))return;
    e.preventDefault();e.stopImmediatePropagation();
    if(this.held.delete(e.code)){this.send(this.target()?.trackId??null,[...new Set(this.held.values())]);this.render();}
  }
  private render():void {
    this.toggle.setAttribute('aria-pressed',String(this.enabled));
    uiText(this.toggle,this.enabled?'컴퓨터 건반 ON':'컴퓨터 건반 OFF');
    uiAttr(this.toggle,'title',`${tr('컴퓨터 키보드 MIDI 연주')}${shortcutLabel('midi.computerToggle')?' ('+shortcutLabel('midi.computerToggle')+')':''}`,false);
    uiText(element('computer-midi-octave'),`${noteName(this.root)}–${noteName(this.root+12)}`,false);
    element<HTMLButtonElement>('computer-midi-down').disabled=this.root===0;
    element<HTMLButtonElement>('computer-midi-up').disabled=this.root===108;
    const t=this.target();uiText(element('computer-midi-track'),t?.name??'MIDI Track을 선택하세요',!t);
    uiText(element('computer-midi-state'),t?.instrument&&t.instrument!=='none'?`${this.held.size}음 · Velocity 100`:'악기 없음');
  }
}
