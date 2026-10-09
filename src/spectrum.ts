import { uiText } from './i18n';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { element } from './dom';
import type { Panels } from './panels';

interface Selection { revision: number; clipIds: string[]; trackIds: string[]; start: number | null; end: number | null }
interface Frame { sequence: number; sampleRate: number; fftSize: number; frequencies: number[]; curves: number[][]; maxima: number[][]; windows: number; duration: number; processingMs: number }
interface Snapshot { enabled: boolean; frame: Frame | null; comparison: Frame | null; sources: string[]; error: string | null; processed: number; droppedBlocks: number; skippedFrames: number; workerMs: number; workerMaxMs: number; trackReadMs:number; ageMs: number | null; selectionProgress: number }
interface Track { trackId:string; name:string; kind:'audio'|'midi' }
const prefKey = 'minidaw.ui.spectrum.v1';
const note = (hz: number) => { const midi = Math.round(69 + 12 * Math.log2(hz / 440)); return ['C', 'C♯', 'D', 'D♯', 'E', 'F', 'F♯', 'G', 'G♯', 'A', 'A♯', 'B'][(midi % 12 + 12) % 12] + (Math.floor(midi / 12) - 1); };
export class SpectrumPanel {
  private canvas = element<HTMLCanvasElement>('spectrum-canvas');
  private mode = element<HTMLSelectElement>('spectrum-mode');
  private channel = element<HTMLSelectElement>('spectrum-channel');
  private fft = element<HTMLSelectElement>('spectrum-fft');
  private smoothing = element<HTMLSelectElement>('spectrum-smoothing');
  private hold = element<HTMLInputElement>('spectrum-hold');
  private source = element<HTMLSelectElement>('spectrum-source');
  private sourceB = element<HTMLSelectElement>('spectrum-source-b');
  private compare = element<HTMLInputElement>('spectrum-compare');
  private tracks: Track[] = [];
  private comparison: Frame | null = null;
  private projectId = '';
  private live: Frame | null = null;
  private selection: Frame | null = null;
  private revision = -1;
  private sequence = 0;
  private timer = 0;
  private pollEpoch = 0;
  private drawing = 0;
  private nextPaintAt = 0;
  private config: Promise<unknown> = Promise.resolve();
  private busy = false;
  private token = 0;
  private hover: number | null = null;
  private drawMs = 0;
  private renderedFrames = 0;
  private measuredAt = performance.now();
  private selectionText = 'Clip 또는 Range를 선택한 뒤 분석하세요.';
  private suspended=false;
  constructor(private panels: Panels, private selected: () => Selection) {
    try {
      const p = JSON.parse(localStorage.getItem(prefKey) ?? '{}');
      if ([2048, 4096, 8192, 16384].includes(p.fft)) this.fft.value = String(p.fft);
      if ([0, 100, 200, 500].includes(p.smoothing)) this.smoothing.value = String(p.smoothing);
      if ([0, 1, 2].includes(p.channel)) this.channel.value = String(p.channel);
      if (typeof p.hold === 'boolean') this.hold.checked = p.hold;
    } catch { /* Defaults remain usable. */ }
    for (const control of [this.channel, this.hold]) control.onchange = () => { this.save(); this.draw(); };
    this.fft.onchange = () => { this.cancel(); this.selection = null; this.selectionText = 'FFT 크기가 바뀌었습니다. 선택 구간을 다시 분석하세요.'; this.save(); this.configure(); };
    this.smoothing.onchange = () => { this.save(); this.configure(); };
    this.mode.onchange = () => { this.configure(); this.status(); };
    this.source.onchange = () => {this.updateSources();this.configure();};
    this.sourceB.onchange = () => this.configure();
    this.compare.onchange = () => {this.updateSources();this.configure();};
    element('spectrum-reset').onclick = () => this.configure();
    element('spectrum-analyze').onclick = () => void this.analyze();
    element('spectrum-cancel').onclick = () => this.cancel();
    document.addEventListener('panel-visibility', e => { if ((e as CustomEvent).detail.id === 'spectrum') { if(panels.isOpen('spectrum'))this.suspended=false; if (!this.open) this.cancel(); this.configure(); } });
    document.addEventListener('visibilitychange', () => this.configure());
    document.addEventListener('project-view', e => {
      const view = (e as CustomEvent<{revision:number;document:{projectId:string;tracks:Track[]}}>).detail;
      const rev = view.revision;
      if(this.projectId!==view.document.projectId){this.source.value='master';this.compare.checked=false;this.projectId=view.document.projectId;}
      this.tracks=view.document.tracks;this.updateSources();
      if (this.revision !== rev) {
        this.revision = rev; this.cancel(); this.selection = null;
        this.selectionText = 'Clip 또는 Range를 선택한 뒤 분석하세요.';
        if (this.open) this.configure();
      }
    });
    new ResizeObserver(() => this.draw()).observe(this.canvas);
    this.canvas.onpointermove = e => { const b = this.canvas.getBoundingClientRect(); this.hover = (e.clientX - b.x - 48) / Math.max(1, b.width - 62); this.draw(); };
    this.canvas.onpointerleave = () => { this.hover = null; this.draw(); };
    this.configure();
  }
  private get open(): boolean { return !this.suspended && this.panels.isOpen('spectrum') && !document.hidden; }
  captureWindowState(){return {project:this.projectId,mode:this.mode.value,channel:this.channel.value,fft:this.fft.value,smoothing:this.smoothing.value,hold:this.hold.checked,source:this.source.value,sourceB:this.sourceB.value,compare:this.compare.checked,selection:this.selection,selectionText:this.selectionText};}
  restoreWindowState(s:ReturnType<SpectrumPanel['captureWindowState']>):void {
    if(!s)return;this.mode.value=s.mode;this.channel.value=s.channel;this.fft.value=s.fft;this.smoothing.value=s.smoothing;this.hold.checked=s.hold;
    if(s.project===this.projectId){this.compare.checked=s.compare;this.updateSources();this.source.value=s.source;this.updateSources();this.sourceB.value=s.sourceB;this.selection=s.selection;this.selectionText=s.selectionText;}
    this.status();this.draw();
  }
  async suspendWindow():Promise<void>{
    this.suspended=true;++this.pollEpoch;clearTimeout(this.timer);cancelAnimationFrame(this.drawing);this.drawing=0;this.cancel();
    await this.config.catch(()=>{});
    if(isTauri()&&this.panels.owns('spectrum'))await invoke('spectrum_configure',{settings:{enabled:false,fftSize:Number(this.fft.value),smoothingMs:Number(this.smoothing.value)},sources:this.sources()});
  }
  private updateSources():void {
    let a=this.source.value,b=this.sourceB.value;
    if(a!=='master'&&!this.tracks.some(t=>t.trackId===a)){a='master';this.compare.checked=false;}
    if(b&&!this.tracks.some(t=>t.trackId===b))this.compare.checked=false;
    if(this.tracks.length<2)this.compare.checked=false;
    if(this.compare.checked&&a==='master')a=this.tracks[0].trackId;
    const option=(id:string,label:string)=>{const o=document.createElement('option');o.value=id;uiText(o, label);return o;};
    this.source.replaceChildren(...(this.compare.checked?[]:[option('master','Master')]),...this.tracks.map(t=>option(t.trackId,`${t.name} · ${t.kind==='midi'?'Synth':'Audio'}`)));
    this.source.value=a;
    const choices=this.tracks.filter(t=>t.trackId!==a);
    this.sourceB.replaceChildren(...choices.map(t=>option(t.trackId,`${t.name} · ${t.kind==='midi'?'Synth':'Audio'}`)));
    this.sourceB.value=choices.some(t=>t.trackId===b)?b:choices[0]?.trackId??'';
    this.status();
  }
  private sources():string[] {return this.source.value==='master'?[]:[this.source.value,...(this.compare.checked?[this.sourceB.value]:[])].filter(Boolean);}
  private labels():string[] {return this.mode.value!=='live'?['선택 구간']:this.sources().length?this.sources().map(id=>this.tracks.find(t=>t.trackId===id)?.name??'Track'):['Master'];}
  sampleRenderFps(): number {
    const now = performance.now(), elapsed = now - this.measuredAt;
    const fps = this.open && elapsed > 0 ? this.renderedFrames * 1000 / elapsed : 0;
    this.renderedFrames = 0; this.measuredAt = now;
    return fps;
  }
  private save(): void { try { localStorage.setItem(prefKey, JSON.stringify({ fft: Number(this.fft.value), smoothing: Number(this.smoothing.value), channel: Number(this.channel.value), hold: this.hold.checked })); } catch { /* Optional UI preference. */ } }
  private configure(): void {
    if(this.suspended||!this.panels.owns('spectrum'))return;
    const epoch = ++this.pollEpoch;
    clearTimeout(this.timer); cancelAnimationFrame(this.drawing); this.drawing = 0;
    this.nextPaintAt = 0;
    this.renderedFrames = 0; this.measuredAt = performance.now();
    this.live = null; this.comparison = null; this.sequence = 0;
    const settings = { enabled: this.open && this.mode.value === 'live', fftSize: Number(this.fft.value), smoothingMs: Number(this.smoothing.value) };
    const sources=this.sources();
    if (isTauri()) this.config = this.config.catch(() => {}).then(() => epoch===this.pollEpoch?invoke('spectrum_configure', { settings, sources }):undefined).catch(e => {if(epoch===this.pollEpoch)this.fail(e);}).then(() => { if (epoch === this.pollEpoch && this.open) this.timer = window.setTimeout(() => void this.poll(epoch), 0); });
    this.status(); this.draw();
  }
  private async poll(epoch: number): Promise<void> {
    if (!this.open || epoch !== this.pollEpoch) return;
    const began = performance.now();
    try {
      const s = await invoke<Snapshot>('spectrum_snapshot', { after: this.sequence });
      if (!this.open || epoch !== this.pollEpoch) return;
      if (s.frame && JSON.stringify(s.sources)===JSON.stringify(this.sources())) { this.live = s.frame; this.comparison=s.comparison;this.sequence = s.frame.sequence; if (this.mode.value === 'live') this.draw(); }
      uiText(element('spectrum-metrics'), `FFT ${s.workerMs.toFixed(2)} ms · Canvas ${this.drawMs.toFixed(2)} ms · 누락 ${s.droppedBlocks} blocks · 생략 ${s.skippedFrames} frames${this.sources().length?` · Track ${s.trackReadMs.toFixed(2)} ms`:''}`);
      if (this.busy) uiText(element('spectrum-status'), `선택 구간 분석 중… ${(s.selectionProgress * 100).toFixed(0)}%`);
      if(s.error)this.fail(s.error);
    } catch (e) { this.fail(e); }
    if (this.open && epoch === this.pollEpoch && (this.mode.value === 'live' || this.busy))
      this.timer = window.setTimeout(() => void this.poll(epoch), Math.max(1, (this.mode.value === 'live' ? 33 : 150) - (performance.now() - began)));
  }
  private status(): void {
    uiText(element('spectrum-status'), this.mode.value === 'live' ? (this.source.value==='master'?'Master 실제 출력':'Track 출력 · Mute/Solo·Volume/Pan 반영 · Master 합산 전')+' · 평균 곡선 / Peak Hold' : this.selectionText);
    const live=this.mode.value==='live';this.source.disabled=!live;this.sourceB.disabled=!live;this.compare.disabled=!live||this.tracks.length<2;
    element('spectrum-source-b-label').hidden=!this.compare.checked;
    const colors=['#8dc1b4','#e7a6cf'];
    element('spectrum-legend').replaceChildren(...this.labels().map((name,i)=>{const span=document.createElement('span');span.style.color=colors[i];uiText(span, `${i?'B':'A'} ━ ${name}`);return span;}));
    this.smoothing.disabled = this.mode.value !== 'live';
    element<HTMLButtonElement>('spectrum-reset').disabled = this.mode.value !== 'live';
  }
  private fail(e: unknown): void { uiText(element('spectrum-status'), (e as { message?: string })?.message ?? String(e)); }
  private cancel(): void {
    this.token++;
    if (this.busy) { void invoke('spectrum_cancel').catch(e => this.fail(e)); this.selectionText = '분석을 취소했습니다.'; }
    this.busy = false; element<HTMLButtonElement>('spectrum-analyze').disabled = false; element('spectrum-cancel').hidden = true;
    this.status();
  }
  private async analyze(): Promise<void> {
    if (this.busy || !isTauri()) return;
    const request = { ...this.selected(), fftSize: Number(this.fft.value) };
    if (request.start === null && !request.clipIds.length) { this.fail({ message: 'Audio Clip 또는 Range를 먼저 선택해 주세요.' }); return; }
    const token = ++this.token;
    this.busy = true; this.selection = null; this.mode.value = 'selection';
    this.selectionText = request.start === null ? `선택 Clip ${request.clipIds.length}개` : `Range ${request.start.toFixed(3)}–${request.end!.toFixed(3)} s`;
    element<HTMLButtonElement>('spectrum-analyze').disabled = true; element('spectrum-cancel').hidden = false;
    this.configure();
    try {
      const result = await invoke<Frame>('spectrum_analyze', { request });
      if (token !== this.token || request.revision !== this.selected().revision) return;
      this.selection = result;
      this.selectionText += ` · ${result.duration.toFixed(3)} s 전체 평균 · ${result.windows} windows · ${(result.processingMs / 1000).toFixed(2)} s 처리`;
      this.status(); this.draw();
    } catch (e) { if (token === this.token) this.fail(e); }
    finally { if (token === this.token) { this.busy = false; element<HTMLButtonElement>('spectrum-analyze').disabled = false; element('spectrum-cancel').hidden = true; } }
  }
  private draw(): void {
    if (!this.open || this.drawing) return;
    this.drawing = requestAnimationFrame(now => {
      this.drawing = 0;
      if (!this.open) return;
      if (this.mode.value !== 'live') { this.paint(); return; }
      // Display the latest cached FFT at up to 60 Hz, independently of analysis/IPC.
      // Advance a deadline to support high-refresh displays without halving 60 Hz
      // rendering due to sub-millisecond rAF timestamp jitter.
      const interval = 1000 / 60;
      if (now + .5 >= this.nextPaintAt) {
        this.paint();
        if (this.nextPaintAt === 0 || now - this.nextPaintAt > interval) this.nextPaintAt = now;
        this.nextPaintAt += interval;
      }
      this.draw();
    });
  }
  private paint(): void {
    const began = performance.now(), w = this.canvas.clientWidth, h = this.canvas.clientHeight;
    if (w < 80 || h < 50) return;
    const dpr = devicePixelRatio || 1;
    this.canvas.width = Math.round(w * dpr); this.canvas.height = Math.round(h * dpr);
    const ctx = this.canvas.getContext('2d')!; ctx.scale(dpr, dpr);
    const font = parseFloat(getComputedStyle(document.documentElement).fontSize);
    ctx.font = `${Math.max(10, font * .72)}px sans-serif`;
    const x0 = 48, y0 = 18, width = Math.max(1, w - 62), height = Math.max(1, h - 48);
    const f = this.mode.value === 'live' ? this.live : this.selection;
    const upper = f ? Math.min(20000, f.sampleRate / 2) : 20000;
    const x = (hz: number) => x0 + Math.log(hz / 20) / Math.log(upper / 20) * width;
    const y = (db: number) => y0 + Math.max(0, Math.min(1, -db / 120)) * height;
    ctx.fillStyle = '#181c1e'; ctx.fillRect(0, 0, w, h);
    ctx.textAlign = 'right'; ctx.textBaseline = 'middle';
    for (let db = 0; db >= -120; db -= 20) { ctx.strokeStyle = '#343a3d'; ctx.beginPath(); ctx.moveTo(x0, y(db)); ctx.lineTo(x0 + width, y(db)); ctx.stroke(); ctx.fillStyle = '#a5aeb1'; ctx.fillText(String(db), x0 - 7, y(db)); }
    ctx.textAlign = 'center'; ctx.textBaseline = 'top'; let labelRight = -1;
    for (const hz of [20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000].filter(hz => hz <= upper)) {
      const px = x(hz), label = hz >= 1000 ? `${hz / 1000}k` : String(hz);
      ctx.strokeStyle = '#303639'; ctx.beginPath(); ctx.moveTo(px, y0); ctx.lineTo(px, y0 + height); ctx.stroke();
      const half = ctx.measureText(label).width / 2;
      if (px - half > labelRight + 5) { ctx.fillStyle = '#a5aeb1'; ctx.fillText(label, px, y0 + height + 8); labelRight = px + half; }
    }
    ctx.fillStyle = '#a5aeb1'; ctx.textAlign = 'left'; ctx.fillText('dBFS', 2, 0);
    const ch = Number(this.channel.value);
    const frames=this.mode.value==='live'&&this.compare.checked?[f,this.comparison]:[f];
    const colors=['#8dc1b4','#e7a6cf'];
    const curve = (frame:Frame,values: number[], color: string, dashed: boolean) => { ctx.strokeStyle = color; ctx.lineWidth = dashed ? 1 : 1.6; ctx.globalAlpha=dashed?.55:1;ctx.setLineDash(dashed ? [4, 3] : []); ctx.beginPath(); values.forEach((db, i) => { const px = x(frame.frequencies[i]), py = y(db); if (i) ctx.lineTo(px, py); else ctx.moveTo(px, py); }); ctx.stroke(); ctx.setLineDash([]);ctx.globalAlpha=1; };
    frames.forEach((frame,i)=>{if(frame){if(this.hold.checked)curve(frame,frame.maxima[ch],colors[i],true);curve(frame,frame.curves[ch],colors[i],false);}});
    if (f && this.hover !== null && this.hover >= 0 && this.hover <= 1) {
      const hz = 20 * (upper / 20) ** this.hover;
      const i = Math.min(f.frequencies.length - 1, Math.round(this.hover * (f.frequencies.length - 1)));
      ctx.strokeStyle = '#e6c77d'; ctx.beginPath(); ctx.moveTo(x(hz), y0); ctx.lineTo(x(hz), y0 + height); ctx.stroke();
      const labels=this.labels();
      const values=frames.map((frame,n)=>{const db=frame?.curves[ch][i];return `${labels[n]}: ${db===undefined?'—':db<=-120?'≤ −120':db.toFixed(1)} dBFS`;});
      uiText(element('spectrum-hover'), `${hz.toFixed(hz < 1000 ? 1 : 0)} Hz · ${note(hz)} · ${values.join(' · ')}`);
    } else uiText(element('spectrum-hover'), '마우스: Hz · 가장 가까운 음이름 · dBFS');
    this.drawMs = performance.now() - began;
    this.canvas.dataset.sequence = String(f?.sequence ?? 0);
    this.canvas.dataset.traces=String(frames.filter(Boolean).length);
    // Count completed Canvas paints only, including demand-driven selection renders.
    this.renderedFrames++;
  }
}
