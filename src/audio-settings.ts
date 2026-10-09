import { uiOption } from './i18n';
import { invoke } from "@tauri-apps/api/core";
import { element, text } from "./dom";
import type { AudioPreferences, Snapshot } from "./types";

const dialog = element<HTMLDialogElement>("audio-settings");
const driver = element<HTMLSelectElement>("driver-type");
const device = element<HTMLSelectElement>("output-device");
let current: Snapshot | null = null;
let draft: AudioPreferences | null = null;
let busy = false;
let generation = 0;

export function updateAudioSettings(next: Snapshot): void {
  current = next;
  const o = next.output, frames = next.metrics.bufferFrames;
  text("audio-current", next.outputError ? next.outputError.message : o ? `${o.driverType === "asio" ? "ASIO" : "WASAPI Shared"} · ${o.device}` : "출력 연결 없음");
  text("audio-format", o ? `${o.sampleRate.toLocaleString()} Hz · ${o.channels} 채널 · ${o.sampleFormat}` : "—");
  text("audio-buffer", o && frames ? `${frames} frames · ${(frames / o.sampleRate * 1000).toFixed(2)} ms` : "측정 대기");
  text("audio-driver-latency", o?.driverOutputLatencyFrames != null ? `드라이버 보고 출력 ${o.driverOutputLatencyFrames} frames (${(o.driverOutputLatencyFrames / o.sampleRate * 1000).toFixed(2)} ms) · 입력 ${o.driverInputLatencyFrames ?? "—"} frames` : "드라이버 보고 지연: 제공되지 않음");
  element<HTMLButtonElement>("asio-panel").disabled = busy || !o?.controlPanel || driver.value !== "asio" || device.value !== o.device;
}

export function initializeAudioSettings(apply: (settings: AudioPreferences, buffer?: number) => Promise<void>, panel: () => Promise<void>, fail: (error: unknown) => void): void {
  const setBusy = (value: boolean) => {
    busy = value;
    for (const id of ["driver-type", "output-device", "transport-declick", "buffer-size", "reconnect", "refresh-devices"]) element<HTMLInputElement>(id).disabled = value;
    element<HTMLSelectElement>("buffer-size").disabled = value || driver.value === "asio";
    text("audio-settings-progress", value ? "출력 설정 처리 중…" : "");
    if (current) updateAudioSettings(current);
  };
  const refresh = async () => {
    if (!draft) return;
    const version = ++generation;
    draft.driverType = driver.value as AudioPreferences["driverType"];
    const type = draft.driverType;
    text("device-label", type === "asio" ? "ASIO Driver" : "Output Device");
    element("asio-panel-row").hidden = type !== "asio";
    element("wasapi-buffer-row").hidden = type === "asio";
    element<HTMLSelectElement>("buffer-size").disabled = type === "asio";
    if (type === "asio") element<HTMLSelectElement>("buffer-size").value = "0";
    device.replaceChildren(uiOption("장치 목록 읽는 중…", "")); device.disabled = true;
    try {
      const names = await invoke<string[]>("output_devices", { driverType: type });
      if (version !== generation) return;
      device.replaceChildren();
      if (type === "wasapi") device.add(uiOption("Windows 기본 출력 장치", ""));
      for (const name of names) device.add(new Option(name, name));
      const saved = type === "asio" ? draft.asioDriver : draft.outputDevice;
      if (saved && !names.includes(saved)) { const missing = uiOption(`연결 불가: ${saved}`, saved); device.add(missing); }
      if (saved) device.value = saved;
      if (!device.options.length) device.add(uiOption("설치된 ASIO 드라이버 없음", ""));
    } finally { if (version === generation) device.disabled = false; }
    if (current) updateAudioSettings(current);
  };
  element("open-audio-settings").addEventListener("click", () => {
    if (!current || busy) return;
    draft = { ...current.preferences };
    driver.value = draft.driverType;
    element<HTMLInputElement>("transport-declick").checked = draft.transportDeclick;
    dialog.showModal(); void refresh().catch(fail);
  });
  element("close-audio-settings").addEventListener("click", () => dialog.close());
  driver.addEventListener("change", () => { void refresh().catch(fail); });
  device.addEventListener("change", () => { if (current) updateAudioSettings(current); });
  element("refresh-devices").addEventListener("click", () => { void refresh().catch(fail); });
  element("reconnect").addEventListener("click", () => {
    if (!draft || busy) return;
    const next = { ...draft, transportDeclick: element<HTMLInputElement>("transport-declick").checked };
    if (next.driverType === "asio") next.asioDriver = device.value || null;
    else next.outputDevice = device.value || null;
    setBusy(true);
    void apply(next, Number(element<HTMLSelectElement>("buffer-size").value) || undefined)
      .then(() => { draft = next; }).catch(fail).finally(() => setBusy(false));
  });
  element("asio-panel").addEventListener("click", () => {
    if (busy) return;
    setBusy(true); void panel().catch(fail).finally(() => setBusy(false));
  });
}
