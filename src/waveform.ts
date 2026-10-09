import { uiText } from './i18n';
import { invoke } from "@tauri-apps/api/core";
import { element, time } from "./dom";
import type { FileInfo, WaveformView } from "./types";
import { PlayheadDrag } from "./playhead";
import { MusicalClock, type Cycle } from './musical-time';

export class Waveform {
  readonly musical = new MusicalClock();
  snapEnabled = false;
  grid = 'beat';
  private rulerFormat: 'bars' | 'seconds' = 'bars';
  cycle: Cycle | null = null;
  snap(seconds: number): number { return this.snapEnabled ? this.musical.snap(seconds, this.grid) : seconds; }
  editTick(seconds: number): number { return this.musical.editTick(seconds, this.snapEnabled ? this.grid : null); }
  editPosition(seconds: number): string {
    return this.rulerFormat === 'bars' ? this.musical.preciseLabel(this.musical.ticks(seconds)) : `${seconds.toFixed(9)} s`;
  }
  public editing = false;
  get dragging():boolean{return this.drag.dragging;}
  public viewportChanged: (() => void) | null = null;
  get viewport(): {start:number;span:number;width:number;height:number;ruler:number} {return {start:this.start,span:this.span,width:this.width,height:this.height,ruler:parseFloat(getComputedStyle(this.canvas).getPropertyValue('--timeline-height'))*parseFloat(getComputedStyle(document.documentElement).fontSize)};}
  timelineDuration = 0;
  private timelineContentDuration=0;
  timelineProject = '';
  setTimeline(project:string,duration:number):void {
    if(this.timelineProject===project&&this.timelineContentDuration===duration)return;
    this.timelineContentDuration=duration;
    const changed=this.timelineProject!==project;this.timelineProject=project;this.timelineDuration=Math.max(10,duration*1.25);
    if(this.editing&&!this.file&&duration>0)this.file={name:'Audio Timeline',sampleRate:48000,channels:2,frames:Math.round(duration*48000),duration:this.timelineDuration,sanitizedSamples:0};
    if(this.editing&&duration===0){this.file=null;this.clipId=0;this.start=0;this.span=1;element('empty').hidden=false;this.overlay.hidden=true;this.request();return;}
    if(duration>0)element('empty').hidden=true;
    if(this.file){this.file={...this.file,duration:this.timelineDuration};if(changed){this.start=0;this.span=this.timelineDuration;}this.clamp();this.request();}
  }
  private canvas = element<HTMLCanvasElement>("waveform");
  private overlay = element<HTMLDivElement>("playhead");
  private scroll = element<HTMLInputElement>("scroll");
  private file: FileInfo | null = null;
  private audioDuration = 0;
  private clipId = 0;
  private start = 0;
  private span = 1;
  private width = 1;
  private height = 1;
  private density = window.devicePixelRatio || 1;
  private scheduled = false;
  private revision = 0;
  private active = false;
  private pending = false;
  private cache = new Map<string, WaveformView>();
  private peaks: WaveformView | null = null;
  private position = 0;
  private appliedCommand = 0;
  private cacheVersion = 0;
  private deferredPaint = false;
  private drag: PlayheadDrag;
  public drawCount = 0;

  constructor(seek: (seconds: number) => Promise<number | null>, private onError: (error: unknown) => void) {
    // Click and drag share one full-waveform gesture: preview on down/move,
    // one seek on up. The playhead overlay is display-only and never intercepts input.
    this.drag = new PlayheadDrag(this.canvas, x => this.secondsAt(x), seconds => this.paintPosition(seconds), seek,event=>!this.editing || event.clientY-this.canvas.getBoundingClientRect().top<=this.viewport.ruler);
    new ResizeObserver(() => this.request()).observe(this.canvas);
    element('drop-area').addEventListener("wheel", event => {
      if (!this.file || (!event.ctrlKey && !event.shiftKey)) return;
      event.preventDefault();
      const rect = this.canvas.getBoundingClientRect();
      const unit = event.deltaMode === WheelEvent.DOM_DELTA_LINE ? 16 : event.deltaMode === WheelEvent.DOM_DELTA_PAGE ? rect.width : 1;
      if (event.ctrlKey) {
        const ratio = Math.max(0, Math.min(1, (event.clientX - rect.left) / Math.max(1, rect.width)));
        this.zoom(Math.exp(Math.max(-4, Math.min(4, -event.deltaY * unit * 0.003))), ratio);
      } else {
        const previous = this.start;
        this.start += (event.deltaX || event.deltaY) * unit / Math.max(1, rect.width) * this.span;
        this.clamp();
        if (this.start !== previous) this.request();
      }
    }, { passive: false });
    this.scroll.addEventListener("input", () => { this.start = Number(this.scroll.value); this.request(); });
    element<HTMLSelectElement>('ruler-format').addEventListener('change', e => {
      this.rulerFormat = (e.target as HTMLSelectElement).value === 'seconds' ? 'seconds' : 'bars';
      this.request();
      document.dispatchEvent(new Event('ruler-format-changed'));
    });
  }

