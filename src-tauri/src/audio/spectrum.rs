//! Spectrum sidechain: bounded SPSC copies on RT; all FFT, averaging and serialization off RT.
use super::{
    streaming::FrameReader,
    timeline::{PlaybackPlan, TimelineReader},
};
use crate::error::{AppError, AppResult};
use rtrb::{Consumer, Producer, RingBuffer};
use rustfft::{num_complex::Complex, Fft, FftPlanner};
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::*},
    Arc, Mutex,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Immutable analysis context, owned only by control/analysis threads.
#[derive(Clone)]
pub struct TrackCapture {
    pub source_id: u64,
    pub plan: Arc<PlaybackPlan>,
    pub tracks: Vec<(String, usize)>,
}

const BLOCK: usize = 256;
const QUEUE: usize = 8;
const POINTS: usize = 768;
pub const FLOOR_DB: f32 = -120.0;
pub fn valid_size(n: usize) -> bool {
    matches!(n, 2048 | 4096 | 8192 | 16384)
}
fn error(message: &str) -> AppError {
    AppError::new("spectrum", message)
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub enabled: bool,
    pub fft_size: usize,
    pub smoothing_ms: u32,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            fft_size: 4096,
            smoothing_ms: 200,
        }
    }
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpectrumFrame {
    pub sequence: u64,
    pub sample_rate: u32,
    pub fft_size: usize,
    pub frequencies: Vec<f32>,
    /// L, R, mean stereo power. Combined never cancels opposite-phase stereo.
    pub curves: [Vec<f32>; 3],
    pub maxima: [Vec<f32>; 3],
    pub windows: u64,
    pub duration: f64,
    pub processing_ms: f64,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpectrumSnapshot {
    pub enabled: bool,
    pub frame: Option<SpectrumFrame>,
    pub comparison: Option<SpectrumFrame>,
    pub sources: Vec<String>,
    pub error: Option<String>,
    pub processed: u64,
    pub dropped_blocks: u64,
    pub skipped_frames: u64,
    pub worker_ms: f64,
    pub worker_max_ms: f64,
    pub track_read_ms: f64,
    pub age_ms: Option<f64>,
    pub selection_progress: f64,
}

/// Fourfold zero padding limits scalloping error (<0.1 dB) without pretending
/// to increase the resolving power of the chosen Hann window length.
pub struct SpectrumFft {
    pub size: usize,
    pub rate: u32,
    fft: Arc<dyn Fft<f32>>,
    buffer: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    pub power: [Vec<f32>; 3],
}
impl SpectrumFft {
    pub fn new(size: usize, rate: u32) -> AppResult<Self> {
        if !valid_size(size) || !(8000..=384000).contains(&rate) {
            return Err(error("FFT 크기 또는 sample rate가 올바르지 않습니다."));
        }
        let fft = FftPlanner::new().plan_fft_forward(size * 4);
        let scratch = vec![Complex::default(); fft.get_inplace_scratch_len()];
        Ok(Self {
            size,
            rate,
            fft,
            scratch,
            buffer: vec![Complex::default(); size * 4],
            power: std::array::from_fn(|_| vec![0.0; size * 2 + 1]),
        })
    }
    pub fn analyze(&mut self, samples: &[[f32; 2]]) {
        let len = samples.len().min(self.size);
        // The final short selection window uses its own coherent gain, then is
        // weighted by its real duration in the whole-selection power average.
        let sum = (len as f32 * 0.5).max(1.0);
        for ch in 0..2 {
            self.buffer.fill(Complex::default());
            for (i, sample) in samples.iter().take(len).enumerate() {
                let w = 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / len as f32).cos();
                self.buffer[i].re = sample[ch] * w;
            }
            self.fft
                .process_with_scratch(&mut self.buffer, &mut self.scratch);
            for (i, power) in self.power[ch].iter_mut().enumerate() {
                let scale = if i == 0 || i == self.size * 2 {
                    1.0
                } else {
                    4.0
                };
                *power = self.buffer[i].norm_sqr() * scale / (sum * sum);
            }
        }
        for i in 0..self.power[2].len() {
            self.power[2][i] = (self.power[0][i] + self.power[1][i]) * 0.5;
        }
    }
    pub fn frame(&self, power: &[Vec<f32>; 3], max: &[Vec<f32>; 3]) -> SpectrumFrame {
        let upper = 20000.0_f32.min(self.rate as f32 * 0.5);
        let frequencies: Vec<_> = (0..POINTS)
            .map(|i| 20.0 * (upper / 20.0).powf(i as f32 / (POINTS - 1) as f32))
            .collect();
        let project = |values: &Vec<f32>| {
            frequencies
                .iter()
                .enumerate()
                .map(|(i, &hz)| {
                    let bin = hz * (self.size * 4) as f32 / self.rate as f32;
                    let lo = bin.floor() as usize;
                    let hi = (lo + 1).min(values.len() - 1);
                    let mut p = values[lo] + (values[hi] - values[lo]) * bin.fract();
                    // Peak-preserving reduction at high frequencies (many bins per pixel).
                    let next = frequencies.get(i + 1).copied().unwrap_or(hz);
                    let edge = ((hz * next).sqrt() * (self.size * 4) as f32 / self.rate as f32)
                        .floor() as usize;
                    let prev = frequencies.get(i.wrapping_sub(1)).copied().unwrap_or(hz);
                    let start = ((hz * prev).sqrt() * (self.size * 4) as f32 / self.rate as f32)
                        .ceil() as usize;
                    for &v in values
                        .get(start..=edge.min(values.len() - 1))
                        .unwrap_or(&[])
                    {
                        p = p.max(v);
                    }
                    ((10.0 * p.max(1e-12).log10()).max(FLOOR_DB) * 100.0).round() / 100.0
                })
                .collect()
        };
        SpectrumFrame {
            sequence: 0,
            sample_rate: self.rate,
            fft_size: self.size,
            curves: std::array::from_fn(|c| project(&power[c])),
            maxima: std::array::from_fn(|c| project(&max[c])),
            frequencies,
            windows: 0,
            duration: 0.0,
            processing_ms: 0.0,
        }
    }
}

