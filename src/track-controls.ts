import { uiAttr, uiAppend, uiText } from './i18n';
import { numericInput } from './numeric-input';
export interface TrackMix { mute: boolean; solo: boolean; volumeDb: number; pan: number }
export interface TrackInfo { trackId: string; name: string; kind: 'audio' | 'midi'; mix?: TrackMix }
export const neutralMix = (): TrackMix => ({mute:false,solo:false,volumeDb:0,pan:0});

// One lightweight Track inspector popup; no new panel or docking state.
export class TrackControls {
  private popup = document.createElement('div');
  private title = document.createElement('strong');
  private volume = document.createElement('input');
  private pan = document.createElement('input');
  private panValue = document.createElement('input');
  private up = document.createElement('button');
  private down = document.createElement('button');
  private remove = document.createElement('button');
  private transfer = document.createElement('button');
  private tracks: TrackInfo[] = [];
  private current: string | null = null;
  constructor(private edit:(request:Record<string,unknown>)=>Promise<void>, private canTransfer:(id:string)=>boolean, private moveClips:(id:string)=>void) {
    this.popup.id='track-settings';this.popup.className='track-settings';this.popup.popover='auto';
    this.volume.type='number';this.volume.min='-96';this.volume.max='12';this.volume.step='.1';this.volume.id='track-volume';
    this.panValue.type='number';this.panValue.min='-100';this.panValue.max='100';this.panValue.step='1';this.panValue.id='track-pan-value';uiAttr(this.panValue, 'aria-label', 'Pan value');uiAttr(this.panValue, 'title', 'Pan · −100 L / 0 C / +100 R');
    this.pan.type='range';this.pan.min='-100';this.pan.max='100';this.pan.step='1';this.pan.id='track-pan';
    const volLabel=document.createElement('label');uiAppend(volLabel, 'Volume ', this.volume, ' dB');
    const panLabel=document.createElement('label');uiAppend(panLabel, 'Pan ', this.pan, this.panValue);
    uiAttr(this.pan, 'title', 'Stereo Balance · 중앙 0 dB · L/R');
    uiText(this.up, '↑ Track 위로');this.up.id='track-up';uiText(this.down, '↓ Track 아래로');this.down.id='track-down';
    uiText(this.remove, 'Track 삭제');this.remove.id='track-delete';uiAttr(this.remove, 'title', '이 Track과 Clip 삭제 · Undo로 복원 가능');
    uiText(this.transfer, '선택 Clip을 이 Track으로');this.transfer.id='track-transfer';uiAttr(this.transfer, 'title', '같은 종류의 Track으로 이동 · 시간 위치 그대로 유지');
    this.popup.append(this.title,volLabel,panLabel,this.up,this.down,this.transfer,this.remove);document.body.append(this.popup);
    numericInput(this.volume,v=>void this.commit({volumeDb:v}));
    numericInput(this.panValue,v=>{this.pan.value=String(v);void this.commit({pan:v/100});});
    this.pan.oninput=()=>this.panText(Number(this.pan.value));this.pan.onchange=()=>void this.commit({pan:Number(this.pan.value)/100});
    this.up.onclick=()=>void this.action('track.move',{direction:-1});this.down.onclick=()=>void this.action('track.move',{direction:1});
    this.remove.onclick=()=>{this.popup.hidePopover();void this.action('track.delete');};
    this.transfer.onclick=()=>{if(this.current){this.popup.hidePopover();this.moveClips(this.current);}};
    document.addEventListener('workspace-changing',()=>this.popup.hidePopover());
  }
  private panText(v:number):void {if(document.activeElement!==this.panValue)this.panValue.value=String(v);this.pan.setAttribute('aria-valuetext',v===0?'C':`${v<0?'L':'R'} ${Math.abs(v)}`);}
  private async commit(fields:Record<string,unknown>):Promise<void>{await this.action('track.mix',fields);}
  private async action(command:string,fields:Record<string,unknown>={}):Promise<void>{if(this.current)await this.edit({command,trackIds:[this.current],...fields});this.refresh();}
  update(tracks:TrackInfo[]):void{this.tracks=tracks;this.refresh();}
  refresh():void{
    const index=this.tracks.findIndex(t=>t.trackId===this.current),track=this.tracks[index];
    if(!track){this.popup.hidePopover();return;}
    const m=track.mix??neutralMix();uiText(this.title, track.name, false);
    if(document.activeElement!==this.volume)this.volume.value=String(m.volumeDb);
    if(document.activeElement!==this.pan)this.pan.value=String(Math.round(m.pan*100));this.panText(Number(this.pan.value));
    this.up.disabled=index===0;this.down.disabled=index===this.tracks.length-1;this.transfer.disabled=!this.canTransfer(track.trackId);
  }
  header(track:TrackInfo,anySolo:boolean):HTMLElement{
    const root=document.createElement('div');root.className='track-controls';const m=track.mix??neutralMix();
    for(const [field,label]of [['mute','M'],['solo','S']] as const){
      const b=document.createElement('button');uiText(b, label);b.className=`track-${field}`;b.setAttribute('aria-pressed',String(m[field]));uiAttr(b, 'aria-label', `${track.name} ${field}`);
      uiAttr(b, 'title', field==='mute'?(anySolo&&!m.solo?'Solo된 다른 Track 때문에 무음':'Mute'):'Solo · 여러 Track 동시 선택 가능');
      b.onclick=()=>void this.edit({command:'track.mix',trackIds:[track.trackId],[field]:!m[field]});root.append(b);
    }
    const auto=document.createElement('button');uiText(auto, 'A');auto.className='track-automation';uiAttr(auto, 'title', 'Automation Lane 열기/닫기');auto.onclick=()=>document.dispatchEvent(new CustomEvent('automation-toggle',{detail:track.trackId}));root.append(auto);
    const more=document.createElement('button');uiText(more, '⋯');more.className='track-settings-open';uiAttr(more, 'aria-label', `${track.name} Track 설정`);uiAttr(more, 'title', `Volume ${m.volumeDb} dB · Pan ${Math.round(m.pan*100)} · 순서/삭제/Clip 이동`);
    more.onclick=()=>{this.current=track.trackId;this.refresh();this.popup.showPopover();const r=more.getBoundingClientRect();this.popup.style.left=`${Math.max(4,Math.min(r.left,innerWidth-this.popup.offsetWidth-4))}px`;this.popup.style.top=`${Math.max(4,Math.min(r.bottom+4,innerHeight-this.popup.offsetHeight-4))}px`;};root.append(more);return root;
  }
}
