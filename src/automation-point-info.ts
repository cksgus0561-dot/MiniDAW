import { uiAttr, uiText, uiAppend } from './i18n';
import { shortcutCommand } from './shortcuts';
import type { MusicalClock } from './musical-time';
import { parseNoteTime } from './note-time';
import type { ParameterSpec } from './automation';
import { keyboardBlocked } from './keyboard';

export interface AutomationPoint {pointId:string;tick:string;value:number;shape:'linear'|'step'}
interface Host {
  current(): {point:AutomationPoint;spec:ParameterSpec;clock:MusicalClock} | null;
  save(point:AutomationPoint):Promise<void>;
  remove():Promise<void>;
}
// The point editor is separate from the live Parameter/Automation Write control.
// It edits one stored point via automation.point.set, without Grid rounding.
export class AutomationPointInfo {
  private root=document.createElement('div');
  private position=document.createElement('input');
  private value=document.createElement('input');
  private format=document.createElement('select');
  private message=document.createElement('small');
  private label=document.createElement('span');
  private signature='';
  private busy=false;
  constructor(private host:Host){
    this.root.id='automation-point-editor';this.root.popover='auto';uiAttr(this.root, 'aria-label', 'Automation Point 수치 편집');
    const title=document.createElement('strong');uiText(title, 'Automation Point');
    const close=document.createElement('button');uiText(close, '×');uiAttr(close, 'aria-label', 'Point 편집 닫기');close.onclick=()=>this.root.hidePopover();
    const heading=document.createElement('header');heading.append(title,close);
    uiAttr(this.format, 'aria-label', 'Point Position 형식');
    for(const [value,text]of [['musical','Bar.Beat.Sixteenth.Tick'],['ticks','Absolute Tick']]){const o=document.createElement('option');o.value=value;uiText(o, text);this.format.append(o);}
    this.format.onchange=()=>this.refresh(true);
    this.position.type='text';this.position.id='automation-point-position';this.position.spellcheck=false;
    this.value.type='number';this.value.id='automation-point-value';this.value.step='any';
    const p=document.createElement('label');uiAppend(p, 'Position', this.position);
    const v=document.createElement('label');uiAppend(v, 'Value', this.value);
    const help=document.createElement('small');uiText(help, 'Enter/포커스 이동 적용 · Esc 취소 · Snap과 무관한 정확한 위치');
    this.message.setAttribute('role','status');this.root.append(heading,this.label,this.format,p,v,help,this.message);document.body.append(this.root);
    this.root.addEventListener('keydown',e=>{
      if(keyboardBlocked(e))return;
      if(shortcutCommand(e)==='edit.delete'){e.preventDefault();e.stopPropagation();if(!e.repeat)void this.host.remove();}
      else if(['edit.cut','edit.copy','edit.paste','edit.duplicate','audio.crossfade'].includes(shortcutCommand(e)??'')){e.preventDefault();e.stopPropagation();}
    });
    for(const [input,field]of [[this.position,'tick'],[this.value,'value']] as const){
      input.oninput=()=>{input.removeAttribute('aria-invalid');uiText(this.message, '');};
      input.onchange=()=>void this.apply(field);
      input.onkeydown=e=>{if(e.key==='Enter'){e.preventDefault();e.stopPropagation();input.blur();}else if(e.key==='Escape'){e.preventDefault();e.stopPropagation();this.refresh(true);input.select();}};
    }
    document.addEventListener('workspace-changing',()=>this.root.hidePopover());
  }
  open(anchor:HTMLElement):void{
    if(!this.host.current())return;this.refresh(true);this.root.showPopover();
    const b=anchor.getBoundingClientRect();this.root.style.left=`${Math.max(4,Math.min(b.right+5,innerWidth-this.root.offsetWidth-4))}px`;this.root.style.top=`${Math.max(4,Math.min(b.top,innerHeight-this.root.offsetHeight-4))}px`;
  }
  refresh(force=false):void{
    const c=this.host.current();if(!c){this.root.hidePopover();this.signature='';return;}
    const signature=JSON.stringify([c.point,c.spec.parameter,c.clock.time,this.format.value]);
    if(!force&&signature===this.signature)return;this.signature=signature;
    this.position.value=this.format.value==='ticks'?c.point.tick:c.clock.preciseLabel(Number(c.point.tick));uiAttr(this.position, 'title', `${c.point.tick} ticks`);
    this.value.value=String(c.point.value);this.value.min=String(c.spec.min);this.value.max=String(c.spec.max);
    uiText(this.label, c.spec.label);uiText(this.message, '');
    for(const input of [this.position,this.value]){input.removeAttribute('aria-invalid');input.disabled=this.busy;}
  }
  private async apply(field:'tick'|'value'):Promise<void>{
    const c=this.host.current();if(!c||this.busy)return;
    const input=field==='tick'?this.position:this.value;
    const next=field==='tick'?parseNoteTime(c.clock,input.value,'start',0,this.format.value==='ticks'):input.valueAsNumber;
    const valid=next!==null&&Number.isFinite(next)&&(field==='tick'||(next>=c.spec.min&&next<=c.spec.max&&(!c.spec.choices||c.spec.choices.some(([v])=>v===next))));
    if(!valid){input.setAttribute('aria-invalid','true');uiText(this.message, field==='tick'?'유효한 Bar.Beat.Sixteenth.Tick 또는 0 이상의 정수 Tick을 입력하세요.':c.spec.choices?`허용 값: ${c.spec.choices.map(([v])=>v).join(', ')}`:`범위: ${c.spec.min} … ${c.spec.max}`);return;}
    if(String(next)===String(c.point[field])){this.refresh(true);return;}
    this.busy=true;this.position.disabled=true;this.value.disabled=true;
    try{await this.host.save({...c.point,[field]:field==='tick'?String(next):next});}
    finally{this.busy=false;this.refresh(true);}
  }
}
