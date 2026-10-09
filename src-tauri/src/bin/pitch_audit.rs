//! Production f64 pitch/time DSP -> raw PCM for independent spectral measurement.
use minidaw_lib::{
    audio::{
        decoder::{AudioData, FileInfo},
        resample::PlaybackReader,
        source::AudioAsset,
        stretch::render_pitched_to,
    },
    project::{schema::Frames, stretch::Recipe},
};
use serde_json::json;
use std::{path::Path, sync::Arc};
fn memory(samples: Vec<f32>, rate: u32) -> Arc<AudioAsset> {
    AudioAsset::memory(AudioData {
        info: FileInfo {
            name: "pitch-audit".into(),
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
            for cents in [-1300, -1200, -700, -37, -1, 0, 1, 37, 500, 1200, 1300] {
                let filename = format!("tone-{rate}-{freq}-{cents}.f32");
                let file = directory.join(&filename);
                let _ = std::fs::remove_file(&file);
                let recipe = Recipe {
                    source_start: Frames(0),
                    source_end: Frames(n as u64),
                    output_frames: Frames(n as u64),
                };
                let stats = render_pitched_to(source.clone(), &recipe, cents, &file).unwrap();
                records.push(json!({"kind":"tone","file":filename,"rate":rate,"frequency":freq,"cents":cents,"ratio":1.,"stats":stats}));
            }
            if rate == 48000 && freq == 440. {
                for ratio in [0.5, 0.75, 1.5, 2.] {
                    for cents in [-1200, -37, 500, 1200] {
                        let file = format!("combined-{ratio}-{cents}.f32");
                        let path = directory.join(&file);
                        let _ = std::fs::remove_file(&path);
                        let stats = render_pitched_to(
                            source.clone(),
                            &Recipe {
                                source_start: Frames(0),
                                source_end: Frames(n as u64),
                                output_frames: Frames((n as f64 * ratio).round() as u64),
                            },
                            cents,
                            &path,
                        )
                        .unwrap();
                        records.push(json!({"kind":"tone","file":file,"rate":rate,"frequency":freq,"cents":cents,"ratio":ratio,"stats":stats}));
                    }
                }
            }
        }
    }
    // Out-of-band shifted tones must be filtered, not folded into audio.
    for rate in [44100u32, 48000, 96000] {
        let n = rate as usize * 2;
        let source = memory(
            (0..n)
                .flat_map(|i| {
                    let x = (i as f64 * std::f64::consts::TAU * 0.4).sin() as f32 * 0.2;
                    [x, -x]
                })
                .collect(),
            rate,
        );
        let file = format!("alias-{rate}.f32");
        let path = directory.join(&file);
        let _ = std::fs::remove_file(&path);
        let stats = render_pitched_to(
            source,
            &Recipe {
                source_start: Frames(0),
                source_end: Frames(n as u64),
                output_frames: Frames(n as u64),
            },
            1200,
            &path,
        )
        .unwrap();
        records.push(json!({"kind":"alias","file":file,"rate":rate,"frequency":rate as f64*0.4,"cents":1200,"ratio":1.,"stats":stats}));
    }
    // Percussion with decaying fundamentals/partials for transient and sideband checks.
    let rate = 48000usize;
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
    for cents in [-1200, -700, -37, 0, 37, 500, 1200] {
        let file = format!("percussion-{cents}.f32");
        let path = directory.join(&file);
        let _ = std::fs::remove_file(&path);
        let stats = render_pitched_to(
            percussion.clone(),
            &Recipe {
                source_start: Frames(0),
                source_end: Frames((rate * 4) as u64),
                output_frames: Frames((rate * 4) as u64),
            },
            cents,
            &path,
        )
        .unwrap();
        records.push(json!({"kind":"percussion","file":file,"rate":rate,"cents":cents,"ratio":1.,"stats":stats}));
    }
    if let Some(music) = std::env::args().nth(2) {
        let source = AudioAsset::open_with_budget(Path::new(&music), 0).unwrap();
        let rate = source.info.sample_rate;
        let start = (rate as usize * 90).min(source.info.frames / 3);
        let n = (rate as usize * 12).min(source.info.frames - start);
        let mut reader = PlaybackReader::new(source.clone(), rate).unwrap();
        reader.seek(start, Arc::new(|| false)).unwrap();
        let samples: Vec<_> = (0..n).flat_map(|_| reader.read_frame().unwrap()).collect();
        let resident = memory(samples, rate);
        for ratio in [1., 1.5] {
            for cents in [-1200, -700, -37, 0, 37, 500, 1200] {
                let mut recipe = Recipe {
                    source_start: Frames(start as u64),
                    source_end: Frames((start + n) as u64),
                    output_frames: Frames((n as f64 * ratio).round() as u64),
                };
                let file = format!("music-{ratio}-{cents}.f32");
                let path = directory.join(&file);
                let _ = std::fs::remove_file(&path);
                let stats = render_pitched_to(source.clone(), &recipe, cents, &path).unwrap();
                let other = directory.join("memory-check.f32");
                let _ = std::fs::remove_file(&other);
                recipe.source_start = Frames(0);
                recipe.source_end = Frames(n as u64);
                render_pitched_to(resident.clone(), &recipe, cents, &other).unwrap();
                let identical = std::fs::read(&path).unwrap() == std::fs::read(&other).unwrap();
                std::fs::remove_file(other).unwrap();
                assert!(identical);
                records.push(json!({"kind":"music","file":file,"rate":rate,"cents":cents,"ratio":ratio,"memoryStreamingIdentical":identical,"stats":stats}));
            }
        }
    }
    std::fs::write(
        directory.join("renders.json"),
        serde_json::to_vec_pretty(&records).unwrap(),
    )
    .unwrap();
    println!("{} production renders complete", records.len());
}
