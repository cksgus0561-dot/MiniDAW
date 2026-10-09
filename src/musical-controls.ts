import { uiText } from './i18n';
import { shortcutCommand } from './shortcuts';
import { globalShortcut } from './keyboard';
import { element } from './dom';
import type { Waveform } from './waveform';
import type { Cycle, MusicalTime } from './musical-time';

export class MusicalControls {
  private busy = false;
  private gesture: { pointer: number; x: number; tick: number; before: Cycle; kind: string } | null = null;
  private zone = element('cycle-ruler');
  constructor(private wave: Waveform, private edit: (r: unknown) => Promise<void>, private error: (e: unknown) => void) {
    document.addEventListener('project-view', e => {
      this.cancel();
      const { document: p } = (e as CustomEvent<{ document: { musicalTime: MusicalTime; cycle?: Cycle } }>).detail;
      this.wave.musical.time = p.musicalTime;
      this.wave.cycle = p.cycle ?? { enabled: false, startTick: '0', endTick: String(p.musicalTime.ticksPerQuarter * 4) };
      this.refresh();
    });
    element<HTMLInputElement>('project-bpm').onchange = e => void this.commit({ command: 'project.tempo', bpm: Number((e.target as HTMLInputElement).value) });
    for (const id of ['signature-numerator', 'signature-denominator']) element(id).onchange = () => void this.commit({ command: 'project.signature', numerator: Number(element<HTMLInputElement>('signature-numerator').value), denominator: Number(element<HTMLSelectElement>('signature-denominator').value) });
    element('snap-toggle').onclick = () => { this.wave.snapEnabled = !this.wave.snapEnabled; this.refresh(); };
    element<HTMLSelectElement>('grid-type').onchange = e => { this.wave.grid = (e.target as HTMLSelectElement).value; this.refresh(); };
    globalShortcut(e => {
      const id=shortcutCommand(e);
      if(id==='grid.snap')return()=>element('snap-toggle').click();
      if(id==='transport.cycle')return()=>element('cycle-toggle').click();
    });
    element('cycle-toggle').onclick = () => void this.commitCycle({ ...this.cycle(), enabled: !this.cycle().enabled });
    for (const id of ['cycle-start', 'cycle-end']) element(id).onchange = () => {
      const start = this.wave.musical.parse(element<HTMLInputElement>('cycle-start').value), end = this.wave.musical.parse(element<HTMLInputElement>('cycle-end').value);
      if (start === null || end === null || start >= end) { this.error({ message: 'Loop: 마디.박.tick 형식으로 Start < End를 지정하세요.' }); this.refresh(); return; }
      void this.commitCycle({ ...this.cycle(), startTick: String(start), endTick: String(end) });
    };
    this.zone.onpointerdown = e => {
      if (e.button !== 0 || this.busy) return;
      e.preventDefault(); const tick = this.tick(e.clientX), before = { ...this.cycle() };
      this.gesture = { pointer: e.pointerId, x: e.clientX, tick, before, kind: (e.target as HTMLElement).dataset.locator ?? 'range' };
      this.zone.setPointerCapture(e.pointerId);
    };
    this.zone.onpointermove = e => this.move(e);
    this.zone.onpointerup = e => {
      const g = this.gesture; if (!g || g.pointer !== e.pointerId) return;
      this.move(e); const next = { ...this.cycle() }; this.gesture = null;
      this.zone.releasePointerCapture(e.pointerId);
      if (Math.abs(e.clientX - g.x) < 3) { this.wave.cycle = g.before; this.refresh(); return; }
      this.wave.cycle = g.before;
      void this.commitCycle(next);
    };
    this.zone.onpointercancel = () => this.cancel(); this.zone.onlostpointercapture = () => this.cancel();
    window.addEventListener('blur', () => this.cancel());
    document.addEventListener('workspace-changing', () => this.cancel());
    this.refresh();
  }
  private cycle(): Cycle { return this.wave.cycle ?? { enabled: false, startTick: '0', endTick: String(this.wave.musical.time.ticksPerQuarter * 4) }; }
  private tick(x: number): number {
    const b = this.zone.getBoundingClientRect(), v = this.wave.viewport;
    const seconds = this.wave.snap(Math.max(0, v.start + (x - b.left) / b.width * v.span));
    return Math.max(0, Math.round(this.wave.musical.ticks(seconds)));
  }
  private move(e: PointerEvent): void {
    const g = this.gesture; if (!g || g.pointer !== e.pointerId) return;
    const tick = this.tick(e.clientX), c = { ...g.before };
    if (g.kind === 'start') c.startTick = String(Math.min(tick, Number(c.endTick) - 1));
    else if (g.kind === 'end') c.endTick = String(Math.max(tick, Number(c.startTick) + 1));
    else { c.startTick = String(Math.min(g.tick, tick)); c.endTick = String(Math.max(g.tick + 1, tick)); }
    this.wave.cycle = c; this.wave.refreshStyle();
  }
  private cancel(): void {
    const g = this.gesture; if (!g) return;
    this.gesture = null; this.wave.cycle = g.before;
    if (this.zone.hasPointerCapture(g.pointer)) this.zone.releasePointerCapture(g.pointer);
    this.refresh();
  }
  private async commitCycle(cycle: Cycle): Promise<void> { await this.commit({ command: 'project.cycle', cycle }); }
  private async commit(request: unknown): Promise<void> {
    if (this.busy) return;
    this.busy = true;
    const tempo = (request as {command?:string;bpm?:number}).command === 'project.tempo';
    if (tempo) { element<HTMLInputElement>('project-bpm').disabled=true; element('tempo-prepare').hidden=false; uiText(element('tempo-prepare'), `${(request as {bpm:number}).bpm} BPM · Audio Tempo 준비 중…`); }
    try { await this.edit(request); } catch (e) { this.error(e); }
    finally { if (tempo) { element<HTMLInputElement>('project-bpm').disabled=false; element('tempo-prepare').hidden=true; } this.busy = false; this.refresh(); }
  }
  private refresh(): void {
    const m = this.wave.musical.time, c = this.cycle();
    element<HTMLInputElement>('project-bpm').value = String(m.tempoMap[0].bpm);
    element<HTMLInputElement>('signature-numerator').value = String(m.timeSignatures[0].numerator);
    element<HTMLSelectElement>('signature-denominator').value = String(m.timeSignatures[0].denominator);
    element('snap-toggle').setAttribute('aria-pressed', String(this.wave.snapEnabled));
    element('cycle-toggle').setAttribute('aria-pressed', String(c.enabled));
    element<HTMLInputElement>('cycle-start').value = this.wave.musical.label(Number(c.startTick));
    element<HTMLInputElement>('cycle-end').value = this.wave.musical.label(Number(c.endTick));
    this.wave.refreshStyle();
    document.dispatchEvent(new CustomEvent('musical-grid-changed'));
  }
}
