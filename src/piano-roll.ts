import { uiText, uiAttr, tr } from './i18n';
import { globalShortcut } from './keyboard';
import { shortcutCommand } from './shortcuts';
import { NoteInfo } from './note-info';
import { MidiLane } from './midi-lane';
import { MidiKeyboard } from './midi-input';
import { element } from './dom';
import { noteName, type MidiClip, type MidiNote } from './midi';
import { collectParts, noteTick, partsBounds, type PianoPart, type PianoTrack } from './piano-parts';
import type { Waveform } from './waveform';
import type { Panels } from './panels';

interface View { document: { projectId: string; tracks: PianoTrack[] }; revision: number }
interface Drag { pointer: number; kind: 'add' | 'move' | 'left' | 'right'; originTick: number; originPitch: number; original: MidiNote; preview: MidiNote; moved: boolean }
const KEYS = 56, RULER = 28;
const clamp = (n: number, a: number, b: number) => Math.max(a, Math.min(b, n));

// Integer note geometry; pointer moves always refer to the original note, never
// accumulate rounded deltas from an earlier preview. Same helper for both edges.
export function notePreview(note: MidiNote, kind: 'move' | 'left' | 'right', tick: number, pitchDelta: number, length: number): MidiNote {
  let start = Number(note.startTick), end = start + Number(note.lengthTick);
  if (kind === 'move') { start = clamp(tick, 0, length - Number(note.lengthTick)); end = start + Number(note.lengthTick); }
  if (kind === 'left') start = clamp(tick, 0, end - 1);
  if (kind === 'right') end = clamp(tick, start + 1, length);
  return { ...note, startTick: String(start), lengthTick: String(end - start), pitch: kind === 'move' ? clamp(note.pitch + pitchDelta, 0, 127) : note.pitch };
}

