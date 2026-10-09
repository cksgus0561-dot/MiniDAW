import { uiAttr, uiText } from './i18n';
import {sync} from './audio-tempo-sync';
import { element } from './dom';
export const STRETCH='minidaw.timeStretch.v1';
export interface StretchRecipe {sourceStart:string;sourceEnd:string;outputFrames:string}
interface Clip {clipId:string;sourceStart:string;sourceEnd:string;extensions?:Record<string,unknown>}
interface Item {clip:Clip;rate:number}
export function recipe(c:Clip):StretchRecipe|undefined{return c.extensions?.[STRETCH] as StretchRecipe|undefined;}
export function ratio(c:Clip):number{const r=recipe(c);return r?Number(r.outputFrames)/(Number(r.sourceEnd)-Number(r.sourceStart)):1;}
// Reuse original multi-resolution peaks at their stretched time coordinates.
export function originalTime(c:Clip,rate:number,virtual:number):number{const r=recipe(c);return r?Number(r.sourceStart)/rate+virtual/ratio(c):virtual;}
export function renderedTime(c:Clip,rate:number,original:number):number{const r=recipe(c);return r?(original-Number(r.sourceStart)/rate)*ratio(c):original;}
export class AudioStretch {
  private percent=element<HTMLInputElement>('clip-stretch');
  private duration=element<HTMLInputElement>('clip-stretch-length');
  private status=element('clip-stretch-status');
  constructor(private host:{item():Item|undefined;busy():boolean;apply(id:string,frames:string):Promise<void>}){
    for(const input of [this.percent,this.duration]){
      input.onchange=()=>void this.apply(input);
      input.onkeydown=e=>{if(e.key==='Enter'){e.preventDefault();e.stopPropagation();input.blur();}else if(e.key==='Escape'){e.preventDefault();e.stopPropagation();this.render(true);input.select();}};
      input.oninput=()=>input.removeAttribute('aria-invalid');
    }
  }
  render(force=false):void{
    const i=this.host.item(),busy=this.host.busy();
    for(const input of [this.percent,this.duration]){input.disabled=!i||busy||!!sync(i.clip)?.enabled;uiAttr(input, 'title', i&&sync(i.clip)?.enabled?'Tempo Sync가 길이를 결정합니다. 수동 Stretch는 Sync를 끈 뒤 사용하세요.':'');}
    if(force||document.activeElement!==this.percent)this.percent.value=i?String(Number((ratio(i.clip)*100).toFixed(6))):'';
    if(force||document.activeElement!==this.duration)this.duration.value=i?((Number(i.clip.sourceEnd)-Number(i.clip.sourceStart))/i.rate).toFixed(9):'';
    if(force)for(const input of [this.percent,this.duration])input.removeAttribute('aria-invalid');
    uiText(this.status, busy?'처리 중…':i?`${Number(i.clip.sourceEnd)-Number(i.clip.sourceStart)} samples`:'');
  }
  private async apply(input:HTMLInputElement):Promise<void>{
    const i=this.host.item();if(!i||this.host.busy()||sync(i.clip)?.enabled)return;
    const value=Number(input.value),old=Number(i.clip.sourceEnd)-Number(i.clip.sourceStart);
    const frames=input===this.duration?Math.round(value*i.rate):Math.round(old/ratio(i.clip)*value/100);
    const resultRatio=ratio(i.clip)*frames/old;
    if(!input.value.trim()||!Number.isFinite(value)||!Number.isSafeInteger(frames)||frames<1||resultRatio<.5-1e-9||resultRatio>2+1e-9){input.setAttribute('aria-invalid','true');uiText(this.status, '길이 비율 50–200% · 1 sample 이상');return;}
    if(frames===old){this.render(true);return;}
    await this.host.apply(i.clip.clipId,String(frames));this.render(true);
  }
}
