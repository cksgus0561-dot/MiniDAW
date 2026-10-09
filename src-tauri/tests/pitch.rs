use minidaw_lib::{
    audio::{
        decoder::{AudioData, FileInfo},
        source::AudioAsset,
        streaming::FrameReader,
        timeline::{PlaybackPlan, TimelineReader},
    },
    project::{
        edit::{self, Clipboard, EditRequest},
        history::History,
        schema::*,
        time::Time,
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
use minidaw_lib::project::pitch::Pitch;
fn shift(p: &Project, semitones: i16, cents: i16) -> Project {
    let mut r = request(p, "audio.pitch");
    r.pitch_shift = Some(Pitch { semitones, cents });
    apply(p, r)
}
fn duration(p: &Project) -> u64 {
    let c = p.clips().next().unwrap();
    c.source_end.0 - c.source_start.0
}
#[test]
fn pitch_preserves_length_original_and_history_roundtrip() {
    let a = source();
    let mut original = document(&a);
    let mut r = request(&original, "audio.trim");
    r.source_start = Some(Frames(7000));
    r.source_end = Some(Frames(47000));
    original = apply(&original, r);
    let mut r = request(&original, "audio.fade");
    r.fade_in = Some(Frames(6000));
    r.fade_out = Some(Frames(2000));
    original = apply(&original, r);
    let mut p = original.clone();
    for (semitones, cents) in [(12, 100), (-12, -100), (0, 1), (0, -37), (7, 23), (-3, -99)] {
        p = shift(&p, semitones, cents);
        assert_eq!(duration(&p), 40000);
        assert_eq!(p.assets, original.assets);
        assert_eq!(
            p.clips().next().unwrap().position,
            original.clips().next().unwrap().position
        );
        let mut h = History::default();
        h.record(original.clone(), p.clone(), "Pitch");
        assert_eq!(h.undo().unwrap(), original);
        assert_eq!(h.redo().unwrap(), p);
        let parsed: Project = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        parsed.validate().unwrap();
        assert_eq!(parsed, p);
    }
    let restored = shift(&p, 0, 0);
    assert_eq!(restored, original);
    for rate in [44100, 48000, 96000] {
        assert_eq!(
            render(&original, a.clone(), rate),
            render(&restored, a.clone(), rate)
        );
    }
}
#[test]
fn stretch_and_pitch_commute_reset_does_not_remove_stretch() {
    let a = source();
    let original = document(&a);
    let mut r = request(&original, "audio.stretch");
    r.stretch_frames = Some(Frames(72000));
    let stretched = apply(&original, r.clone());
    let pitch_last = shift(&stretched, 5, 37);
    let stretch_last = apply(&shift(&original, 5, 37), r);
    assert_eq!(pitch_last, stretch_last);
    assert_eq!(duration(&pitch_last), 72000);
    for rate in [44100, 48000, 96000] {
        assert_eq!(
            render(&pitch_last, a.clone(), rate),
            render(&stretch_last, a.clone(), rate)
        );
        assert_eq!(
            render(&shift(&pitch_last, 0, 0), a.clone(), rate),
            render(&stretched, a.clone(), rate)
        );
    }
    assert_eq!(shift(&pitch_last, 0, 0), stretched);
}
#[test]
fn pitch_trim_move_split_fade_crossfade_gain_mute() {
    let a = source();
    let mut p = shift(&document(&a), -5, 37);
    let mut r = request(&p, "audio.trim");
    r.source_start = Some(Frames(4000));
    r.source_end = Some(Frames(44000));
    p = apply(&p, r);
    let mut r = request(&p, "audio.move");
    r.delta = Some(pos(12000));
    p = apply(&p, r);
    let mut r = request(&p, "audio.fade");
    r.fade_in = Some(Frames(32000));
    r.fade_out = Some(Frames(10000));
    p = apply(&p, r);
    let mut r = request(&p, "audio.splitAtCursor");
    r.cursor = Some(pos(28000));
    let split = apply(&p, r);
    assert_eq!(split.clips().count(), 2);
    for rate in [44100, 48000, 96000] {
        assert_eq!(render(&p, a.clone(), rate), render(&split, a.clone(), rate));
    }
    p = apply(&split, request(&split, "audio.crossfade"));
    p.validate().unwrap();
    let mut r = request(&p, "audio.gain");
    r.gain_db = Some(-6.);
    p = apply(&p, r);
    assert!(render(&p, a.clone(), 48000)
        .iter()
        .flatten()
        .all(|v| v.is_finite()));
    p = apply(&p, request(&p, "audio.muteEvents"));
    assert!(render(&p, a, 48000).iter().all(|f| *f == [0.; 2]));
}
#[test]
fn split_envelope_context_restores_exactly_after_pitch_reset() {
    let a = source();
    let mut p = document(&a);
    let mut r = request(&p, "audio.trim");
    r.source_start = Some(Frames(5000));
    r.source_end = Some(Frames(45000));
    p = apply(&p, r);
    let mut r = request(&p, "audio.fade");
    r.fade_in = Some(Frames(28000));
    p = apply(&p, r);
    let mut r = request(&p, "audio.splitAtCursor");
    r.cursor = Some(pos(14000));
    p = apply(&p, r);
    let shifted = shift(&p, 7, -20);
    let restored = shift(&shifted, 0, 0);
    assert_eq!(restored, p);
    assert_eq!(render(&restored, a.clone(), 48000), render(&p, a, 48000));
}
#[test]
fn invalid_pitch_rejected_and_cache_separates_settings() {
    use minidaw_lib::{
        audio::{source::AssetStorage, stretch},
        project::stretch::Recipe,
    };
    let a = source();
    let p = document(&a);
    for (semitones, cents) in [(13, 0), (-13, 0), (0, 101), (0, -101), (i16::MAX, 0)] {
        let mut r = request(&p, "audio.pitch");
        r.pitch_shift = Some(Pitch { semitones, cents });
        assert!(edit::apply(&p, &r, &mut Clipboard::default()).is_err());
    }
    let r = Recipe {
        source_start: Frames(0),
        source_end: Frames(48000),
        output_frames: Frames(48000),
    };
    let x = stretch::prepare_pitched(a.clone(), &r, 537, "pitch-cache").unwrap();
    let y = stretch::prepare_pitched(a.clone(), &r, 537, "pitch-cache").unwrap();
    let z = stretch::prepare_pitched(a, &r, -537, "pitch-cache").unwrap();
    assert!(Arc::ptr_eq(&x, &y));
    assert!(!Arc::ptr_eq(&x, &z));
    let path = match &x.storage {
        AssetStorage::Rendered(f) => f.path.clone(),
        _ => panic!(),
    };
    drop(x);
    drop(y);
    assert!(!path.exists());
    let canceled = shift(&p, 1, -100);
    assert_eq!(
        render(&p, source(), 48000),
        render(&canceled, source(), 48000)
    );
}
#[test]
fn silence_and_one_channel_short_pitch_regions_are_finite() {
    use minidaw_lib::{
        audio::{resample::PlaybackReader, stretch},
        project::stretch::Recipe,
    };
    for n in [1, 64, 511, 8000] {
        for silent in [false, true] {
            let a = AudioAsset::memory(AudioData {
                info: FileInfo {
                    name: "short-pitch".into(),
                    sample_rate: 48000,
                    channels: 2,
                    frames: n,
                    duration: n as f64 / 48000.,
                    sanitized_samples: 0,
                },
                samples: (0..n)
                    .flat_map(|i| {
                        [
                            if silent {
                                0.
                            } else {
                                (i as f32 * 0.1).sin() * 0.2
                            },
                            0.,
                        ]
                    })
                    .collect(),
            });
            for cents in [-1300, 1, 1300] {
                let p = stretch::prepare_pitched(
                    a.clone(),
                    &Recipe {
                        source_start: Frames(0),
                        source_end: Frames(n as u64),
                        output_frames: Frames(n as u64),
                    },
                    cents,
                    "short-pitch",
                )
                .unwrap();
                let mut reader = PlaybackReader::new(p, 48000).unwrap();
                reader.seek(0, Arc::new(|| false)).unwrap();
                for _ in 0..n {
                    let f = reader.read_frame().unwrap();
                    assert!(f.iter().all(|x| x.is_finite()));
                    assert_eq!(f[1], 0.);
                    if silent {
                        assert_eq!(f, [0.; 2]);
                    }
                }
            }
        }
    }
}
