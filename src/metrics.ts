import { uiText, uiAttr } from './i18n';
import { element } from "./dom";
import type { Snapshot } from "./types";

const ms = (value: number | null): string => value === null ? "—" : `${value.toFixed(3)} ms`;
let updated = 0;

export function updateMetrics(snapshot: Snapshot): void {
  if (performance.now() - updated < 200) return;
  updated = performance.now();
  const metrics = snapshot.metrics;
  uiText(element("metric-buffer"), metrics.bufferFrames === null ? "—" : `${metrics.bufferFrames} frames`);
  uiAttr(element("metric-buffer"), 'title', `관측 범위: ${metrics.minBufferFrames ?? "—"}–${metrics.maxBufferFrames ?? "—"} 프레임 / 채널`);
  uiText(element("metric-avg"), ms(metrics.deviceCallbackAvgMs ?? metrics.callbackAvgMs));
  uiText(element("metric-max"), ms(metrics.deviceCallbackMaxMs ?? metrics.callbackMaxMs));
  uiText(element("metric-count"), metrics.callbacks.toLocaleString());
  uiText(element("metric-overruns"), String(Math.max(metrics.overruns, metrics.deviceCallbackOverruns)));
  uiText(element("metric-interval"), `${ms(metrics.callbackIntervalAvgMs)} / ${ms(metrics.callbackIntervalMaxMs)}`);
  uiText(element("metric-play"), ms(metrics.playMs));
  uiText(element("metric-seek"), ms(metrics.seekMs));
  uiText(element("metric-pause"), ms(metrics.pauseMs));
  uiText(element("metric-resume"), ms(metrics.resumeMs));
  uiText(element("metric-stop"), ms(metrics.stopMs));
  uiText(element("metric-cache"), ms(snapshot.cacheBuildMs));
  const source = snapshot.source;
  const mib = (bytes: number) => `${(bytes / 1048576).toFixed(2)} MiB`;
  uiText(element("metric-source"), source?.mode ?? "—");
  uiText(element("metric-resident"), source ? mib(source.pcmResidentBytes + source.bufferBytes) : "—");
  uiText(element("metric-ahead"), source?.capacityFrames ? `${(source.bufferedFrames / source.bufferSampleRate).toFixed(2)} s / ${(source.capacityFrames / source.bufferSampleRate).toFixed(2)} s` : "—");
  uiText(element("metric-starvation"), String(source?.starvation ?? 0));
  uiText(element("metric-priming"), String(source?.primingCallbacks ?? 0));
  uiText(element("metric-refill"), source?.mode === "Streaming" ? `${source.refillMs.toFixed(2)} / ${source.refillMaxMs.toFixed(2)} ms` : "—");
  uiText(element("metric-wave-memory"), snapshot.waveform ? mib(snapshot.waveform.bytes) : "—");
}
