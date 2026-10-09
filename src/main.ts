import { uiAttr, uiText, tr } from './i18n';
import { initializeLocalization } from './i18n';
import { shortcutCommand, refreshShortcutLabels } from './shortcuts';
import { initializeAppSettings } from './app-settings';
import { ComputerMidi } from './computer-midi';
import { Plugins } from './plugins';
import { globalShortcut } from './keyboard';
import { invoke, isTauri } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { open } from "@tauri-apps/plugin-dialog";
import { element, time, text } from "./dom";
import { Waveform } from "./waveform";
import { updateMetrics } from "./metrics";
import { initializeFontScale } from "./preferences";
import { initializeAudioSettings, updateAudioSettings } from "./audio-settings";
import { AudioExport } from './export-audio';
import { ProjectUI } from "./project";
import { Editing } from './editing';
import { Panels } from './panels';
import { Mixer } from './mixer';
import { SpectrumPanel } from './spectrum';
import { PianoRoll } from './piano-roll';
import { PanelWindows } from './panel-windows';
import { language } from './i18n';
import { bindingsFor, type ShortcutId } from './shortcuts';
import { MusicalControls } from './musical-controls';
import type { AppError, Snapshot } from "./types";
import "./style.css";
import './workspace.css';

initializeLocalization();
let snapshot: Snapshot | null = null;
let receivedAt = 0;
let loading = false;
let dialogOpen = false;
let minimumCommand = 0;
let commandChain: Promise<unknown> = Promise.resolve();
let sourceErrorKey = "";
const desktop = isTauri();

function showError(error: unknown): void {
  const details = typeof error === "object" && error !== null ? error as Partial<AppError> : null;
  text("error-text", details?.message ?? "작업을 완료하지 못했습니다. 다시 시도해 주세요.");
  uiAttr(element("error-text"), 'title', details?.detail ?? String(error));
  element("error").hidden = false;
  text("audio-settings-error", details?.message ?? String(error));
  uiAttr(element("audio-settings-error"), 'title', details?.detail ?? String(error));
}