  setFile(file: FileInfo, id: number): void {
    if (id === this.clipId) return;
    // An edit/undo replaces the engine plan, not the current timeline. A late
    // snapshot of that commit must not cancel a ruler drag started meanwhile.
    if(!this.editing||this.audioDuration!==file.duration)this.cancelDrag();
    const changedGeometry=!this.file||(!this.editing&&this.file.duration!==file.duration)||this.span===1;
    this.audioDuration=file.duration;
    this.file = this.editing ? {...file,duration:this.timelineDuration||file.duration}:file; this.clipId = id;
    if(!this.editing){this.start=0;this.span=file.duration;}else if(this.span===1){this.span=this.file.duration;}this.clamp();
    this.peaks = null; this.cache.clear(); this.cacheVersion = 0;
    element("empty").hidden = true;
    if(!this.editing||changedGeometry)this.request();
  }

  clear(): void {
    if(this.editing&&this.timelineContentDuration>0){this.clipId=0;this.audioDuration=0;this.overlay.hidden=true;element('empty').hidden=true;return;}
    if (!this.file) return;
    this.cancelDrag(); this.file = null; this.clipId = 0; this.start = 0; this.span = 1;
    this.peaks = null; this.cache.clear(); this.cacheVersion = 0; this.pending = false;
    element("empty").hidden = false; this.overlay.hidden = true; this.request();
  }

  zoom(factor: number, anchor = 0.5): void {
    if (!this.file) return;
    const previousStart = this.start, previousSpan = this.span;
    const point = this.start + this.span * anchor;
    this.span = Math.min(this.file.duration, Math.max(Math.min(0.002, this.file.duration), this.span / factor));
    this.start = point - this.span * anchor;
    this.clamp();
    if (this.start !== previousStart || this.span !== previousSpan) this.request();
  }

  updateCache(version: number): void {
    if(this.editing)return; // Clip canvases independently share the asset cache.
    // Defer progressive redraw during a gesture: pointer moves stay preview-only.
    if (this.drag.dragging) return;
    if (this.pending) void this.fetch();
    if (version === this.cacheVersion) {
      if (this.deferredPaint) { this.deferredPaint = false; this.paint(this.peaks); }
      return;
    }
    this.cacheVersion = version;
    this.cache.clear(); this.request();
  }

  fit(): void {
    if (!this.file) return;
    this.start = 0; this.span = this.file.duration; this.request();
  }
  reveal(start: number, end: number): void {
    if (!this.file || (start >= this.start && end <= this.start + this.span)) return;
    this.start = Math.max(0, start - this.span * .05); this.clamp(); this.request();
  }

  private clamp(): void { this.start = Math.max(0, Math.min(this.start, (this.file?.duration ?? 1) - this.span)); }

  private request(): void {
    // Invalidate even before the next rAF so late IPC responses cannot paint an old view.
    this.revision++;
    if (this.scheduled) return;
    this.scheduled = true;
    requestAnimationFrame(() => {
      this.scheduled = false;
      this.width = this.canvas.clientWidth; this.height = this.canvas.clientHeight;
      if (!this.width || !this.height) return; // Closed panel: no Canvas work or zero-width timeline geometry.
      this.scroll.max = String(Math.max(0, (this.file?.duration ?? 0) - this.span));
      this.scroll.step = String(this.span / Math.max(1, this.width));
      this.scroll.value = String(this.start);
      this.scroll.disabled = !this.file || Number(this.scroll.max) <= 0;
      uiText(element("zoom-value"), this.file ? `${(this.file.duration / this.span).toFixed(1)}×` : "1.0×");
      uiText(element("view-range"), `${time(this.start)} — ${time(this.start + this.span)}`);
      this.paint(this.peaks);
      this.viewportChanged?.();
      this.setPosition(this.position, this.appliedCommand);
      this.pending = true;
      void this.fetch();
    });
  }

