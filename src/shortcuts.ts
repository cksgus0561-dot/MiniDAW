import { keyboardBlocked } from './keyboard';
import { tr, uiText, uiAttr } from './i18n';

// Stable command IDs and the existing MiniDAW/Cubase defaults. Empty bindings
// are deliberate; adding a command to this list never invents a default key.
export const editCommands = [
  ['tool.objectSelection', '개체 선택', '1'], ['tool.rangeSelection', '범위 선택', '2'], ['tool.split', '자르기', '3'], ['tool.glue', 'Glue', '4'],
  ['edit.undo', '실행 취소', 'Ctrl+Z'], ['edit.redo', '다시 실행', 'Ctrl+Shift+Z'],
  ['edit.cut', '잘라내기', 'Ctrl+X'], ['edit.copy', '복사', 'Ctrl+C'], ['edit.paste', '붙여넣기', 'Ctrl+V'], ['edit.duplicate', '복제', 'Ctrl+D'], ['edit.delete', '삭제', 'Delete'],
  ['audio.glue', 'Glue', ''], ['audio.dissolvePart', 'Audio Part 해제', ''], ['audio.consolidate', 'Bounce Selection…', ''],
  ['audio.splitAtCursor', '커서에서 분할', 'Alt+X'], ['audio.splitRange', '범위 분할', 'Shift+X'],
  ['audio.muteEvents', 'Clip 음소거', 'Shift+M'], ['audio.unmuteEvents', 'Clip 음소거 해제', 'Shift+U'],
  ['audio.crossfade', 'Crossfade', 'X'], ['audio.normalize', 'Peak Normalize…', ''], ['audio.gain', 'Clip Gain', ''], ['audio.fade', 'Fade 설정', '']
] as const;
const extra = [
  ['midi.computerToggle','컴퓨터 키보드 MIDI 연주',''], ['midi.computerOctaveDown','컴퓨터 건반 옥타브 내리기',''], ['midi.computerOctaveUp','컴퓨터 건반 옥타브 올리기',''],
  ['transport.toggle','재생 / 일시정지','Space'], ['transport.play','재생','Enter'], ['transport.stop','정지 · 처음으로','Num 0'],
  ['transport.start','처음 위치로 이동','Num .'], ['transport.left','왼쪽 로케이터로 이동','Num 1'], ['transport.right','오른쪽 로케이터로 이동','Num 2'],
  ['transport.cycle','Cycle 켜기/끄기','Num /'], ['grid.snap','Snap 켜기/끄기','J'],
  ['project.new','새 프로젝트','Ctrl+N'], ['project.open','프로젝트 열기…','Ctrl+Shift+O'],
  ['project.save','저장','Ctrl+S'], ['project.saveAs','다른 이름으로 저장…','Ctrl+Shift+S'], ['file.importAudio','오디오 가져오기','Ctrl+O'],
  ['view.zoomIn','수평 확대','='], ['view.zoomOut','수평 축소','-'],
  ['midi.selectAll','모든 노트 선택','Ctrl+A'], ['midi.quantize','Quantize','Q'],
  ['midi.transposeUp','반음 올리기','ArrowUp'], ['midi.transposeDown','반음 내리기','ArrowDown'], ['midi.draw','노트 그리기','8'],
  ['view.media','미디어 패널',''], ['view.arrangement','Arrangement 패널',''], ['view.piano','Piano Roll 패널',''],
  ['view.spectrum','Spectrum 패널',''], ['view.performance','Performance 패널',''], ['view.mixer','Mixer 패널','']
] as const;
export const shortcutCommands = [...editCommands, ...extra];
export type ShortcutId = typeof shortcutCommands[number][0];
export type EditCommandId = typeof editCommands[number][0];
const key = 'minidaw.ui.shortcuts.v1';
const defaults = new Map<ShortcutId, string[]>(shortcutCommands.map(([id,,binding]) => [id, binding ? [binding] : []]));
defaults.set('edit.delete', ['Delete','Backspace']);
defaults.set('view.zoomIn', ['=','Shift+=']);
let overrides: Partial<Record<ShortcutId, string[]>> = {};
try {
  const data = JSON.parse(localStorage.getItem(key) ?? '{}');
  if (data.version === 1 && data.bindings && typeof data.bindings === 'object') {
    for (const [id] of shortcutCommands) {
      const values = data.bindings[id];
      if (Array.isArray(values) && values.every(v => typeof v === 'string' && validBinding(v))) overrides[id] = [...new Set(values)];
    }
  }
} catch { /* A damaged preference does not disable the default commands. */ }