let editing:Editing;
const computer = new ComputerMidi(showError);
const panels = new Panels([
  { id: 'media', title: 'Media', element: 'panel-media' },
  { id: 'arrangement', title: 'Arrangement', element: 'panel-arrangement' },
  { id: 'performance', title: 'Performance', element: 'panel-performance' },
  { id: 'spectrum', title: 'Spectrum', element: 'panel-spectrum' },
  { id: 'piano', title: 'Piano Roll', element: 'panel-piano' },
  { id: 'mixer', title: 'Mixer', element: 'panel-mixer' },
]);
const waveform = new Waveform(seconds => {editing.cursor=seconds;return command("seek", seconds);}, showError);
document.addEventListener('workspace-changing', () => waveform.cancelDrag());
const projects = new ProjectUI(controls, async work => {
  if (loading) throw { message: "오디오 작업이 진행 중입니다. 잠시 후 다시 시도해 주세요." };
  loading = true; waveform.cancelDrag(); controls();
  try { await commandChain; await work(); accept(await invoke<Snapshot>("engine_snapshot")); element("error").hidden = true; }
  finally { loading = false; controls(); }
}, showError, () => loading || dialogOpen, panels);
new AudioExport(() => !projects.busy && !loading && !dialogOpen,()=>editing.exportSelection());
new Plugins(request=>projects.edit(request),()=>!projects.busy&&!loading&&!dialogOpen,showError);
new MusicalControls(waveform, request => projects.edit(request), showError);
editing=new Editing(waveform,request=>projects.edit(request),showError);
const piano = new PianoRoll(panels, waveform, request => projects.edit(request));
const spectrum = new SpectrumPanel(panels, () => editing.spectrumSelection());
const mixer = new Mixer(panels, request => projects.edit(request), () => !projects.busy && !loading && !dialogOpen);
let projectView:unknown=null,selectedTrack:unknown=null,remoteSpectrumFps=0;
document.addEventListener('project-view',e=>{projectView=(e as CustomEvent).detail;});
document.addEventListener('track-selected',e=>{selectedTrack=(e as CustomEvent).detail;});
const panelWindows=new PanelWindows(panels,{
  piano:{capture:()=>piano.captureWindowState(),restore:s=>piano.restoreWindowState(s)},
  spectrum:{capture:()=>spectrum.captureWindowState(),restore:s=>spectrum.restoreWindowState(s),suspend:()=>spectrum.suspendWindow()},
  mixer:{capture:()=>({scroll:element('mixer-content').scrollLeft}),restore:s=>{if(s)element('mixer-content').scrollLeft=s.scroll;},suspend:async()=>{while(mixer.hasPendingChanges())await new Promise(r=>setTimeout(r,20));}},
},()=>({project:projectView,track:selectedTrack,selection:editing.spectrumSelection(),grid:{grid:waveform.grid,snap:waveform.snapEnabled},font:document.documentElement.style.getPropertyValue('--ui-font-scale'),language:language(),shortcuts:localStorage.getItem('minidaw.ui.shortcuts.v1'),computer:computer.captureWindowState()}),async(kind,value)=>{
  if(kind==='edit'){await projects.editFromWindow(value);return;}
  if(kind==='transport')return command(value.action,value.seconds);
  if(kind==='panel'){panels.setOpen(value.id,value.open);return;}
  if(kind==='computer'){computer.restoreWindowState(value);panelWindows.broadcast('computer-midi-changed',value);return;}
  if(kind==='grid'){waveform.grid=value.grid;waveform.snapEnabled=value.snap;element<HTMLSelectElement>('grid-type').value=value.grid;element('snap-toggle').setAttribute('aria-pressed',String(value.snap));waveform.refreshStyle();document.dispatchEvent(new Event('musical-grid-changed'));return;}
  if(kind==='shortcut'){
    if(value==='edit.undo'||value==='edit.redo'){await projects.editFromWindow({command:value});return;}
    // Dispatch the command's current binding through the same focus-safe registry.
    const binding=bindingsFor(value as ShortcutId)[0];if(!binding)return;
    const parts=binding.split('+'),key=parts.at(-1)!;const numeric=key.startsWith('Num '),suffix=key.replace('Num ','');
    const code=numeric?({'/':'NumpadDivide','.':'NumpadDecimal','*':'NumpadMultiply','+':'NumpadAdd','-':'NumpadSubtract'}[suffix]??`Numpad${suffix}`):key==='Space'?'Space':key.length===1?`Key${key}`:key;
    const options={key:key==='Space'?' ':suffix,code,ctrlKey:parts.includes('Ctrl'),shiftKey:parts.includes('Shift'),altKey:parts.includes('Alt'),bubbles:true,cancelable:true};
    element('play-pause').dispatchEvent(new KeyboardEvent('keydown',options));element('play-pause').dispatchEvent(new KeyboardEvent('keyup',options));
  }
},showError);
document.addEventListener('computer-midi-changed',()=>panelWindows.broadcast('computer-midi-changed',computer.captureWindowState()));
document.addEventListener('spectrum-window-fps',e=>{remoteSpectrumFps=Number((e as CustomEvent).detail);});
element('font-scale').addEventListener('change',()=>queueMicrotask(()=>panelWindows.broadcast('musical-grid-changed',null)));
initializeAppSettings();
refreshShortcutLabels();
document.addEventListener('language-changed',()=>waveform.refreshStyle());
let editingPeakVersion=-1;
initializeFontScale(() => waveform.refreshStyle(), showError);
initializeAudioSettings(async (settings, buffer) => {
  loading = true; controls(); waveform.cancelDrag();
  try {
    await commandChain;
    accept(await invoke<Snapshot>("apply_audio_settings", { settings }));
    // A positive WASAPI buffer request deliberately reconnects, just as before.
    if (buffer && settings.driverType === "wasapi") await invoke("reconnect_output", { buffer });
    element("error").hidden = true;
    text("audio-settings-error", "");
  } finally { minimumCommand = 0; loading = false; controls(); }
}, async () => {
  loading = true; controls(); waveform.cancelDrag();
  try {
    await commandChain;
    await invoke("asio_control_panel");
    // Some drivers open a non-modal external panel and return immediately.
    // Explicit Apply reconnects after the user has finished changing the driver.
  } finally { minimumCommand = 0; loading = false; controls(); }
}, showError);

function controls(): void {
  const available = desktop && Boolean(snapshot?.file) && !loading && !snapshot?.outputError;
  waveform.setInteractive(available);
  const toggle = element<HTMLButtonElement>("play-pause");
  toggle.disabled = !available;
  const playing = snapshot?.transport.state === "playing";
  const label = playing ? "일시정지" : "재생";
  text("play-pause", playing ? "⏸ 일시정지" : "▶ 재생");
  uiAttr(toggle, "aria-label", label);
  toggle.setAttribute("aria-pressed", String(playing));
  // Shortcut labels are owned by the preference map, not transport polling.
  element<HTMLButtonElement>("stop").disabled = !available;
  element<HTMLButtonElement>("open-file").disabled = !desktop || loading || dialogOpen;
  if (projects?.busy) element<HTMLButtonElement>("open-file").disabled = true;
  element<HTMLButtonElement>("open-audio-settings").disabled = !desktop || loading || projects.busy;
  element<HTMLButtonElement>("reconnect").disabled = !desktop || loading;
  projects.controls();
}

