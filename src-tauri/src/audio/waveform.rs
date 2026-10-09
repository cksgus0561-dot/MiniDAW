use super::decoder::AudioData;
use crate::error::{AppError, AppResult};
use serde::Serialize;
use std::time::Instant;

const BASE: usize = 64;
type Peak = [f32; 2];

struct Level {
    stride: usize,
    channels: Vec<Vec<Peak>>,
}

pub struct WaveformCache {
    levels: Vec<Level>,
    pub build_ms: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WaveformView {
    pub clip_id: u64,
    pub start: f64,
    pub end: f64,
    pub channels: Vec<Vec<Peak>>,
}

fn merge(target: &mut Peak, other: Peak) {
    target[0] = target[0].min(other[0]);
    target[1] = target[1].max(other[1]);
}

impl WaveformCache {
    pub fn bytes(&self) -> usize {
        self.levels
            .iter()
            .flat_map(|l| &l.channels)
            .map(|c| c.capacity() * size_of::<Peak>())
            .sum()
    }
    pub fn build(audio: &AudioData) -> Self {
        let started = Instant::now();
        let mut channels = vec![Vec::new(); audio.info.channels];
        for channel in &mut channels {
            channel.reserve(audio.info.frames.div_ceil(BASE));
        }
        for block in audio.samples.chunks(BASE * audio.info.channels) {
            for (c, channel) in channels.iter_mut().enumerate() {
                let mut peak = [f32::INFINITY, f32::NEG_INFINITY];
                for frame in block.chunks_exact(audio.info.channels) {
                    merge(&mut peak, [frame[c], frame[c]]);
                }
                channel.push(peak);
            }
        }
        let mut levels = vec![Level {
            stride: BASE,
            channels,
        }];
        while levels.last().unwrap().channels[0].len() > 1 {
            let previous = levels.last().unwrap();
            let channels = previous
                .channels
                .iter()
                .map(|channel| {
                    channel
                        .chunks(2)
                        .map(|pair| {
                            let mut peak = pair[0];
                            if pair.len() == 2 {
                                merge(&mut peak, pair[1]);
                            }
                            peak
                        })
                        .collect()
                })
                .collect();
            levels.push(Level {
                stride: previous.stride * 2,
                channels,
            });
        }
        Self {
            levels,
            build_ms: started.elapsed().as_secs_f64() * 1000.0,
        }
    }

    // Exact extrema for [start, end): raw boundary fragments + aligned cached
    // interior. Large windows cost O(log N) merges, never O(file length).
    fn peak(&self, audio: &AudioData, channel: usize, mut start: usize, end: usize) -> Peak {
        let mut result = [f32::INFINITY, f32::NEG_INFINITY];
        while start < end {
            if start.is_multiple_of(BASE) && end - start >= BASE {
                let level = self
                    .levels
                    .iter()
                    .rev()
                    .find(|level| start.is_multiple_of(level.stride) && level.stride <= end - start)
                    .unwrap();
                merge(&mut result, level.channels[channel][start / level.stride]);
                start += level.stride;
            } else {
                let sample = audio.samples[start * audio.info.channels + channel];
                merge(&mut result, [sample, sample]);
                start += 1;
            }
        }
        if result[0].is_infinite() {
            [0.0, 0.0]
        } else {
            result
        }
    }

    pub fn view(
        &self,
        audio: &AudioData,
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
        let rate = audio.info.sample_rate as f64;
        let start_frame = (start * rate).min(audio.info.frames as f64);
        let end_frame = (end * rate).min(audio.info.frames as f64);
        let channels = (0..audio.info.channels)
            .map(|channel| {
                (0..width)
                    .map(|x| {
                        let a = (start_frame + (end_frame - start_frame) * x as f64 / width as f64)
                            .floor() as usize;
                        let b = (start_frame
                            + (end_frame - start_frame) * (x + 1) as f64 / width as f64)
                            .ceil() as usize;
                        self.peak(
                            audio,
                            channel,
                            a.min(audio.info.frames),
                            b.min(audio.info.frames),
                        )
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

#[cfg(test)]
mod tests {
    use super::super::decoder::FileInfo;
    use super::*;

    #[test]
    fn hierarchy_matches_raw_extrema_at_all_boundaries_and_zoom_levels() {
        let frames = 12345;
        let audio = AudioData {
            info: FileInfo {
                name: "test".into(),
                sample_rate: 1000,
                channels: 2,
                frames,
                duration: frames as f64 / 1000.0,
                sanitized_samples: 0,
            },
            samples: (0..frames * 2)
                .map(|n| ((n * 197 % 1009) as f32 - 504.0) / 504.0)
                .collect(),
        };
        let cache = WaveformCache::build(&audio);
        for start in [0, 1, 63, 64, 127, 2048, frames - 1] {
            for len in [1, 33, 64, 513, frames] {
                let end = (start + len).min(frames);
                for channel in 0..2 {
                    let mut expected = [f32::INFINITY, f32::NEG_INFINITY];
                    for frame in start..end {
                        let sample = audio.samples[frame * 2 + channel];
                        merge(&mut expected, [sample, sample]);
                    }
                    assert_eq!(cache.peak(&audio, channel, start, end), expected);
                }
            }
        }
        assert!(cache.view(&audio, 1, f64::NAN, 1.0, 20).is_err());
        assert!(cache.view(&audio, 1, 0.0, 1.0, 10000).is_err());
        let view = cache
            .view(&audio, 1, 0.0, audio.info.duration, 200)
            .unwrap();
        assert_eq!(view.channels[0].len(), 200);
        assert_eq!(view.channels.len(), 2);
    }
}
