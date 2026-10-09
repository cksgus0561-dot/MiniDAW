import { uiAttr, uiText } from './i18n';
import {choosePlugin,pluginControls,type PluginSelection} from './plugins';
import { numericInput } from './numeric-input';
import { automatedValue, type AutomationDocument } from './automation';
export interface Insert {effectId:string;enabled:boolean;kind:string;[key:string]:unknown}
interface View {document:AutomationDocument}
const names:Record<string,string>={eq:'Parametric EQ',compressor:'Compressor',limiter:'Limiter',reverb:'Reverb',delay:'Delay'};

export class EffectsEditor {
  private root=document.createElement('div');
  private title=document.createElement('strong');
  private list=document.createElement('div');
  private form=document.createElement('div');
  private status=document.createElement('output');
  private kind=document.createElement('select');
  private add=document.createElement('button');
  private view:View|null=null;
  private channel='master';
  private selected='';
  private formKey='';
  private fields:{input:HTMLInputElement|HTMLSelectElement;get:(e:Insert)=>string;name:string;scale:number}[]=[];
  private queue:Promise<void>=Promise.resolve();
  constructor(private edit:(r:Record<string,unknown>)=>Promise<void>,private ready:()=>boolean){
    this.root.id='effect-editor';this.root.popover='auto';uiAttr(this.root, 'aria-label', 'Insert Effects');
    const heading=document.createElement('header');const close=document.createElement('button');uiText(close, '×');close.id='effect-close';uiAttr(close, 'aria-label', 'Effects 닫기');close.onclick=()=>this.root.hidePopover();heading.append(this.title,close);
    const toolbar=document.createElement('div');toolbar.className='effect-toolbar';this.kind.id='effect-kind';uiAttr(this.kind, 'aria-label', '추가할 Effect');
    for(const [id,label]of Object.entries(names)){const o=document.createElement('option');o.value=id;uiText(o, label);this.kind.append(o);}
    this.add.id='effect-add';uiText(this.add, '+ Insert');this.add.onclick=()=>this.send(()=>({command:'effect.add',effectKind:this.kind.value}));toolbar.append(this.kind,this.add);const external=document.createElement('button');external.id='effect-external-add';uiText(external, '+ VST3 / CLAP');external.onclick=()=>{this.root.hidePopover();choosePlugin(this.channel,'effect');};toolbar.append(external);
    const body=document.createElement('div');body.className='effect-body';this.list.id='effect-list';this.form.id='effect-parameters';body.append(this.list,this.form);
    const note=document.createElement('p');note.className='effect-note';uiText(note, '위 → 아래 순서 · Volume/Pan 뒤 Insert · Limiter Ceiling은 해당 슬롯 출력 기준이며 마지막 배치를 권장합니다.');
    this.status.id='effect-reduction';uiText(this.status, '');this.root.append(heading,toolbar,body,this.status,note);document.body.append(this.root);
    document.addEventListener('project-view',event=>{const v=(event as CustomEvent<View>).detail;if(this.view?.document.projectId!==v.document.projectId){this.root.hidePopover();this.selected='';}this.view=v;if(this.channel!=='master'&&!v.document.tracks.some(t=>t.trackId===this.channel))this.root.hidePopover();if(this.root.matches(':popover-open'))this.render();});
    document.addEventListener('workspace-changing',()=>this.root.hidePopover());
  }
  open(channel:string):void{this.channel=channel;this.selected='';this.formKey='';this.render();this.root.showPopover();}
  private chain():Insert[]{return this.channel==='master'?this.view?.document.master?.inserts??[]:this.view?.document.tracks.find(t=>t.trackId===this.channel)?.inserts??[];}
  private send(make:()=>Record<string,unknown>|null):void{
    const project=this.view?.document.projectId,channel=this.channel;
    this.queue=this.queue.catch(()=>{}).then(async()=>{
      while(!this.ready())await new Promise(r=>setTimeout(r,40));
      if(project!==this.view?.document.projectId||channel!==this.channel)return;
      const request=make();if(request){await this.edit({...request,trackIds:channel==='master'?[]:[channel]});if(request.command==='effect.add')this.selected=this.chain().at(-1)?.effectId??'';}
      if(this.root.matches(':popover-open'))this.render();
    });
  }
  private updateEffect(id:string,change:(e:Insert)=>void,name?:string,value?:number):void{this.send(()=>{const original=this.chain().find(e=>e.effectId===id);if(!original)return null;const effect=structuredClone(original);change(effect);return {command:'effect.set',effectId:id,effect,...(name?{parameter:{effectId:id,name},value}:{})};});}
  private render():void{
    const chain=this.chain();uiText(this.title, `${this.channel==='master'?'Master':this.view?.document.tracks.find(t=>t.trackId===this.channel)?.name??'Track'} · Inserts`);
    if(!chain.some(e=>e.effectId===this.selected))this.selected=chain.at(-1)?.effectId??'';
    this.add.disabled=chain.length>=8;this.list.replaceChildren();
    chain.forEach((effect,index)=>{
      const row=document.createElement('div');row.className='effect-slot';row.dataset.effectId=effect.effectId;row.classList.toggle('selected',this.selected===effect.effectId);
      const power=document.createElement('button');power.className='effect-bypass';uiText(power, effect.enabled?'ON':'Bypass');power.setAttribute('aria-pressed',String(effect.enabled));uiAttr(power, 'aria-label', `${effect.kind==='external'?(effect.plugin as PluginSelection).descriptor.name:names[effect.kind]} 활성화`);power.onclick=()=>{const bypass=power.getAttribute('aria-pressed')==='true';this.updateEffect(effect.effectId,e=>{e.enabled=!bypass;},'bypass',bypass?1:0);};
      const select=document.createElement('button');select.className='effect-select';uiText(select, `${index+1}. ${effect.kind==='external'?(effect.plugin as PluginSelection).descriptor.name:names[effect.kind]}`);select.onclick=()=>{this.selected=effect.effectId;this.render();};
      const actions=document.createElement('div');actions.className='effect-slot-actions';
      for(const [label,command,direction]of [['↑','effect.move',-1],['↓','effect.move',1],['×','effect.remove',0]] as const){const b=document.createElement('button');uiText(b, label);b.className=direction===-1?'effect-up':direction===1?'effect-down':'effect-remove';uiAttr(b, 'aria-label', direction===0?'Effect 삭제':direction===-1?'Effect 위로':'Effect 아래로');b.disabled=direction===-1?index===0:direction===1?index===chain.length-1:false;b.onclick=()=>this.send(()=>({command,effectId:effect.effectId,...(direction?{direction}:{})}));actions.append(b);}
      row.append(power,select,actions);this.list.append(row);
    });
    const effect=chain.find(e=>e.effectId===this.selected);
    if(!effect){this.form.replaceChildren();uiText(this.form, 'Effect를 추가하세요.');this.fields=[];this.formKey='';uiText(this.status, '');return;}
    if(this.formKey!==effect.effectId){this.formKey=effect.effectId;this.controls(effect);}
    for(const {input,get}of this.fields){if(document.activeElement!==input)input.value=get(effect);}
    const delay=this.form.querySelector('output');if(delay&&effect.kind==='delay'){const bpm=this.view?.document.musicalTime.tempoMap[0].bpm??120;const seconds=effect.syncBeats?Number(effect.syncBeats)*60/bpm:Number(effect.timeMs)/1000;uiText(delay, `${bpm} BPM · ${(seconds*1000).toFixed(1)} ms`);}
    this.status.hidden=!['compressor','limiter'].includes(effect.kind);if(!effect.enabled)uiText(this.status, 'Bypass · GR 0.0 dB');
  }
  private controls(e:Insert):void{
    this.form.replaceChildren();this.fields=[];
    const heading=document.createElement('h3');uiText(heading, e.kind==='external'?(e.plugin as PluginSelection).descriptor.name:names[e.kind]);this.form.append(heading);
    const field=(label:string,key:string,min:number,max:number,step:number,scale=1,band?:number)=>{
      const wrapper=document.createElement('label');uiText(wrapper, label);const input=document.createElement('input');input.type='number';input.min=String(min);input.max=String(max);input.step=String(step);input.dataset.param=band===undefined?key:`band-${band}-${key}`;uiAttr(input, 'aria-label', label);
      const get=(x:Insert)=>String(Number(band===undefined?x[key]:(x.bands as Record<string,number>[])[band][key])*scale);
      input.value=get(e);numericInput(input,v=>{const value=v/scale;this.updateEffect(e.effectId,x=>{if(band===undefined)x[key]=value;else(x.bands as Record<string,number>[])[band][key]=value;},band===undefined?key:`band${band}.${key}`,value);});
      wrapper.append(input);this.form.append(wrapper);this.fields.push({input,get,name:band===undefined?key:`band${band}.${key}`,scale});
    };
    switch(e.kind){
      case 'external':this.form.append(pluginControls(structuredClone(e.plugin as PluginSelection),e.effectId,(id,value)=>this.updateEffect(e.effectId,x=>{const p=(x.plugin as PluginSelection).parameters.find(p=>p.id===id);if(p)p.value=value;},'plugin.'+id,value)));break;
      case 'eq':for(let band=0;band<3;band++){const h=document.createElement('h4');uiText(h, `Band ${band+1}`);this.form.append(h);field('Frequency · Hz','frequency',20,20000,1,1,band);field('Gain · dB','gainDb',-24,24,.1,1,band);field('Q','q',.1,20,.01,1,band);}break;
      case 'compressor':field('Threshold · dB','thresholdDb',-60,0,.1);field('Ratio','ratio',1,20,.1);field('Attack · ms','attackMs',.1,200,.1);field('Release · ms','releaseMs',10,2000,1);field('Makeup · dB','makeupDb',-12,24,.1);break;
      case 'limiter':field('Output Ceiling · dBFS','ceilingDb',-24,0,.1);field('Input Gain · dB','inputDb',-24,24,.1);break;
      case 'reverb':field('Decay · s','decay',.2,8,.1);field('Wet · %','wet',0,100,1,100);break;
      case 'delay':{
        field('Time · ms','timeMs',1,2000,1);field('Feedback · %','feedback',0,85,1,100);field('Wet · %','wet',0,100,1,100);
        const label=document.createElement('label');uiText(label, 'Tempo Sync');const select=document.createElement('select');select.dataset.param='syncBeats';uiAttr(select, 'aria-label', 'Tempo Sync');
        for(const [value,text]of [['','Off'],['0.125','1/32'],['0.25','1/16'],['0.5','1/8'],['1','1/4'],['2','1/2'],['4','1/1']]){const o=document.createElement('option');o.value=value;uiText(o, text);select.append(o);}
        select.onchange=()=>{const v=select.value?Number(select.value):null;this.updateEffect(e.effectId,x=>{x.syncBeats=v;},'syncBeats',v??0);};label.append(select);this.form.append(label,document.createElement('output'));this.fields.push({input:select,name:'syncBeats',scale:1,get:x=>x.syncBeats===null?'':String(x.syncBeats)});break;
      }
    }
  }
  meters(readings:{channelId:string;effectId:string;reductionDb:number}[],seconds=0,playing=false):void{
    if(!this.root.matches(':popover-open')||!this.view)return;
    const enabled=(e:Insert)=>automatedValue(this.view!.document,this.channel,{effectId:e.effectId,name:'bypass'},seconds,e.enabled?0:1,playing)<.5;
    for(const effect of this.chain()){const row=Array.from(this.list.children).find(n=>(n as HTMLElement).dataset.effectId===effect.effectId);const power=row?.querySelector('.effect-bypass');if(power){const on=enabled(effect);uiText(power, on?'ON':'Bypass');power.setAttribute('aria-pressed',String(on));}}
    const e=this.chain().find(e=>e.effectId===this.selected);if(!e)return;
    for(const f of this.fields){if(document.activeElement!==f.input){const base=Number(f.get(e))/f.scale;const value=automatedValue(this.view.document,this.channel,{effectId:e.effectId,name:f.name},seconds,base,playing)*f.scale;f.input.value=f.name==='syncBeats'&&value===0?'':String(Math.round(value*1000)/1000);}}
    const r=readings.find(r=>r.channelId===this.channel&&r.effectId===this.selected);uiText(this.status, enabled(e)?`Gain Reduction · ${(r?.reductionDb??0).toFixed(1)} dB`:'Bypass · GR 0.0 dB');
  }
}
