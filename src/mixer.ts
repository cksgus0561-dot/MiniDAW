import { uiAttr, uiText, uiAppend } from './i18n';
import { automatedValue, automationChannel, type AutomationDocument } from './automation';
import { element } from './dom';
import { numericInput } from './numeric-input';
import { EffectsEditor, type Insert } from './effects';
import type { Panels } from './panels';
import type { Snapshot } from './types';
import { neutralMix, type TrackInfo } from './track-controls';

type Track = TrackInfo & { instrument?: string; inserts?: Insert[] };
interface View { document: Omit<AutomationDocument,'tracks'> & { tracks: Track[] } }
interface Strip { inserts:HTMLButtonElement; root:HTMLElement; name:HTMLElement; kind:HTMLElement; volume:HTMLInputElement; value:HTMLInputElement; pan?:HTMLInputElement; panValue?:HTMLInputElement; mute?:HTMLButtonElement; solo?:HTMLButtonElement }
interface Change { key:string; project:string; request:Record<string,unknown> }
const panText = (value:number) => value === 0 ? 'C' : `${value < 0 ? 'L' : 'R'} ${Math.round(Math.abs(value)*100)}`;

// Controls edit the same project document as Arrangement; no duplicate mixer model.
export class Mixer {
  hasPendingChanges():boolean{return this.pending.size>0||this.sending!==null;}
  private effects: EffectsEditor;
  private view: View | null = null;
  private position=0; private playing=false;
  private strips = new Map<string,Strip>();
  private master: Strip;
  private pending = new Map<string,Change>();
  private sending: Change | null = null;
  private timer = 0;
  private meterBars: HTMLElement[] = [];
  private meterValues: HTMLElement[] = [];
  constructor(private panels:Panels, private edit:(r:Record<string,unknown>)=>Promise<void>, private ready:()=>boolean) {
    this.effects = new EffectsEditor(edit,ready);
    this.master = this.strip('master');
    element('mixer-master').append(this.master.root);
    document.addEventListener('project-view', e => {
      const view=(e as CustomEvent<View>).detail;
      if (this.view?.document.projectId !== view.document.projectId) this.pending.clear();
      this.view=view;
      for(const [key,c] of this.pending) if(c.request.trackIds && !view.document.tracks.some(t=>t.trackId===(c.request.trackIds as string[])[0])) this.pending.delete(key);
      if(this.panels.isOpen('mixer')) this.render();
    });
    document.addEventListener('panel-visibility', e => {
      if((e as CustomEvent).detail.id==='mixer' && this.panels.isOpen('mixer')) this.render();
    });
  }
  private enqueue(id:string,field:string,value:number|boolean,group?:string):void {
    if(!this.view)return;
    const key=`${id}:${field}`;
    this.pending.set(key,{key,project:this.view.document.projectId,request:{command:id==='master'?'master.volume':'track.mix',...(id==='master'?{}:{trackIds:[id]}),[field]:value,historyGroup:group??null}});
    this.schedule();
  }
  private schedule():void { if(!this.timer&&!this.sending)this.timer=window.setTimeout(()=>void this.flush(),40); }
  private async flush():Promise<void> {
    this.timer=0;
    if(!this.pending.size)return;
    if(!this.ready()){this.schedule();return;}
    const change=this.pending.values().next().value!;
    this.pending.delete(change.key);this.sending=change;
    try { if(change.project===this.view?.document.projectId)await this.edit(change.request); }
    finally {this.sending=null;if(this.panels.isOpen('mixer'))this.render();this.schedule();}
  }
  private range(input:HTMLInputElement,id:string,field:string,preview:()=>void,scale=1):void {
    let group:string|null=null;
    const start=()=>{group??=crypto.randomUUID();};
    input.addEventListener('pointerdown',start);
    input.addEventListener('keydown',e=>{if(['ArrowUp','ArrowDown','ArrowLeft','ArrowRight','Home','End','PageUp','PageDown'].includes(e.key))start();});
    input.oninput=()=>{start();preview();this.enqueue(id,field,Number(input.value)*scale,group!);};
    input.onchange=()=>{preview();this.enqueue(id,field,Number(input.value)*scale,group??undefined);group=null;};
    input.addEventListener('blur',()=>{group=null;});
    // Native ranges own pointer capture/keyboard behavior; no Arrangement handlers.
  }
  private strip(id:string):Strip {
    const root=document.createElement('section');root.className='mixer-channel';root.dataset.trackId=id;
    const name=document.createElement('strong');name.className='mixer-name';
    const kind=document.createElement('span');kind.className='mixer-kind';
    const volume=document.createElement('input');volume.type='range';volume.min='-96';volume.max='12';volume.step='.1';volume.className='mixer-fader';uiAttr(volume, 'aria-label', 'Volume');
    const value=document.createElement('input');value.type='number';value.min='-96';value.max='12';value.step='.1';value.className='mixer-volume';uiAttr(value, 'aria-label', 'Volume dB');
    const controls=document.createElement('div');controls.className='mixer-switches';
    const panRow=document.createElement('label');panRow.className='mixer-pan-row';
    const faders=document.createElement('div');faders.className='mixer-faders';
    const scale=document.createElement('div');scale.className='mixer-scale';scale.setAttribute('aria-hidden','true');
    for(const [label,db]of [['+12',12],['0',0],['−12',-12],['−24',-24],['−48',-48],['−96',-96]] as const){const tick=document.createElement('span');uiText(tick, label);tick.style.top=`${(12-db)/108*100}%`;scale.append(tick);}
    faders.append(volume,scale);
    const units=document.createElement('span');units.className='mixer-units';uiText(units, 'dB');
    const inserts=document.createElement('button');inserts.className='mixer-inserts';uiText(inserts, 'Inserts 0');inserts.onclick=()=>this.effects.open(id);
    const strip:Strip={root,name,kind,volume,value,inserts};
    if(id!=='master') {
      const pan=document.createElement('input');pan.type='range';pan.min='-100';pan.max='100';pan.step='1';pan.className='mixer-pan';uiAttr(pan, 'aria-label', 'Pan');uiAttr(pan, 'title', 'Stereo Balance · 중앙 0 dB');
      const panValue=document.createElement('input');panValue.type='number';panValue.min='-100';panValue.max='100';panValue.step='1';panValue.className='mixer-pan-value';uiAttr(panValue, 'aria-label', 'Pan value');uiAttr(panValue, 'title', 'Pan · −100 L / 0 C / +100 R');strip.pan=pan;strip.panValue=panValue;
      uiAppend(panRow, 'Pan ', panValue, pan);
      this.range(pan,id,'pan',()=>{panValue.value=pan.value;pan.setAttribute('aria-valuetext',panText(Number(pan.value)/100));},.01);
      numericInput(panValue,v=>{pan.value=String(v);pan.setAttribute('aria-valuetext',panText(v/100));this.enqueue(id,'pan',v/100);});
      for(const [field,label]of [['mute','M'],['solo','S']] as const){const b=document.createElement('button');b.className=`mixer-${field}`;uiText(b, label);uiAttr(b, 'title', field==='mute'?'Mute':'Solo');b.onclick=()=>this.enqueue(id,field,b.getAttribute('aria-pressed')!=='true');strip[field]=b;controls.append(b);}
    } else {
      root.classList.add('master-channel');uiText(name, 'Master');uiText(kind, 'Stereo Out');
      uiText(panRow, 'L / R · dBFS');uiText(controls, '');
      const meters=document.createElement('div');meters.className='master-meters';uiAttr(meters, 'aria-label', 'Master 출력 sample peak');
      for(const channel of ['L','R']){
        const lane=document.createElement('div');lane.className='master-meter';lane.setAttribute('role','meter');uiAttr(lane, 'aria-label', `Master ${channel} dBFS`);lane.setAttribute('aria-valuemin','-60');lane.setAttribute('aria-valuemax','0');
        const bar=document.createElement('div');bar.className='master-meter-fill';lane.append(bar);meters.append(lane);this.meterBars.push(lane);
        const reading=document.createElement('output');reading.className='master-peak';reading.dataset.channel=channel;uiText(reading, `${channel} −∞`);this.meterValues.push(reading);
      }
      faders.append(meters);units.replaceChildren(...this.meterValues);uiAttr(units, 'title', '출력 sample peak · dBFS · 빠른 attack / 24 dB/s fall');
      uiAttr(value, 'title', '−96 dB: −∞ (무음)');
    }
    for(const flag of ['read','write','lane'] as const){const b=document.createElement('button');uiText(b, flag==='read'?'R':flag==='write'?'W':'A');b.className=`mixer-automation-${flag}`;uiAttr(b, 'title', flag==='lane'?'Automation Lane 열기/닫기':flag==='read'?'Automation Read':'Automation Write · Auto-Latch');b.onclick=()=>{if(flag==='lane'){this.panels.setOpen('arrangement',true);document.dispatchEvent(new CustomEvent('automation-toggle',{detail:id}));}else if(this.view)void this.edit({command:'automation.channel',trackIds:id==='master'?[]:[id],[flag]:!(automationChannel(this.view.document,id)?.[flag]??flag==='read')});};controls.append(b);}
    this.range(volume,id,'volumeDb',()=>{value.value=volume.value;});
    numericInput(value,v=>{volume.value=String(v);this.enqueue(id,'volumeDb',v);});
    root.append(name,kind,inserts,panRow,controls,faders,value,units);
    return strip;
  }
  private displayed(id:string,field:string,fallback:number|boolean):number|boolean {
    const key=`${id}:${field}`, pending=this.pending.get(key);
    const change=pending??(this.sending?.key===key&&this.sending.project===this.view?.document.projectId?this.sending:null);
    return change?change.request[field] as number|boolean:typeof fallback==='number'&&this.view?automatedValue(this.view.document,id,{name:field},this.position,fallback,this.playing):fallback;
  }
  private render():void {
    if(!this.view)return;
    const tracks=this.view.document.tracks,anySolo=tracks.some(t=>t.mix?.solo);
    const container=element('mixer-tracks');
    for(const [id,s]of this.strips)if(!tracks.some(t=>t.trackId===id)){s.root.remove();this.strips.delete(id);}
    let previous:Element|null=null;
    tracks.forEach((track,index)=>{
      let s=this.strips.get(track.trackId);if(!s){s=this.strip(track.trackId);this.strips.set(track.trackId,s);}
      // Reorder only when needed, preserving focus and active fader capture.
      const expected=previous?previous.nextElementSibling:container.firstElementChild;
      if(expected!==s.root)container.insertBefore(s.root,expected);previous=s.root;
      uiText(s.name, track.name, false);uiAttr(s.name, 'title', track.name, false);uiAttr(s.root, 'aria-label', `${track.name} Mixer Channel`);
      uiText(s.kind, `${index+1} · ${track.kind==='audio'?'Audio':track.instrument==='basicSynth'?'MIDI · Synth':track.instrument==='external'?'MIDI · Plugin':'MIDI'}`);
      this.inserts(s,track.inserts??[]);
      const mix=track.mix??neutralMix();
      for(const field of ['mute','solo'] as const){s[field]!.setAttribute('aria-pressed',String(this.displayed(track.trackId,field,mix[field])));uiAttr(s[field]!, 'aria-label', `${track.name} ${field}`);}
      s.root.classList.toggle('solo-excluded',anySolo&&!mix.solo);
      this.pan(s,Number(this.displayed(track.trackId,'pan',mix.pan)));
      this.volume(s,Number(this.displayed(track.trackId,'volumeDb',mix.volumeDb)));
    });
    for(const [id,s]of [...this.strips.entries(),['master',this.master] as const]){const c=automationChannel(this.view.document,id);for(const flag of ['read','write'] as const)s.root.querySelector(`.mixer-automation-${flag}`)?.setAttribute('aria-pressed',String(c?.[flag]??flag==='read'));}
    this.inserts(this.master,this.view.document.master?.inserts??[]);
    this.volume(this.master,Number(this.displayed('master','volumeDb',this.view.document.master?.volumeDb??0)));
  }
  private inserts(s:Strip,effects:Insert[]):void {uiText(s.inserts, `Inserts ${effects.filter(e=>e.enabled).length}/${effects.length}`);s.inserts.dataset.active=String(effects.some(e=>e.enabled));}
  private pan(s:Strip,pan:number):void {s.pan!.value=String(Math.round(pan*100));if(document.activeElement!==s.panValue)s.panValue!.value=String(Math.round(pan*100));s.pan!.setAttribute('aria-valuetext',panText(pan));}
  private volume(s:Strip,db:number):void {s.volume.value=String(db);if(document.activeElement!==s.value)s.value.value=db.toFixed(1);s.volume.setAttribute('aria-valuetext',db<=-96&&s===this.master?'−∞':`${db.toFixed(1)} dB`);}
  meter(snapshot:Snapshot):void {
    if(!this.panels.isOpen('mixer')||document.hidden)return;
    this.position=snapshot.position;this.playing=snapshot.transport.state==='playing';
    if(this.view){for(const t of this.view.document.tracks){const s=this.strips.get(t.trackId);if(!s)continue;const mix=t.mix??neutralMix();this.volume(s,Number(this.displayed(t.trackId,'volumeDb',mix.volumeDb)));this.pan(s,Number(this.displayed(t.trackId,'pan',mix.pan)));}this.volume(this.master,Number(this.displayed('master','volumeDb',this.view.document.master?.volumeDb??0)));}
    this.effects.meters(snapshot.effects??[],this.position,this.playing);
    const peaks=snapshot.outputError?[-120,-120]:snapshot.master.peakDb;
    peaks.forEach((db,ch)=>{const lane=this.meterBars[ch];(lane.firstElementChild as HTMLElement).style.clipPath=`inset(${100-Math.max(0,Math.min(1,(db+60)/60))*100}% 0 0)`;lane.setAttribute('aria-valuenow',Math.max(-60,db).toFixed(1));lane.classList.toggle('at-ceiling',db>=-.05);uiText(this.meterValues[ch], `${ch?'R':'L'} ${db<=-100?'−∞':db.toFixed(1)}`);});
  }
}
