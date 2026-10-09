//! Offline production-DSP measurements. Raw PCM is read by tests/stretch-quality.py.
use minidaw_lib::{
    audio::{
        decoder::{AudioData, FileInfo},
        resample::PlaybackReader,
        source::AudioAsset,
        stretch::render_to,
    },
    project::{schema::Frames, stretch::Recipe},
};
use serde_json::json;
use std::{path::Path, sync::Arc};
fn memory(samples: Vec<f32>, rate: u32) -> Arc<AudioAsset> {
    AudioAsset::memory(AudioData {
        info: FileInfo {
            name: "audit".into(),
            sample_rate: rate,
            channels: 2,
            frames: samples.len() / 2,
            duration: samples.len() as f64 / 2. / rate as f64,
            sanitized_samples: 0,
        },
        samples,
    })
}
fn main() {
    let directory = std::env::args().nth(1).expect("output directory");
    let directory = Path::new(&directory);
    std::fs::create_dir_all(directory).unwrap();
    let mut records = vec![];
    for rate in [44100u32, 48000, 96000] {
        for freq in [100., 440., 1000., 5000., 10000.] {
            let n = rate as usize * 2;
            let source = memory(
                (0..n)
                    .flat_map(|i| {
                        let x = (i as f64 * std::f64::consts::TAU * freq / rate as f64).sin()
                            as f32
                            * 0.2;
                        [x, -x]
                    })
                    .collect(),
                rate,
            );
            let ratios: &[f64] = if rate == 48000 {
                &[0.5, 0.75, 1., 1.25, 1.5, 2.]
            } else {
                &[0.5, 0.75, 1., 1.5, 2.]
            };
            for &ratio in ratios {
                let filename = format!("tone-{rate}-{freq}-{ratio}.f32");
                let file = directory.join(&filename);
                let _ = std::fs::remove_file(&file);
                let recipe = Recipe {
                    source_start: Frames(0),
                    source_end: Frames(n as u64),
                    output_frames: Frames((n as f64 * ratio).round() as u64),
                };
                let stats = render_to(source.clone(), &recipe, &file).unwrap();
                records.push(json!({"kind":"tone","file":filename,"rate":rate,"frequency":freq,"ratio":ratio,"stats":stats}));
            }
        }
    }
    // Sparse percussion and pitched decays: inspect onset displacement/smearing.
    let rate = 48000;
    let mut samples = vec![0f32; rate * 8];
    for beat in 0..6 {
        let at = rate / 2 + beat * rate / 2;
        for i in 0..rate / 3 {
            let t = i as f64 / rate as f64;
            let x = (0.5
                * (-t * 65.).exp()
                * (std::f64::consts::TAU * (80. * t + 0.7 * (1. - (-t * 20.).exp()))).sin()
                + 0.15 * (-t * 18.).exp() * (std::f64::consts::TAU * 660. * t).sin())
                as f32;
            samples[(at + i) * 2] += x;
            samples[(at + i) * 2 + 1] += x;
        }
    }
    let percussion = memory(samples, rate as u32);
    for ratio in [0.5, 0.75, 1., 1.5, 2.] {
        let file = format!("percussion-{ratio}.f32");
        let path = directory.join(&file);
        let _ = std::fs::remove_file(&path);
        let stats = render_to(
            percussion.clone(),
            &Recipe {
                source_start: Frames(0),
                source_end: Frames((rate * 4) as u64),
                output_frames: Frames((rate as f64 * 4. * ratio) as u64),
            },
            &path,
        )
        .unwrap();
        records
            .push(json!({"kind":"percussion","file":file,"rate":rate,"ratio":ratio,"stats":stats}));
    }
    if let Some(music) = std::env::args().nth(2) {
        let source = AudioAsset::open_with_budget(Path::new(&music), 0).unwrap();
        let rate = source.info.sample_rate;
        let start = (rate as usize * 90).min(source.info.frames / 3);
        let frames = (rate as usize * 12).min(source.info.frames - start);
        let mut reader = PlaybackReader::new(source.clone(), rate).unwrap();
        reader.seek(start, Arc::new(|| false)).unwrap();
        let samples: Vec<_> = (0..frames)
            .flat_map(|_| reader.read_frame().unwrap())
            .collect();
        let resident = memory(samples, rate);
        for ratio in [0.5, 0.75, 1., 1.25, 1.5, 2.] {
            let mut recipe = Recipe {
                source_start: Frames(start as u64),
                source_end: Frames((start + frames) as u64),
                output_frames: Frames((frames as f64 * ratio).round() as u64),
            };
            let file = format!("music-{ratio}.f32");
            let path = directory.join(&file);
            let _ = std::fs::remove_file(&path);
            let stats = render_to(source.clone(), &recipe, &path).unwrap();
            let other = directory.join("memory-check.f32");
            let _ = std::fs::remove_file(&other);
            recipe.source_start = Frames(0);
            recipe.source_end = Frames(frames as u64);
            render_to(resident.clone(), &recipe, &other).unwrap();
            let identical = std::fs::read(&path).unwrap() == std::fs::read(&other).unwrap();
            std::fs::remove_file(other).unwrap();
            assert!(identical);
            records.push(json!({"kind":"music","file":file,"rate":rate,"ratio":ratio,"memoryStreamingIdentical":identical,"stats":stats}));
        }
    }
    std::fs::write(
        directory.join("renders.json"),
        serde_json::to_vec_pretty(&records).unwrap(),
    )
    .unwrap();
    println!("{} production renders complete", records.len());
}
