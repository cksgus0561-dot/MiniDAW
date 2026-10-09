import type { MusicalClock } from './musical-time';
export type NoteTimeField = 'start' | 'end' | 'length';

function parts(text: string): number[] | null {
  if (!/^\d+\.\d+\.\d+\.\d+$/.test(text.trim())) return null;
  const p=text.trim().split('.').map(Number);
  return p.every(Number.isSafeInteger) ? p : null;
}
// Start/End: project position, 1-based Bar/Beat/Sixteenth, 0-based Tick.
// Length: 0-based counts in the signature at the reference note's start.
export function parseNoteTime(clock: MusicalClock, text: string, field: NoteTimeField, start: number, ticks: boolean): number | null {
  if (ticks) {if (!/^\d+$/.test(text.trim())) return null;const v=Number(text);return Number.isSafeInteger(v) && (field!=='length'||v>0) ? v : null;}
  const p=parts(text);if(!p)return null;
  const [bar,beat,sixteenth,tick]=p, unit=clock.time.ticksPerQuarter/4;
  if(field==='length') {
    const s=clock.signature(start), sub=sixteenth*unit+tick;
    const value=bar*s.barTicks+beat*s.beat+sub;
    return beat<s.numerator && tick<unit && sub<s.beat && Number.isSafeInteger(value) && value>0 ? value : null;
  }
  for(const s of clock.signatures()) {
    const sub=(sixteenth-1)*unit+tick;
    const value=s.tick+(bar-s.bar)*s.barTicks+(beat-1)*s.beat+sub;
    if(bar>=s.bar&&beat>=1&&beat<=s.numerator&&sixteenth>=1&&tick<unit&&sub<s.beat&&value>=s.tick&&value<s.next&&Number.isSafeInteger(value))return value;
  }
  return null;
}
export function formatNoteTime(clock: MusicalClock, value: number, field: NoteTimeField, start: number, ticks: boolean): string {
  if(ticks)return String(value);
  if(field!=='length')return clock.preciseLabel(value);
  const s=clock.signature(start),unit=clock.time.ticksPerQuarter/4,withinBar=value%s.barTicks,withinBeat=withinBar%s.beat;
  return `${Math.floor(value/s.barTicks)}.${Math.floor(withinBar/s.beat)}.${Math.floor(withinBeat/unit)}.${String(withinBeat%unit).padStart(6,'0')}`;
}