  private async fetch(): Promise<void> {
    if (this.editing || this.active || !this.pending || !this.file || this.drag.dragging) return;
    this.pending = false; this.active = true;
    const revision = this.revision;
    const args = { clipId: this.clipId, start: this.start, end: this.start + this.span, width: Math.max(1, Math.min(4096, Math.round(this.width))) };
    const key = `${this.cacheVersion}:${JSON.stringify(args)}`;
    try {
      const view = this.cache.get(key) ?? await invoke<WaveformView>("waveform_view", args);
      this.cache.set(key, view);
      if (this.cache.size > 8) this.cache.delete(this.cache.keys().next().value!);
      if (revision === this.revision && view.clipId === this.clipId) {
        this.peaks = view;
        if (this.drag.dragging) this.deferredPaint = true;
        else { this.deferredPaint = false; this.paint(view); }
      }
    } catch (error) {
      if (revision === this.revision) this.onError(error);
    } finally {
      this.active = false;
      if (this.pending) void this.fetch();
    }
  }

  setInteractive(enabled: boolean): void {
    this.drag.setEnabled(enabled);
  }

  cancelDrag(): void { this.drag.cancel(); }

  refreshStyle(): void { this.request(); }

  secondsAt(clientX: number): number {
    const rect = this.canvas.getBoundingClientRect();
    // Pointer capture can deliver positions outside the viewport. Map through
    // the current zoom/scroll first, then clamp against the actual audio file.
    const seconds = this.start + (clientX - rect.left) / Math.max(1, rect.width) * this.span;
    return Math.max(0, Math.min(this.audioDuration, seconds));
  }

  setPosition(seconds: number, appliedCommand: number): number {
    // The existing display frame also notices monitor/DPI changes with an
    // unchanged CSS box. Ordinary frames and dragging never redraw the canvas.
    const density = window.devicePixelRatio || 1;
    if (density !== this.density) { this.density = density; this.request(); }
    this.position = seconds;
    this.appliedCommand = appliedCommand;
    const displayed = this.drag.displayPosition(seconds, appliedCommand);
    this.paintPosition(displayed);
    return displayed;
  }

  private paintPosition(seconds: number): void {
    const x = (seconds - this.start) / this.span * this.width;
    this.overlay.hidden = !this.file || (!this.drag.dragging && (x < -0.01 || x > this.width + 0.01));
    this.overlay.style.transform = `translateX(${x}px)`;
  }

