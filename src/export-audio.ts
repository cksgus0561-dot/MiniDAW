import {uiText, tr} from './i18n';
import {invoke, isTauri} from '@tauri-apps/api/core';
import {save, open as chooseFolder} from '@tauri-apps/plugin-dialog';
import {element} from './dom';
import type {Snapshot,AppError} from './types';
interface ExportFile {path:string;trackId:string|null;startFrame:number;endFrame:number;frames:number}
interface Report {path:string;frames:number;sampleRate:number;seconds:number;elapsedMs:number;realtimeFactor:number;clippedSamples:number;files:ExportFile[]}
interface Status {jobId:string;stage:string;progress:number;frames:number;timelineFrames:number;fileIndex:number;fileCount:number;target:string}
interface Track {trackId:string;name:string;kind:string;mix?:{mute:boolean;solo:boolean}}
interface View {revision:number;path:string|null;document:{projectId:string;name:string;tracks:Track[]}}
interface Selection {start:number|null;end:number|null;trackIds:string[]}
export class AudioExport {
  private dialog=element<HTMLDialogElement>('export-dialog');
  private path=element<HTMLInputElement>('export-path');
  private format=element<HTMLSelectElement>('export-format');
  private mode=element<HTMLSelectElement>('export-mode');
  private extent=element<HTMLSelectElement>('export-range');
  private tracks=element('export-tracks');
  private message=element('export-message');
  private progress=element<HTMLProgressElement>('export-progress');
  private start=element<HTMLButtonElement>('export-start');
  private browse=element<HTMLButtonElement>('export-browse');
  private cancel=element<HTMLButtonElement>('export-cancel');
  private replace=element<HTMLButtonElement>('export-replace');
  private job:string|null=null;
  private working=false;
  private cancelRequested=false;
  private approvedPath:string|null=null;
  private rate=48000;
  private defaultName='Mixdown.wav';
  private view:View|null=null;
  private selection:Selection={start:null,end:null,trackIds:[]};
  private chosen=new Set<string>();
  private lastFile='';
  private lastFolder='';
  private batch=false;
  constructor(private ready:()=>boolean,private getSelection:()=>Selection) {
    element('audio-export').onclick=()=>{if(isTauri()&&this.ready())void this.open();};
    this.browse.onclick=()=>void this.choose();
    this.start.onclick=()=>void this.render(false);
    this.replace.onclick=()=>void this.render(true);
    this.path.oninput=()=>{this.approvedPath=null;this.replace.hidden=true;this.update();};
    this.format.onchange=()=>{this.replace.hidden=true;};
    this.mode.onchange=()=>{this.approvedPath=null;this.replace.hidden=true;this.trackOptions();this.update();};
    this.extent.onchange=()=>this.update();
    this.cancel.onclick=()=>{if(this.job)void this.stop();else this.dialog.close();};
    this.dialog.oncancel=e=>{if(this.job){e.preventDefault();void this.stop();}};
  }
  private error(e:unknown):void {const error=e as AppError;uiText(this.message,[error.message??String(e),error.detail].filter(Boolean).join(' · '));this.message.dataset.error='true';}
  private isChannel():boolean{return this.mode.value==='track'||this.mode.value==='stems';}
  private useRange():boolean{return this.mode.value==='selection'||(this.isChannel()&&this.extent.value==='selection');}
  private range():{start:string;end:string}|null {
    const {start,end}=this.selection;
    if(start===null||end===null||!Number.isFinite(start)||!Number.isFinite(end)||start<0)return null;
    const a=Math.round(start*this.rate),b=Math.round(end*this.rate);
    return Number.isSafeInteger(a)&&Number.isSafeInteger(b)&&b>a?{start:String(a),end:String(b)}:null;
  }
  private update():void {
    const batch=this.mode.value==='stems';
    if(batch!==this.batch){if(batch){this.lastFile=this.path.value;this.path.value=this.lastFolder||this.path.value.replace(/[^/\\]+$/,'');}else{this.lastFolder=this.path.value;this.path.value=this.lastFile;}this.batch=batch;}
    this.tracks.hidden=!this.isChannel();element('export-range-option').hidden=!this.isChannel();element('export-naming').hidden=!batch;
    uiText(element('export-routing'),this.isChannel()
      ?'Track 출력 · 악기 / Insert / Volume / Pan / Automation 포함 · Master Volume / Effects / Automation 제외 · 해당 Track Mute 유지, 다른 Track Solo 무시'
      :'Master Stereo 출력 · 전체 Track 및 Master 처리 포함 · 프로젝트 Mute / Solo 반영');
    const range=this.range();
    uiText(element('export-boundaries'),this.useRange()
      ?range?'선택 구간 · '+(Number(range.start)/this.rate).toFixed(9)+'–'+(Number(range.end)/this.rate).toFixed(9)+' s · '+(Number(range.end)-Number(range.start))+' samples ['+range.start+', '+range.end+')':'Arrangement에서 Range 도구로 시간 구간을 선택한 뒤 Export를 다시 여세요.'
      :batch?'0초부터 프로젝트 끝과 선택 Track의 잔향까지 · 모든 Stem의 시작과 sample 길이를 동일하게 맞춥니다.':'0초부터 프로젝트 끝과 잔향까지');
    this.start.disabled=this.working||!this.path.value.trim()||(this.useRange()&&!range)||(this.isChannel()&&!this.chosen.size);
  }
  private trackOptions():void {
    if(this.mode.value==='track'&&this.chosen.size>1)this.chosen=new Set([[...this.chosen][0]]);
    this.tracks.replaceChildren();
    for(const t of this.view?.document.tracks??[]){
      const label=document.createElement('label'),input=document.createElement('input'),text=document.createElement('span');
      input.type=this.mode.value==='track'?'radio':'checkbox';input.name='export-track';input.value=t.trackId;input.checked=this.chosen.has(t.trackId);
      uiText(text,t.name+' · '+(t.kind==='midi'?'MIDI / Instrument':'Audio')+(t.mix?.mute?' · Mute':''),false);
      input.onchange=()=>{if(input.type==='radio')this.chosen.clear();if(input.checked)this.chosen.add(t.trackId);else this.chosen.delete(t.trackId);this.update();};
      label.append(input,text);this.tracks.append(label);
    }
  }
  private async open():Promise<void> {
    element<HTMLDetailsElement>('file-menu').open=false;
    this.selection=this.getSelection();this.approvedPath=null;uiText(this.message,'');delete this.message.dataset.error;this.progress.value=0;this.replace.hidden=true;
    this.dialog.showModal();this.busy(true);uiText(this.cancel,'닫기');
    try {
      const [p,s]=await Promise.all([invoke<View>('project_snapshot'),invoke<Snapshot>('engine_snapshot')]);
      this.view=p;this.rate=s.output?.sampleRate??48000;this.defaultName=p.document.name.replace(/[<>:"/\\|?*]/g,'_')+' Mixdown.wav';
      this.chosen=new Set(this.selection.trackIds.filter(id=>p.document.tracks.some(t=>t.trackId===id)));
      if(!this.chosen.size&&p.document.tracks.length)this.chosen.add(p.document.tracks[0].trackId);
      if(p.path){const folder=p.path.replace(/[^/\\]+$/,'');this.path.value=this.batch?folder:folder+this.defaultName;}
      uiText(element('export-rate'),this.rate.toLocaleString()+' Hz · Stereo'+(s.output?'':' · 출력 장치 없음: 48 kHz 기준'));
      this.trackOptions();this.busy(false);
    }catch(e){this.busy(false);this.error(e);this.start.disabled=true;}
  }
  private busy(value:boolean):void {
    this.working=value;this.path.disabled=this.format.disabled=this.browse.disabled=this.mode.disabled=this.extent.disabled=value;
    for(const input of this.tracks.querySelectorAll('input'))input.disabled=value;
    this.replace.disabled=value;this.cancel.disabled=false;uiText(this.cancel,value?'취소':'닫기');this.update();
  }
  private async choose():Promise<void> {
    try {
      const path=this.batch?await chooseFolder({title:tr('Stem 저장 폴더'),directory:true,multiple:false,defaultPath:this.path.value||undefined})
        :await save({title:'Export Audio Mixdown',defaultPath:this.path.value||this.defaultName,filters:[{name:'Wave Audio',extensions:['wav']}]});
      if(typeof path==='string'){this.approvedPath=this.batch?null:path;this.path.value=path;this.replace.hidden=true;this.update();}
    }catch(e){this.error(e);}
  }
  private async stop():Promise<void> {
    if(!this.job)return;this.cancelRequested=true;this.cancel.disabled=true;uiText(this.message,'취소 요청 중… 현재 준비 작업이 끝나면 중단합니다.');
    try{await invoke('export_cancel',{jobId:this.job});}catch(e){this.error(e);}
  }
  private async render(overwrite:boolean):Promise<void> {
    if(this.job||this.start.disabled)return;
    const job=crypto.randomUUID();this.job=job;this.cancelRequested=false;this.busy(true);this.replace.hidden=true;this.progress.removeAttribute('value');
    delete this.message.dataset.error;uiText(this.message,'프로젝트와 오디오 준비 중…');let ended=false;
    const poll=async()=>{
      try{const s=await invoke<Status>('export_status');if(!ended&&s.jobId===job){if(this.cancelRequested){await invoke('export_cancel',{jobId:job});this.cancelRequested=false;}if(ended)return;
        if(s.stage==='preparing')this.progress.removeAttribute('value');else this.progress.value=s.progress;
        const stage=tr(s.stage==='preparing'?'프로젝트와 오디오 준비 중…':s.stage==='preroll'?'선택 구간 이전 DSP 준비':s.stage==='tail'?'잔향 렌더링':s.stage==='finalizing'?'파일 마무리':'Offline 렌더링');
        if(!this.cancel.disabled)uiText(this.message,stage+' · '+s.fileIndex+'/'+s.fileCount+' · '+s.target+'\n'+(s.frames/this.rate).toFixed(2)+' s',false);
      }}catch{/* Command promise reports final errors. */}
      if(!ended)setTimeout(()=>void poll(),200);
    };void poll();
    try {
      const p=await invoke<View>('project_snapshot');
      if(p.document.projectId!==this.view?.document.projectId)throw new Error(tr('프로젝트가 변경됐습니다. Export 창을 다시 열어 주세요.'));
      const r=await invoke<Report>('export_audio',{request:{jobId:job,revision:p.revision,path:this.path.value,format:this.format.value,sampleRate:this.rate,overwrite:!this.batch&&(overwrite||this.approvedPath===this.path.value),mode:this.mode.value,trackIds:this.isChannel()?[...this.chosen]:[],range:this.useRange()?this.range():null}});
      ended=true;this.progress.value=1;
      uiText(this.message,'완료 · '+r.seconds.toFixed(3)+' s · '+(r.elapsedMs/1000).toFixed(2)+'초 처리 ('+r.realtimeFactor.toFixed(1)+'×)\n'+r.files.map(f=>f.path).join('\n')+(r.clippedSamples?'\nPCM 범위 초과 '+r.clippedSamples.toLocaleString()+' samples · 출력 레벨을 낮추거나 32-bit float를 사용하세요.':''));
    }catch(e){ended=true;this.progress.value=0;this.error(e);this.replace.hidden=this.batch||(e as AppError).code!=='export_exists';}
    finally{ended=true;this.job=null;this.busy(false);}
  }
}
