//! Shared immutable assets and independent playback instances. A future second
//! clip can create another AudioSource from the same asset without sharing cursors.
use super::{
    decoder::{AudioData, FileInfo},
    reader::AudioReader,
    streaming::{StreamPlayback, StreamSnapshot},
};
use crate::error::{AppError, AppResult};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

/// A residency policy, NOT a file acceptance limit. Larger/underestimated files
/// switch to streaming. Peak caches have their own budget.
pub const MEMORY_PCM_BYTES: usize = 64 * 1024 * 1024;
pub enum AssetStorage {
    Memory(AudioData),
    File(PathBuf),
    Rendered(Arc<super::stretch::RenderedFile>),
}
pub struct AudioAsset {
    pub info: FileInfo,
    pub storage: AssetStorage,
}
impl AudioAsset {
    pub fn open(path: &Path) -> AppResult<Arc<Self>> {
        Self::open_with_budget(path, MEMORY_PCM_BYTES)
    }
    pub fn open_with_budget(path: &Path, budget: usize) -> AppResult<Arc<Self>> {
        let mut reader = AudioReader::open(path, None)?;
        let info = reader.info.clone();
        let bytes = info.frames.saturating_mul(info.channels).saturating_mul(4);
        if bytes <= budget {
            let mut samples = Vec::new();
            samples.try_reserve_exact(bytes / 4).map_err(|e| {
                AppError::new("memory", "오디오를 불러올 메모리가 부족합니다.").detail(e)
            })?;
            while let Some(chunk) = reader.read_chunk()? {
                if samples.len().saturating_add(chunk.samples.len()) > budget / 4 {
                    return Ok(Arc::new(Self {
                        info,
                        storage: AssetStorage::File(path.to_owned()),
                    }));
                }
                samples
                    .try_reserve_exact(chunk.samples.len())
                    .map_err(|e| {
                        AppError::new("memory", "오디오를 불러올 메모리가 부족합니다.").detail(e)
                    })?;
                samples.extend_from_slice(&chunk.samples);
            }
            if samples.is_empty() {
                return Err(super::reader::decode_error(
                    "재생할 오디오 샘플이 없습니다.",
                ));
            }
            let mut info = reader.info;
            info.frames = samples.len() / info.channels;
            info.duration = info.frames as f64 / info.sample_rate as f64;
            return Ok(Self::memory(AudioData { info, samples }));
        }
        // Validate the first packet before replacing a playable current source.
        if reader.read_chunk()?.is_none() {
            return Err(super::reader::decode_error(
                "재생할 오디오 샘플이 없습니다.",
            ));
        }
        Ok(Arc::new(Self {
            info,
            storage: AssetStorage::File(path.to_owned()),
        }))
    }
    pub fn memory(audio: AudioData) -> Arc<Self> {
        Arc::new(Self {
            info: audio.info.clone(),
            storage: AssetStorage::Memory(audio),
        })
    }
}

