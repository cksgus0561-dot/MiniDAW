use minidaw_lib::{
    audio::{
        decoder::{AudioData, FileInfo},
        resample::PlaybackReader,
        source::{AssetStorage, AudioAsset},
        streaming::FrameReader,
        stretch,
        timeline::{PlaybackPlan, TimelineReader},
    },
    project::{
        edit::{self, Clipboard, EditRequest},
        history::History,
        schema::*,
        stretch::Recipe,
        time::{time, Time},
    },
};
use std::{collections::HashMap, sync::Arc};
fn source() -> Arc<AudioAsset> {
    AudioAsset::memory(AudioData {
        info: FileInfo {
            name: "stretch-test.wav".into(),
            sample_rate: 48000,
            channels: 2,
            frames: 48000,
            duration: 1.,
            sanitized_samples: 0,
        },
        samples: (0..48000)
            .flat_map(|i| {
                let x = (i as f32 * 0.31).sin() * 0.2;
                [x, -x]
            })
            .collect(),
    })
}
fn document(a: &AudioAsset) -> Project {
    let mut p = Project::new();
    p.import(Asset {
        asset_id: id(),
        filename: a.info.name.clone(),
        path: PathReference {
            project_relative_path: Some("test.wav".into()),
            original_absolute_path: None,
        },
        metadata: AudioMetadata {
            sample_rate: 48000,
            channels: 2,
            source_frames: Frames(48000),
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
fn request(p: &Project, command: &str) -> EditRequest {
    serde_json::from_value(serde_json::json!({"command":command,"clipIds":p.clips().map(|c|&c.clip_id).collect::<Vec<_>>()})).unwrap()
}
fn apply(p: &Project, r: EditRequest) -> Project {
    edit::apply(p, &r, &mut Clipboard::default()).unwrap()
}
fn render(p: &Project, a: Arc<AudioAsset>, rate: u32) -> Vec<[f32; 2]> {
    let plan = PlaybackPlan::compile(
        Arc::new(p.clone()),
        HashMap::from([(p.assets[0].asset_id.clone(), a)]),
        rate,
    )
    .unwrap();
    let frames = plan.frames;
    let mut r = TimelineReader::new(plan);
    r.seek(0, Arc::new(|| false)).unwrap();
    (0..frames).map(|_| r.read_frame().unwrap()).collect()
}
fn pos(frame: u64) -> Position {
    Time::frames(frame, 48000).position().unwrap()
}
#[test]
fn stretch_editing_history_and_original_recipe_survive_roundtrip() {
    let asset = source();
    let original = document(&asset);
    let mut r = request(&original, "audio.stretch");
    r.stretch_frames = Some(Frames(72000));
    let mut p = apply(&original, r);
    let c = p.clips().next().unwrap();
    assert_eq!(c.source_end.0 - c.source_start.0, 72000);
    let recipe = minidaw_lib::project::stretch::recipe(c).unwrap().unwrap();
    assert_eq!(recipe.source_end.0, 48000);
    assert_eq!(original.assets, p.assets);
    let mut h = History::default();
    h.record(original.clone(), p.clone(), "Stretch");
    assert_eq!(h.undo().unwrap(), original);
    assert_eq!(h.redo().unwrap(), p);
    let parsed: Project = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
    parsed.validate().unwrap();
    assert_eq!(parsed, p);
    let mut r = request(&p, "audio.trim");
    r.source_start = Some(Frames(8000));
    r.source_end = Some(Frames(70000));
    p = apply(&p, r);
    let mut r = request(&p, "audio.move");
    r.delta = Some(pos(48000));
    p = apply(&p, r);
    let mut r = request(&p, "audio.fade");
    r.fade_in = Some(Frames(4000));
    r.fade_out = Some(Frames(3000));
    p = apply(&p, r);
    let mut r = request(&p, "audio.splitAtCursor");
    r.cursor = Some(pos(80000));
    let split = apply(&p, r);
    assert_eq!(split.clips().count(), 2);
    for rate in [44100,48000,96000] {assert_eq!(render(&p,asset.clone(),rate),render(&split,asset.clone(),rate));}
    let id = p.clips().next().unwrap().clip_id.clone();
    for _ in 0..12 {
        for frames in [40000, 62000] {
            let mut r = request(&p, "audio.stretch");
            r.clip_ids = vec![id.clone()];
            r.stretch_frames = Some(Frames(frames));
            p = apply(&p, r);
            assert_eq!(
                p.clips().next().unwrap().source_end.0 - p.clips().next().unwrap().source_start.0,
                frames
            );
        }
    }
    let c = p.clips().next().unwrap();
    let rr = minidaw_lib::project::stretch::recipe(c).unwrap().unwrap();
    assert_eq!(rr.source_start, recipe.source_start);
    assert_eq!(rr.source_end, recipe.source_end);
    let mut r = request(&p, "audio.stretch");
    r.stretch_frames = Some(Frames(47000));
    r.trim_side = Some(edit::TrimSide::Left);
    let old_end = time(&c.position, &p.musical_time)
        .plus(Time::frames(c.source_end.0 - c.source_start.0, 48000));
    p = apply(&p, r);
    let c = p.clips().next().unwrap();
    assert_eq!(
        old_end,
        time(&c.position, &p.musical_time).plus(Time::frames(47000, 48000))
    );
    let mut r = request(&p, "audio.gain");
    r.gain_db = Some(-6.0);
    p = apply(&p, r);
    let mut r = request(&p, "audio.muteEvents");
    r.clip_ids = vec![id];
    p = apply(&p, r);
    assert!(render(&p, asset, 48000).iter().all(|f| *f == [0.; 2]));
}
#[test]
fn stretch_split_fades_crossfade_and_transparent_unity() {
    let asset = source();
    let mut p = document(&asset);
    let dry = render(&p, asset.clone(), 48000);
    let mut r = request(&p, "audio.stretch");
    r.stretch_frames = Some(Frames(48000));
    p = apply(&p, r);
    assert_eq!(render(&p, asset.clone(), 48000), dry);
    let mut r = request(&p, "audio.fade");
    r.fade_in = Some(Frames(30000));
    r.fade_out = Some(Frames(10000));
    p = apply(&p, r);
    let mut r = request(&p, "audio.splitAtCursor");
    r.cursor = Some(pos(16000));
    p = apply(&p, r);
    let ids: Vec<_> = p.clips().map(|c| c.clip_id.clone()).collect();
    let mut r = request(&p, "audio.stretch");
    r.clip_ids = vec![ids[1].clone()];
    r.stretch_frames = Some(Frames(48000));
    p = apply(&p, r);
    p.validate().unwrap();
    let mut r = request(&p, "audio.crossfade");
    r.clip_ids = ids;
    p = apply(&p, r);
    assert!(render(&p, asset, 44100)
        .iter()
        .flatten()
        .all(|s| s.is_finite()));
}
#[test]
fn stretch_validation_and_cache_lifetime() {
    let asset = source();
    let r = Recipe {
        source_start: Frames(0),
        source_end: Frames(48000),
        output_frames: Frames(72000),
    };
    let rendered = stretch::prepare(asset.clone(), &r, "test-cache").unwrap();
    let reused = stretch::prepare(asset.clone(), &r, "test-cache").unwrap();
    assert!(Arc::ptr_eq(&rendered, &reused));
    let path = match &rendered.storage {
        AssetStorage::Rendered(f) => f.path.clone(),
        _ => panic!(),
    };
    assert!(path.exists());
    let mut reader = PlaybackReader::new(rendered.clone(), 48000).unwrap();
    reader.seek(111, Arc::new(|| false)).unwrap();
    let value = reader.read_frame().unwrap();
    reader.seek(111, Arc::new(|| false)).unwrap();
    assert_eq!(reader.read_frame().unwrap(), value);
    drop(rendered);
    drop(reused);
    assert!(path.exists());
    drop(reader);
    assert!(
        !path.exists(),
        "close cache handle before last lease deletion"
    );
    let p = document(&asset);
    for frames in [0, 1, 23999, 96001, u64::MAX] {
        let mut req = request(&p, "audio.stretch");
        req.stretch_frames = Some(Frames(frames));
        assert!(edit::apply(&p, &req, &mut Clipboard::default()).is_err());
    }
}

#[test]
fn short_regions_silence_and_exact_seek_keep_valid_bounded_output() {
    for frames in [1, 64, 511, 8000] {
        for silent in [true, false] {
            let data = AudioData {
                info: FileInfo {
                    name: "short.wav".into(),
                    sample_rate: 48000,
                    channels: 2,
                    frames,
                    duration: frames as f64 / 48000.,
                    sanitized_samples: 0,
                },
                samples: (0..frames)
                    .flat_map(|i| {
                        let v = if silent {
                            0.
                        } else {
                            0.2 * (i as f32 * 0.31).cos()
                        };
                        [v, 0.]
                    })
                    .collect(),
            };
            let asset = AudioAsset::memory(data);
            for target in [frames.div_ceil(2), frames * 2] {
                let rendered = stretch::prepare(
                    asset.clone(),
                    &Recipe {
                        source_start: Frames(0),
                        source_end: Frames(frames as u64),
                        output_frames: Frames(target as u64),
                    },
                    "short",
                )
                .unwrap();
                let mut reader = PlaybackReader::new(rendered, 48000).unwrap();
                reader.seek(0, Arc::new(|| false)).unwrap();
                let output: Vec<_> = (0..target).map(|_| reader.read_frame().unwrap()).collect();
                assert!(output.iter().flatten().all(|v| v.is_finite()));
                assert!(
                    output.iter().all(|v| v[1].abs() < 1e-6),
                    "L-only must not leak into R"
                );
                if silent {
                    assert!(output.iter().flatten().all(|v| v.abs() < 1e-6));
                } else {
                    assert!(
                        output.iter().any(|v| v[0].abs() > 1e-6),
                        "short audio must not become silence"
                    );
                }
                for i in [0, target / 2, target - 1] {
                    reader.seek(i, Arc::new(|| false)).unwrap();
                    assert_eq!(reader.read_frame().unwrap(), output[i]);
                }
            }
        }
    }
}
