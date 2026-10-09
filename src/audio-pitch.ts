import { uiText } from './i18n';
import { element } from './dom';
interface Pitch {semitones:number;cents:number}
interface Clip {clipId:string;extensions?:Record<string,unknown>}
const KEY='minidaw.pitchShift.v1';
export class AudioPitch {
  private semitones=element<HTMLInputElement>('clip-pitch');
  private cents=element<HTMLInputElement>('clip-pitch-cents');
  private reset=element<HTMLButtonElement>('clip-pitch-reset');
  private status=element('clip-pitch-status');
  private signature='';
  constructor(private host:{clip():Clip|undefined;busy():boolean;apply(id:string,pitch:Pitch):Promise<void>}){
    for(const input of [this.semitones,this.cents]){
      input.onchange=()=>void this.apply(input);
      input.oninput=()=>{input.removeAttribute('aria-invalid');uiText(this.status, '');};
      input.onkeydown=e=>{if(e.key==='Enter'){e.preventDefault();e.stopPropagation();input.blur();}else if(e.key==='Escape'){e.preventDefault();e.stopPropagation();this.render(true);input.select();}};
    }
    this.reset.onclick=()=>{const c=this.host.clip();if(c&&!this.host.busy())void this.host.apply(c.clipId,{semitones:0,cents:0}).then(()=>this.render(true));};
  }
  render(force=false):void{
    const clip=this.host.clip(),pitch=(clip?.extensions?.[KEY] as Pitch|undefined)??{semitones:0,cents:0};
    const signature=JSON.stringify([clip?.clipId,pitch]);if(signature!==this.signature){this.signature=signature;force=true;}
    const busy=this.host.busy();
    for(const input of [this.semitones,this.cents]){
      input.disabled=!clip||busy;
      if(force)input.removeAttribute('aria-invalid');
    }
    if(force||document.activeElement!==this.semitones)this.semitones.value=clip?String(pitch.semitones):'';
    if(force||document.activeElement!==this.cents)this.cents.value=clip?String(pitch.cents):'';
    this.reset.disabled=!clip||busy||(pitch.semitones===0&&pitch.cents===0);
    if(force)uiText(this.status, '');
    if(busy)uiText(this.status, 'Pitch/Time 처리 중…');
    else if(!this.semitones.hasAttribute('aria-invalid')&&!this.cents.hasAttribute('aria-invalid'))uiText(this.status, '');
  }
  private async apply(input:HTMLInputElement):Promise<void>{
    const c=this.host.clip();if(!c||this.host.busy())return;
    const pitch=(c.extensions?.[KEY] as Pitch|undefined)??{semitones:0,cents:0};
    const value=Number(input.value),limit=input===this.semitones?12:100;
    if(!input.value.trim()||!Number.isInteger(value)||Math.abs(value)>limit){input.setAttribute('aria-invalid','true');uiText(this.status, `${-limit}–${limit} 정수를 입력하세요.`);return;}
    const next={...pitch,[input===this.semitones?'semitones':'cents']:value};
    if(next.semitones!==pitch.semitones||next.cents!==pitch.cents)await this.host.apply(c.clipId,next);
    this.render(true);
  }
}