pub struct AudioSource {
    pub timeline: Option<Arc<super::timeline::PlaybackPlan>>,
    pub asset: Arc<AudioAsset>,
    pub stream: Option<StreamPlayback>,
    pub playback_rate: u32,
    pub playback_frames: usize,
    pub cycle: Option<super::cycle::CycleFrames>,
}
impl AudioSource {
    pub fn new(asset: Arc<AudioAsset>) -> AppResult<Arc<Self>> {
        let rate = asset.info.sample_rate;
        Self::for_output(asset, rate)
    }
    /// Must be prepared on a control/loader thread for the actual device rate.
    pub fn for_output(asset: Arc<AudioAsset>, rate: u32) -> AppResult<Arc<Self>> {
        let stream =
            if rate == asset.info.sample_rate && matches!(asset.storage, AssetStorage::Memory(_)) {
                None
            } else {
                Some(StreamPlayback::for_output(asset.clone(), rate)?)
            };
        let playback_frames =
            super::resample::output_frames(asset.info.frames, asset.info.sample_rate, rate);
        Ok(Arc::new(Self {
            timeline: None,
            cycle: None,
            asset,
            stream,
            playback_rate: rate,
            playback_frames,
        }))
    }
    pub fn memory(audio: AudioData) -> Arc<Self> {
        Arc::new(Self {
            timeline: None,
            cycle: None,
            playback_rate: audio.info.sample_rate,
            playback_frames: audio.info.frames,
            asset: AudioAsset::memory(audio),
            stream: None,
        })
    }
    // Callback-facing API: bounded RAM reads and atomics only. No reader, path,
    // decoder or waveform operation is reachable through these methods.
    pub fn pair(&self, index: usize, next: usize) -> Option<[[f32; 2]; 2]> {
        if let Some(stream) = &self.stream {
            return stream.pair(index, next);
        }
        match &self.asset.storage {
            AssetStorage::Memory(audio) => {
                let ch = audio.info.channels;
                let frame = |i| [audio.samples[i * ch], audio.samples[i * ch + ch - 1]];
                Some([frame(index), frame(next)])
            }
            AssetStorage::File(_) | AssetStorage::Rendered(_) => {
                self.stream.as_ref()?.pair(index, next)
            }
        }
    }
    pub fn seek(&self, frame: usize) {
        if let Some(s) = &self.stream {
            s.request(frame);
        }
    }
    pub fn reset(&self) {
        if let Some(s) = &self.stream {
            s.reset();
        }
    }
    pub fn consumed(&self, frame: usize) {
        if let Some(s) = &self.stream {
            s.consumed(frame);
        }
    }
    pub fn missing(&self) {
        if let Some(s) = &self.stream {
            s.missing();
        }
    }
    pub fn failed(&self) -> bool {
        self.stream.as_ref().is_some_and(StreamPlayback::failed)
    }
    pub fn snapshot(&self) -> StreamSnapshot {
        let mut snapshot = self.stream.as_ref().map_or_else(
            || StreamSnapshot::memory(self.asset.info.frames * self.asset.info.channels * 4),
            StreamPlayback::snapshot,
        );
        if let Some(plan) = &self.timeline {
            snapshot.mode = if plan.streaming() {
                "Streaming"
            } else {
                "Memory"
            };
            snapshot.pcm_resident_bytes = plan.resident_bytes();
        } else if matches!(self.asset.storage, AssetStorage::Memory(_)) {
            snapshot.mode = "Memory";
            snapshot.pcm_resident_bytes = self.asset.info.frames * self.asset.info.channels * 4;
        }
        snapshot
    }
    pub fn timeline(
        plan: Arc<super::timeline::PlaybackPlan>,
        start: usize,
    ) -> AppResult<Arc<Self>> {
        let info = FileInfo {
            name: if plan.document.midi_clips().next().is_some() {
                "Project Timeline"
            } else {
                "Audio Timeline"
            }
            .into(),
            sample_rate: plan.rate,
            channels: 2,
            frames: plan.frames,
            duration: plan.frames as f64 / plan.rate as f64,
            sanitized_samples: 0,
        };
        let cycle = super::cycle::CycleFrames::compile(&plan.document, plan.rate)?;
        let stream = StreamPlayback::from_reader(
            super::cycle::CycleReader::new(
                super::timeline::TimelineReader::new(plan.clone()),
                cycle,
            ),
            plan.rate,
            if cycle.is_some() {
                usize::MAX / 2
            } else {
                plan.frames
            },
            start.min(plan.frames),
        )?;
        Ok(Arc::new(Self {
            cycle,
            asset: AudioAsset::memory(AudioData {
                info,
                samples: vec![],
            }),
            playback_rate: plan.rate,
            playback_frames: plan.frames,
            stream: Some(stream),
            timeline: Some(plan),
        }))
    }
    pub fn at_rate(&self, rate: u32) -> AppResult<Arc<Self>> {
        match &self.timeline {
            Some(plan) => Self::timeline(plan.at_rate(rate)?, 0),
            None => Self::for_output(self.asset.clone(), rate),
        }
    }
    pub fn seek_if_needed(&self, frame: usize) {
        if frame < self.playback_frames && self.pair(frame, frame).is_none() {
            self.seek(frame);
        }
    }
    pub fn timeline_frame(&self, frame: usize) -> usize {
        self.cycle.map_or(frame, |c| c.position(frame))
    }
}