  private paint(view: WaveformView | null): void {
    if (this.drag.dragging) { this.deferredPaint = true; return; }
    const dpr = window.devicePixelRatio || 1;
    this.canvas.width = Math.round(this.width * dpr);
    this.canvas.height = Math.round(this.height * dpr);
    const ctx = this.canvas.getContext("2d");
    if (!ctx) return;
    this.drawCount++;
    ctx.scale(dpr, dpr);
    // Canvas shares the CSS theme and computed font (including Windows DPI /
    // future root UI scale). Read only on waveform redraw, never per playhead.
    const style = getComputedStyle(this.canvas);
    const color = (name: string) => style.getPropertyValue(name).trim();
    const rootSize = parseFloat(getComputedStyle(document.documentElement).fontSize);
    const ruler = parseFloat(color("--timeline-height")) * rootSize; // rem token
    ctx.fillStyle = color("--wave-background"); ctx.fillRect(0, 0, this.width, this.height);
    ctx.font = style.font;
    const labelWidth = ctx.measureText("000:00.000").width + 28;
    const raw = this.span / Math.max(1, this.width / labelWidth);
    const magnitude = 10 ** Math.floor(Math.log10(raw));
    const step = [1, 2, 5, 10].find(n => n * magnitude >= raw)! * magnitude;
    const minor = step / 5;
    if (this.editing) {
      const lines = this.musical.lines(this.start, this.span, this.width, this.grid);
      const grid = document.createElement('canvas'); grid.width = Math.ceil(this.width * dpr); grid.height = 1;
      const g = grid.getContext('2d')!;
      for (const line of lines) {
        const stroke = color(line.bar ? '--grid-major' : '--grid-minor');
        g.fillStyle = stroke; g.fillRect(Math.round(line.x * dpr), 0, line.bar ? 2 : 1, 1);
        if (this.rulerFormat === 'bars') {
          ctx.strokeStyle = stroke; ctx.beginPath(); ctx.moveTo(line.x, ruler * .77); ctx.lineTo(line.x, ruler); ctx.stroke();
        }
      }
      const labels: [number, number][] = [];
      // Keep Bar labels readable first; subdivisions remain grid lines, not long tick labels.
      for (const line of this.rulerFormat === 'bars' ? [...lines.filter(l => l.bar), ...lines.filter(l => l.beat && !l.bar)] : []) {
        const label = line.bar ? line.label.replace(/\.1$/, '') : line.label;
        const left = line.x + 6, right = left + ctx.measureText(label).width;
        if (left >= 0 && right <= this.width && !labels.some(([a, b]) => left < b + 10 && right > a - 10)) {
          ctx.fillStyle = color('--text-secondary'); ctx.fillText(label, left, ruler * .68); labels.push([left, right]);
        }
      }
      const layer = element('clip-layer');
      layer.style.backgroundImage = `url(${grid.toDataURL()})`;
      layer.style.backgroundSize = `${this.width}px 1px`; layer.style.backgroundPosition = '0 0'; layer.style.backgroundRepeat = 'repeat-y';
      const c = this.cycle;
      const x = (tick: string) => (this.musical.seconds(Number(tick)) - this.start) / this.span * this.width;
      const range = element('cycle-range'); range.hidden = !c;
      if (c) {
        range.style.left = `${x(c.startTick)}px`; range.style.width = `${Math.max(1, x(c.endTick) - x(c.startTick))}px`;
        range.classList.toggle('enabled', c.enabled);
      }
      if (this.rulerFormat === 'bars') return;
    }
    // Seconds reuses the original time-based ruler spacing and formatting.
    // Arrangement's musical editing grid and Cycle geometry above stay independent.
    ctx.strokeStyle = color("--grid-minor");
    ctx.beginPath();
    for (let tick = Math.ceil(this.start / minor) * minor; tick <= this.start + this.span; tick += minor) {
      const x = (tick - this.start) / this.span * this.width;
      ctx.moveTo(x, this.editing ? ruler * .85 : ruler); ctx.lineTo(x, this.height);
    }
    ctx.stroke();
    for (let tick = Math.ceil(this.start / step) * step; tick <= this.start + this.span; tick += step) {
      const x = (tick - this.start) / this.span * this.width;
      ctx.strokeStyle = color("--grid-major"); ctx.beginPath(); ctx.moveTo(x, this.editing ? ruler * .77 : ruler); ctx.lineTo(x, this.height); ctx.stroke();
      const label = time(tick, step < 1);
      // Omit labels that would be clipped at the right edge.
      if (x + 6 + ctx.measureText(label).width <= this.width) {
        ctx.fillStyle = color("--text-secondary"); ctx.fillText(label, x + 6, ruler * 0.62);
      }
    }
    if (this.editing) return;
    const channels = this.file?.channels ?? 2;
    const lane = (this.height - ruler) / channels;
    for (let channel = 0; channel < channels; channel++) {
      const center = ruler + lane * (channel + 0.5);
      ctx.strokeStyle = color("--grid-major"); ctx.beginPath(); ctx.moveTo(0, center); ctx.lineTo(this.width, center); ctx.stroke();
      ctx.fillStyle = color("--text-secondary"); ctx.fillText(channels === 1 ? "MONO" : channel === 0 ? "L" : "R", 10, ruler + rootSize * 1.3 + lane * channel);
      if (view) {
        ctx.strokeStyle = color(channel === 0 ? "--wave-left" : "--wave-right");
        ctx.lineWidth = Math.max(1, Math.min(3, this.width / view.channels[channel].length));
        ctx.beginPath();
        view.channels[channel].forEach(([min, max], i) => {
          const seconds = view.start + i / view.channels[channel].length * (view.end - view.start);
          const x = (seconds - this.start) / this.span * this.width;
          if (x < -2 || x > this.width + 2) return;
          const top = center - Math.max(-1, Math.min(1, max)) * lane * 0.41;
          const bottom = center - Math.max(-1, Math.min(1, min)) * lane * 0.41;
          ctx.moveTo(x, top); ctx.lineTo(x, Math.max(top + 0.7, bottom));
        });
        ctx.stroke();
      }
    }
  }
}
