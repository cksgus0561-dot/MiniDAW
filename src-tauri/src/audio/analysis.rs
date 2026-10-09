//! Progressive bounded peak pyramid. The analysis worker has its own decoder and
//! drops each PCM chunk immediately. All locks and detail reads are non-RT.
use super::{
    reader::{AudioReader, Cancel},
    source::{AssetStorage, AudioAsset},
    waveform::{WaveformCache, WaveformView},
};
use crate::error::{AppError, AppResult};
use serde::Serialize;
use std::{
    sync::{
        atomic::{
            AtomicBool, AtomicU64,
            Ordering::{Acquire, Relaxed, Release},
        },
        Arc, Mutex, RwLock,
    },
    thread::{self, JoinHandle},
    time::Instant,
};
const MAX_BINS: usize = 262_144;
const DETAIL_FRAMES: usize = 262_144;
type Peak = [f32; 2];
fn merge(a: Peak, b: Peak) -> Peak {
    [a[0].min(b[0]), a[1].max(b[1])]
}
struct Pyramid {
    levels: Vec<Vec<[Peak; 2]>>,
    count: usize,
    base: usize,
}
impl Pyramid {
    fn new(frames: usize) -> AppResult<Self> {
        let base = frames
            .div_ceil(MAX_BINS)
            .max(64)
            .checked_next_power_of_two()
            .ok_or_else(|| AppError::waveform_read("peak base overflow"))?;
        let mut count = frames.div_ceil(base);
        let mut levels = Vec::new();
        loop {
            let mut level = Vec::new();
            level
                .try_reserve_exact(count)
                .map_err(AppError::waveform_read)?;
            level.resize(count, [[0.0; 2]; 2]);
            levels.push(level);
            if count <= 1 {
                break;
            }
            count = count.div_ceil(2);
        }
        Ok(Self {
            levels,
            count: 0,
            base,
        })
    }
    fn push(&mut self, peak: [Peak; 2]) -> AppResult<()> {
        if self.count >= self.levels[0].len() {
            return Err(AppError::waveform_read(
                "peak capacity exceeded verified timeline",
            ));
        }
        let mut index = self.count;
        self.levels[0][index] = peak;
        for level in 1..self.levels.len() {
            let left = index & !1;
            let a = self.levels[level - 1][left];
            let b = if index & 1 == 1 {
                self.levels[level - 1][index]
            } else {
                a
            };
            index /= 2;
            self.levels[level][index] = [merge(a[0], b[0]), merge(a[1], b[1])];
        }
        self.count += 1;
        Ok(())
    }
    fn peak(&self, channel: usize, start: usize, end: usize) -> Peak {
        let mut a = start / self.base;
        let b = end.div_ceil(self.base).min(self.count);
        let mut peak = [f32::INFINITY, f32::NEG_INFINITY];
        while a < b {
            let level = (0..self.levels.len())
                .rev()
                .find(|l| a.is_multiple_of(1 << l) && (1 << l) <= b - a)
                .unwrap_or(0);
            peak = merge(peak, self.levels[level][a >> level][channel]);
            a += 1 << level;
        }
        if peak[0].is_infinite() {
            [0.0, 0.0]
        } else {
            peak
        }
    }
}
struct Progress {
    pyramid: RwLock<Pyramid>,
    cancel: AtomicBool,
    frames: AtomicU64,
    version: AtomicU64,
    done: AtomicBool,
    build_ns: AtomicU64,
    bytes: usize,
    error: Mutex<Option<AppError>>,
}
pub struct Analysis {
    memory: Option<WaveformCache>,
    progress: Option<Arc<Progress>>,
    worker: Option<JoinHandle<()>>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WaveformSnapshot {
    pub version: u64,
    pub progress: f64,
    pub complete: bool,
    pub bytes: usize,
    pub build_ms: f64,
    pub error: Option<AppError>,
}
impl Analysis {
    pub fn new(asset: &Arc<AudioAsset>) -> AppResult<Self> {
        if let AssetStorage::Memory(audio) = &asset.storage {
            return Ok(Self {
                memory: Some(WaveformCache::build(audio)),
                progress: None,
                worker: None,
            });
        }
        let pyramid = Pyramid::new(asset.info.frames)?;
        let bytes = pyramid
            .levels
            .iter()
            .map(|l| l.capacity() * size_of::<[Peak; 2]>())
            .sum();
        let progress = Arc::new(Progress {
            pyramid: RwLock::new(pyramid),
            cancel: AtomicBool::new(false),
            frames: AtomicU64::new(0),
            version: AtomicU64::new(1),
            done: AtomicBool::new(false),
            build_ns: AtomicU64::new(0),
            bytes,
            error: Mutex::new(None),
        });
        let state = progress.clone();
        let source = asset.clone();
        let worker = thread::Builder::new()
            .name("minidaw-wave-analysis".into())
            .spawn(move || {
                let started = Instant::now();
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    analyze(&source, &state)
                }));
                if !state.cancel.load(Acquire) {
                    let error = match result {
                        Ok(Ok(())) => None,
                        Ok(Err(e)) if e.code == "waveform_read" => Some(e),
                        Ok(Err(e)) => Some(AppError::waveform_read(format!(
                            "analysis / {}: {e}",
                            e.code
                        ))),
                        Err(_) => Some(AppError::waveform_read("분석 작업 오류")),
                    };
                    if let Ok(mut slot) = state.error.lock() {
                        *slot = error;
                    }
                    state
                        .build_ns
                        .store(started.elapsed().as_nanos() as u64, Relaxed);
                    state.done.store(true, Release);
                    state.version.fetch_add(1, Release);
                }
            })
            .map_err(AppError::waveform_read)?;
        Ok(Self {
            memory: None,
            progress: Some(progress),
            worker: Some(worker),
        })
    }
    pub fn snapshot(&self, asset: &AudioAsset) -> WaveformSnapshot {
        if let Some(cache) = &self.memory {
            return WaveformSnapshot {
                version: 1,
                progress: 1.0,
                complete: true,
                bytes: cache.bytes(),
                build_ms: cache.build_ms,
                error: None,
            };
        }
        let p = self.progress.as_ref().unwrap();
        WaveformSnapshot {
            version: p.version.load(Acquire),
            progress: (p.frames.load(Relaxed) as f64 / asset.info.frames as f64).min(1.0),
            complete: p.done.load(Acquire),
            bytes: p.bytes,
            build_ms: p.build_ns.load(Relaxed) as f64 / 1e6,
            error: p.error.lock().ok().and_then(|e| e.clone()),
        }
    }
    pub fn view(
        &self,
        asset: &AudioAsset,
        clip_id: u64,
        start: f64,
        end: f64,
        width: usize,
    ) -> AppResult<WaveformView> {
        if !start.is_finite()
            || !end.is_finite()
            || end <= start
            || start < 0.0
            || width == 0
            || width > 4096
        {
            return Err(AppError::new(
                "waveform_range",
                "유효하지 않은 파형 표시 범위입니다.",
            ));
        }
        if let (Some(cache), AssetStorage::Memory(audio)) = (&self.memory, &asset.storage) {
            return cache.view(audio, clip_id, start, end, width);
        }
        let rate = asset.info.sample_rate as f64;
        let a = (start * rate).min(asset.info.frames as f64);
        let b = (end * rate).min(asset.info.frames as f64);
        let p = self.progress.as_ref().unwrap();
        let pyramid = p.pyramid.read().map_err(AppError::waveform_read)?;
        if b - a <= DETAIL_FRAMES as f64 && b - a < (width * pyramid.base) as f64 && b > a {
            drop(pyramid);
            return detail(
                asset,
                clip_id,
                start,
                end,
                width,
                a.floor() as usize,
                b.ceil() as usize,
            );
        }
        let channels = (0..asset.info.channels)
            .map(|ch| {
                (0..width)
                    .map(|x| {
                        let first = (a + (b - a) * x as f64 / width as f64).floor() as usize;
                        let last = (a + (b - a) * (x + 1) as f64 / width as f64).ceil() as usize;
                        pyramid.peak(ch, first, last)
                    })
                    .collect()
            })
            .collect();
        Ok(WaveformView {
            clip_id,
            start,
            end,
            channels,
        })
    }
}
impl Drop for Analysis {
    fn drop(&mut self) {
        if let Some(p) = &self.progress {
            p.cancel.store(true, Release);
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn analyze(asset: &AudioAsset, state: &Arc<Progress>) -> AppResult<()> {
    let AssetStorage::File(path) = &asset.storage else {
        return Ok(());
    };
    let shared = state.clone();
    let cancel: Cancel = Arc::new(move || shared.cancel.load(Acquire));
    let mut reader = AudioReader::for_asset(path, Some(cancel), &asset.info)?;
    let base = state.pyramid.read().map_err(AppError::waveform_read)?.base;
    let mut peak = [[f32::INFINITY, f32::NEG_INFINITY]; 2];
    let mut count = 0;
    let mut total = 0;
    let mut published = Instant::now();
    let mut batch = Vec::with_capacity(128);
    while !state.cancel.load(Acquire) {
        let Some(chunk) = reader.read_chunk()? else {
            break;
        };
        if chunk.start as u64 != total {
            return Err(AppError::waveform_read(format!(
                "analysis chunk discontinuity: expected={total}, actual={}",
                chunk.start
            )));
        }
        for frame in chunk.samples.chunks_exact(asset.info.channels) {
            peak[0] = merge(peak[0], [frame[0]; 2]);
            peak[1] = merge(peak[1], [frame[asset.info.channels - 1]; 2]);
            count += 1;
            total += 1;
            if count == base {
                batch.push(peak);
                peak = [[f32::INFINITY, f32::NEG_INFINITY]; 2];
                count = 0;
            }
        }
        if !batch.is_empty() {
            let mut cache = state.pyramid.write().map_err(AppError::waveform_read)?;
            for peak in batch.drain(..) {
                cache.push(peak)?;
            }
        }
        state.frames.store(total, Relaxed);
        if published.elapsed().as_millis() >= 250 {
            state.version.fetch_add(1, Release);
            published = Instant::now();
            thread::yield_now();
        }
    }
    if count > 0 {
        state
            .pyramid
            .write()
            .map_err(AppError::waveform_read)?
            .push(peak)?;
    }
    if !state.cancel.load(Acquire) && total != asset.info.frames as u64 {
        return Err(AppError::waveform_read(format!(
            "analysis EOF frame count: decoded={total}, verified={}",
            asset.info.frames
        )));
    }
    Ok(())
}
fn detail(
    asset: &AudioAsset,
    clip_id: u64,
    start: f64,
    end: f64,
    width: usize,
    first: usize,
    last: usize,
) -> AppResult<WaveformView> {
    let AssetStorage::File(path) = &asset.storage else {
        unreachable!()
    };
    let mut reader = AudioReader::for_asset(path, None, &asset.info)?;
    reader.seek(first)?;
    let mut samples = Vec::with_capacity((last - first) * asset.info.channels);
    while samples.len() / asset.info.channels < last - first {
        let Some(chunk) = reader.read_chunk()? else {
            break;
        };
        let remaining = (last - first) * asset.info.channels - samples.len();
        samples.extend_from_slice(&chunk.samples[..remaining.min(chunk.samples.len())]);
    }
    let rate = asset.info.sample_rate as f64;
    let a = (start * rate).min(asset.info.frames as f64);
    let b = (end * rate).min(asset.info.frames as f64);
    let available = samples.len() / asset.info.channels;
    let channels = (0..asset.info.channels)
        .map(|ch| {
            (0..width)
                .map(|x| {
                    // Use the same GLOBAL frame boundaries as the memory view. Subtracting
                    // seconds before rounding can shift integer boundaries by one sample.
                    let lo = ((a + (b - a) * x as f64 / width as f64).floor() as usize)
                        .saturating_sub(first)
                        .min(available);
                    let hi = ((a + (b - a) * (x + 1) as f64 / width as f64).ceil() as usize)
                        .saturating_sub(first)
                        .min(available);
                    let mut peak = [f32::INFINITY, f32::NEG_INFINITY];
                    for frame in lo..hi {
                        peak = merge(peak, [samples[frame * asset.info.channels + ch]; 2]);
                    }
                    if peak[0].is_infinite() {
                        [0.0, 0.0]
                    } else {
                        peak
                    }
                })
                .collect()
        })
        .collect();
    Ok(WaveformView {
        clip_id,
        start,
        end,
        channels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn progressive_pyramid_matches_aligned_raw_extrema_and_caps_memory() {
        let frames = 12345;
        let raw: Vec<f32> = (0..frames)
            .map(|i| ((i * 197 % 1009) as f32 - 504.0) / 504.0)
            .collect();
        let mut pyramid = Pyramid::new(frames).unwrap();
        for chunk in raw.chunks(pyramid.base) {
            let peak = chunk
                .iter()
                .fold([f32::INFINITY, f32::NEG_INFINITY], |p, &s| merge(p, [s; 2]));
            pyramid.push([peak, peak]).unwrap();
        }
        for a in [0, 64, 128, 1024, 4096] {
            for end in [64, 128, 512, 2048, frames] {
                let b = (a + end).min(frames);
                let expected = raw[a..b]
                    .iter()
                    .fold([f32::INFINITY, f32::NEG_INFINITY], |p, &s| merge(p, [s; 2]));
                assert_eq!(pyramid.peak(0, a, b), expected);
            }
        }
        for frames in [48000 * 1200, 48000 * 3600 * 24, 192000 * 3600 * 24] {
            let pyramid = Pyramid::new(frames).unwrap();
            let bytes: usize = pyramid
                .levels
                .iter()
                .map(|l| l.capacity() * size_of::<[Peak; 2]>())
                .sum();
            assert!(bytes <= 8 * 1024 * 1024 + 512);
        }
    }
}
