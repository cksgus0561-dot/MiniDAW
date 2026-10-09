use minidaw_lib::{
    audio::{
        decoder::{AudioData, FileInfo},
        reader::Cancel,
        resample::PlaybackReader,
        source::{AssetStorage, AudioAsset},
        spectrum::{analyze_section, SpectrumFft, SpectrumFrame},
        streaming::FrameReader,
        timeline::{PlaybackPlan, TimelineReader},
    },
    error::AppResult,
    project::{schema::*, time::Time},
};
use std::{
    collections::HashMap,
    path::Path,
    sync::{
        atomic::{AtomicU32, Ordering::Relaxed},
        Arc,
    },
};

fn samples(rate: u32, n: usize, hz: f64, channels: [f32; 2]) -> Vec<[f32; 2]> {
    (0..n)
        .map(|i| {
            let s = (std::f64::consts::TAU * hz * i as f64 / rate as f64).sin() as f32;
            channels.map(|g| g * s)
        })
        .collect()
}
fn peak(frame: &SpectrumFrame, ch: usize, hz: f32) -> (f32, f32) {
    frame
        .frequencies
        .iter()
        .zip(&frame.curves[ch])
        .filter(|(f, _)| (**f / hz).ln().abs() < 0.1)
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(f, db)| (*f, *db))
        .unwrap()
}
#[test]
fn tones_levels_channels_and_fft_sizes() {
    let mut maximum_error = 0.0f32;
    for rate in [44100, 48000] {
        for size in [2048, 4096, 8192, 16384] {
            let mut fft = SpectrumFft::new(size, rate).unwrap();
            for hz in [100.0, 440.0, 1000.0, 5000.0, 10000.0] {
                for gains in [[0.5, 0.5], [0.5, 0.0], [0.0, 0.5], [0.5, -0.5]] {
                    fft.analyze(&samples(rate, size, hz, gains));
                    let frame = fft.frame(&fft.power, &fft.power);
                    for (ch, gain) in [
                        gains[0].abs(),
                        gains[1].abs(),
                        ((gains[0].powi(2) + gains[1].powi(2)) * 0.5).sqrt(),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        if gain == 0.0 {
                            assert!(frame.curves[ch].iter().all(|v| *v == -120.0));
                            continue;
                        }
                        let (f, db) = peak(&frame, ch, hz as f32);
                        let expected = 20.0 * gain.log10();
                        maximum_error = maximum_error.max((db - expected).abs());
                        assert!(
                            (db - expected).abs() < 0.22,
                            "{rate}/{size}/{hz}/{ch}: {db} vs {expected}"
                        );
                        assert!(
                            (f - hz as f32).abs()
                                <= (rate as f32 / (size * 4) as f32).max(hz as f32 * 0.012),
                            "frequency {f} vs {hz}"
                        );
                    }
                }
            }
            fft.analyze(&vec![[0.0; 2]; size]);
            assert!(fft
                .frame(&fft.power, &fft.power)
                .curves
                .iter()
                .flatten()
                .all(|v| *v == -120.0));
        }
    }
    println!("Maximum displayed sine level error across sample rates, sizes, tones and channels: {maximum_error:.4} dB");
}
struct Generated {
    at: usize,
    size: usize,
}
impl FrameReader for Generated {
    fn seek(&mut self, at: usize, _: Cancel) -> AppResult<()> {
        self.at = at;
        Ok(())
    }
    fn read_frame(&mut self) -> AppResult<[f32; 2]> {
        let hz = if self.at < self.size / 2 {
            1000.0
        } else {
            5000.0
        };
        let s = (std::f64::consts::TAU * hz * self.at as f64 / 48000.0).sin() as f32 * 0.5;
        self.at += 1;
        Ok([s, 0.0])
    }
}
#[test]
fn entire_selection_is_power_averaged_and_cancellable() {
    let mut reader = Generated {
        at: 0,
        size: 4096 * 24,
    };
    let progress = AtomicU32::new(0);
    let end = reader.size;
    let frame = analyze_section(
        &mut reader,
        0,
        end,
        48000,
        4096,
        Arc::new(|| false),
        &progress,
    )
    .unwrap();
    assert_eq!(frame.windows, 24);
    assert_eq!(progress.load(Relaxed), 1000);
    for hz in [1000.0, 5000.0] {
        assert!((peak(&frame, 0, hz).1 + 9.03).abs() < 0.2);
    }
    assert!(analyze_section(
        &mut reader,
        0,
        end,
        48000,
        4096,
        Arc::new(|| true),
        &progress
    )
    .is_err());
}
fn asset(rate: u32) -> Arc<AudioAsset> {
    AudioAsset::memory(AudioData {
        info: FileInfo {
            name: "sine.wav".into(),
            sample_rate: rate,
            channels: 2,
            frames: rate as usize * 2,
            duration: 2.0,
            sanitized_samples: 0,
        },
        samples: samples(rate, rate as usize * 2, 440.0, [0.5, 0.5])
            .into_iter()
            .flatten()
            .collect(),
    })
}
fn document(a: &AudioAsset) -> Project {
    let mut p = Project::new();
    p.import(Asset {
        asset_id: id(),
        filename: a.info.name.clone(),
        path: PathReference {
            project_relative_path: Some("tone.wav".into()),
            original_absolute_path: None,
        },
        metadata: AudioMetadata {
            sample_rate: a.info.sample_rate,
            channels: 2,
            source_frames: Frames(a.info.frames as u64),
            container: "wav".into(),
            codec: None,
        },
        fingerprint: Fingerprint {
            file_bytes: Frames(123),
            sampled_sha256: "a".repeat(64),
        },
        extensions: Extensions::new(),
    });
    p
}
fn analysis(p: &Project, a: Arc<AudioAsset>, start: usize, end: usize) -> SpectrumFrame {
    let plan = PlaybackPlan::compile(
        Arc::new(p.clone()),
        HashMap::from([(p.assets[0].asset_id.clone(), a)]),
        48000,
    )
    .unwrap();
    analyze_section(
        &mut TimelineReader::new(plan),
        start,
        end,
        48000,
        2048,
        Arc::new(|| false),
        &AtomicU32::new(0),
    )
    .unwrap()
}
#[test]
fn edited_timeline_trim_gain_fade_mute_overlap_and_range() {
    let a = asset(48000);
    let mut p = document(&a);
    let Clip::Audio(c) = &mut p.tracks[0].clips[0] else {
        panic!("Audio fixture")
    };
    c.source_start = Frames(12000);
    c.source_end = Frames(84000);
    c.position = Time::frames(48000, 48000).position().unwrap();
    let normal = peak(&analysis(&p, a.clone(), 48000, 120000), 0, 440.0).1;
    let Clip::Audio(c) = &mut p.tracks[0].clips[0] else {
        panic!("Audio fixture")
    };
    c.gain = 0.5;
    let gain = peak(&analysis(&p, a.clone(), 48000, 120000), 0, 440.0).1;
    assert!((gain - normal + 6.0206).abs() < 0.03);
    let Clip::Audio(c) = &mut p.tracks[0].clips[0] else {
        panic!("Audio fixture")
    };
    c.fade_in = Fade {
        source_frames: Frames(36000),
        curve: FadeCurve::Linear,
    };
    c.fade_out = c.fade_in.clone();
    let fade = peak(&analysis(&p, a.clone(), 48000, 120000), 0, 440.0).1;
    assert!(
        (fade - gain + 4.7712).abs() < 0.2,
        "fade average {fade} vs {gain}"
    );
    let early = peak(&analysis(&p, a.clone(), 48000, 60000), 0, 440.0).1;
    assert!(early < fade - 4.0, "Range analyzes its own edited segment");
    let mut duplicate = p.tracks[0].clips[0].clone();
    let Clip::Audio(c) = &mut duplicate else {
        panic!("Audio fixture")
    };
    c.clip_id = id();
    p.tracks[0].clips.push(duplicate);
    assert!(
        (peak(&analysis(&p, a.clone(), 48000, 120000), 0, 440.0).1 - fade - 6.0206).abs() < 0.03
    );
    for c in &mut p.tracks[0].clips {
        let Clip::Audio(c) = c else {
            panic!("Audio fixture")
        };
        c.mute = true;
    }
    assert!(analysis(&p, a, 48000, 120000)
        .curves
        .iter()
        .flatten()
        .all(|v| *v == -120.0));
}
#[test]
fn file_streaming_matches_memory_without_whole_pcm_loading() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/stereo-44100.wav");
    let file = AudioAsset::open_with_budget(&path, 0).unwrap();
    assert!(matches!(file.storage, AssetStorage::File(_)));
    let memory = AudioAsset::open(&path).unwrap();
    let run = |a| {
        analyze_section(
            &mut PlaybackReader::new(a, 48000).unwrap(),
            1700,
            280000,
            48000,
            4096,
            Arc::new(|| false),
            &AtomicU32::new(0),
        )
        .unwrap()
    };
    let a = run(file);
    let b = run(memory);
    assert_eq!(a.curves, b.curves);
    assert_eq!(a.maxima, b.maxima);
}