struct Published {
    frames: Vec<SpectrumFrame>,
    sources: Vec<String>,
    at: Instant,
}

pub struct Spectrum {
    settings: Mutex<Settings>,
    enabled: AtomicBool,
    epoch: AtomicU64,
    sequence: AtomicU64,
    latest: Mutex<Option<Published>>,
    context: Mutex<Option<TrackCapture>>,
    route_source: AtomicU64,
    routes: AtomicU64,
    error: Mutex<Option<String>>,
    dropped: AtomicU64,
    skipped: AtomicU64,
    worker_ns: AtomicU64,
    worker_max_ns: AtomicU64,
    track_read_ns: AtomicU64,
    pub selection_generation: Arc<AtomicU64>,
    pub selection_progress: AtomicU32,
    pub selection_lock: Mutex<()>,
}
impl Default for Spectrum {
    fn default() -> Self {
        Self {
            settings: Mutex::new(Settings::default()),
            enabled: AtomicBool::new(false),
            epoch: AtomicU64::new(1),
            sequence: AtomicU64::new(0),
            latest: Mutex::new(None),
            context: Mutex::new(None),
            route_source: AtomicU64::new(0),
            routes: AtomicU64::new(u64::MAX),
            error: Mutex::new(None),
            dropped: AtomicU64::new(0),
            skipped: AtomicU64::new(0),
            worker_ns: AtomicU64::new(0),
            worker_max_ns: AtomicU64::new(0),
            track_read_ns: AtomicU64::new(0),
            selection_generation: Arc::new(AtomicU64::new(0)),
            selection_progress: AtomicU32::new(0),
            selection_lock: Mutex::new(()),
        }
    }
}
impl Spectrum {
    pub fn configure(&self, settings: Settings) -> AppResult<()> {
        self.configure_tracks(settings, None)
    }
    pub fn configure_tracks(
        &self,
        settings: Settings,
        context: Option<TrackCapture>,
    ) -> AppResult<()> {
        if !valid_size(settings.fft_size) || !matches!(settings.smoothing_ms, 0 | 100 | 200 | 500) {
            return Err(error("Spectrum 설정이 올바르지 않습니다."));
        }
        if let Some(c) = &context {
            if !(1..=2).contains(&c.tracks.len())
                || c.tracks.iter().any(|(id, i)| {
                    c.plan
                        .document
                        .tracks
                        .get(*i)
                        .is_none_or(|t| &t.track_id != id)
                })
                || (c.tracks.len() == 2 && c.tracks[0].0 == c.tracks[1].0)
            {
                return Err(error("서로 다른 Track을 최대 두 개 선택하세요."));
            }
        }
        // Bit 32 distinguishes Audio-only capture from Master; low slots contain
        // MIDI-plan bus indices, NOT Arrangement indices (Audio tracks are absent
        // from MidiPlan). Empty slots use 0xffff and require no Synth bus read.
        let mut routes = if context.is_some() {
            !(1u64 << 32)
        } else {
            u64::MAX
        };
        if let Some(c) = &context {
            for (slot, (id, _)) in c.tracks.iter().enumerate() {
                if let Some(index) = c.plan.midi.track_ids.iter().position(|key| key == id) {
                    routes = (routes & !(0xffff << (slot * 16))) | ((index as u64) << (slot * 16));
                }
            }
        }
        self.route_source
            .store(context.as_ref().map_or(0, |c| c.source_id), Release);
        self.routes.store(routes, Release);
        *self
            .context
            .lock()
            .map_err(|_| error("Spectrum 상태 오류"))? = context;
        *self.error.lock().map_err(|_| error("Spectrum 상태 오류"))? = None;
        *self
            .settings
            .lock()
            .map_err(|_| error("Spectrum 상태 오류"))? = settings;
        self.enabled.store(settings.enabled, Release);
        self.epoch.fetch_add(1, AcqRel);
        *self
            .latest
            .lock()
            .map_err(|_| error("Spectrum 상태 오류"))? = None;
        Ok(())
    }
    pub fn snapshot(&self, after: u64) -> AppResult<SpectrumSnapshot> {
        let latest = self
            .latest
            .lock()
            .map_err(|_| error("Spectrum 상태 오류"))?;
        Ok(SpectrumSnapshot {
            enabled: self.enabled.load(Acquire),
            frame: latest
                .as_ref()
                .filter(|p| p.frames[0].sequence != after)
                .map(|p| p.frames[0].clone()),
            comparison: latest
                .as_ref()
                .filter(|p| p.frames[0].sequence != after)
                .and_then(|p| p.frames.get(1).cloned()),
            // Source IDs travel with their frames, even if configure races a poll.
            sources: if let Some(p) = latest.as_ref() {
                p.sources.clone()
            } else {
                self.context
                    .lock()
                    .map_err(|_| error("Spectrum 상태 오류"))?
                    .as_ref()
                    .map_or_else(Vec::new, |c| {
                        c.tracks.iter().map(|(id, _)| id.clone()).collect()
                    })
            },
            error: self
                .error
                .lock()
                .map_err(|_| error("Spectrum 상태 오류"))?
                .clone(),
            processed: self.sequence.load(Relaxed),
            dropped_blocks: self.dropped.load(Relaxed),
            skipped_frames: self.skipped.load(Relaxed),
            worker_ms: self.worker_ns.load(Relaxed) as f64 / 1e6,
            worker_max_ms: self.worker_max_ns.load(Relaxed) as f64 / 1e6,
            track_read_ms: self.track_read_ns.load(Relaxed) as f64 / 1e6,
            age_ms: latest
                .as_ref()
                .map(|p| p.at.elapsed().as_secs_f64() * 1000.0),
            selection_progress: self.selection_progress.load(Relaxed) as f64 / 1000.0,
        })
    }
    pub fn cancel_selection(&self) {
        self.selection_generation.fetch_add(1, AcqRel);
    }
    pub fn tap(self: &Arc<Self>, rate: u32) -> AppResult<SpectrumTap> {
        let (producer, consumer) = RingBuffer::new(QUEUE);
        let stop = Arc::new(AtomicBool::new(false));
        let shared = self.clone();
        let stopped = stop.clone();
        self.epoch.fetch_add(1, AcqRel);
        let worker = thread::Builder::new()
            .name("minidaw-spectrum".into())
            .spawn(move || live(shared, stopped, consumer, rate))
            .map_err(|e| error("Spectrum worker 시작 실패").detail(e))?;
        Ok(SpectrumTap {
            shared: self.clone(),
            producer,
            stop,
            worker: Some(worker),
            block: Block {
                epoch: 0,
                number: 0,
                source_id: 0,
                samples: [[[0.0; 2]; 2]; BLOCK],
                positions: [u64::MAX; BLOCK],
            },
            used: 0,
        })
    }
}
#[derive(Clone, Copy)]
struct Block {
    epoch: u64,
    number: u64,
    source_id: u64,
    samples: [[[f32; 2]; 2]; BLOCK],
    positions: [u64; BLOCK],
}
pub struct SpectrumTap {
    shared: Arc<Spectrum>,
    producer: Producer<Block>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    block: Block,
    used: usize,
}
impl SpectrumTap {
    /// Called once per callback; disabled capture performs no per-sample work.
    pub fn begin(&mut self) -> bool {
        self.begin_source(0)
    }
    pub fn begin_source(&mut self, source_id: u64) -> bool {
        let epoch = self.shared.epoch.load(Acquire);
        if epoch != self.block.epoch || source_id != self.block.source_id {
            self.used = 0;
            self.block.epoch = epoch;
            self.block.source_id = source_id;
        }
        if !self.shared.enabled.load(Acquire) {
            self.used = 0;
            return false;
        }
        true
    }
    #[inline]
    pub fn push(&mut self, sample: [f32; 2]) {
        self.push_tracks([sample, [0.0; 2]], None);
    }
    pub fn tracks(&self, source_id: u64) -> [Option<usize>; 2] {
        if self.shared.route_source.load(Acquire) != source_id {
            return [None; 2];
        }
        let packed = self.shared.routes.load(Acquire);
        std::array::from_fn(|i| {
            let n = ((packed >> (i * 16)) & 0xffff) as usize;
            (n != 65535).then_some(n)
        })
    }
    pub fn is_master(&self) -> bool {
        self.shared.routes.load(Acquire) == u64::MAX
    }
    pub fn push_tracks(&mut self, samples: [[f32; 2]; 2], position: Option<usize>) {
        self.block.samples[self.used] = samples;
        self.block.positions[self.used] = position.map_or(u64::MAX, |p| p as u64);
        self.used += 1;
        if self.used == BLOCK {
            // Full queue is an analysis-only loss: never wait, retry or allocate.
            if self.producer.push(self.block).is_err() {
                self.shared.dropped.fetch_add(1, Relaxed);
            }
            self.block.number += 1;
            self.used = 0;
        }
    }
}
impl Drop for SpectrumTap {
    fn drop(&mut self) {
        // Renderer/output owners are destroyed on the non-RT output controller.
        self.stop.store(true, Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
struct LiveTrace {
    fft: SpectrumFft,
    ring: Vec<[f32; 2]>,
    input: Vec<[f32; 2]>,
    smooth: [Vec<f32>; 3],
    maxima: [Vec<f32>; 3],
    reader: Option<TimelineReader>,
    next: Option<usize>,
}
impl LiveTrace {
    fn new(size: usize, rate: u32, reader: Option<TimelineReader>) -> Self {
        let fft = SpectrumFft::new(size, rate).expect("validated settings");
        Self {
            ring: vec![[0.0; 2]; size],
            input: vec![[0.0; 2]; size],
            smooth: fft.power.clone(),
            maxima: fft.power.clone(),
            fft,
            reader,
            next: None,
        }
    }
    fn sample(
        &mut self,
        captured: [f32; 2],
        position: u64,
        cancel: &super::reader::Cancel,
    ) -> AppResult<[f32; 2]> {
        let Some(reader) = &mut self.reader else {
            return Ok(captured);
        };
        if position == u64::MAX {
            self.next = None;
            return Ok([0.0; 2]);
        }
        let position = position as usize;
        if self.next != Some(position) {
            reader.seek(position, cancel.clone())?;
        }
        self.next = Some(position + 1);
        reader.read_frame()
    }
    fn analyze(&mut self, at: usize, alpha: f32) -> SpectrumFrame {
        for (i, p) in self.input.iter_mut().enumerate() {
            *p = self.ring[(at + i) % self.fft.size];
        }
        self.fft.analyze(&self.input);
        for ch in 0..3 {
            for i in 0..self.smooth[ch].len() {
                self.smooth[ch][i] =
                    alpha * self.smooth[ch][i] + (1.0 - alpha) * self.fft.power[ch][i];
                self.maxima[ch][i] = self.maxima[ch][i].max(self.fft.power[ch][i]);
            }
        }
        self.fft.frame(&self.smooth, &self.maxima)
    }
}
fn live(shared: Arc<Spectrum>, stop: Arc<AtomicBool>, mut rx: Consumer<Block>, rate: u32) {
    let mut epoch = 0;
    let mut settings = Settings::default();
    let mut traces = Vec::new();
    let mut source_id = None;
    let mut sources = Vec::new();
    let (mut at, mut filled, mut pending) = (0usize, 0usize, 0usize);
    let mut previous = None;
    let mut windows = 0;
    let mut cancel: super::reader::Cancel = Arc::new(|| false);
    // Same analysis cadence and window/smoothing arithmetic as the Master analyzer.
    let hop = ((rate as usize / 30 + BLOCK / 2) / BLOCK).max(1) * BLOCK;
    while !stop.load(Acquire) {
        let next_epoch = shared.epoch.load(Acquire);
        if epoch != next_epoch {
            let Ok(next) = shared.settings.lock().map(|s| *s) else {
                break;
            };
            settings = next;
            epoch = next_epoch;
            let Ok(context) = shared.context.lock().map(|c| c.clone()) else {
                break;
            };
            traces.clear();
            source_id = context.as_ref().map(|c| c.source_id);
            sources = context.as_ref().map_or_else(Vec::new, |c| {
                c.tracks.iter().map(|(id, _)| id.clone()).collect()
            });
            let generation = epoch;
            let shared_cancel = shared.clone();
            let stopping = stop.clone();
            cancel = Arc::new(move || {
                stopping.load(Acquire) || shared_cancel.epoch.load(Acquire) != generation
            });
            if settings.enabled {
                if let Some(c) = context {
                    let plan = if c.plan.rate == rate {
                        Ok(c.plan.clone())
                    } else {
                        c.plan.at_rate(rate)
                    };
                    match plan {
                        Ok(plan) => {
                            for (_, index) in &c.tracks {
                                let reader = (plan.document.tracks[*index].kind
                                    == crate::project::schema::TrackKind::Audio)
                                    .then(|| TimelineReader::for_track(plan.clone(), *index));
                                traces.push(LiveTrace::new(settings.fft_size, rate, reader));
                            }
                        }
                        Err(e) => {
                            if let Ok(mut slot) = shared.error.lock() {
                                *slot = Some(e.message);
                            }
                        }
                    }
                } else {
                    traces.push(LiveTrace::new(settings.fft_size, rate, None));
                }
            }
            at = 0;
            filled = 0;
            pending = 0;
            previous = None;
            windows = 0;
        }
        let began = Instant::now();
        for _ in 0..QUEUE {
            let Ok(block) = rx.pop() else {
                break;
            };
            if !settings.enabled
                || block.epoch != epoch
                || source_id.is_some_and(|id| id != block.source_id)
                || traces.is_empty()
            {
                continue;
            }
            if previous.is_some_and(|p| block.number != p + 1) {
                filled = 0;
                pending = 0;
            }
            previous = Some(block.number);
            let mut failed = false;
            for (i, samples) in block.samples.iter().enumerate() {
                for (slot, trace) in traces.iter_mut().enumerate() {
                    match trace.sample(samples[slot], block.positions[i], &cancel) {
                        Ok(pair) => trace.ring[at] = pair,
                        Err(e) => {
                            if !cancel() {
                                if let Ok(mut error) = shared.error.lock() {
                                    *error = Some(e.message);
                                }
                            }
                            failed = true;
                            break;
                        }
                    }
                }
                if failed {
                    break;
                }
                at = (at + 1) % settings.fft_size;
            }
            if failed {
                filled = 0;
                pending = 0;
                if !cancel() {
                    shared.dropped.fetch_add(1, Relaxed);
                }
                break;
            }
            filled = (filled + BLOCK).min(settings.fft_size);
            pending += BLOCK;
        }
        if settings.enabled && !traces.is_empty() && filled == settings.fft_size && pending >= hop {
            shared
                .track_read_ns
                .store(began.elapsed().as_nanos() as u64, Relaxed);
            let began = Instant::now();
            let alpha = if windows == 0 || settings.smoothing_ms == 0 {
                0.0
            } else {
                (-(pending as f32) / rate as f32 / (settings.smoothing_ms as f32 / 1000.0)).exp()
            };
            let mut frames: Vec<_> = traces.iter_mut().map(|t| t.analyze(at, alpha)).collect();
            if windows > 0 {
                shared
                    .skipped
                    .fetch_add((pending / hop).saturating_sub(1) as u64, Relaxed);
            }
            pending = 0;
            windows += 1;
            let ns = began.elapsed().as_nanos() as u64;
            shared.worker_ns.store(ns, Relaxed);
            shared.worker_max_ns.fetch_max(ns, Relaxed);
            if shared.epoch.load(Acquire) == epoch {
                if let Ok(mut latest) = shared.latest.lock() {
                    if shared.epoch.load(Acquire) != epoch {
                        continue;
                    }
                    let sequence = shared.sequence.fetch_add(1, Relaxed) + 1;
                    for frame in &mut frames {
                        frame.windows = windows;
                        frame.processing_ms = ns as f64 / 1e6;
                        frame.sequence = sequence;
                    }
                    *latest = Some(Published {
                        frames,
                        sources: sources.clone(),
                        at: Instant::now(),
                    });
                }
            }
        }
        thread::sleep(Duration::from_millis(if settings.enabled { 4 } else { 20 }));
    }
}

/// Whole-section power mean, bounded by FFT storage and the caller's streaming reader.
pub fn analyze_section(
    reader: &mut dyn super::streaming::FrameReader,
    start: usize,
    end: usize,
    rate: u32,
    size: usize,
    cancel: super::reader::Cancel,
    progress: &AtomicU32,
) -> AppResult<SpectrumFrame> {
    if start >= end {
        return Err(error("분석할 구간이 없습니다."));
    }
    let began = Instant::now();
    let mut fft = SpectrumFft::new(size, rate)?;
    let mut sum: [Vec<f64>; 3] = std::array::from_fn(|_| vec![0.0; size * 2 + 1]);
    let mut maxima = fft.power.clone();
    let mut input = vec![[0.0; 2]; size];
    reader.seek(start, cancel.clone())?;
    let mut read = 0;
    let mut windows = 0;
    while read < end - start {
        if cancel() {
            return Err(AppError::new(
                "cancelled",
                "Spectrum 분석이 취소되었습니다.",
            ));
        }
        let count = size.min(end - start - read);
        for pair in &mut input[..count] {
            *pair = reader.read_frame()?.map(|v| v.clamp(-1.0, 1.0));
        }
        fft.analyze(&input[..count]);
        for ch in 0..3 {
            for i in 0..sum[ch].len() {
                sum[ch][i] += fft.power[ch][i] as f64 * count as f64;
                maxima[ch][i] = maxima[ch][i].max(fft.power[ch][i]);
            }
        }
        read += count;
        windows += 1;
        progress.store(
            (read as f64 / (end - start) as f64 * 1000.0) as u32,
            Relaxed,
        );
    }
    let mean = std::array::from_fn(|ch| sum[ch].iter().map(|v| (v / read as f64) as f32).collect());
    let mut frame = fft.frame(&mean, &maxima);
    frame.windows = windows;
    frame.duration = read as f64 / rate as f64;
    frame.processing_ms = began.elapsed().as_secs_f64() * 1000.0;
    Ok(frame)
}
