//! A bounded, single-producer/single-consumer atomic frame ring. One u64 stores
//! both f32 channel bit patterns (no precision conversion). No unsafe memory.
//! The worker publishes end after stores. The callback advances consumed only
//! AFTER reading; the worker never overwrites [consumed, end). A seek invalidates
//! the entire generation before reading anything from the new position.
use super::{
    decoder::FileInfo,
    reader::Cancel,
    resample::{output_frames, PlaybackReader},
    source::{AssetStorage, AudioAsset},
};
use crate::error::{AppError, AppResult};
use serde::Serialize;
use std::{
    path::PathBuf,
    sync::{
        atomic::{
            AtomicBool, AtomicU64,
            Ordering::{Acquire, Relaxed, Release},
        },
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub const CAPACITY: usize = 262_144; // 2 MiB; 5.46 s at 48 kHz (mono also packed).
const REFILL: usize = 8192;
pub trait FrameReader: Send {
    fn set_cycle(&mut self, _cycle: Option<super::cycle::CycleFrames>) {}
    fn wrap(&mut self, target: usize, cancel: Cancel) -> AppResult<()> {
        self.seek(target, cancel)
    }
    fn lookahead(&self) -> usize {
        CAPACITY
    }
    fn seek(&mut self, target: usize, cancel: Cancel) -> AppResult<()>;
    fn read_frame(&mut self) -> AppResult<[f32; 2]>;
}
impl FrameReader for PlaybackReader {
    fn seek(&mut self, target: usize, cancel: Cancel) -> AppResult<()> {
        PlaybackReader::seek(self, target, cancel)
    }
    fn read_frame(&mut self) -> AppResult<[f32; 2]> {
        PlaybackReader::read_frame(self)
    }
}
struct Shared {
    frames: Box<[AtomicU64]>,
    lookahead: usize,
    request: AtomicU64,
    target: AtomicU64,
    consumed: AtomicU64,
    ready: AtomicU64,
    end: AtomicU64,
    cancelled: AtomicBool,
    failed: AtomicBool,
    starvation: AtomicU64,
    priming: AtomicU64,
    refill_ns: AtomicU64,
    refill_max_ns: AtomicU64,
    refill_count: AtomicU64,
    stale: AtomicU64,
    error: Mutex<Option<AppError>>,
}
pub struct StreamPlayback {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    sample_rate: u32,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamSnapshot {
    pub mode: &'static str,
    pub pcm_resident_bytes: usize,
    pub buffered_frames: usize,
    pub capacity_frames: usize,
    pub buffer_bytes: usize,
    pub buffer_sample_rate: u32,
    pub starvation: u64,
    pub priming_callbacks: u64,
    pub refill_ms: f64,
    pub refill_max_ms: f64,
    pub refill_count: u64,
    pub discarded_requests: u64,
    pub generation: u64,
    pub ready_generation: u64,
    pub error: Option<AppError>,
}
impl StreamSnapshot {
    pub fn memory(bytes: usize) -> Self {
        Self {
            mode: "Memory",
            pcm_resident_bytes: bytes,
            buffered_frames: 0,
            capacity_frames: 0,
            buffer_bytes: 0,
            buffer_sample_rate: 0,
            starvation: 0,
            priming_callbacks: 0,
            refill_ms: 0.0,
            refill_max_ms: 0.0,
            refill_count: 0,
            discarded_requests: 0,
            generation: 0,
            ready_generation: 0,
            error: None,
        }
    }
}
impl Shared {
    fn obsolete(&self, generation: u64) -> bool {
        self.cancelled.load(Acquire) || self.request.load(Acquire) != generation
    }
    fn fail(&self, error: AppError) {
        if let Ok(mut slot) = self.error.lock() {
            *slot = Some(error);
        }
        self.failed.store(true, Release);
    }
}
impl StreamPlayback {
    pub fn new(path: PathBuf, info: FileInfo) -> AppResult<Self> {
        let rate = info.sample_rate;
        Self::for_output(
            Arc::new(AudioAsset {
                info,
                storage: AssetStorage::File(path),
            }),
            rate,
        )
    }
    pub fn for_output(asset: Arc<AudioAsset>, rate: u32) -> AppResult<Self> {
        let reader = PlaybackReader::new(asset.clone(), rate)?;
        let frames = output_frames(asset.info.frames, asset.info.sample_rate, rate);
        Self::from_reader(reader, rate, frames, 0)
    }
    pub fn from_reader(
        reader: impl FrameReader + 'static,
        rate: u32,
        frames: usize,
        start: usize,
    ) -> AppResult<Self> {
        let lookahead = reader.lookahead().clamp(1024, CAPACITY);
        let shared = Arc::new(Shared {
            lookahead,
            frames: (0..CAPACITY).map(|_| AtomicU64::new(0)).collect(),
            request: AtomicU64::new(1),
            target: AtomicU64::new(start as u64),
            consumed: AtomicU64::new(start as u64),
            ready: AtomicU64::new(0),
            end: AtomicU64::new(0),
            cancelled: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            starvation: AtomicU64::new(0),
            priming: AtomicU64::new(0),
            refill_ns: AtomicU64::new(0),
            refill_max_ns: AtomicU64::new(0),
            refill_count: AtomicU64::new(0),
            stale: AtomicU64::new(0),
            error: Mutex::new(None),
        });
        let state = shared.clone();
        let worker = thread::Builder::new()
            .name("minidaw-read-ahead".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run(&state, reader, frames)
                }));
                match result {
                    Ok(Err(error)) => state.fail(AppError::stream_read(error)),
                    Err(_) => state.fail(AppError::stream_worker()),
                    _ => (),
                }
            })
            .map_err(|e| AppError::stream_worker().detail(e))?;
        let result = Self {
            shared,
            worker: Some(worker),
            sample_rate: rate,
        };
        let started = Instant::now();
        while result.shared.ready.load(Acquire) == 0 && !result.failed() {
            if started.elapsed() > Duration::from_secs(5) {
                return Err(AppError::stream_read("첫 재생 버퍼 준비 시간 초과"));
            }
            thread::sleep(Duration::from_millis(1));
        }
        if let Some(e) = result.snapshot().error {
            return Err(e);
        }
        Ok(result)
    }
    /// Exactly one caller writes requests: the audio callback, in command order.
    pub fn request(&self, frame: usize) {
        self.shared.target.store(frame as u64, Relaxed);
        self.shared.consumed.store(frame as u64, Relaxed);
        self.shared.request.fetch_add(1, Release);
    }
    pub fn reset(&self) {
        if self.shared.target.load(Acquire) != 0 || self.shared.consumed.load(Acquire) != 0 {
            self.request(0);
        }
    }
    pub fn pair(&self, index: usize, next: usize) -> Option<[[f32; 2]; 2]> {
        let s = &self.shared;
        let generation = s.request.load(Acquire);
        if s.ready.load(Acquire) != generation
            || (index as u64) < s.consumed.load(Acquire)
            || next as u64 >= s.end.load(Acquire)
        {
            return None;
        }
        let unpack = |frame: u64| {
            [
                f32::from_bits(frame as u32),
                f32::from_bits((frame >> 32) as u32),
            ]
        };
        Some([
            unpack(s.frames[index % CAPACITY].load(Relaxed)),
            unpack(s.frames[next % CAPACITY].load(Relaxed)),
        ])
    }
    pub fn consumed(&self, frame: usize) {
        self.shared.consumed.store(frame as u64, Release);
    }
    pub fn missing(&self) {
        let s = &self.shared;
        if s.ready.load(Acquire) == s.request.load(Acquire) {
            s.starvation.fetch_add(1, Relaxed);
        } else {
            s.priming.fetch_add(1, Relaxed);
        }
    }
    pub fn failed(&self) -> bool {
        self.shared.failed.load(Acquire)
    }
    pub fn snapshot(&self) -> StreamSnapshot {
        let s = &self.shared;
        let generation = s.request.load(Acquire);
        let ready = s.ready.load(Acquire);
        StreamSnapshot {
            mode: "Streaming",
            pcm_resident_bytes: 0,
            buffered_frames: if generation == ready {
                s.end
                    .load(Acquire)
                    .saturating_sub(s.consumed.load(Acquire))
                    .min(CAPACITY as u64) as usize
            } else {
                0
            },
            capacity_frames: CAPACITY,
            buffer_bytes: CAPACITY * 8,
            buffer_sample_rate: self.sample_rate,
            starvation: s.starvation.load(Relaxed),
            priming_callbacks: s.priming.load(Relaxed),
            refill_ms: s.refill_ns.load(Relaxed) as f64 / 1e6,
            refill_max_ms: s.refill_max_ns.load(Relaxed) as f64 / 1e6,
            refill_count: s.refill_count.load(Relaxed),
            discarded_requests: s.stale.load(Relaxed),
            generation,
            ready_generation: ready,
            error: s.error.lock().ok().and_then(|e| e.clone()),
        }
    }
}
impl Drop for StreamPlayback {
    fn drop(&mut self) {
        // AudioService retains sources until the callback has released its Arc.
        self.shared.cancelled.store(true, Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn run(s: &Arc<Shared>, mut reader: impl FrameReader, frames: usize) -> AppResult<()> {
    let mut generation = 0;
    let mut position = 0;
    loop {
        if s.cancelled.load(Acquire) {
            return Ok(());
        }
        let requested = s.request.load(Acquire);
        if requested != generation {
            generation = requested;
            position = s.target.load(Acquire) as usize;
            if s.obsolete(generation) {
                continue;
            }
            s.ready.store(0, Release);
            let shared = s.clone();
            let cancel: Cancel = Arc::new(move || shared.obsolete(generation));
            let started = Instant::now();
            let result = reader.seek(position, cancel);
            if s.obsolete(generation) {
                s.stale.fetch_add(1, Relaxed);
                continue;
            }
            result?;
            s.end.store(position as u64, Release);
            s.refill_max_ns
                .fetch_max(started.elapsed().as_nanos() as u64, Relaxed);
        }
        if position >= frames {
            s.ready.store(generation, Release);
            thread::sleep(Duration::from_millis(2));
            continue;
        }
        let limit = (s.consumed.load(Acquire) as usize)
            .saturating_add(s.lookahead)
            .min(frames);
        if position >= limit {
            thread::sleep(Duration::from_millis(2));
            continue;
        }
        let started = Instant::now();
        let batch_end = (position + REFILL).min(limit);
        while position < batch_end && !s.obsolete(generation) {
            let result = reader.read_frame();
            if s.obsolete(generation) {
                break;
            }
            let [left, right] = result?;
            s.frames[position % CAPACITY].store(
                left.to_bits() as u64 | ((right.to_bits() as u64) << 32),
                Relaxed,
            );
            position += 1;
        }
        if s.obsolete(generation) {
            s.stale.fetch_add(1, Relaxed);
            continue;
        }
        s.end.store(position as u64, Release);
        s.ready.store(generation, Release);
        let ns = started.elapsed().as_nanos() as u64;
        s.refill_ns.store(ns, Relaxed);
        s.refill_max_ns.fetch_max(ns, Relaxed);
        s.refill_count.fetch_add(1, Relaxed);
    }
}
