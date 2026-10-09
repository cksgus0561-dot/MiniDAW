export type ZoneId = 'left' | 'right' | 'lower';
export interface ZoneState { open:boolean; active:string; size:number }
export interface WorkspaceState { arrangement:boolean; zones:Record<ZoneId,ZoneState> }
export const workspaceKey = 'minidaw.ui.zones.v1';
export const zonePanels:Record<ZoneId,readonly string[]> = {
  left:['media'], right:['spectrum','performance'], lower:['piano','mixer'],
};
export const clamp = (n:number,a:number,b:number):number => Math.max(a,Math.min(b,n));
const object = (v:unknown):Record<string,unknown> => v && typeof v==='object' && !Array.isArray(v) ? v as Record<string,unknown> : {};

export function readWorkspace(value:unknown,legacy:unknown):WorkspaceState {
  const saved=object(value), zones=object(saved.zones), old=object(legacy);
  const state:WorkspaceState={arrangement:true,zones:{
    left:{open:true,active:'media',size:220},
    right:{open:false,active:'spectrum',size:340},
    lower:{open:false,active:'piano',size:420},
  }};
  if(typeof saved.arrangement==='boolean')state.arrangement=saved.arrangement;
  else if(typeof object(old.arrangement).open==='boolean')state.arrangement=object(old.arrangement).open as boolean;
  for(const id of Object.keys(zonePanels) as ZoneId[]){
    const z=state.zones[id], s=object(zones[id]);
    if(typeof s.open==='boolean')z.open=s.open;
    else if(!Object.keys(saved).length){
      const visible=zonePanels[id].find(p=>object(old[p]).open===true);
      if(visible){z.open=true;z.active=visible;}
      else if(zonePanels[id].some(p=>object(old[p]).open===false))z.open=false;
    }
    if(typeof s.active==='string'&&zonePanels[id].includes(s.active))z.active=s.active;
    if(typeof s.size==='number'&&Number.isFinite(s.size))z.size=clamp(s.size,120,1600);
  }
  return state;
}

// Window/font clamping never overwrites the user's preferred dimensions.
export function workspaceGeometry(state:WorkspaceState,width:number,height:number,scale=1) {
  const z=state.zones, gap=6;
  const sides=(z.left.open?1:0)+(z.right.open?1:0);
  const sideBudget=Math.max(0,width-gap*sides-Math.min(480*scale,width*.55));
  let left=z.left.open?Math.max(150*scale,z.left.size):0;
  let right=z.right.open?Math.max(230*scale,z.right.size):0;
  if(left+right>sideBudget){const ratio=sideBudget/(left+right);left*=ratio;right*=ratio;}
  const lower=z.lower.open?(state.arrangement?clamp(z.lower.size,Math.min(260*scale,height*.5),Math.max(0,height-gap-Math.min(281*scale,height*.5))):height):0;
  return {left,right,lower,leftGap:z.left.open?gap:0,rightGap:z.right.open?gap:0,lowerGap:z.lower.open&&state.arrangement?gap:0};
}
