import { uiText } from './i18n';
import { shortcutCommand } from './shortcuts';
import { element } from './dom';
import type { MidiClip, MidiControl } from './midi';

interface Host {
  clip(): MidiClip | null; selected(): Set<string>; select(ids: Set<string>): void;
  x(tick: number): number; tick(x: number): number; snap(tick: number): number;
  draw(): void; drawing(): boolean; busy(): boolean; edit(r: unknown): Promise<void>;
}
type Gesture = { pointer: number; event?: MidiControl; velocity?: number; notes?: string[]; changed: boolean; startX: number; startY: number };
const bound = (n: number, lo: number, hi: number) => Math.max(lo, Math.min(hi, n));
export class MidiLane {
  private canvas = element<HTMLCanvasElement>('midi-lane-canvas');
  private type = element<HTMLSelectElement>('midi-lane-type');
  private cc = element<HTMLInputElement>('midi-cc');
  private channel = element<HTMLInputElement>('midi-channel');
  private value = element<HTMLInputElement>('midi-lane-value');
  private selected: string | null = null;
  private gesture: Gesture | null = null;
  constructor(private host: Host) {
    for (const input of [this.type,this.cc,this.channel]) input.onchange = () => {this.cancel();this.selected=null;this.cc.value=String(bound(Math.round(Number(this.cc.value)||0),0,127));this.channel.value=String(bound(Math.round(Number(this.channel.value)||1),1,16));this.host.draw();};
    this.canvas.onpointerdown=e=>this.down(e);this.canvas.onpointermove=e=>this.move(e);this.canvas.onpointerup=e=>void this.up(e);
    this.canvas.onpointercancel=()=>this.cancel();this.canvas.onlostpointercapture=()=>this.cancel();
    element('midi-lane-apply').onclick=()=>void this.apply();
    element('midi-lane-delete').onclick=()=>void this.remove();
    this.canvas.addEventListener('keydown',e=>{if(shortcutCommand(e)==='edit.delete'){e.preventDefault();e.stopPropagation();void this.remove();}});
    new ResizeObserver(()=>this.host.draw()).observe(this.canvas);
  }
  captureWindowState(){return {type:this.type.value,cc:this.cc.value,channel:this.channel.value,value:this.value.value,selected:this.selected};}
  restoreWindowState(s:ReturnType<MidiLane['captureWindowState']>):void {if(!s)return;this.type.value=s.type;this.cc.value=s.cc;this.channel.value=s.channel;this.value.value=s.value;this.selected=s.selected;}
  private controller(): number {return this.type.value==='sustain'?64:bound(Math.round(Number(this.cc.value)),0,127);}
  private events(): MidiControl[] {return this.host.clip()?.controls.filter(e=>e.channel===Number(this.channel.value)-1 && (this.type.value==='pitchBend'?e.data.kind==='pitchBend':e.data.kind==='cc'&&e.data.controller===this.controller())).slice().sort((a,b)=>Number(a.tick)-Number(b.tick))??[];}
  private limits(): [number,number] {return this.type.value==='velocity'?[1,127]:this.type.value==='pitchBend'?[-8192,8191]:[0,127];}
  private y(value:number):number {const [a,b]=this.limits();return 8+(b-value)/(b-a)*Math.max(1,this.canvas.clientHeight-16);}
  private at(e:PointerEvent) {
    const box=this.canvas.getBoundingClientRect(),x=e.clientX-box.left,y=e.clientY-box.top,[a,b]=this.limits();
    let value=Math.round(bound(b-(y-8)/Math.max(1,this.canvas.clientHeight-16)*(b-a),a,b));
    if(this.type.value==='sustain') value=value>=64?127:0;
    return {x,y,value,tick:bound(this.host.snap(this.host.tick(x))-Number(this.host.clip()?.startTick??0),0,Number(this.host.clip()?.lengthTick??0))};
  }
  private down(e:PointerEvent):void {
    const c=this.host.clip();if(e.button!==0||!c||this.host.busy()||this.gesture)return;
    const at=this.at(e);if(at.x<56)return;e.preventDefault();this.canvas.focus();
    if(this.type.value==='velocity') {
      const n=c.notes.slice().reverse().find(n=>Math.abs(this.host.x(Number(c.startTick)+Number(n.startTick))-at.x)<8);
      if(!n)return;
      if(!this.host.selected().has(n.noteId))this.host.select(new Set([n.noteId]));
      this.gesture={pointer:e.pointerId,velocity:n.velocity,notes:[...this.host.selected()],changed:false,startX:at.x,startY:at.y};
      this.value.value=String(n.velocity);
    } else {
      const hit=this.events().slice().reverse().find(n=>Math.abs(this.host.x(Number(c.startTick)+Number(n.tick))-at.x)<9&&Math.abs(this.y(n.data.value)-at.y)<10);
      if(!hit&&!this.host.drawing()){this.selected=null;this.host.draw();return;}
      const event:MidiControl=hit?structuredClone(hit):{eventId:'',tick:String(at.tick),channel:Number(this.channel.value)-1,data:this.type.value==='pitchBend'?{kind:'pitchBend',value:at.value}:{kind:'cc',controller:this.controller(),value:at.value}};
      this.selected=event.eventId;this.value.value=String(event.data.value);
      this.gesture={pointer:e.pointerId,event,changed:!hit,startX:at.x,startY:at.y};
    }
    this.canvas.setPointerCapture(e.pointerId);this.host.draw();
  }
  private move(e:PointerEvent):void {
    const at=this.at(e),g=this.gesture;
    uiText(element('midi-lane-readout'), `${at.tick} clip ticks · ${at.value}`);
    if(!g||g.pointer!==e.pointerId)return;
    g.changed ||= Math.abs(at.x-g.startX)>=2||Math.abs(at.y-g.startY)>=2;
    if(!g.changed)return;
    if(g.event){g.event.tick=String(at.tick);g.event.data.value=at.value;}else g.velocity=at.value;
    this.value.value=String(at.value);this.host.draw();
  }
  private async up(e:PointerEvent):Promise<void> {
    const g=this.gesture,c=this.host.clip();if(!g||g.pointer!==e.pointerId||!c)return;
    this.move(e);this.gesture=null;if(this.canvas.hasPointerCapture(e.pointerId))this.canvas.releasePointerCapture(e.pointerId);
    if(g.changed) {
      const before=new Set(c.controls.map(e=>e.eventId));
      await this.host.edit(g.event?{command:'midi.control.put',clipIds:[c.clipId],event:g.event}:{command:'midi.velocity',clipIds:[c.clipId],noteIds:g.notes,velocity:g.velocity});
      if(g.event)this.selected=g.event.eventId||this.host.clip()?.controls.find(e=>!before.has(e.eventId))?.eventId||null;
    }
    this.host.draw();
  }
  private async apply():Promise<void> {
    const c=this.host.clip(),v=Number(this.value.value),[a,b]=this.limits();if(!c||!Number.isInteger(v)||v<a||v>b)return;
    if(this.type.value==='velocity'){if(this.host.selected().size)await this.host.edit({command:'midi.velocity',clipIds:[c.clipId],noteIds:[...this.host.selected()],velocity:v});}
    else {const e=this.events().find(e=>e.eventId===this.selected);if(e)await this.host.edit({command:'midi.control.put',clipIds:[c.clipId],event:{...e,data:{...e.data,value:v}}});}
  }
  private async remove():Promise<void> {const c=this.host.clip();if(c&&this.selected&&this.type.value!=='velocity'){await this.host.edit({command:'midi.control.delete',clipIds:[c.clipId],eventId:this.selected});this.selected=null;this.host.draw();}}
  cancel():void {const g=this.gesture;this.gesture=null;if(g&&this.canvas.hasPointerCapture(g.pointer))this.canvas.releasePointerCapture(g.pointer);}
  resetSelection():void {this.cancel();this.selected=null;}
  paint():void {
    const w=this.canvas.clientWidth,h=this.canvas.clientHeight,dpr=devicePixelRatio||1,c=this.host.clip();if(!w||!h)return;
    if(this.selected&&!c?.controls.some(e=>e.eventId===this.selected))this.selected=null;
    this.canvas.width=Math.round(w*dpr);this.canvas.height=Math.round(h*dpr);const ctx=this.canvas.getContext('2d')!;ctx.scale(dpr,dpr);
    ctx.fillStyle='#222428';ctx.fillRect(0,0,w,h);ctx.font=`${parseFloat(getComputedStyle(document.documentElement).fontSize)*.7}px Segoe UI`;
    const [min,max]=this.limits();this.value.min=String(min);this.value.max=String(max);
    element('midi-cc-label').hidden=this.type.value!=='cc';element('midi-channel-label').hidden=this.type.value==='velocity';
    element<HTMLButtonElement>('midi-lane-delete').disabled=!c||!this.selected||this.type.value==='velocity'||this.host.busy();
    element<HTMLButtonElement>('midi-lane-apply').disabled=!c||this.host.busy()||(this.type.value==='velocity'?!this.host.selected().size:!this.selected);
    for(const v of [min,Math.round((min+max)/2),max]){const y=this.y(v);ctx.fillStyle='#bfc1c6';ctx.fillText(String(v),4,bound(y+4,12,h-2));ctx.strokeStyle='#393c41';ctx.beginPath();ctx.moveTo(56,y);ctx.lineTo(w,y);ctx.stroke();}
    if(!c)return;ctx.save();ctx.beginPath();ctx.rect(56,0,w-56,h);ctx.clip();
    if(this.type.value==='velocity') {
      for(const n of c.notes){const v=this.gesture?.notes?.includes(n.noteId)?this.gesture.velocity!:n.velocity,x=this.host.x(Number(c.startTick)+Number(n.startTick)),y=this.y(v);ctx.fillStyle=this.host.selected().has(n.noteId)?'#e8ca6b':'#8695bf';ctx.fillRect(x-2,y,5,h-8-y);ctx.fillRect(x-4,y-2,9,4);}
    } else {
      let events=this.events().map(e=>e.eventId===this.gesture?.event?.eventId?this.gesture.event:e);
      if(this.gesture?.event&&!this.gesture.event.eventId)events.push(this.gesture.event);
      events=events.sort((a,b)=>Number(a.tick)-Number(b.tick));
      for(let i=0;i<events.length;i++){const e=events[i],x=this.host.x(Number(c.startTick)+Number(e.tick)),y=this.y(e.data.value),end=i+1<events.length?this.host.x(Number(c.startTick)+Number(events[i+1].tick)):w;ctx.strokeStyle='#8695bf';ctx.beginPath();ctx.moveTo(x,y);ctx.lineTo(end,y);if(i+1<events.length)ctx.lineTo(end,this.y(events[i+1].data.value));ctx.stroke();ctx.fillStyle=e.eventId===this.selected?'#e8ca6b':'#bac3e4';ctx.fillRect(x-3,y-3,7,7);}
    }ctx.restore();
  }
}
