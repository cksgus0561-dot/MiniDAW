use minidaw_lib::{
    audio::{
        decoder::{AudioData, FileInfo},
        resample::PlaybackReader,
        source::AudioAsset,
        streaming::FrameReader,
        timeline::{PlaybackPlan, TimelineReader},
    },
    project::{
        edit::{self, Clipboard, EditRequest},
        history::History,
        migrations,
        normalize::{NormalizeStrategy, Peak},
        schema::*,
        time::{time, Time},
    },
};
use std::{collections::HashMap, path::Path, sync::Arc};
fn source(rate: u32) -> Arc<AudioAsset> {
    let frames = rate as usize;
    AudioAsset::memory(AudioData {
        info: FileInfo {
            name: "synthetic.wav".into(),
            sample_rate: rate,
            channels: 2,
            frames,
            duration: 1.0,
            sanitized_samples: 0,
        },
        samples: (0..frames)
            .flat_map(|i| {
                let x = (i as f64 * 0.31).sin() as f32 * 0.2;
                [x, -x]
            })
            .collect(),
    })
}
fn document(asset: &AudioAsset) -> Project {
    let mut p = Project::new();
    p.import(Asset {
        asset_id: id(),
        filename: asset.info.name.clone(),
        path: PathReference {
            project_relative_path: Some("media.wav".into()),
            original_absolute_path: None,
        },
        metadata: AudioMetadata {
            sample_rate: asset.info.sample_rate,
            channels: 2,
            source_frames: Frames(asset.info.frames as u64),
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
fn pos(n: u64, rate: u32) -> Position {
    Time::frames(n, rate).position().unwrap()
}

#[test]
fn absolute_tick_move_and_sample_trim_roundtrips_do_not_drift() {
    let asset = source(44100);
    let mut p = document(&asset);
    p.musical_time.tempo_map[0].bpm = 137.3;
    let clip_id = p.clips().next().unwrap().clip_id.clone();
    let target = 1_234_567i64;
    let moved = time(
        &Position::Ticks {
            ticks: Signed(target),
        },
        &p.musical_time,
    )
    .position()
    .unwrap();
    let tick_at = |p: &Project, source_frame: u64| {
        ((time(&moved, &p.musical_time).seconds() + source_frame as f64 / 44100.0)
            * 137.3
            * 960000.0
            / 60.0)
            .round() as i64
    };
    for _ in 0..1000 {
        let mut r = request(&p, "audio.move");
        r.target_tick = Some(Signed(target + 1));
        r.anchor_clip_id = Some(clip_id.clone());
        p = apply(&p, r);
        let mut r = request(&p, "audio.move");
        r.target_tick = Some(Signed(target));
        r.anchor_clip_id = Some(clip_id.clone());
        p = apply(&p, r);
        assert_eq!(p.clips().next().unwrap().position, moved);
        let mut trim = request(&p, "audio.trim");
        trim.target_tick = Some(Signed(tick_at(&p, 123)));
        trim.trim_side = Some(edit::TrimSide::Left);
        p = apply(&p, trim);
        let c = p.clips().next().unwrap();
        assert_eq!(c.source_start.0, 123);
        assert_eq!(
            time(&c.position, &p.musical_time),
            time(&moved, &p.musical_time).plus(Time::frames(123, 44100))
        );
        // Preserve the exact rational remainder in a saved project, not a rounded ns on every edit.
        p = migrations::decode(&migrations::encode(&p).unwrap()).unwrap();
        let mut trim = request(&p, "audio.trim");
        trim.target_tick = Some(Signed(target));
        trim.trim_side = Some(edit::TrimSide::Left);
        p = apply(&p, trim);
        assert_eq!(p.clips().next().unwrap().position, moved);
        assert_eq!(p.clips().next().unwrap().source_start.0, 0);
    }
    let split_frame = 5432;
    let mut split = request(&p, "audio.splitAtCursor");
    split.cursor = Some(Position::Ticks {
        ticks: Signed(tick_at(&p, split_frame)),
    });
    p = apply(&p, split);
    let clips = p.clips().collect::<Vec<_>>();
    assert_eq!(clips.len(), 2);
    assert_eq!(clips[0].source_end.0, split_frame);
    assert_eq!(clips[1].source_start.0, split_frame);
    assert_eq!(
        time(&clips[1].position, &p.musical_time),
        time(&moved, &p.musical_time).plus(Time::frames(split_frame, 44100))
    );
}

#[test]
fn tick_move_group_preserves_offsets_and_audio_timebase() {
    let asset = source(48000);
    let mut p = document(&asset);
    let mut second = p.tracks[0].clips[0].audio().clone();
    second.clip_id = id();
    second.position = pos(1234, 48000);
    p.tracks[0].clips.push(Clip::Audio(second));
    let anchor = p.clips().next().unwrap().clip_id.clone();
    let mut r = request(&p, "audio.move");
    r.anchor_clip_id = Some(anchor);
    r.target_tick = Some(Signed(30001));
    let moved = apply(&p, r);
    let clips = moved.clips().collect::<Vec<_>>();
    assert_eq!(
        time(&clips[1].position, &moved.musical_time)
            .minus(time(&clips[0].position, &moved.musical_time)),
        Time::frames(1234, 48000)
    );
    let mut tempo = request(&moved, "project.tempo");
    tempo.bpm = Some(77.7);
    let changed = apply(&moved, tempo);
    assert_eq!(changed.tracks, moved.tracks);
}

#[test]
fn tick_trim_rejects_an_event_wholly_before_zero_without_panicking() {
    let asset = source(44100);
    let mut p = document(&asset);
    let Clip::Audio(c) = &mut p.tracks[0].clips[0] else {
        panic!("Audio fixture")
    };
    c.position = Position::Seconds {
        numerator: Signed(-2),
        denominator: 1,
    };
    let mut r = request(&p, "audio.trim");
    r.target_tick = Some(Signed(0));
    r.trim_side = Some(edit::TrimSide::Left);
    assert!(edit::apply(&p, &r, &mut Clipboard::default()).is_err());
}
#[test]
fn split_src_bit_exact_no_loss_duplicate_or_shift() {
    for rate in [44100, 48000] {
        let a = source(rate);
        let p = document(&a);
        for output in [44100, 48000] {
            let before = render(&p, a.clone(), output);
            for at in [1, 17, 147, 10001, rate as u64 - 1] {
                let mut r = request(&p, "audio.splitAtCursor");
                r.cursor = Some(pos(at, rate));
                let split = apply(&p, r);
                assert_eq!(split.clips().count(), 2);
                assert_eq!(split.clips().next().unwrap().source_end.0, at);
                assert_eq!(render(&split, a.clone(), output), before);
            }
        }
    }
}
#[test]
fn trim_move_gap_delete_range_reference() {
    let a = source(48000);
    let p = document(&a);
    let original = render(&p, a.clone(), 48000);
    let mut r = request(&p, "audio.trim");
    r.source_start = Some(Frames(100));
    r.source_end = Some(Frames(2000));
    let trimmed = apply(&p, r);
    let out = render(&trimmed, a.clone(), 48000);
    assert!(out[..100].iter().all(|x| *x == [0.; 2]));
    assert_eq!(out[100..], original[100..2000]);
    let mut r = request(&trimmed, "audio.move");
    r.delta = Some(pos(123, 48000));
    let moved = apply(&trimmed, r);
    let out = render(&moved, a.clone(), 48000);
    assert!(out[..223].iter().all(|x| *x == [0.; 2]));
    assert_eq!(out[223..], original[100..2000]);
    let mut r = request(&p, "edit.delete");
    r.cursor = Some(pos(500, 48000));
    r.range_end = Some(pos(900, 48000));
    let deleted = apply(&p, r);
    let out = render(&deleted, a, 48000);
    assert_eq!(out[..500], original[..500]);
    assert!(out[500..900].iter().all(|x| *x == [0.; 2]));
    assert_eq!(out[900..], original[900..]);
}
#[test]
fn copy_paste_duplicate_mute_gain_fades_crossfade() {
    let a = source(48000);
    let p = document(&a);
    let mut clipboard = Clipboard::default();
    edit::apply(&p, &request(&p, "edit.copy"), &mut clipboard).unwrap();
    let mut r = request(&p, "edit.paste");
    r.cursor = Some(pos(48000, 48000));
    let copied = edit::apply(&p, &r, &mut clipboard).unwrap();
    assert_ne!(
        copied.tracks[0].clips[0].audio().clip_id,
        copied.tracks[0].clips[1].audio().clip_id
    );
    let original = render(&p, a.clone(), 48000);
    assert_eq!(
        render(&copied, a.clone(), 48000),
        [original.clone(), original.clone()].concat()
    );
    assert_eq!(
        render(&apply(&p, request(&p, "edit.duplicate")), a.clone(), 48000),
        [original.clone(), original.clone()].concat()
    );
    let muted = apply(&p, request(&p, "audio.muteEvents"));
    assert!(render(&muted, a.clone(), 48000)
        .iter()
        .all(|f| *f == [0.; 2]));
    let mut r = request(&p, "audio.gain");
    r.gain_db = Some(20.);
    let louder = apply(&p, r);
    let out = render(&louder, a.clone(), 48000);
    assert!(out.iter().flatten().any(|x| x.abs() > 1.));
    for (i, f) in out.iter().enumerate() {
        assert!((f[0] - original[i][0] * 10.).abs() < 0.000001);
    }
    let mut r = request(&p, "audio.fade");
    r.fade_in = Some(Frames(101));
    r.fade_out = Some(Frames(101));
    r.curve = Some(FadeCurve::Linear);
    let faded = apply(&p, r);
    let out = render(&faded, a.clone(), 48000);
    for i in 0..101 {
        assert!((out[i][0] - original[i][0] * (i as f32 / 100.)).abs() < 1e-7);
    }
    assert_eq!(*out.last().unwrap(), [0.; 2]);
    let mut overlap = p.clone();
    let mut right = overlap.tracks[0].clips[0].audio().clone();
    right.clip_id = id();
    right.position = pos(24000, 48000);
    overlap.tracks[0].clips.push(Clip::Audio(right));
    let cross = apply(&overlap, request(&overlap, "audio.crossfade"));
    let out = render(&cross, a, 48000);
    assert!(out.iter().flatten().all(|x| x.is_finite()));
    assert_eq!(
        cross.tracks[0].clips[0].audio().fade_out.source_frames.0,
        24000
    );
    assert_eq!(
        cross.tracks[0].clips[1].audio().fade_in.source_frames.0,
        24000
    );
    for i in 24000..48000 {
        let x = (i - 24000) as f64 / 23999.;
        let incoming = 0.5 - 0.5 * (std::f64::consts::PI * x).cos();
        let expected =
            original[i][0] as f64 * (1. - incoming) + original[i - 24000][0] as f64 * incoming;
        assert!((out[i][0] as f64 - expected).abs() < 1e-7);
    }
}
#[test]
fn history_branch_and_audio_equivalence() {
    let a = source(44100);
    let original = document(&a);
    let mut history = History::default();
    let mut p = original.clone();
    for command in ["audio.splitAtCursor", "audio.gain", "audio.trim"] {
        let mut r = request(&p, command);
        r.cursor = Some(pos(22050, 44100));
        r.gain_db = Some(-6.);
        if command == "audio.trim" {
            r.clip_ids = vec![p.clips().next().unwrap().clip_id.clone()];
            r.source_end = Some(Frames(10000));
        }
        let next = apply(&p, r);
        history.record(p, next.clone(), command);
        p = next;
    }
    let edited = render(&p, a.clone(), 48000);
    for _ in 0..3 {
        p = history.undo().unwrap();
    }
    assert_eq!(p, original);
    assert_eq!(
        render(&p, a.clone(), 48000),
        render(&original, a.clone(), 48000)
    );
    for _ in 0..3 {
        p = history.redo().unwrap();
    }
    assert_eq!(render(&p, a, 48000), edited);
    history.undo();
    history.record(original.clone(), p, "branch");
    assert!(history.redo().is_none());
}
#[test]
fn memory_streaming_src_and_roundtrip_pcm() {
    for extension in ["wav", "mp3", "flac"] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../tests/fixtures/stereo-44100.{extension}"));
        let memory = AudioAsset::open(&path).unwrap();
        let streaming = AudioAsset::open_with_budget(&path, 0).unwrap();
        let mut p = document(&memory);
        let mut r = request(&p, "audio.splitAtCursor");
        r.cursor = Some(pos(12347, 44100));
        p = apply(&p, r);
        let mut r = request(&p, "audio.gain");
        r.clip_ids = vec![p.clips().last().unwrap().clip_id.clone()];
        r.gain_db = Some(-3.);
        p = apply(&p, r);
        let mut r = request(&p, "audio.fade");
        r.fade_in = Some(Frames(137));
        r.fade_out = Some(Frames(521));
        r.curve = Some(FadeCurve::Cosine);
        p = apply(&p, r);
        for output in [44100, 48000] {
            let x = render(&p, memory.clone(), output);
            let y = render(&p, streaming.clone(), output);
            let max = x
                .iter()
                .zip(&y)
                .flat_map(|(a, b)| [a[0] - b[0], a[1] - b[1]])
                .fold(0.0f32, |m, e| m.max(e.abs()));
            assert_eq!(max, 0.0, "{extension} Memory/Streaming");
            let restored = migrations::decode(&migrations::encode(&p).unwrap()).unwrap();
            assert_eq!(render(&restored, memory.clone(), output), x);
        }
    }
}
#[test]
fn peak_normalize_range_headroom_and_silence() {
    let mut peak = Peak::default();
    peak.observe([2., -0.2]);
    assert_eq!(peak.gain(0.), Some(0.5));
    assert_eq!(Peak::default().gain(0.), None);
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/stereo-44100.wav");
    let mut peak = Peak::default();
    minidaw_lib::project::normalize::analyze(&path, 1000, 6000, &mut peak).unwrap();
    let a = AudioAsset::open(&path).unwrap();
    let mut reader = PlaybackReader::new(a, 44100).unwrap();
    reader.seek(1000, Arc::new(|| false)).unwrap();
    let mut expected = 0_f64;
    for _ in 1000..6000 {
        for f in reader.read_frame().unwrap() {
            expected = expected.max(f.abs() as f64);
        }
    }
    assert_eq!(peak.peak, expected);
    assert!((peak.gain(-1.).unwrap() * peak.peak - 10_f64.powf(-1. / 20.)).abs() < 1e-12);
}
#[test]
fn thousand_clips_bounded_plan_and_history() {
    let a = source(48000);
    let mut p = document(&a);
    let template = p.tracks[0].clips[0].audio().clone();
    p.tracks[0].clips.clear();
    p.primary_clip_id = None;
    for i in 0..1200 {
        let mut c = template.clone();
        c.clip_id = id();
        c.source_end = Frames(40);
        c.position = pos(i * 50, 48000);
        p.tracks[0].clips.push(Clip::Audio(c));
    }
    let t = std::time::Instant::now();
    let plan = PlaybackPlan::compile(
        Arc::new(p.clone()),
        HashMap::from([(p.assets[0].asset_id.clone(), a)]),
        48000,
    )
    .unwrap();
    assert_eq!(plan.voices.len(), 1200);
    assert!(plan.spans.iter().all(|s| s.voices.len() <= 1));
    assert!(t.elapsed().as_secs() < 5);
    let bytes = migrations::encode(&p).unwrap();
    assert!(bytes.len() < 2_000_000);
    assert_eq!(migrations::decode(&bytes).unwrap(), p);
    let mut history = History::default();
    for _ in 0..160 {
        history.record(p.clone(), p.clone(), "stress");
    }
    assert!(history.view().undo <= 128);
    assert!(history.view().bytes <= 32 * 1024 * 1024);
    assert_eq!(time(&template.position, &p.musical_time).n, 0);
}
#[test]
fn splitting_inside_existing_fades_preserves_every_sample_and_roundtrip() {
    let a = source(44100);
    let mut p = document(&a);
    let Clip::Audio(c) = &mut p.tracks[0].clips[0] else {
        panic!("Audio fixture")
    };
    c.fade_in.source_frames = Frames(22050);
    c.fade_out.source_frames = Frames(22050);
    c.gain = 0.713;
    for output in [44100, 48000] {
        let before = render(&p, a.clone(), output);
        for at in [147, 10001, 33099] {
            let mut r = request(&p, "audio.splitAtCursor");
            r.cursor = Some(pos(at, 44100));
            let split = apply(&p, r);
            assert_eq!(render(&split, a.clone(), output), before);
            assert_eq!(
                migrations::decode(&migrations::encode(&split).unwrap()).unwrap(),
                split
            );
        }
    }
}

#[test]
fn trim_restore_range_clipboard_and_adjacent_right_handle() {
    let a = source(48000);
    let original = document(&a);
    let mut r = request(&original, "audio.trim");
    r.source_start = Some(Frames(1000));
    r.source_end = Some(Frames(40000));
    let trimmed = apply(&original, r);
    let mut r = request(&trimmed, "audio.trim");
    r.source_start = Some(Frames(0));
    r.source_end = Some(Frames(48000));
    assert_eq!(apply(&trimmed, r), original);
    let mut r = request(&original, "edit.cut");
    r.cursor = Some(pos(1000, 48000));
    r.range_end = Some(pos(2000, 48000));
    let mut clipboard = Clipboard::default();
    let cut = edit::apply(&original, &r, &mut clipboard).unwrap();
    assert_eq!(cut.clips().count(), 2);
    let mut r = request(&cut, "edit.paste");
    r.cursor = Some(pos(1000, 48000));
    let pasted = edit::apply(&cut, &r, &mut clipboard).unwrap();
    assert_eq!(
        render(&pasted, a.clone(), 48000),
        render(&original, a.clone(), 48000)
    );
    let mut p = original.clone();
    let mut second = p.tracks[0].clips[0].audio().clone();
    second.clip_id = id();
    second.position = pos(48000, 48000);
    second.source_start = Frames(24000);
    p.tracks[0].clips.push(Clip::Audio(second));
    let faded = apply(&p, request(&p, "audio.crossfade"));
    assert_eq!(faded.tracks[0].clips[1].audio().source_start.0, 23520);
    assert_eq!(
        faded.tracks[0].clips[0].audio().fade_out.source_frames.0,
        480
    );
    let mut r = request(&p, "audio.move");
    r.delta = Some(Position::Seconds {
        numerator: Signed(1),
        denominator: 0,
    });
    assert!(edit::apply(&p, &r, &mut Clipboard::default()).is_err());
}

#[test]
fn src_trim_move_and_fade_follow_source_frames_and_output_clock() {
    let a = source(44100);
    let p = document(&a);
    let mut original = PlaybackReader::new(a.clone(), 48000).unwrap();
    original.seek(0, Arc::new(|| false)).unwrap();
    let reference: Vec<_> = (0..48000).map(|_| original.read_frame().unwrap()).collect();
    assert_eq!(render(&p, a.clone(), 48000), reference);
    let mut r = request(&p, "audio.trim");
    r.source_start = Some(Frames(1001));
    r.source_end = Some(Frames(22001));
    let trimmed = apply(&p, r);
    let start = Time::frames(1001, 44100).ceil_frame(48000) as usize;
    let end = Time::frames(22001, 44100).ceil_frame(48000) as usize;
    let out = render(&trimmed, a.clone(), 48000);
    assert!(out[..start].iter().all(|f| *f == [0.; 2]));
    assert_eq!(out[start..], reference[start..end]);
    let mut r = request(&trimmed, "audio.move");
    r.delta = Some(pos(240, 48000));
    let moved = apply(&trimmed, r);
    let out = render(&moved, a.clone(), 48000);
    assert_eq!(out[start + 240..], reference[start..end]);
    let mut r = request(&moved, "audio.fade");
    r.fade_in = Some(Frames(441));
    r.curve = Some(FadeCurve::Linear);
    let out = render(&apply(&moved, r), a, 48000);
    for i in start..end {
        let gain = ((i as f64 * 44100. / 48000. - 1001.) / 440.).clamp(0., 1.);
        for ch in 0..2 {
            assert_eq!(out[i + 240][ch], (reference[i][ch] as f64 * gain) as f32);
        }
    }
}