function accept(next: Snapshot): void {
  if(next.sourcePending&&!next.outputError)return;
  if (next.transport.appliedCommand < minimumCommand && !next.outputError) return;
  minimumCommand = Math.max(minimumCommand, next.transport.appliedCommand);
  snapshot = next; receivedAt = performance.now();
  if(!waveform.dragging)editing.cursor=next.position;
  projects.observeRevision(next.projectRevision);
  updateAudioSettings(next);
  mixer.meter(next);
  editing.automationDisplay(next.position,next.transport.state==='playing');
  if (panels.isOpen('performance')) updateMetrics(next);
  const state = element("transport-state");
  uiText(state, { playing: "재생 중", paused: "일시정지", stopped: "정지" }[next.transport.state]);
  state.classList.toggle("playing", next.transport.state === "playing");
  text("duration", time(next.duration));
  if (next.file) {
    waveform.setFile(next.file, next.transport.clipId);
    if (next.waveform) waveform.updateCache(next.waveform.version);
    if(next.waveform && editingPeakVersion!==next.waveform.version){editingPeakVersion=next.waveform.version;editing.refreshPeaks();}
    text("file-name", next.file.name);
    uiAttr(element("file-name"), 'title', next.file.name, false);
    text("file-rate", `${next.file.sampleRate.toLocaleString()} Hz`);
    text("file-channels", next.file.channels === 2 ? "스테레오 · 2 채널" : "모노 · 1 채널");
    text("file-length", time(next.file.duration));
    text("file-frames", `${next.source?.mode ?? "Memory"} · f32 PCM 환산 ${(next.file.frames * next.file.channels * 4 / 1048576).toFixed(1)} MiB`);
    text("file-badge", next.file.name.split(".").at(-1)?.toUpperCase() ?? "AUDIO");
    text("cache-status", !next.waveform ? "MIDI Timeline · 오디오 파형 없음" : next.waveform.error ? "파형 분석 오류" : next.waveform?.complete ? "다단계 peak cache" : `파형 분석 ${Math.floor((next.waveform?.progress ?? 0) * 100)}%`);
  } else {
    waveform.clear(); text("file-name", "새로운 사운드를 열어 보세요"); uiAttr(element("file-name"), 'title', "");
    text("file-rate", "— Hz"); text("file-channels", "— 채널"); text("file-length", "—"); text("file-frames", "PCM · f32"); text("file-badge", "WAV / MP3 / FLAC"); text("cache-status", "파일을 기다리는 중");
  }
  const sourceError = next.source?.error ?? next.waveform?.error;
  const errorKey = sourceError ? `${next.transport.clipId}:${sourceError.code}` : "";
  if (errorKey && errorKey !== sourceErrorKey) showError(sourceError);
  sourceErrorKey = errorKey;
  text("output-label", next.outputError ? next.outputError.message : next.output?.device ?? "출력 장치 없음");
  uiAttr(element("output-label"), 'title', next.outputError?.detail ?? next.output?.device ?? "");
  element("output-led").classList.toggle("ready", !next.outputError && Boolean(next.output));
  text("metric-rate", next.output ? `${next.output.sampleRate.toLocaleString()} Hz` : "—");
  text("metric-errors", String(next.streamErrors));
  text("status-text", loading ? "파일 준비 중 · 현재 오디오는 계속 처리됩니다" : next.outputError ? "출력 연결을 확인해 주세요" : "엔진 연결됨 · f32 PCM · 출력 전용");
  controls();
}

function command(action: string, seconds?: number): Promise<number | null> {
  if (!snapshot?.file || loading || !desktop) return Promise.resolve(null);
  if (action !== "seek") waveform.cancelDrag();
  const result = commandChain.then(async () => {
    const id = await invoke<number>("transport_command", { action, seconds: seconds ?? null });
    minimumCommand = Math.max(minimumCommand, id);
    return id;
  }).catch(error => { showError(error); return null; });
  commandChain = result;
  return result;
}

async function load(path: string): Promise<void> {
  if (loading || projects.busy) return;
  loading = true; controls(); element("loading").hidden = false; element("error").hidden = true;
  try {
    await commandChain;
    const next = await invoke<Snapshot>("load_audio", { path, trackId: editing?.audioImportTrack() ?? null });
    accept(next);
    await projects.refresh();
    if (next.file?.sanitizedSamples) showError({ message: "유효하지 않은 일부 오디오 샘플을 무음으로 처리했습니다." });
  } catch (error) { showError(error); }
  finally { loading = false; element("loading").hidden = true; controls(); }
}

async function chooseFile(): Promise<void> {
  if (!desktop || dialogOpen || loading || projects.busy) return;
  dialogOpen = true; controls();
  try {
    const path = await open({ title: tr("오디오 파일 열기"), multiple: false, directory: false,
      filters: [{ name: tr("오디오 파일"), extensions: ["wav", "mp3", "flac"] }] });
    if (typeof path === "string") await load(path);
  } catch (error) { showError(error); }
  finally { dialogOpen = false; controls(); }
}