export class PianoRoll {
  private root = element('panel-piano');
  private canvas = element<HTMLCanvasElement>('piano-canvas');
  private vscroll = element('piano-vscroll');
  private hscroll = element<HTMLInputElement>('piano-scroll');
  private clip: MidiClip | null = null;
  private view: View | null = null;
  private clipId: string | null = null;
  private clipIds:string[]=[];
  private parts:PianoPart[]=[];
  private partList=element('piano-parts');
  private partListKey='';
  private activeRoute='';
  private selected = new Set<string>();
  private lane: MidiLane;
  private noteInfo: NoteInfo;
  private box: {pointer:number;x:number;y:number;endX:number;endY:number;before:Set<string>} | null = null;
  private tool: 'select' | 'draw' = 'select';
  private drag: Drag | null = null;
  private busy = false;
  private scheduled = 0;
  private viewportKey: string | null = null;
  private viewports = new Map<string, { start: number; span: number; absolute:boolean }>();
  // Single-Part cameras follow that Part (legacy behavior). A multi-Part camera
  // uses project ticks and never follows active-Part switches or document Moves.
  private absolute=false;
  private start = 0;
  private span = 1;
  private position = 0;
  private centerPitch = true;
  private restoredScroll:number|null=null;
  constructor(private panels: Panels, private wave: Waveform, private edit: (r: unknown) => Promise<void>) {
    this.lane = new MidiLane({clip:()=>this.clip,selected:()=>this.selected,select:ids=>{this.selected=ids;this.draw();},x:tick=>this.x(tick),tick:x=>this.tickAt(x),snap:tick=>this.snap(tick),draw:()=>this.draw(),drawing:()=>this.tool==='draw',busy:()=>this.busy,edit:r=>this.commit(r)});
    this.noteInfo=new NoteInfo({clip:()=>this.clip,selected:()=>this.selected,clock:()=>this.wave.musical,busy:()=>this.busy||!!this.drag||!!this.box,edit:r=>this.commit(r)});
    new MidiKeyboard(()=>panels.owns('piano'));
    element('piano-velocity-apply').onclick=()=>void this.notesCommand('midi.velocity',{velocity:Number(element<HTMLInputElement>('piano-velocity').value)});
    element('piano-quantize').onclick=()=>void this.notesCommand('midi.quantize',{grid:this.wave.grid});
    element('piano-transpose-up').onclick=()=>void this.notesCommand('midi.transpose',{semitones:1});
    element('piano-transpose-down').onclick=()=>void this.notesCommand('midi.transpose',{semitones:-1});
    document.addEventListener('project-view' , e => {
      const v = (e as CustomEvent<View>).detail;
      if (this.view?.revision === v.revision && this.view.document.projectId === v.document.projectId) return;
      this.cancel();
      if (this.view?.document.projectId !== v.document.projectId) {
        this.viewports.clear(); this.viewportKey = null; this.clip = null;this.parts=[];this.clipId=null;this.clipIds=[];this.absolute=false;
      }
      this.view = v; this.loadClip();
    });
    document.addEventListener('midi-clip-selected', e => {
      const { clipId, clipIds, open } = (e as CustomEvent<{ clipId: string | null; clipIds?:string[]; open: boolean }>).detail;
      const ids=clipIds??(clipId?[clipId]:[]);
      if (ids.slice().sort().join(':') !== this.clipIds.slice().sort().join(':')) {
        this.cancel();this.clipIds=ids;this.loadClip();
      }
      if (open && this.clip) { panels.setOpen('piano', true); this.canvas.focus(); }
    });
    document.addEventListener('musical-grid-changed', () => this.draw());
    document.addEventListener('workspace-changing', () => this.cancel());
    document.addEventListener('panel-visibility', () => { this.draw(); this.setPosition(this.position); });
    for (const tool of ['select', 'draw'] as const) element(`piano-${tool}`).onclick = () => { this.cancel(); this.tool = tool; this.draw(); this.canvas.focus(); };
    element('piano-delete').onclick = () => void this.remove();
    element('piano-fit').onclick = () => this.fit();
    element('piano-zoom-in').onclick = () => this.zoom(.5, .5);
    element('piano-zoom-out').onclick = () => this.zoom(2, .5);
    this.hscroll.oninput = () => { this.start = Number(this.hscroll.value); this.draw(); };
    this.vscroll.onscroll = () => this.draw();
    new ResizeObserver(() => this.draw()).observe(this.canvas);
    this.canvas.onpointerdown = e => this.down(e);
    this.canvas.onpointermove = e => this.move(e);
    this.canvas.onpointerup = e => void this.up(e);
    this.canvas.onpointercancel = () => this.cancel(); this.canvas.onlostpointercapture = () => this.cancel();
    window.addEventListener('blur', () => this.cancel());
    this.canvas.addEventListener('wheel', e => {
      if (!this.clip || this.drag) return;
      e.preventDefault(); const unit = e.deltaMode === 1 ? 20 : e.deltaMode === 2 ? this.canvas.clientHeight : 1;
      if (e.ctrlKey) this.zoom(Math.exp(clamp(e.deltaY * unit, -1000, 1000) * .002), clamp((e.clientX - this.canvas.getBoundingClientRect().left - KEYS) / this.width(), 0, 1));
      else if (e.shiftKey) { this.start += (e.deltaY || e.deltaX) * unit / this.width() * this.span; this.draw(); }
      else this.vscroll.scrollTop += e.deltaY * unit;
    }, { passive: false });
    globalShortcut(e => {
      if(!this.root.contains(e.target as Node))return;
      const id=shortcutCommand(e);
      if(id==='edit.delete'&&(e.target as HTMLElement).closest('#midi-lane-canvas'))return;
      if(id==='midi.selectAll')return()=>{this.selected=new Set(this.clip?.notes.map(n=>n.noteId));this.draw();};
      if(id==='midi.quantize')return()=>{void this.notesCommand('midi.quantize',{grid:this.wave.grid});};
      if(id==='midi.transposeUp'||id==='midi.transposeDown')return()=>{void this.notesCommand('midi.transpose',{semitones:id==='midi.transposeUp'?1:-1});};
      if(id==='midi.draw'||id==='tool.objectSelection')return()=>{this.cancel();this.tool=id==='midi.draw'?'draw':'select';this.draw();};
      if(id==='edit.delete')return()=>{void this.remove();};
      if(id==='edit.undo'||id==='edit.redo')return()=>{this.cancel();void this.commit({command:id});};
      if(id==='view.zoomIn'||id==='view.zoomOut')return()=>this.zoom(id==='view.zoomIn'?.5:2,.5);
    });
    document.addEventListener('language-changed',()=>this.draw());
    this.draw();
  }
  captureWindowState() {
    return {project:this.view?.document.projectId,clipIds:this.clipIds,clipId:this.clipId,selected:[...this.selected],start:this.start,span:this.span,absolute:this.absolute,viewportKey:this.viewportKey,viewports:[...this.viewports],scroll:this.vscroll.scrollTop,tool:this.tool,lane:this.lane.captureWindowState(),format:element<HTMLSelectElement>('note-time-format').value};
  }
  restoreWindowState(s:ReturnType<PianoRoll['captureWindowState']>):void {
    if(!s||s.project!==this.view?.document.projectId)return;
    this.cancel();this.clipIds=s.clipIds;this.clipId=s.clipId;this.viewports=new Map(s.viewports);this.loadClip();
    this.start=s.start;this.span=s.span;this.absolute=s.absolute;this.viewportKey=s.viewportKey;this.selected=new Set(s.selected);this.tool=s.tool;
    this.centerPitch=false;this.restoredScroll=s.scroll;this.lane.restoreWindowState(s.lane);element<HTMLSelectElement>('note-time-format').value=s.format;this.draw();
  }
  private loadClip(): void {
    const previous=this.parts, absoluteStart=this.origin()+this.start;
    const parts=collectParts(this.view?.document.tracks??[],this.clipIds);
    const clip=parts.find(p=>p.clip.clipId===this.clipId)?.clip??parts[0]?.clip??null,changed=clip?.clipId!==this.clip?.clipId;
    const key=parts.length?`${this.view!.document.projectId}:${parts.map(p=>p.clip.clipId).sort().join(':')}`:null;
    if(key!==this.viewportKey&&this.viewportKey)this.viewports.set(this.viewportKey,{start:this.start,span:this.span,absolute:this.absolute});
    this.parts=parts;this.clip=clip;this.clipId=clip?.clipId??null;
    if(key!==this.viewportKey){
      const overlap=parts.some(p=>previous.some(old=>old.clip.clipId===p.clip.clipId));
      const saved=key?this.viewports.get(key):undefined;
      if(overlap&&this.panels.isOpen('piano')){
        this.absolute=this.absolute||parts.length>1;
        this.start=absoluteStart-this.origin();
      }else if(saved){this.start=saved.start;this.span=saved.span;this.absolute=saved.absolute;}
      else if(clip){this.absolute=parts.length>1;this.fit();this.centerPitch=true;}
    }
    this.selected = new Set([...this.selected].filter(id=>clip?.notes.some(n=>n.noteId===id)));
    const selected = clip?.notes.find(n => this.selected.has(n.noteId));
    if (selected) this.readout(selected);
    else uiText(element('piano-position'), clip ? `Clip ${this.wave.musical.preciseLabel(Number(clip.startTick))}` : 'Bar.Beat.Sixteenth.Tick');
    if(changed){this.selected.clear();this.lane.resetSelection();}
    this.viewportKey = key;
    this.showParts();
    const part=parts.find(p=>p.clip.clipId===this.clipId),route=`${part?.trackId??''}:${this.clipId??''}`;
    if(route!==this.activeRoute){this.activeRoute=route;document.dispatchEvent(new CustomEvent('piano-active-part',{detail:{clipId:this.clipId,trackId:part?.trackId??null}}));}
    this.draw();
  }
  private activate(id:string):void {
    if(this.busy||id===this.clipId||!this.parts.some(p=>p.clip.clipId===id))return;
    this.cancel();this.clipId=id;this.loadClip();
    document.dispatchEvent(new CustomEvent('piano-part-activated',{detail:{clipId:id}}));
    this.canvas.focus();
  }
  private showParts():void {
    const key=JSON.stringify([this.parts.map(p=>[p.trackId,p.trackName,p.color,p.clip.clipId,p.clip.name,p.clip.startTick,p.clip.lengthTick]),this.clipId,this.busy]);
    if(key===this.partListKey)return;this.partListKey=key;this.partList.replaceChildren();
    this.partList.hidden=this.parts.length<2;
    for(const p of this.parts){
      const b=document.createElement('button');b.className='piano-part';b.dataset.partId=p.clip.clipId;b.dataset.trackId=p.trackId;b.style.setProperty('--part-color',p.color);b.setAttribute('aria-pressed',String(p.clip.clipId===this.clipId));b.disabled=this.busy;
      uiText(b,`${p.trackName} / ${p.clip.name}`,false);uiAttr(b,'title',`${p.trackName} / ${p.clip.name} · ${this.wave.musical.preciseLabel(Number(p.clip.startTick))} — ${this.wave.musical.preciseLabel(Number(p.clip.startTick)+Number(p.clip.lengthTick))}`,false);
      b.onclick=()=>this.activate(p.clip.clipId);this.partList.append(b);
    }
  }
  private row(): number { return parseFloat(getComputedStyle(document.documentElement).fontSize) * 1.45; }
  private width(): number { return Math.max(1, this.canvas.clientWidth - KEYS); }
  private clipStart(): number { return Number(this.clip?.startTick ?? 0); }
  private origin():number {return this.absolute?0:this.clipStart();}
  private limits(): [number, number] { return this.absolute?partsBounds(this.parts):[0, Number(this.clip?.lengthTick ?? 1)]; }
  private fit(): void { const [a, b] = this.limits(); this.start = a; this.span = Math.max(1, b - a); this.draw(); }
  private zoom(factor: number, anchor: number): void {
    if (!this.clip || this.drag) return;
    const [, b] = this.limits(), under = this.start + anchor * this.span;
    const minimum = Math.max(1, Math.min(b, 8));
    this.span = clamp(this.span * factor, minimum, Math.max(minimum, b, this.start + this.span));
    this.start = under - anchor * this.span; this.draw();
  }
  private at(e: PointerEvent): { tick: number; pitch: number; x: number; y: number } {
    const b = this.canvas.getBoundingClientRect(), x = e.clientX - b.left, y = e.clientY - b.top;
    return { x, y, tick: this.tickAt(x), pitch: clamp(127 - Math.floor((y - RULER + this.vscroll.scrollTop) / this.row()), 0, 127) };
  }
  private snap(tick: number): number { return this.wave.editTick(this.wave.musical.seconds(Math.max(0, tick))); }
  private tickAt(x: number): number { return this.origin() + this.start + (x - KEYS) / this.width() * this.span; }
  private x(tick: number): number { return KEYS + (tick - this.origin() - this.start) / this.span * this.width(); }
  private rect(n: MidiNote,clip=this.clip!) {
    const tick = noteTick(clip,n), x = this.x(tick);
    return { x, y: RULER + (127 - n.pitch) * this.row() - this.vscroll.scrollTop + 1, w: Math.max(2, this.x(tick + Number(n.lengthTick)) - x), h: this.row() - 2 };
  }
  private hit(x: number, y: number): MidiNote | undefined {
    return this.clip?.notes.slice().reverse().find(n => { const r = this.rect(n); return x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h; });
  }
  private down(e: PointerEvent): void {
    if (e.button !== 0 || !this.clip || this.busy || this.drag) return;
    const at = this.at(e); if (at.x < KEYS || at.y < RULER) return;
    e.preventDefault(); this.canvas.focus();
    const hit = this.hit(at.x, at.y);
    let kind: Drag['kind'] = 'move', note: MidiNote;
    if (hit) {
      note = { ...hit };
      if(e.shiftKey||e.ctrlKey||e.metaKey){if(this.selected.has(hit.noteId))this.selected.delete(hit.noteId);else this.selected.add(hit.noteId);this.readout(hit);this.draw();return;}
      if(!this.selected.has(hit.noteId))this.selected = new Set([hit.noteId]);
      const r = this.rect(hit), handle = Math.min(7, r.w / 3);
      if (at.x - r.x <= handle) kind = 'left'; else if (r.x + r.w - at.x <= handle) kind = 'right';
    } else if (this.tool === 'draw') {
      if (at.tick < this.clipStart() || at.tick >= this.clipStart() + Number(this.clip.lengthTick)) return;
      const start = clamp(this.snap(at.tick) - this.clipStart(), 0, Number(this.clip.lengthTick) - 1);
      note = { noteId: '', startTick: String(start), lengthTick: String(Math.max(1, Math.min(Number(this.clip.lengthTick) - start, Math.round(this.wave.musical.step(this.clipStart() + start, this.wave.grid))))), pitch: at.pitch, velocity:100,releaseVelocity:0,channel:0 };
      kind = 'add'; this.selected.clear();
    } else { const before=e.shiftKey?new Set(this.selected):new Set<string>();this.selected=new Set(before);this.box={pointer:e.pointerId,x:at.x,y:at.y,endX:at.x,endY:at.y,before};this.canvas.setPointerCapture(e.pointerId);this.draw();return; }
    this.drag = { pointer: e.pointerId, kind, originTick: at.tick, originPitch: at.pitch, original: note, preview: { ...note }, moved: false };
    this.canvas.setPointerCapture(e.pointerId); this.draw(); this.readout(note);
  }
  private move(e: PointerEvent): void {
    const at = this.at(e), g = this.drag;
    if(this.box?.pointer===e.pointerId){const box=this.box;box.endX=at.x;box.endY=at.y;const left=Math.min(box.x,at.x),right=Math.max(box.x,at.x),top=Math.min(box.y,at.y),bottom=Math.max(box.y,at.y);this.selected=new Set(box.before);for(const n of this.clip?.notes??[]){const r=this.rect(n);if(r.x<=right&&r.x+r.w>=left&&r.y<=bottom&&r.y+r.h>=top)this.selected.add(n.noteId);}this.draw();return;}
    if (!g) {
      const hit = this.hit(at.x, at.y), r = hit ? this.rect(hit) : null;
      const owner=this.parts.find(p=>p.clip.clipId===this.clipId&&!!hit)??this.parts.find(p=>p.clip.clipId!==this.clipId&&p.clip.notes.some(n=>{const r=this.rect(n,p.clip);return at.x>=r.x&&at.x<=r.x+r.w&&at.y>=r.y&&at.y<=r.y+r.h;}));
      uiAttr(this.canvas,'title',owner?`${owner.trackName} / ${owner.clip.name} · ${tr(owner.clip.clipId===this.clipId?'활성 Part':'참고 Part · 상단에서 활성화 후 편집')}`:'',false);
      this.canvas.style.cursor = r && (at.x - r.x < Math.min(7, r.w / 3) || r.x + r.w - at.x < Math.min(7, r.w / 3)) ? 'ew-resize' : hit ? 'move' : this.tool === 'draw' ? 'crosshair' : 'default';
      if (this.clip) uiText(element('piano-position'), `${this.wave.musical.preciseLabel(Math.max(0, at.tick))} · ${noteName(at.pitch)}`);
      return;
    }
    if (g.pointer !== e.pointerId || !this.clip) return;
    const delta = at.tick - g.originTick, length = Number(this.clip.lengthTick), start = Number(g.original.startTick);
    // Avoid an unsnapped click becoming an accidental edit on pointerup.
    g.moved ||= Math.abs(this.x(at.tick) - this.x(g.originTick)) >= 2 || at.pitch !== g.originPitch;
    if (!g.moved) return;
    if (g.kind === 'add') {
      const end = clamp(this.snap(at.tick) - this.clipStart(), start + 1, length);
      g.preview = { ...g.original, lengthTick: String(end - start) };
    } else {
      const edge = g.kind === 'right' ? start + Number(g.original.lengthTick) : start;
      const target = this.snap(this.clipStart() + edge + delta) - this.clipStart();
      g.preview = notePreview(g.original, g.kind, target, at.pitch - g.originPitch, length);
    }
    this.readout(g.preview); this.draw();
  }
  private readout(n: MidiNote): void {
    uiText(element('piano-position'), `${this.wave.musical.preciseLabel(this.clipStart() + Number(n.startTick))} · ${noteName(n.pitch)} · ${n.lengthTick} ticks · Vel ${n.velocity} · Ch ${n.channel+1}`);
    element<HTMLInputElement>('piano-velocity').value=String(n.velocity);
  }
  private async up(e: PointerEvent): Promise<void> {
    if(this.box?.pointer===e.pointerId){this.move(e);this.box=null;if(this.canvas.hasPointerCapture(e.pointerId))this.canvas.releasePointerCapture(e.pointerId);this.draw();return;}
    const g = this.drag; if (!g || g.pointer !== e.pointerId || !this.clip) return;
    this.move(e); this.drag = null;
    if (this.canvas.hasPointerCapture(e.pointerId)) this.canvas.releasePointerCapture(e.pointerId);
    if (g.kind === 'add' || g.moved) {
      const before = new Set(this.clip.notes.map(n => n.noteId)), n = g.preview;
      await this.commit({ command: g.kind === 'add' ? 'midi.note.add' : 'midi.note.change', clipIds: [this.clip.clipId], noteId: n.noteId || null, targetTick: n.startTick, lengthTick: n.lengthTick, pitch: n.pitch });
      if (g.kind === 'add') this.selected = new Set(this.clip?.notes.filter(n=>!before.has(n.noteId)).map(n=>n.noteId));
      this.readout(n);
    }
    this.draw();
  }
  private cancel(): void { this.lane?.cancel();const box=this.box;this.box=null;if(box&&this.canvas.hasPointerCapture(box.pointer))this.canvas.releasePointerCapture(box.pointer); const g = this.drag; this.drag = null; if (g && this.canvas.hasPointerCapture(g.pointer)) this.canvas.releasePointerCapture(g.pointer); this.draw(); }
  private async commit(r: unknown): Promise<void> { if (this.busy) return; this.busy = true; try { await this.edit(r); } finally { this.busy = false; this.draw(); } }
  private async notesCommand(command:string,extra:Record<string,unknown>):Promise<void>{if(this.clip&&this.selected.size){this.cancel();await this.commit({command,clipIds:[this.clip.clipId],noteIds:[...this.selected],...extra});}}
  private async remove(): Promise<void> {
    if (!this.clip || !this.selected.size) return;
    this.cancel(); await this.commit({ command: 'midi.note.delete', clipIds: [this.clip.clipId], noteIds: [...this.selected] }); this.selected.clear();
  }
  setPosition(seconds: number): void {
    this.position = seconds;
    const line = element('piano-playhead'), x = this.x(this.wave.musical.ticks(seconds));
    line.hidden = !this.panels.isOpen('piano') || !this.clip || x < KEYS || x > this.canvas.clientWidth;
    if (!line.hidden) line.style.transform = `translateX(${x}px)`;
  }
  private draw(): void {
    if (this.scheduled || !this.panels.isOpen('piano')) return;
    this.scheduled = requestAnimationFrame(() => { this.scheduled = 0; if (this.panels.isOpen('piano')) this.paint(); });
  }
  private paint(): void {
    const w = this.canvas.clientWidth, h = this.canvas.clientHeight, dpr = devicePixelRatio || 1;
    if (!w || !h) return;
    // Preserve the portion of the Part being edited, including its on-screen notes.
    // Project ticks are resolved only at the grid, editing and playhead boundaries.
    const [, b] = this.limits(); this.span = Math.max(1, this.span); this.start = Math.max(0, this.start);
    this.hscroll.min = '0'; this.hscroll.max = String(Math.max(0, b - this.span, this.start)); this.hscroll.step = String(this.span / this.width()); this.hscroll.value = String(this.start);
    this.canvas.width = Math.round(w * dpr); this.canvas.height = Math.round(h * dpr);
    const ctx = this.canvas.getContext('2d')!; ctx.scale(dpr, dpr);
    const font = parseFloat(getComputedStyle(document.documentElement).fontSize);
    ctx.font = `${font * .76}px Segoe UI, sans-serif`;
    ctx.fillStyle = '#202123'; ctx.fillRect(0, 0, w, h);
    const active=this.parts.find(p=>p.clip.clipId===this.clipId);
    uiText(element('piano-clip-name'), active?`${active.trackName} / ${active.clip.name}`:'MIDI Clip을 선택하세요',!active);
    this.showParts();
    uiText(element('piano-grid-info'), `Snap ${this.wave.snapEnabled ? 'ON' : 'OFF'} · Grid ${this.wave.grid === 'beat' ? 'Beat' : this.wave.grid === 'bar' ? 'Bar' : '1/' + this.wave.grid}`);
    uiText(element('piano-selected-count'), `${this.selected.size}개 선택`);
    for(const id of ['piano-delete','piano-velocity-apply','piano-quantize','piano-transpose-up','piano-transpose-down'])element<HTMLButtonElement>(id).disabled = !this.selected.size || this.busy;
    this.lane.paint();
    this.noteInfo.render();
    for (const t of ['select','draw']) element('piano-' + t).setAttribute('aria-pressed', String(this.tool === t));
    (this.vscroll.firstElementChild as HTMLElement).style.height = `${128 * this.row() + RULER}px`;
    if(this.restoredScroll!==null){this.vscroll.scrollTop=this.restoredScroll;this.restoredScroll=null;}
    if (this.centerPitch && this.clip) { this.centerPitch = false; this.vscroll.scrollTop = Math.max(0,(127 - (this.clip.notes[0]?.pitch ?? 64)) * this.row() - (h - RULER) / 2); }
    if (!this.clip) { ctx.fillStyle = '#b9bdc4'; ctx.fillText(tr('Arrangement에서 MIDI Clip을 선택하세요.'), 14, 50); this.setPosition(this.position); return; }
    ctx.save(); ctx.beginPath(); ctx.rect(0, RULER, w, h - RULER); ctx.clip();
    for (let pitch = 0; pitch < 128; pitch++) {
      const y = RULER + (127 - pitch) * this.row() - this.vscroll.scrollTop;
      if (y + this.row() < RULER || y > h) continue;
      const black = [1,3,6,8,10].includes(pitch % 12);
      ctx.fillStyle = black ? '#252629' : '#2e3033'; ctx.fillRect(KEYS, y, w - KEYS, this.row() - 1);
      ctx.fillStyle = black ? '#44464c' : '#b6b7b9'; ctx.fillRect(0, y, KEYS - 1, this.row() - 1);
      ctx.fillStyle = black ? '#e1e1e4' : '#222326'; ctx.fillText(noteName(pitch), 7, y + this.row() * .73);
    }
    ctx.restore();
    let labelRight = -1;
    const from = this.wave.musical.seconds(this.origin() + this.start);
    const duration = this.wave.musical.seconds(this.origin() + this.start + this.span) - from;
    for (const line of this.wave.musical.lines(from, duration, this.width(), this.wave.grid)) {
      const x = this.x(this.wave.musical.ticks(from + line.x / this.width() * duration));
      ctx.strokeStyle = line.bar ? '#676970' : line.beat ? '#45474e' : '#36383e'; ctx.beginPath(); ctx.moveTo(x, RULER); ctx.lineTo(x, h); ctx.stroke();
      if (x >= labelRight && (line.bar || line.beat)) { ctx.fillStyle = '#c4c6cc'; ctx.fillText(line.label, x + 3, 18); labelRight = x + ctx.measureText(line.label).width + 15; }
    }
    ctx.save(); ctx.beginPath(); ctx.rect(KEYS, RULER, w - KEYS, h - RULER); ctx.clip();
    // Ghost Parts share project coordinates, but never enter hit-testing,
    // selection, controller lanes, numeric editors, or edit command payloads.
    for(const part of this.parts.filter(p=>p.clip.clipId!==this.clipId)){
      ctx.globalAlpha=.28;
      for(const n of part.clip.notes)this.paintNote(ctx,n,part.clip,part.color,false);
    }
    ctx.globalAlpha=1;
    if(this.parts.length>1){
      const a=this.x(this.clipStart()),b=this.x(this.clipStart()+Number(this.clip.lengthTick));
      ctx.fillStyle='#00000035';ctx.fillRect(KEYS,RULER,Math.max(0,a-KEYS),h-RULER);ctx.fillRect(Math.max(KEYS,b),RULER,Math.max(0,w-b),h-RULER);
      ctx.strokeStyle=active!.color;ctx.setLineDash([3,3]);for(const x of [a,b]){ctx.beginPath();ctx.moveTo(x,RULER);ctx.lineTo(x,h);ctx.stroke();}ctx.setLineDash([]);
    }
    const notes = this.clip.notes.map(n => this.drag?.original.noteId === n.noteId ? this.drag.preview : n);
    if (this.drag?.kind === 'add') notes.push(this.drag.preview);
    for (const n of notes)this.paintNote(ctx,n,this.clip,active!.color,true);
    if(this.box){const b=this.box;ctx.fillStyle='#e8ca6b22';ctx.strokeStyle='#e8ca6b';ctx.fillRect(b.x,b.y,b.endX-b.x,b.endY-b.y);ctx.strokeRect(b.x,b.y,b.endX-b.x,b.endY-b.y);}
    ctx.restore(); this.setPosition(this.position);
  }
  private paintNote(ctx:CanvasRenderingContext2D,n:MidiNote,clip:MidiClip,color:string,active:boolean):void {
    const r=this.rect(n,clip);if(r.x+r.w<KEYS||r.x>this.canvas.clientWidth||r.y+r.h<RULER||r.y>this.canvas.clientHeight)return;
    ctx.fillStyle=color;ctx.fillRect(r.x,r.y,r.w,r.h);
    ctx.strokeStyle=active&&(this.selected.has(n.noteId)||n===this.drag?.preview)?'#ffe191':color;
    ctx.strokeRect(r.x+.5,r.y+.5,Math.max(0,r.w-1),r.h-1);
    if(active&&r.w>38){ctx.fillStyle='#161921';ctx.fillText(noteName(n.pitch),Math.max(KEYS+2,r.x+5),r.y+r.h*.76);}
  }
}
