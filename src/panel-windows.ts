import { isTauri } from '@tauri-apps/api/core';
import { emitTo, listen } from '@tauri-apps/api/event';
import { WebviewWindow } from '@tauri-apps/api/webviewWindow';
import { availableMonitors } from '@tauri-apps/api/window';
import { floatingPanels, isPanel, panelTitles, type FloatingPanel, type PanelStateAdapter } from './panel-context';
import type { Panels } from './panels';

export const windowPreference='minidaw.ui.windows.v1';
export const panelEvent='panel-workspace';
export interface WindowPlacement { detached:boolean;open:boolean;x?:number;y?:number;width:number;height:number;maximized?:boolean }
export interface PanelMessage {kind:string; panel:FloatingPanel; token?:string; data?:any}
const label=(id:string)=>`panel-${id}`;
type Context=Record<string,unknown>;
export class PanelWindows {
  private placements={} as Record<FloatingPanel,WindowPlacement>;
  private ready=new Map<string,()=>void>();
  private waiting=new Map<string,(data:any)=>void>();
  private changing=new Set<string>();
  private chain:Promise<unknown>=Promise.resolve();
  private latest=new Map<string,unknown>();
  private serial=0;
  private liveWindows=new Set<string>();
  constructor(private panels:Panels,private adapters:Partial<Record<FloatingPanel,PanelStateAdapter>>,private context:()=>Context,private action:(kind:string,data:any)=>Promise<unknown>,private error:(e:unknown)=>void){
    let saved:any;try{saved=JSON.parse(localStorage.getItem(windowPreference)??'{}');}catch{}
    for(const id of floatingPanels){const s=saved?.[id];this.placements[id]={detached:s?.detached===true,open:s?.open===true,width:Math.max(480,Math.min(3000,Number(s?.width)||900)),height:Math.max(380,Math.min(2000,Number(s?.height)||650)),...(Number.isFinite(s?.x)&&Number.isFinite(s?.y)?{x:s.x,y:s.y}:{}),maximized:s?.maximized===true};}
  }
  detached(id:string):boolean{return isPanel(id)&&this.placements[id].detached;}
  visible(id:string):boolean{return isPanel(id)&&this.placements[id].detached&&this.placements[id].open;}
  async initialize():Promise<void>{
    if(!isTauri())return;
    await listen<PanelMessage>(panelEvent,e=>void this.message(e.payload).catch(this.error));
    this.panels.windows=this;
    for(const name of ['project-view','midi-clip-selected','track-selected','musical-grid-changed','language-changed','shortcuts-changed'])document.addEventListener(name,e=>{
      if('detail' in e)this.latest.set(name,(e as CustomEvent).detail);
      if(name==='shortcuts-changed')queueMicrotask(()=>this.broadcast(name,null));else this.broadcast(name,'detail' in e?(e as CustomEvent).detail:null);
    });
    for(const id of floatingPanels)if(this.detached(id)){
      this.panels.floatingChanged(id);
      if(this.placements[id].open)await this.open(id,false).catch(e=>{this.placements[id].detached=false;this.placements[id].open=false;this.panels.floatingChanged(id,true);this.error(e);});
    }
    this.save();
  }
  broadcast(name:string,data:unknown):void {
    for(const id of floatingPanels)if(this.liveWindows.has(id))void emitTo(label(id),panelEvent,{kind:'event',panel:id,data:{name,value:data,context:['language-changed','shortcuts-changed','musical-grid-changed','track-selected','midi-clip-selected'].includes(name)?this.context():undefined}}).catch(this.error);
  }
  detach(id:string):void {if(isPanel(id))void this.guard(id,()=>this.open(id,true));}
  show(id:string,open:boolean):void {if(isPanel(id))void this.guard(id,()=>open?this.open(id,false):this.returnFromWindow(id,false));}
  private async guard(id:FloatingPanel,work:()=>Promise<void>):Promise<void>{
    if(this.changing.has(id))return;this.changing.add(id);
    document.dispatchEvent(new Event('workspace-changing'));
    try{await work();}catch(e){this.error(e);}finally{this.changing.delete(id);}
  }
  private async window(id:FloatingPanel):Promise<WebviewWindow>{
    const existing=await WebviewWindow.getByLabel(label(id));if(existing&&this.liveWindows.has(id))return existing;
    const s=this.placements[id],monitors=await availableMonitors();
    const visible=s.x!==undefined&&s.y!==undefined&&monitors.some(m=>{const x=m.position.x/m.scaleFactor,y=m.position.y/m.scaleFactor;return s.x!>=x&&s.x!<x+m.size.width/m.scaleFactor-100&&s.y!>=y&&s.y!<y+m.size.height/m.scaleFactor-60;});
    const pending=new Promise<void>((resolve,reject)=>{
      const timer=setTimeout(()=>{this.ready.delete(id);reject(new Error(`${panelTitles[id]} window initialization timed out`));},20000);
      this.ready.set(id,()=>{clearTimeout(timer);resolve();});
    });
    const w=existing??new WebviewWindow(label(id),{url:`index.html?panel=${id}`,title:`${panelTitles[id]} — MiniDAW`,width:s.width,height:s.height,minWidth:480,minHeight:380,visible:false,focus:false,skipTaskbar:false,decorations:true,resizable:true,maximizable:true,minimizable:true,dragDropEnabled:false,...(visible?{x:s.x,y:s.y}:{center:true})});
    try{await pending;this.liveWindows.add(id);return w;}catch(e){await w.destroy().catch(()=>{});throw e;}
  }
  private async open(id:FloatingPanel,detach:boolean):Promise<void>{
    if(!isTauri())return;
    const s=this.placements[id];
    if(!detach&&s.open&&this.liveWindows.has(id)){const w=await WebviewWindow.getByLabel(label(id));await w?.unminimize();await w?.show();await w?.setFocus();return;}
    const w=await this.window(id);
    const state=detach?this.adapters[id]?.capture():undefined;
    if(detach)await this.adapters[id]?.suspend?.();
    s.detached=true;s.open=true;this.panels.floatingChanged(id);
    await this.request(id,'activate',{context:this.context(),events:Object.fromEntries(this.latest),state});
    if(s.maximized)await w.maximize();await w.show();await w.setFocus();this.save();
  }
  private request(id:FloatingPanel,kind:string,data?:unknown):Promise<any>{
    const token=String(++this.serial);
    return new Promise((resolve,reject)=>{
      const timer=setTimeout(()=>{this.waiting.delete(token);reject(new Error(`${panelTitles[id]} window did not respond`));},15000);
      this.waiting.set(token,result=>{clearTimeout(timer);resolve(result);});
      void emitTo(label(id),panelEvent,{kind,panel:id,token,data}).catch(e=>{clearTimeout(timer);this.waiting.delete(token);reject(e);});
    });
  }
  private async returnFromWindow(id:FloatingPanel,dock:boolean):Promise<void>{
    const state=await this.request(id,'deactivate');
    this.placements[id].open=false;
    await (await WebviewWindow.getByLabel(label(id)))?.hide();
    if(dock){this.adapters[id]?.restore(state);this.placements[id].detached=false;this.panels.floatingChanged(id,true);}
    else this.panels.floatingChanged(id);
    this.save();
  }
  private async message(m:PanelMessage):Promise<void>{
    if(!isPanel(m.panel))return;
    if(m.kind==='ready'){this.ready.get(m.panel)?.();this.ready.delete(m.panel);return;}
    if(m.kind==='reply'){this.waiting.get(m.token!)?.(m.data);this.waiting.delete(m.token!);return;}
    if(m.kind==='geometry'){Object.assign(this.placements[m.panel],m.data);this.save();return;}
    if(m.kind==='dock'||m.kind==='close'){await this.guard(m.panel,()=>this.returnFromWindow(m.panel,m.kind==='dock'));return;}
    if(m.kind==='fps'){if(this.detached('spectrum')){this.broadcast('spectrum-window-fps',m.data);document.dispatchEvent(new CustomEvent('spectrum-window-fps',{detail:m.data}));}return;}
    if(m.kind==='ui-event'){document.dispatchEvent(new CustomEvent(m.data.name,{detail:m.data.value}));return;}
    if(m.kind==='action'){
      // One command queue in the main UI; all edits still use Rust's project revision.
      this.chain=this.chain.catch(()=>{}).then(async()=>{
        try{const value=await this.action(m.data.kind,m.data.value);await emitTo(label(m.panel),panelEvent,{kind:'result',panel:m.panel,token:m.token,data:{value}});}
        catch(e){await emitTo(label(m.panel),panelEvent,{kind:'result',panel:m.panel,token:m.token,data:{error:String((e as {message?:string})?.message??e)}});}
      });
    }
  }
  private save():void{localStorage.setItem(windowPreference,JSON.stringify(this.placements));}
}