element("open-file").addEventListener("click", () => { void chooseFile(); });
element('media-import').addEventListener('click', () => { void chooseFile(); });
element("play-pause").addEventListener("click", () => command("toggle"));
element("stop").addEventListener("click", () => command("stop"));
element("zoom-in").addEventListener("click", () => waveform.zoom(2));
element("zoom-out").addEventListener("click", () => waveform.zoom(0.5));
element("zoom-fit").addEventListener("click", () => waveform.fit());
element("dismiss-error").addEventListener("click", () => { element("error").hidden = true; });

globalShortcut(event => {
  const id=shortcutCommand(event);
  if(id==='file.importAudio')return()=>{void chooseFile();};
  if(id==='transport.toggle')return()=>{void command('toggle');};
  if(id==='transport.play')return()=>{void command('play');};
  if(id==='transport.stop')return()=>{void command('stop');};
  if(id==='transport.start'||id==='transport.left'||id==='transport.right')return()=>{waveform.cancelDrag();void command('seek',id==='transport.start'?0:waveform.musical.seconds(Number(id==='transport.left'?waveform.cycle?.startTick??'0':waveform.cycle?.endTick??'0')));};
  if(id==='view.zoomIn'||id==='view.zoomOut'){
    if((event.target as HTMLElement).closest('#panel-piano'))return;
    return()=>waveform.zoom(id==='view.zoomIn'?2:.5);
  }
  if(id?.startsWith('view.')&&['media','arrangement','piano','spectrum','performance','mixer'].includes(id.slice(5)))return()=>panels.setOpen(id.slice(5),!panels.isOpen(id.slice(5)));
});

let lastFrame = performance.now();
let frameTotal = 0;
let frameCount = 0;
function frame(now: number): void {
  const delta = now - lastFrame; lastFrame = now;
  if (!document.hidden && delta < 1000) { frameTotal += delta; frameCount++; }
  if (frameTotal >= 500) {
    text("metric-fps", (frameCount * 1000 / frameTotal).toFixed(1));
    const spectrumFps=panels.windows?.detached('spectrum')?(panels.windows.visible('spectrum')?remoteSpectrumFps:0):spectrum.sampleRenderFps();
    text("metric-spectrum-fps", spectrumFps.toFixed(1));
    if(!panels.windows?.detached('spectrum'))panelWindows.broadcast('spectrum-window-fps',spectrumFps);
    text("metric-frame", `${(frameTotal / frameCount).toFixed(2)} ms`);
    text("metric-draws", String(waveform.drawCount+editing.drawCount));
    panelWindows.broadcast('arrangement-draw-count',waveform.drawCount+editing.drawCount);
    frameTotal = 0; frameCount = 0;
  }
  // Display-only interpolation, capped at 50 ms. Rust owns the real transport.
  const ready = !snapshot?.source || snapshot.source.capacityFrames === 0 || (snapshot.source.generation === snapshot.source.readyGeneration && snapshot.source.bufferedFrames > 0);
  const advance = snapshot?.transport.state === "playing" && !snapshot.outputError && ready ? Math.min(50, now - receivedAt) / 1000 : 0;
  const position = Math.min(snapshot?.duration ?? 0, (snapshot?.position ?? 0) + advance);
  piano.setPosition(position);
  const displayed = waveform.setPosition(position, snapshot?.transport.appliedCommand ?? 0);
  text("position", time(displayed));
  requestAnimationFrame(frame);
}

async function poll(): Promise<void> {
  try { accept(await invoke<Snapshot>("engine_snapshot")); }
  catch (error) { showError(error); }
  finally { window.setTimeout(() => { void poll(); }, document.hidden ? 250 : 33); }
}

async function initialize(): Promise<void> {
  controls(); requestAnimationFrame(frame);
  if (!desktop) {
    text("output-label", "브라우저 미리보기");
    text("status-text", "오디오 기능은 MiniDAW 데스크톱 앱에서 사용할 수 있습니다.");
    return;
  }
  text("version", `v${await invoke<string>("app_version")}`);
  await projects.initialize();
  await panelWindows.initialize();
  await getCurrentWebviewWindow().onDragDropEvent(event => {
    element("drop-overlay").hidden = event.payload.type !== "over" && event.payload.type !== "enter";
    if (event.payload.type === "drop") {
      element("drop-overlay").hidden = true;
      if (event.payload.paths.length !== 1) showError({ message: "오디오 파일을 한 번에 하나씩 놓아 주세요." });
      else if (event.payload.paths[0].toLowerCase().endsWith(".minidaw")) void projects.openDroppedProject(event.payload.paths[0]);
      else void load(event.payload.paths[0]);
    }
  });
  void poll();
}
void initialize().catch(showError);