export function bindingsFor(id: ShortcutId): readonly string[] { return overrides[id] ?? defaults.get(id)!; }
export function reloadShortcuts(raw:string|null):void {
  try{const data=JSON.parse(raw??'{}');overrides={};if(data.version===1)for(const[id]of shortcutCommands){const a=data.bindings?.[id];if(Array.isArray(a)&&a.every(v=>typeof v==='string'&&validBinding(v)))overrides[id]=[...new Set(a)];}refreshShortcutLabels();document.dispatchEvent(new Event('shortcuts-changed'));}catch{/* Keep valid in-session bindings. */}
}
export function defaultBindings(id: ShortcutId): readonly string[] { return defaults.get(id)!; }
export function shortcutLabel(id: ShortcutId): string { return bindingsFor(id).join(' / '); }
export function commandName(id: ShortcutId): string { return tr(shortcutCommands.find(c => c[0] === id)![1]); }
export function validBinding(binding: string): boolean {
  return /^(Ctrl\+)?(Alt\+)?(Shift\+)?([A-Z0-9]|[=\-\[\];',./\\`]|Space|Enter|Delete|Backspace|Home|End|PageUp|PageDown|ArrowUp|ArrowDown|ArrowLeft|ArrowRight|F(?:[1-9]|1[0-9]|2[0-4])|Num [0-9./*+\-])$/.test(binding);
}
export function eventBinding(e: KeyboardEvent): string | null {
  if (e.metaKey || e.isComposing || e.keyCode === 229) return null;
  let base = e.key;
  if (/^Key[A-Z]$/.test(e.code)) base = e.code.slice(3);
  else if (/^Digit[0-9]$/.test(e.code)) base = e.code.slice(5);
  else if (e.code.startsWith('Numpad')) {
    const n = e.code.slice(6), operators: Record<string,string> = {Decimal:'.',Comma:'.',Divide:'/',Multiply:'*',Add:'+',Subtract:'-'};
    base = n === 'Enter' ? 'Enter' : `Num ${operators[n] ?? n}`;
  } else {
    const physical: Record<string,string> = {Space:'Space',Equal:'=',Minus:'-',BracketLeft:'[',BracketRight:']',Semicolon:';',Quote:"'",Comma:',',Period:'.',Slash:'/',Backslash:'\\',Backquote:'`'};
    base = physical[e.code] ?? (base.length === 1 ? base.toUpperCase() : base);
  }
  const result = `${e.ctrlKey ? 'Ctrl+' : ''}${e.altKey ? 'Alt+' : ''}${e.shiftKey ? 'Shift+' : ''}${base}`;
  return validBinding(result) ? result : null;
}
export function conflicts(binding: string, except?: ShortcutId): ShortcutId[] {
  return shortcutCommands.flatMap(([id]) => id !== except && bindingsFor(id).includes(binding) ? [id] : []);
}
export function shortcutCommand(e: KeyboardEvent): ShortcutId | undefined {
  if (keyboardBlocked(e)) return;
  const binding = eventBinding(e); if (!binding) return;
  // Damaged/external preferences containing duplicates are never ambiguous.
  const ids = conflicts(binding); return ids.length === 1 ? ids[0] : undefined;
}
function save(): void {
  // Apply in session even if storage is unavailable, and tell the settings UI.
  document.dispatchEvent(new Event('shortcuts-changed'));
  localStorage.setItem(key, JSON.stringify({version:1, bindings:overrides}));
}
export function assignShortcut(id: ShortcutId, binding: string, replace = false): void {
  if (!validBinding(binding)) throw new Error(tr('사용할 수 없는 단축키입니다.'));
  const used = conflicts(binding, id);
  if (used.length && !replace) throw new Error(used.map(commandName).join(', '));
  for (const other of used) overrides[other] = bindingsFor(other).filter(b => b !== binding);
  overrides[id] = [binding]; save();
}
export function clearShortcut(id: ShortcutId): void { overrides[id] = []; save(); }
export function resetShortcuts(): void { overrides = {}; save(); }

// Menu/help bindings share the same preference map as dispatch. Labels are
// updated on preference changes, never by a render-frame or polling timer.
export function refreshShortcutLabels(): void {
  for (const button of document.querySelectorAll<HTMLElement>('[data-command]')) {
    const id = button.dataset.command as ShortcutId;
    if (!defaults.has(id)) continue;
    uiText(button, `${commandName(id)}${shortcutLabel(id) ? '   '+shortcutLabel(id) : ''}`, false);
  }
  const titles: [string, ShortcutId, string][] = [
    ['play-pause','transport.toggle','재생 / 일시정지'], ['stop','transport.stop','정지 · 처음으로'], ['open-file','file.importAudio','오디오 가져오기'],
    ['zoom-in','view.zoomIn','수평 확대'], ['zoom-out','view.zoomOut','수평 축소'], ['snap-toggle','grid.snap','Snap'], ['cycle-toggle','transport.cycle','Cycle'],
    ['piano-select','tool.objectSelection','개체 선택'], ['piano-draw','midi.draw','노트 그리기'], ['piano-delete','edit.delete','삭제'],
    ['piano-quantize','midi.quantize','Quantize'], ['piano-transpose-up','midi.transposeUp','반음 올리기'], ['piano-transpose-down','midi.transposeDown','반음 내리기']
  ];
  for (const [elementId,id,label] of titles) {
    const el = document.getElementById(elementId); if (el) uiAttr(el,'title',`${tr(label)}${shortcutLabel(id) ? ' ('+shortcutLabel(id)+')' : ''}`);
  }
  const menu: [string,ShortcutId][] = [['project-new','project.new'],['project-open','project.open'],['project-save','project.save'],['project-save-as','project.saveAs']];
  for (const [elementId,id] of menu) { const el=document.getElementById(elementId); if(el) uiText(el,`${commandName(id)}${shortcutLabel(id) ? '   '+shortcutLabel(id) : ''}`,false); }
  const snap=document.getElementById('snap-toggle'); if(snap) uiText(snap,`${tr('Snap')}${shortcutLabel('grid.snap')?' · '+shortcutLabel('grid.snap'):''}`,false);
  const hint=document.querySelector('.transport-hint');if(hint)uiText(hint,`${shortcutLabel('transport.toggle') || tr('지정 안 함')} · ${tr('재생 / 일시정지')}`,false);
  const pianoSelect=document.getElementById('piano-select');if(pianoSelect)uiText(pianoSelect,`${shortcutLabel('tool.objectSelection')} ${tr('선택')}`,false);
  const pianoDraw=document.getElementById('piano-draw');if(pianoDraw)uiText(pianoDraw,`${shortcutLabel('midi.draw')} ${tr('그리기')}`,false);
  const help=document.querySelector('.piano-help .piano-popup');
  if(help)uiText(help,[tr('여러 Part 보기: Arrangement에서 Ctrl/Shift+클릭 후 Piano Roll 열기 · 상단에서 활성 Part 선택'),tr('다중 선택: Shift/Ctrl+클릭 또는 빈 곳 드래그'),`${shortcutLabel('midi.quantize')||'—'}: ${tr('Quantize')} · ${shortcutLabel('midi.transposeUp')||'—'} / ${shortcutLabel('midi.transposeDown')||'—'}: ${tr('Pitch')}`,tr('그리기: 빈 곳 클릭/드래그 · 선택: 노트 이동/양끝 길이 조절'),`${shortcutLabel('edit.delete')||'—'}: ${tr('삭제')} · ${tr('Ctrl+Wheel 확대 · Shift+Wheel 가로 이동')}`,tr('음이름 C3 = MIDI 60 · Track에서 MiniDAW Synth를 선택하면 소리가 출력됩니다.')].join('\n'),false);
  for(const hint of document.querySelectorAll('[data-shortcut-delete-help]'))uiText(hint,`더블 클릭 추가 · 끌기 이동 · ${shortcutLabel('edit.delete')||'—'}: 삭제`);
}
document.addEventListener('shortcuts-changed',refreshShortcutLabels);
document.addEventListener('language-changed',refreshShortcutLabels);
