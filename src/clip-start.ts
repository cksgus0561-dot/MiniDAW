import { uiText, uiAttr } from './i18n';
import { element } from './dom';
import type { MusicalClock, Position } from './musical-time';
import { parseNoteTime } from './note-time';

interface Item {id:string;kind:'audio'|'midi';start:number;position:Position}
interface Host {
  items():Item[];
  clock():MusicalClock;
  busy():boolean;
  move(command:string,request:Record<string,unknown>):Promise<void>;
}
// Seconds are parsed as decimal integers, not a rounded floating-point delta.
function secondsPosition(text:string):Position|null {
  const m=/^(?:(\d+):)?(\d+)(?:\.(\d{1,9}))?\s*s?$/.exec(text.trim());
  if(!m||(m[1]!==undefined&&Number(m[2])>=60))return null;
  const d=10n**BigInt(m[3]?.length??0),n=(BigInt(m[1]??0)*60n+BigInt(m[2]))*d+BigInt(m[3]??0);
  if(n>9223372036854775807n)return null;
  return {unit:'seconds',numerator:String(n),denominator:Number(d)};
}
function difference(target:Position,origin:Position,clock:MusicalClock):Position {
  const rational=(p:Position):[bigint,bigint]=>p.unit==='seconds'?[BigInt(p.numerator),BigInt(p.denominator)]:[BigInt(Math.round(clock.seconds(Number(p.ticks))*1e9)),1000000000n];
  const [a,b]=rational(target),[c,d]=rational(origin);let n=a*d-c*b,den=b*d;
  let x=n<0n?-n:n,y=den;while(y){[x,y]=[y,x%y];}const g=x||1n;n/=g;den/=g;
  if(den>BigInt(Number.MAX_SAFE_INTEGER)||n< -9223372036854775808n||n>9223372036854775807n)throw Error('지원하는 시간 정밀도 범위를 벗어났습니다.');
  return {unit:'seconds',numerator:String(n),denominator:Number(den)};
}
export class ClipStart {
  private input=element<HTMLInputElement>('clip-start');
  private message=element('clip-start-message');
  private signature='';private dirty=false;private pending=false;
  constructor(private host:Host){
    this.input.oninput=()=>{this.dirty=true;this.input.removeAttribute('aria-invalid');uiText(this.message, '');};
    this.input.onchange=()=>void this.apply();
    this.input.onkeydown=e=>{if(e.key==='Enter'){e.preventDefault();e.stopPropagation();this.input.blur();}else if(e.key==='Escape'){e.preventDefault();e.stopPropagation();this.dirty=false;this.render(true);this.input.select();}};
    document.addEventListener('ruler-format-changed',()=>this.render(true));
  }
  private bars():boolean{return element<HTMLSelectElement>('ruler-format').value!=='seconds';}
  private formatted(item:Item):string{return this.bars()?this.host.clock().preciseLabel(item.position.unit==='ticks'?Number(item.position.ticks):this.host.clock().ticks(item.start)):`${item.start.toFixed(9)} s`;}
  render(force=false):void{
    const items=this.host.items(),first=items[0],mixed=items.some(i=>i.kind!==first?.kind);
    const signature=JSON.stringify([items.map(i=>[i.id,i.position]),this.bars(),this.host.clock().time]);
    if(force||signature!==this.signature){this.signature=signature;this.dirty=false;this.input.removeAttribute('aria-invalid');uiText(this.message, '');}
    this.input.disabled=!first||mixed||this.host.busy()||this.pending;
    if(!this.dirty)this.input.value=first?this.formatted(first):'';
    this.input.classList.toggle('multiple',items.length>1);
    uiAttr(this.input, 'title', mixed?'Audio와 MIDI를 각각 선택해 Start를 편집하세요.':`${this.bars()?'Bar.Beat.Sixteenth.Tick':'초 (예: 3.125 s 또는 00:03.125)'} · Enter/포커스 이동 적용 · Esc 취소 · Snap 무시${items.length>1?' · 첫 선택 기준, 선택 Clip의 간격 유지':''}${first?.kind==='midi'?' · MIDI 위치는 1 tick 단위':''}`);
    uiAttr(this.input, 'placeholder', this.bars()?'1.1.1.000000':'0.000000000 s');
  }
  private async apply():Promise<void>{
    if(!this.dirty||this.pending||this.host.busy())return;
    const items=this.host.items(),first=items[0],clock=this.host.clock();if(!first||items.some(i=>i.kind!==first.kind))return;
    if(this.input.value.trim()===this.formatted(first)){this.dirty=false;this.render();return;}
    let request:Record<string,unknown>;
    try{
      const target=this.bars()?parseNoteTime(clock,this.input.value,'start',0,false):secondsPosition(this.input.value);
      if(target===null)throw Error(this.bars()?'유효한 Bar.Beat.Sixteenth.Tick 위치를 입력하세요.':'0 이상의 초 또는 분:초를 입력하세요 (소수점 최대 9자리).');
      if(typeof target==='number'||first.kind==='midi'){
        const tick=typeof target==='number'?target:Math.round(clock.ticks(clock.position(target)));
        if(!Number.isSafeInteger(tick)||tick<0)throw Error('Tick 위치 범위를 벗어났습니다.');
        if(typeof target!=='number'&&Math.abs(clock.seconds(tick)-clock.position(target))>.5001e-9)throw Error(`MIDI는 1 tick 단위입니다. 가장 가까운 위치: ${clock.seconds(tick).toFixed(9)} s`);
        request={targetTick:String(tick)};
      }else request={delta:difference(target,first.position,clock)};
    }catch(e){this.input.setAttribute('aria-invalid','true');uiText(this.message, (e as Error).message);return;}
    this.dirty=false;this.pending=true;this.render();
    try{await this.host.move(first.kind==='audio'?'audio.move':'midi.clip.move',{...request,clipIds:items.map(i=>i.id),anchorClipId:first.id});}
    finally{this.pending=false;this.render(true);}
  }
}
