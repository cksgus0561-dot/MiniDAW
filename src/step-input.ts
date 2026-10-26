import { invoke, isTauri } from '@tauri-apps/api/core';
import { element } from './dom';
import { globalShortcut, keyboardBlocked } from './keyboard';
import { shortcutCommand, shortcutLabel } from './shortcuts';
import { uiText, uiAttr, tr } from './i18n';
import { noteName, type MidiClip } from './midi';

// FL Studio Step Edit / Cubase Step Input. Playing a key enters one note of the
// current Grid length at the step cursor; keys held together form one chord and
// one Undo step. The cursor advances when every key is released. No timing is
// recorded, so input/output latency never affects the written position.
export interface StepHost {
  clip(): MidiClip | null;
  trackId(): string | null;
  open(): boolean;
  owns(): boolean;
  playhead(): number;
  snap(tick: number): number;
  step(tick: number): number;
  grid(): string;
  label(tick: number): string;
  commit(request: unknown): Promise<void>;
  select(ids: Set<string>): void;
  draw(): void;
}
export interface StepOverlay { cursor: number; length: number; pitches: number[] }

export class StepInput {
  private enabled = false;
  private cursor = 0;
  private clipId: string | null = null;
  private held = new Map<string, number>();
  private chord = new Map<number, number>();
  private entered: { tick: number; pitches: number[] }[] = [];
  private chain: Promise<void> = Promise.resolve();
  private armed = false;
  private polling = false;
  private button = element<HTMLButtonElement>('piano-step');
  private info = element('piano-step-info');
  constructor(private host: StepHost) {
    this.button.onclick = () => this.setEnabled(!this.enabled);
    document.addEventListener('computer-midi-key', e => {
      const d = (e as CustomEvent<{ code: string; pitch: number; on: boolean }>).detail;
      if (d.on) this.press('key:' + d.code, d.pitch, 100); else this.lift('key:' + d.code);
    });
    document.addEventListener('computer-midi-release', () => this.reset());
    // Window capture runs before document-level shortcut handlers (Backspace =
    // Delete), so step-mode cursor keys win only while the mode is ON.
    window.addEventListener('keydown', e => this.key(e), true);
    globalShortcut(e => shortcutCommand(e) === 'midi.stepInput' && this.host.open() ? () => this.setEnabled(!this.enabled) : undefined);
    document.addEventListener('language-changed', () => this.render());
    document.addEventListener('shortcuts-changed', () => this.render());
    window.setInterval(() => void this.poll(), 25);
    this.render();
  }
  active(): boolean { return this.enabled; }
  overlay(): StepOverlay | null {
    const clip = this.host.clip();
    if (!this.enabled || !clip) return null;
    return { cursor: this.cursor, length: this.length(), pitches: [...this.chord.keys()] };
  }
  // Ruler click while ON places the cursor (Snap applies as for drawing).
  place(tick: number): void {
    const clip = this.host.clip(); if (!this.enabled || !clip) return;
    this.cursor = Math.max(Number(clip.startTick), this.host.snap(tick)); this.render(); this.host.draw();
  }
  // The active Part changed or the document was replaced.
  clipChanged(): void {
    const clip = this.host.clip();
    if (clip?.clipId !== this.clipId) {
      this.clipId = clip?.clipId ?? null; this.entered = []; this.reset();
      if (clip) this.cursor = this.home(clip);
    }
    document.dispatchEvent(new CustomEvent('piano-step-target', { detail: { trackId: this.enabled ? this.host.trackId() : null } }));
    this.render();
  }
  setEnabled(on: boolean): void {
    if (on === this.enabled) return;
    this.enabled = on; this.reset(); this.entered = [];
    const clip = this.host.clip(); if (on && clip) { this.clipId = clip.clipId; this.cursor = this.home(clip); }
    // Computer keys must be ON to type notes; USB MIDI input works regardless.
    if (on) document.dispatchEvent(new CustomEvent('computer-midi-request', { detail: { enabled: true } }));
    document.dispatchEvent(new CustomEvent('piano-step-target', { detail: { trackId: on ? this.host.trackId() : null } }));
    this.render(); this.host.draw();
  }
  private home(clip: MidiClip): number {
    // Start at the playhead when it lies inside the Part, otherwise at its start.
    const start = Number(clip.startTick), end = start + Number(clip.lengthTick), at = this.host.snap(this.host.playhead());
    return at >= start && at < end ? at : start;
  }
  private length(): number { return Math.max(1, Math.round(this.host.step(this.cursor))); }
  private reset(): void { this.held.clear(); if (this.chord.size) { this.chord.clear(); this.host.draw(); } }
  private press(source: string, pitch: number, velocity: number): void {
    if (!this.enabled || !this.host.open() || !this.host.clip() || this.held.has(source)) return;
    this.held.set(source, pitch);
    if (!this.chord.has(pitch)) this.chord.set(pitch, Math.max(1, Math.min(127, velocity)));
    this.render(); this.host.draw();
  }
  private lift(source: string): void {
    if (!this.held.delete(source) || this.held.size || !this.chord.size) return;
    const clip = this.host.clip(); if (!clip) { this.reset(); return; }
    const start = Math.max(this.cursor, Number(clip.startTick)), length = this.length();
    const pitches = [...this.chord.keys()].sort((a, b) => a - b), velocities = pitches.map(p => this.chord.get(p)!);
    this.chord.clear();
    // Advance immediately so fast typing never waits for the document round trip.
    this.cursor = start + length; this.entered.push({ tick: start, pitches });
    this.enqueue(async () => {
      const relative = start - Number(clip.startTick);
      await this.host.commit({ command: 'midi.note.step', clipIds: [clip.clipId], targetTick: String(relative), lengthTick: String(length), pitches, velocities });
      const now = this.host.clip();
      if (now?.clipId === clip.clipId) this.host.select(new Set(now.notes.filter(n => Number(n.startTick) === relative && pitches.includes(n.pitch)).map(n => n.noteId)));
    });
    this.render(); this.host.draw();
  }
  private back(): void {
    const clip = this.host.clip(); if (!clip) return;
    const last = this.entered.pop();
    if (!last) { this.move(-1); return; }
    this.cursor = last.tick;
    this.enqueue(async () => {
      const now = this.host.clip(), relative = last.tick - Number(clip.startTick);
      const ids = now?.clipId === clip.clipId ? now.notes.filter(n => Number(n.startTick) === relative && last.pitches.includes(n.pitch)).map(n => n.noteId) : [];
      if (ids.length) { await this.host.commit({ command: 'midi.note.delete', clipIds: [clip.clipId], noteIds: ids }); this.host.select(new Set()); }
    });
    this.render(); this.host.draw();
  }
  private move(direction: number): void {
    const clip = this.host.clip(); if (!clip) return;
    const start = Number(clip.startTick);
    this.cursor = direction > 0 ? this.cursor + this.length() : Math.max(start, this.cursor - Math.max(1, Math.round(this.host.step(Math.max(start, this.cursor - 1)))));
    this.render(); this.host.draw();
  }
  private enqueue(task: () => Promise<void>): void { this.chain = this.chain.then(task).catch(() => { /* edit errors are shown by the project service */ }); }
  private key(e: KeyboardEvent): void {
    if (!this.enabled || !this.host.open() || keyboardBlocked(e) || e.ctrlKey || e.altKey || e.metaKey || e.shiftKey) return;
    if (e.target instanceof Element && e.target.closest('select,input,textarea')) return;
    const action = ({ ArrowRight: () => this.move(1), ArrowLeft: () => this.move(-1), Backspace: () => this.back(), Escape: () => this.setEnabled(false) } as Record<string, () => void>)[e.code];
    if (!action) return;
    e.preventDefault(); e.stopImmediatePropagation(); action();
  }
  // USB keyboards reach Rust directly; while ON, the owning window drains a copy
  // of their Note On/Off. Polling interval only affects display, never position.
  private async poll(): Promise<void> {
    if (!isTauri() || this.polling) return;
    const want = this.enabled && this.host.owns() && this.host.open();
    if (!want && !this.armed) return;
    this.polling = true;
    try {
      const events = await invoke<[number, number, number][]>('midi_input_step', { enabled: want });
      this.armed = want;
      for (const [status, pitch, velocity] of events) {
        if ((status & 0xf0) === 0x90 && velocity > 0) this.press(`usb:${status & 15}:${pitch}`, pitch, velocity);
        else this.lift(`usb:${status & 15}:${pitch}`);
      }
    } catch { /* Next tick retries; failure never blocks keyboard step input. */ }
    finally { this.polling = false; }
  }
  render(): void {
    this.button.setAttribute('aria-pressed', String(this.enabled));
    uiAttr(this.button, 'title', `${tr('스텝 입력')}${shortcutLabel('midi.stepInput') ? ' (' + shortcutLabel('midi.stepInput') + ')' : ''} · ${tr('건반을 누르면 커서 위치에 Grid 길이로 입력 · 동시에 누르면 화음 · ←/→ 커서 이동 · Backspace 마지막 입력 취소 · Esc 종료 · 눈금자 클릭으로 커서 지정')}`, false);
    this.info.hidden = !this.enabled;
    if (!this.enabled) return;
    if (!this.host.clip()) { uiText(this.info, 'MIDI Clip을 선택하세요'); return; }
    const grid = this.host.grid(), chord = [...this.chord.keys()].sort((a, b) => a - b).map(noteName).join(' ');
    uiText(this.info, `${tr('커서')} ${this.host.label(this.cursor)} · ${tr('길이')} ${grid === 'beat' ? 'Beat' : grid === 'bar' ? 'Bar' : '1/' + grid}${chord ? ' · ' + chord : ''}`, false);
  }
}
