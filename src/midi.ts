export interface MidiNote { noteId: string; startTick: string; lengthTick: string; pitch: number; velocity: number; releaseVelocity: number; channel: number }
export type MidiControlData = { kind: 'cc'; controller: number; value: number } | { kind: 'pitchBend'; value: number };
export interface MidiControl { eventId: string; tick: string; channel: number; data: MidiControlData }
export interface MidiClip { kind: 'midi'; clipId: string; name: string; startTick: string; lengthTick: string; contentOffsetTick?: string; notes: MidiNote[]; controls: MidiControl[] }
export const noteName = (pitch: number): string => `${['C','C♯','D','D♯','E','F','F♯','G','G♯','A','A♯','B'][pitch % 12]}${Math.floor(pitch / 12) - 2}`;

// Piano Roll retains its Part-relative editing API; persisted content stays unchanged on resize.
export function midiClipView(clip: MidiClip): MidiClip {
  const offset = Number(clip.contentOffsetTick ?? 0);
  if (!offset) return clip;
  return { ...clip, notes: clip.notes.map(n => ({ ...n, startTick: String(Number(n.startTick) - offset) })), controls: clip.controls.map(e => ({ ...e, tick: String(Number(e.tick) - offset) })), contentOffsetTick: '0' };
}
