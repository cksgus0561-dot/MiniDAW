import { uiOption, tr } from './i18n';
import { noteName, type MidiNote, type Spelling } from './midi';

const natural = (n:number) => [0,2,4,5,7,9,11].includes((n%12+12)%12);
export function keyOffset(pitch:number,key:number):number {
  return (key>0?[5,0,7,2,9,4,11]:[11,4,9,2,7,0,5]).slice(0,Math.abs(key)).includes((pitch%12+12)%12)?Math.sign(key):0;
}
export function inferSpelling(pitch:number,key:number):Spelling {
  if(natural(pitch))return {naturalPitch:pitch,accidental:keyOffset(pitch,key)?'natural':'key'};
  const base=pitch+(key<0?1:-1),offset=pitch-base;
  return {naturalPitch:base,accidental:keyOffset(base,key)===offset?'key':offset>0?'sharp':'flat'};
}
export function soundingPitch(s:Spelling,key:number):number {
  return s.naturalPitch+({key:keyOffset(s.naturalPitch,key),sharp:1,flat:-1,natural:0}[s.accidental]);
}
export function writtenNote(n:MidiNote,key:number):MidiNote {
  if(!key)return n;
  const spelling:Spelling=natural(n.pitch)?{naturalPitch:n.pitch,accidental:'key'}:inferSpelling(n.pitch,key);
  return {...n,spelling,pitch:soundingPitch(spelling,key)};
}
export function movedNote(n:MidiNote,pitch:number,key:number):MidiNote {
  return n.pitch===pitch?n:{...n,pitch,spelling:n.spelling||key?inferSpelling(pitch,key):undefined};
}
export function noteMark(n:MidiNote):{label:string;color:string|null;source:string} {
  const s=n.spelling;if(!s)return {label:noteName(n.pitch),color:null,source:''};
  const offset=n.pitch-s.naturalPitch,explicit=s.accidental!=='key';
  const letter=['C','','D','','E','F','','G','','A','','B'][(s.naturalPitch%12+12)%12];
  const symbol=offset>0?'♯':offset<0?'♭':explicit?'♮':'';
  return {label:`${letter}${symbol}${Math.floor(s.naturalPitch/12)-2}`,color:explicit?'#ffb86b':offset?'#5edbd3':null,source:explicit?tr('임시표'):offset?tr('조표'):''};
}
export function keyLabel(key:number):string {
  const count=key===0?'♮':`${key>0?'♯':'♭'}${Math.abs(key)}`;
  const names=['C♭ / A♭m','G♭ / E♭m','D♭ / B♭m','A♭ / Fm','E♭ / Cm','B♭ / Gm','F / Dm','C / Am','G / Em','D / Bm','A / F♯m','E / C♯m','B / G♯m','F♯ / D♯m','C♯ / A♯m'];
  return `${count} · ${names[key+7]}`;
}
export function keyOptions(select:HTMLSelectElement):void {
  for(let k=-7;k<=7;k++)select.append(uiOption(keyLabel(k),String(k)));
}
