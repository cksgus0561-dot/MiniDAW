export interface AppError { code: string; message: string; detail?: string }
export interface FileInfo {
  name: string; sampleRate: number; channels: number; frames: number;
  duration: number; sanitizedSamples: number;
}
export interface OutputInfo {
  backend: string; sampleFormat: string;
  device: string; sampleRate: number; channels: number; requestedBuffer: number | null;
  supportedBufferMin: number | null; supportedBufferMax: number | null;
  driverType: "wasapi" | "asio"; controlPanel: boolean;
  driverInputLatencyFrames: number | null; driverOutputLatencyFrames: number | null;
}
export interface AudioPreferences { driverType: "wasapi" | "asio"; outputDevice: string | null; asioDriver: string | null; transportDeclick: boolean }
export interface Snapshot {
  pdc: { audioLookaheadSamples: number; trackDelaySamples: number; masterLatencySamples: number; outputLatencySamples: number; error: string | null };
  effects: { channelId:string; effectId:string; reductionDb:number }[];
  master: { peakDb: [number, number] };
  sourcePending: boolean;
  projectRevision?: number;
  preferences: AudioPreferences; asioAvailable: boolean;
  metrics: { callbacks: number; callbackAvgMs: number | null; callbackMaxMs: number | null;
    bufferFrames: number | null; minBufferFrames: number | null; maxBufferFrames: number | null;
    overruns: number; playMs: number | null; seekMs: number | null;
    pauseMs: number | null; resumeMs: number | null; stopMs: number | null;
    callbackIntervalAvgMs: number | null; callbackIntervalMaxMs: number | null;
    deviceCallbackAvgMs: number | null; deviceCallbackMaxMs: number | null; deviceCallbackOverruns: number;
    renderedFrames: number; nonSilentFrames: number };
  cacheBuildMs: number | null;
  source: { mode: "Memory" | "Streaming"; pcmResidentBytes: number; bufferedFrames: number;
    capacityFrames: number; bufferBytes: number; bufferSampleRate: number; starvation: number; primingCallbacks: number;
    refillMs: number; refillMaxMs: number; refillCount: number; discardedRequests: number;
    generation: number; readyGeneration: number; error: AppError | null } | null;
  waveform: { version: number; progress: number; complete: boolean; bytes: number; buildMs: number; error: AppError | null } | null;
  transport: { state: "stopped" | "playing" | "paused"; frame: number; clipId: number; appliedCommand: number };
  file: FileInfo | null; position: number; duration: number;
  output: OutputInfo | null; outputError: AppError | null; streamErrors: number;
}
export interface WaveformView { clipId: number; start: number; end: number; channels: [number, number][][] }
