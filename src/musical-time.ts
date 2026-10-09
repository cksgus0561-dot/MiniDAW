// Shared project musical clock. Audio positions remain seconds; quantization is
// performed in integer PPQ ticks, then converted at the editing boundary.
export interface MusicalTime {
  ticksPerQuarter: number;
  tempoMap: { tick: string; bpm: number }[];
  timeSignatures: { tick: string; numerator: number; denominator: number }[];
}
export interface Cycle { enabled: boolean; startTick: string; endTick: string }
export type Position = { unit: 'seconds'; numerator: string; denominator: number } | { unit: 'ticks'; ticks: string };
export const defaultTime = (): MusicalTime => ({ ticksPerQuarter: 960000, tempoMap: [{ tick: '0', bpm: 120 }], timeSignatures: [{ tick: '0', numerator: 4, denominator: 4 }] });
export class MusicalClock {
  constructor(public time: MusicalTime = defaultTime()) {}
  seconds(tick: number): number {
    let seconds = 0, previous = 0, bpm = this.time.tempoMap[0].bpm;
    for (const p of this.time.tempoMap.slice(1)) {
      if (Number(p.tick) >= tick) break;
      seconds += (Number(p.tick) - previous) * 60 / bpm / this.time.ticksPerQuarter;
      previous = Number(p.tick); bpm = p.bpm;
    }
    return seconds + (tick - previous) * 60 / bpm / this.time.ticksPerQuarter;
  }
  ticks(seconds: number): number {
    let elapsed = 0, previous = 0, bpm = this.time.tempoMap[0].bpm;
    for (const p of this.time.tempoMap.slice(1)) {
      const duration = (Number(p.tick) - previous) * 60 / bpm / this.time.ticksPerQuarter;
      if (elapsed + duration >= seconds) break;
      elapsed += duration; previous = Number(p.tick); bpm = p.bpm;
    }
    return previous + (seconds - elapsed) * bpm * this.time.ticksPerQuarter / 60;
  }
  position(p: Position): number { return p.unit === 'seconds' ? Number(p.numerator) / p.denominator : this.seconds(Number(p.ticks)); }
  signatures() {
    let bar = 1;
    return this.time.timeSignatures.map((s, i, all) => {
      const tick = Number(s.tick), beat = this.time.ticksPerQuarter * 4 / s.denominator;
      const next = Number(all[i + 1]?.tick ?? Infinity), result = { ...s, tick, next, beat, bar, barTicks: beat * s.numerator };
      bar += Math.ceil((next - tick) / result.barTicks);
      return result;
    });
  }
  signature(tick: number) { return this.signatures().find(s => tick < s.next)!; }
  step(tick: number, grid: string): number {
    const s = this.signature(tick);
    return grid === 'bar' ? s.barTicks : grid === 'beat' ? s.beat : this.time.ticksPerQuarter * 4 / Number(grid);
  }
  snap(seconds: number, grid: string): number {
    const tick = Math.max(0, this.ticks(seconds)), s = this.signature(tick), step = this.step(tick, grid);
    return this.seconds(Math.round(Math.min(s.next, s.tick + Math.round((tick - s.tick) / step) * step)));
  }
  // One absolute tick quantization for editing, independent of ruler format.
  editTick(seconds: number, grid: string | null): number {
    return Math.max(0, Math.round(this.ticks(grid === null ? seconds : this.snap(seconds, grid))));
  }
  preciseLabel(tick: number): string {
    tick = Math.max(0, Math.round(tick));
    const s = this.signature(tick), local = tick - s.tick, withinBeat = local % s.beat;
    const sixteenth = this.time.ticksPerQuarter / 4;
    return `${s.bar + Math.floor(local / s.barTicks)}.${1 + Math.floor(local % s.barTicks / s.beat)}.${1 + Math.floor(withinBeat / sixteenth)}.${String(Math.round(withinBeat % sixteenth)).padStart(6, '0')}`;
  }
  label(tick: number): string {
    tick = Math.max(0, Math.round(tick));
    const s = this.signature(tick), local = tick - s.tick;
    return `${s.bar + Math.floor(local / s.barTicks)}.${1 + Math.floor(local % s.barTicks / s.beat)}.${Math.round(local % s.beat)}`;
  }
  parse(label: string): number | null {
    const match = /^(\d+)\.(\d+)(?:\.(\d+))?$/.exec(label.trim());
    if (!match) return null;
    const [, a, b, c = '0'] = match, bar = Number(a), beat = Number(b), sub = Number(c);
    for (const s of this.signatures()) {
      const tick = s.tick + (bar - s.bar) * s.barTicks + (beat - 1) * s.beat + sub;
      if (bar >= s.bar && beat >= 1 && beat <= s.numerator && sub < s.beat && tick < s.next && Number.isSafeInteger(tick)) return tick;
    }
    return null;
  }
  lines(start: number, span: number, width: number, grid: string): { x: number; bar: boolean; beat: boolean; label: string }[] {
    const from = Math.max(0, this.ticks(start)), to = this.ticks(start + span), lines = [];
    for (const s of this.signatures()) {
      const lo = Math.max(from, s.tick), hi = Math.min(to, s.next);
      if (lo > hi) continue;
      let step = Math.min(s.beat, this.step(lo, grid));
      const pixels = (this.seconds(lo + step) - this.seconds(lo)) / span * width;
      if (pixels < 8) step = s.barTicks;
      const barPixels = (this.seconds(lo + step) - this.seconds(lo)) / span * width;
      if (barPixels < 8) step *= Math.ceil(8 / Math.max(.0001, barPixels));
      for (let tick = s.tick + Math.ceil((lo - s.tick) / step) * step, n = 0; tick <= hi && tick < s.next && n < 2000; tick += step, n++) {
        const bar = Math.abs((tick - s.tick) % s.barTicks) < .01;
        lines.push({ x: (this.seconds(tick) - start) / span * width, bar, beat: Math.abs((tick - s.tick) % s.beat) < .01, label: this.label(tick).replace(/\.0$/, '') });
      }
    }
    return lines;
  }
}
