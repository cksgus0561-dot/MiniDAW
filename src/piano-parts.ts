import { midiClipView, type MidiClip, type MidiNote } from './midi';
export interface PianoTrack { trackId:string; name:string; clips:({kind:'audio';clipId:string}|MidiClip)[] }
export interface PianoPart { trackId:string; trackName:string; color:string; clip:MidiClip }
// Match Arrangement's first four Track colors; additional Tracks retain distinct hues.
export function partColor(index:number):string {
  return ['#80aaa0','#8c9fb6','#ad9e82','#a796b1'][index] ?? `hsl(${Math.round(index*137.508)%360} 35% 67%)`;
}
export function collectParts(tracks:PianoTrack[],ids:readonly string[]):PianoPart[] {
  const selected=new Set(ids);
  return tracks.flatMap((t,index)=>t.clips.flatMap(c=>c.kind==='midi'&&selected.has(c.clipId)?[{trackId:t.trackId,trackName:t.name,color:partColor(index),clip:midiClipView(c)}]:[]));
}
export function noteTick(clip:MidiClip,note:MidiNote):number { return Number(clip.startTick)+Number(note.startTick); }
export function partsBounds(parts:readonly PianoPart[]):[number,number] {
  if(!parts.length)return [0,1];
  return [Math.min(...parts.map(p=>Number(p.clip.startTick))),Math.max(...parts.map(p=>Number(p.clip.startTick)+Number(p.clip.lengthTick)))];
}
