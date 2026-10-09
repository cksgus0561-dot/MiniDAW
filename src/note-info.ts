import { uiText, uiAttr } from './i18n';
import { element } from './dom';
import type { MidiClip } from './midi';
import type { MusicalClock } from './musical-time';
import { formatNoteTime, parseNoteTime, type NoteTimeField } from './note-time';

interface Host {clip():MidiClip|null;selected():Set<string>;clock():MusicalClock;busy():boolean;edit(r:unknown):Promise<void>}
export class NoteInfo {
  private format=element<HTMLSelectElement>('note-time-format');
  private message=element('note-time-message');
  private signature='';
  private dirty=new Set<NoteTimeField>();
  private pending=false;
  private fields:NoteTimeField[]=['start','end','length'];
  constructor(private host:Host) {
    this.format.onchange=()=>{this.signature='';this.dirty.clear();this.render();};
    for(const field of this.fields) {
      const input=element<HTMLInputElement>(`note-${field}`);
      input.oninput=()=>{this.dirty.add(field);input.setCustomValidity('');input.removeAttribute('aria-invalid');uiText(this.message, '');};
      input.onblur=()=>{if(this.dirty.has(field))void this.apply(field,false);};
      input.onkeydown=e=>{
        if(e.key==='Enter'){e.preventDefault();e.stopPropagation();void this.apply(field,e.ctrlKey||e.metaKey);}
        if(e.key==='Escape'){e.preventDefault();e.stopPropagation();this.dirty.delete(field);this.signature='';this.render();input.select();}
      };
    }
  }
  private anchor(){const c=this.host.clip(),ids=this.host.selected(),id=ids.values().next().value;return {c,ids,n:c?.notes.find(n=>n.noteId===id)};}
  render():void {
    const {c,ids,n}=this.anchor(),clock=this.host.clock();
    const signature=JSON.stringify([c?.clipId,c?.startTick,[...ids],n,clock.time.timeSignatures,this.format.value]);
    const changed=signature!==this.signature;
    if(changed){this.signature=signature;this.dirty.clear();uiText(this.message, '');}
    element('piano-note-info').classList.toggle('multiple',ids.size>1);
    uiText(element('note-time-hint'), ids.size>1?'첫 선택 기준 · Enter/포커스 이동: 상대 변경 · Ctrl+Enter: 같은 값':'Start/End: 프로젝트 위치 · Length: 0부터 센 길이 · Enter 적용 / Esc 취소');
    for(const field of this.fields) {
      const input=element<HTMLInputElement>(`note-${field}`);input.disabled=!n||this.host.busy()||this.pending;
      if(changed||!this.dirty.has(field)) {
        const start=Number(c?.startTick??0)+Number(n?.startTick??0);
        const value=field==='start'?start:field==='end'?start+Number(n?.lengthTick??0):Number(n?.lengthTick??0);
        input.value=n?formatNoteTime(clock,value,field,start,this.format.value==='ticks'):'';
        uiAttr(input, 'title', n?`${value} ticks${field==='length'?' · 기준 Note 시작 박자표의 Bar/Beat 길이':''}`:'');
        input.setCustomValidity('');input.removeAttribute('aria-invalid');
      }
    }
  }
  private async apply(field:NoteTimeField,absolute:boolean):Promise<void> {
    if(!this.dirty.has(field)||this.pending||this.host.busy())return;
    const {c,ids,n}=this.anchor();if(!c||!n)return;
    const input=element<HTMLInputElement>(`note-${field}`),start=Number(c.startTick)+Number(n.startTick);
    const value=parseNoteTime(this.host.clock(),input.value,field,start,this.format.value==='ticks');
    if(value===null){uiText(this.message, field==='length'?'Length는 0 기반 Bar.Beat.Sixteenth.Tick, 최소 1 tick입니다.':'Start/End는 1 기반 Bar.Beat.Sixteenth.Tick 위치를 입력하세요.');input.setAttribute('aria-invalid','true');return;}
    this.dirty.delete(field);this.pending=true;
    try {await this.host.edit({command:'midi.note.time',clipIds:[c.clipId],noteIds:[...ids],noteId:n.noteId,noteTimeField:field,targetTick:String(value),absolute});}
    finally {this.pending=false;this.signature='';this.render();}
  }
}
