import { uiText, uiAttr } from './i18n';
import {element} from './dom';
export const TEMPO_SYNC='minidaw.tempoSync.v1';
interface Fraction {numerator:string;denominator:string}
export interface TempoSync {enabled:boolean;sourceBpm:number;definition:null|{ticks:string;window:{start:Fraction;end:Fraction}}}
interface Clip {clipId:string;extensions?:Record<string,unknown>}
export function sync(c:Clip):TempoSync|undefined {return c.extensions?.[TEMPO_SYNC] as TempoSync|undefined;}
export function syncTicks(c:Clip):number|null {
  const s=sync(c),d=s?.definition;if(!s?.enabled||!d)return null;
  const at=(f:Fraction)=>{const den=BigInt(f.denominator);return (BigInt(f.numerator)*BigInt(d.ticks)+den/2n)/den;};
  return Number(at(d.window.end)-at(d.window.start));
}
export class AudioTempoSync {
  private toggle=element<HTMLInputElement>('clip-tempo-sync');
  private bpm=element<HTMLInputElement>('clip-source-bpm');
  private status=element('clip-tempo-status');
  private pending=false;
  constructor(private host:{clips():Clip[];bpm():number;busy():boolean;apply(ids:string[],fields:Record<string,unknown>):Promise<void>}) {
    this.toggle.onchange=()=>void this.apply({tempoSync:this.toggle.checked});
    this.bpm.onchange=()=>{const value=Number(this.bpm.value);if(!this.bpm.value.trim()||!Number.isFinite(value)||value<1||value>1000){this.bpm.setAttribute('aria-invalid','true');uiText(this.status, 'Source BPM: 1–1000');return;}void this.apply({sourceBpm:value});};
    this.bpm.oninput=()=>this.bpm.removeAttribute('aria-invalid');
    this.bpm.onkeydown=e=>{if(e.key==='Enter'){e.preventDefault();e.stopPropagation();this.bpm.blur();}else if(e.key==='Escape'){e.preventDefault();e.stopPropagation();this.render(true);this.bpm.select();}};
  }
  render(force=false):void {
    const clips=this.host.clips(),states=clips.map(sync),first=states[0],busy=this.pending||this.host.busy();
    this.toggle.disabled=this.bpm.disabled=!clips.length||busy;
    this.toggle.checked=!!first?.enabled;this.toggle.indeterminate=states.some(s=>!!s?.enabled!==!!first?.enabled);
    const values=states.map(s=>s?.sourceBpm??this.host.bpm()),same=values.every(v=>v===values[0]);
    if(force||document.activeElement!==this.bpm){this.bpm.value=clips.length&&same?String(values[0]):'';uiAttr(this.bpm, 'placeholder', same?'BPM':'여러 값');}
    if(force)this.bpm.removeAttribute('aria-invalid');
    if(!this.bpm.hasAttribute('aria-invalid'))uiText(this.status, busy?'Tempo/Stretch 준비 중…':states.some(s=>s?.enabled)?`Source / Project BPM · ${(values[0]/this.host.bpm()*100).toFixed(3)}% · Tick 고정`:'');
  }
  private async apply(fields:Record<string,unknown>):Promise<void> {
    if(this.pending||this.host.busy())return;const clips=this.host.clips();if(!clips.length)return;
    this.pending=true;this.render();try{await this.host.apply(clips.map(c=>c.clipId),fields);}finally{this.pending=false;this.render(true);}
  }
}
