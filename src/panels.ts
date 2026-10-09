import { uiText, uiAttr } from './i18n';
import { element } from './dom';
import { readWorkspace, workspaceGeometry, workspaceKey, zonePanels, clamp, type ZoneId, type WorkspaceState } from './workspace-layout';
import { satellite, isPanel, type FloatingLayout } from './panel-context';

export interface PanelDefinition { id:string; title:string; element:string }
interface Panel extends PanelDefinition { root:HTMLElement; button:HTMLButtonElement; tab?:HTMLButtonElement; zone?:ZoneId }
interface Zone { root:HTMLElement; tabs:HTMLElement; divider:HTMLElement }
const zoneNames:Record<ZoneId,string>={left:'Left Zone',right:'Right Zone',lower:'Lower Zone'};

// Fixed zones only. Panel DOM and editor state survive tab changes; isOpen and
// visibility events describe actual visible content, including hidden tab peers.
export class Panels {
  windows?:FloatingLayout;
  private satelliteVisible=false;
  private panels:Panel[]=[];
  private workspace=element('workspace');
  private center=document.createElement('div');
  private zones={} as Record<ZoneId,Zone>;
  private state:WorkspaceState;
  private resizing:{pointer:number;zone:ZoneId;handle:HTMLElement;origin:number;size:number}|null=null;
  constructor(definitions:PanelDefinition[]){
    const read=(key:string):unknown=>{try{return JSON.parse(localStorage.getItem(key)??'null');}catch{return null;}};
    this.state=readWorkspace(read(workspaceKey),read('minidaw.ui.panels.v1'));
    if(satellite){
      document.body.classList.add('panel-window');
      for(const definition of definitions){
        const root=element(definition.element);root.hidden=definition.id!==satellite;
        if(definition.id!==satellite)continue;
        root.classList.add('floating-panel');document.body.append(root);
        this.panels.push({...definition,root,button:document.createElement('button')});
        const heading=root.querySelector('.workspace-panel-heading')!;
        const dock=document.createElement('button');dock.id='panel-dock';uiText(dock,'Zone에 도킹');heading.append(dock);
        dock.onclick=()=>document.dispatchEvent(new CustomEvent('panel-window-action',{detail:'dock'}));
        root.querySelector<HTMLButtonElement>('[data-panel-close]')!.onclick=()=>document.dispatchEvent(new CustomEvent('panel-window-action',{detail:'close'}));
      }
      return;
    }
    this.center.id='zone-center';this.center.className='workspace-center';this.workspace.append(this.center);
    this.center.append(element('workspace-empty'));
    for(const id of Object.keys(zonePanels) as ZoneId[]){
      const root=document.createElement('div');root.id=`zone-${id}`;root.className=`workspace-zone zone-${id}`;
      const tabs=document.createElement('nav');tabs.className='zone-tabs';tabs.setAttribute('role','tablist');uiAttr(tabs,'aria-label',`${zoneNames[id]} 탭`);
      const divider=document.createElement('div');divider.id=`resize-${id}`;divider.className=`zone-divider divider-${id}`;divider.dataset.zone=id;divider.tabIndex=0;
      divider.setAttribute('role','separator');divider.setAttribute('aria-controls',root.id);divider.setAttribute('aria-orientation',id==='lower'?'horizontal':'vertical');uiAttr(divider,'aria-label',`${zoneNames[id]} 크기 조절`);
      this.zones[id]={root,tabs,divider};this.workspace.append(root,divider);
      divider.onpointerdown=e=>{
        if(e.button!==0)return;e.preventDefault();this.finish();this.changing();
        this.resizing={pointer:e.pointerId,zone:id,handle:divider,origin:id==='lower'?e.clientY:e.clientX,size:id==='lower'?root.clientHeight:root.clientWidth};
        divider.setPointerCapture(e.pointerId);this.workspace.classList.add('resizing');this.workspace.dataset.resizeAxis=id==='lower'?'y':'x';
      };
      divider.onkeydown=e=>{
        const keys=id==='lower'?['ArrowUp','ArrowDown']:['ArrowLeft','ArrowRight'];
        if(!keys.includes(e.key)||e.defaultPrevented)return;e.preventDefault();e.stopPropagation();this.changing();
        const amount=(e.key===keys[0]?-1:1)*(e.shiftKey?40:12)*(id==='left'?1:-1);
        this.resize(id,(id==='lower'?root.clientHeight:root.clientWidth)+amount);this.save();
      };
    }
    for(const definition of definitions){
      const root=element(definition.element),button=document.createElement('button');
      const zone=(Object.keys(zonePanels) as ZoneId[]).find(z=>zonePanels[z].includes(definition.id));
      button.id=`show-${definition.id}`;uiText(button,definition.title);button.setAttribute('aria-controls',root.id);
      const panel:Panel={...definition,root,button,zone};this.panels.push(panel);
      button.onclick=()=>this.setOpen(panel.id,!(this.windows?.detached(panel.id)?this.windows.visible(panel.id):this.isOpen(panel.id)));element('panel-switches').append(button);
      if(isPanel(panel.id)){
        const detach=document.createElement('button');detach.id=`detach-${panel.id}`;detach.className='panel-detach';uiText(detach,'↗');uiAttr(detach,'title','독립 창으로 분리');uiAttr(detach,'aria-label','독립 창으로 분리');
        detach.onclick=()=>this.windows?.detach(panel.id);root.querySelector('.workspace-panel-heading')!.append(detach);
      }
      root.querySelector<HTMLButtonElement>('[data-panel-close]')!.onclick=()=>this.setOpen(panel.id,false);
      if(zone){
        const tab=document.createElement('button');panel.tab=tab;tab.id=`zone-tab-${panel.id}`;tab.type='button';tab.setAttribute('role','tab');tab.setAttribute('aria-controls',root.id);uiText(tab,panel.title);
        tab.onclick=()=>{this.setOpen(panel.id,true);tab.focus();};
        tab.onkeydown=e=>{
          if(!['ArrowLeft','ArrowRight'].includes(e.key)||e.defaultPrevented)return;
          e.preventDefault();e.stopPropagation();const ids=zonePanels[zone].filter(id=>!this.windows?.detached(id)),index=ids.indexOf(panel.id),next=ids[(index+(e.key==='ArrowRight'?1:ids.length-1))%ids.length];
          this.setOpen(next,true);this.panels.find(p=>p.id===next)?.tab?.focus();
        };
        this.zones[zone].tabs.append(tab);this.zones[zone].root.append(root);
        root.setAttribute('role','tabpanel');root.setAttribute('aria-labelledby',tab.id);
        root.querySelector('.workspace-panel-heading')!.classList.add('zone-panel-heading');
      }else this.center.prepend(root);
    }
    for(const id of Object.keys(zonePanels) as ZoneId[])for(const p of zonePanels[id]){const tab=this.panels.find(panel=>panel.id===p)?.tab;if(tab)this.zones[id].tabs.append(tab);}
    this.workspace.addEventListener('pointermove',e=>this.move(e));
    for(const event of ['pointerup','pointercancel','lostpointercapture'])this.workspace.addEventListener(event,()=>this.finish());
    window.addEventListener('blur',()=>this.finish());
    new ResizeObserver(()=>this.sizes()).observe(this.workspace);
    this.layout();this.save();
  }
  isOpen(id:string):boolean {
    if(satellite)return id===satellite&&this.satelliteVisible;
    if(this.windows?.detached(id))return false;
    const p=this.panels.find(p=>p.id===id);if(!p)return false;
    return p.zone?this.state.zones[p.zone].open&&this.state.zones[p.zone].active===id:this.state.arrangement;
  }
  setOpen(id:string,open:boolean):void {
    if(satellite){document.dispatchEvent(new CustomEvent('panel-host-show',{detail:{id,open}}));return;}
    if(this.windows?.detached(id)){this.windows.show(id,open);return;}
    const p=this.panels.find(p=>p.id===id);if(!p||this.isOpen(id)===open)return;
    const before=new Map(this.panels.map(p=>[p.id,this.isOpen(p.id)]));this.finish();this.changing();
    if(p.zone){const z=this.state.zones[p.zone];z.open=open;if(open)z.active=id;}else this.state.arrangement=open;
    this.layout();this.save();
    for(const panel of this.panels){const visible=this.isOpen(panel.id);if(before.get(panel.id)!==visible)document.dispatchEvent(new CustomEvent('panel-visibility',{detail:{id:panel.id,open:visible}}));}
    if(!open)p.button.focus();
  }
  owns(id:string):boolean {return satellite?id===satellite&&this.satelliteVisible:!this.windows?.detached(id);}
  activateSatellite(open:boolean):void {this.satelliteVisible=open;document.dispatchEvent(new CustomEvent('panel-visibility',{detail:{id:satellite,open}}));}
  floatingChanged(id:string,docked=false):void {
    const before=new Map(this.panels.map(p=>[p.id,!p.root.hidden]));
    const p=this.panels.find(p=>p.id===id);if(!p?.zone)return;
    const z=this.state.zones[p.zone];
    if(docked){z.active=id;z.open=true;}
    else if(z.active===id){const peer=zonePanels[p.zone].find(other=>!this.windows?.detached(other));if(peer)z.active=peer;else z.open=false;}
    this.layout();this.save();
    for(const panel of this.panels)if(before.get(panel.id)!==this.isOpen(panel.id))document.dispatchEvent(new CustomEvent('panel-visibility',{detail:{id:panel.id,open:this.isOpen(panel.id)}}));
  }
  private changing():void {document.dispatchEvent(new CustomEvent('workspace-changing'));}
  private layout():void {
    for(const panel of this.panels){
      const visible=this.isOpen(panel.id);panel.root.hidden=!visible;panel.button.setAttribute('aria-pressed',String(visible||!!this.windows?.visible(panel.id)));
      if(panel.tab){panel.tab.hidden=!!this.windows?.detached(panel.id);panel.tab.setAttribute('aria-selected',String(visible));panel.tab.tabIndex=visible?0:-1;}
    }
    for(const id of Object.keys(zonePanels) as ZoneId[]){
      const z=this.zones[id],s=this.state.zones[id];z.root.hidden=!s.open;z.divider.hidden=!s.open||(id==='lower'&&!this.state.arrangement);
      const active=this.panels.find(p=>p.id===s.active)!;
      active.root.querySelector('.workspace-panel-heading')!.prepend(z.tabs);
    }
    element('workspace-empty').hidden=this.state.arrangement||this.state.zones.lower.open;
    this.center.hidden=!this.state.arrangement&&this.state.zones.lower.open;
    this.workspace.classList.toggle('has-lower',this.state.zones.lower.open);
    this.sizes();
  }
  private sizes():void {
    const g=workspaceGeometry(this.state,this.workspace.clientWidth,this.workspace.clientHeight,parseFloat(getComputedStyle(document.documentElement).fontSize)/16.5);
    this.workspace.style.gridTemplateColumns=`${g.left}px ${g.leftGap}px minmax(0,1fr) ${g.rightGap}px ${g.right}px`;
    this.workspace.style.gridTemplateRows=this.state.zones.lower.open&&!this.state.arrangement?'0px 0px minmax(0,1fr)':`minmax(0,1fr) ${g.lowerGap}px ${g.lower}px`;
    for(const id of Object.keys(zonePanels) as ZoneId[]){
      const d=this.zones[id].divider;d.setAttribute('aria-valuemin','0');d.setAttribute('aria-valuemax',String(Math.round(id==='lower'?this.workspace.clientHeight:this.workspace.clientWidth)));
      d.setAttribute('aria-valuenow',String(Math.round(g[id])));
    }
  }
  private resize(id:ZoneId,size:number):void {
    const extent=id==='lower'?this.workspace.clientHeight:this.workspace.clientWidth;
    this.state.zones[id].size=clamp(size,120,Math.max(120,extent));this.sizes();
    if(id==='lower')this.state.zones.lower.size=this.zones.lower.root.clientHeight;
    else for(const side of ['left','right'] as const){
      // At the Center's width limit both side zones can be constrained. Commit
      // the visible pair together, so reopening reproduces the divider position.
      if(this.state.zones[side].open)this.state.zones[side].size=this.zones[side].root.clientWidth;
    }
    this.sizes();
  }
  private move(e:PointerEvent):void {
    const r=this.resizing;if(!r||r.pointer!==e.pointerId)return;
    const delta=(r.zone==='lower'?e.clientY:e.clientX)-r.origin;
    this.resize(r.zone,r.size+delta*(r.zone==='left'?1:-1));
  }
  private finish():void {
    const r=this.resizing;if(!r)return;this.resizing=null;
    if(r.handle.hasPointerCapture(r.pointer))r.handle.releasePointerCapture(r.pointer);
    this.workspace.classList.remove('resizing');delete this.workspace.dataset.resizeAxis;this.save();
  }
  private save():void {try{localStorage.setItem(workspaceKey,JSON.stringify(this.state));}catch{/* Optional UI preference. */}}
}
