use minidaw_lib::{
    audio::{
        cycle::CycleFrames,
        decoder::{AudioData, FileInfo},
        source::AudioAsset,
        streaming::FrameReader,
        timeline::{PlaybackPlan, TimelineReader},
    },
    project::{
        edit::{self, Clipboard},
        migrations,
        schema::*,
        tempo_sync as sync,
    },
};
use serde_json::{json, Value};
use std::{collections::HashMap, path::Path, sync::Arc};
fn doc(rate: u32, frames: u64) -> Project {
    let mut p = Project::new();
    p.import(Asset {
        asset_id: id(),
        filename: "tone.wav".into(),
        path: PathReference {
            project_relative_path: Some("tone.wav".into()),
            original_absolute_path: None,
        },
        metadata: AudioMetadata {
            sample_rate: rate,
            channels: 2,
            source_frames: Frames(frames),
            container: "wav".into(),
            codec: None,
        },
        fingerprint: Fingerprint {
            file_bytes: Frames(100),
            sampled_sha256: "a".repeat(64),
        },
        extensions: Default::default(),
    });
    p
}
fn ids(p: &Project) -> Vec<String> {
    p.clips().map(|c| c.clip_id.clone()).collect()
}
fn edit(p: &Project, mut r: Value) -> Project {
    if r.get("clipIds").is_none() {
        r["clipIds"] = json!(ids(p));
    }
    edit::apply(
        p,
        &serde_json::from_value(r).unwrap(),
        &mut Clipboard::default(),
    )
    .unwrap()
}
fn bpm(p: &Project, bpm: f64) -> Project {
    edit(p, json!({"command":"project.tempo","bpm":bpm}))
}
fn on(p: &Project, source: f64) -> Project {
    edit(
        p,
        json!({"command":"audio.tempoSync","tempoSync":true,"sourceBpm":source}),
    )
}
fn source(rate: u32, seconds: usize) -> Arc<AudioAsset> {
    let n = rate as usize * seconds;
    AudioAsset::memory(AudioData {
        info: FileInfo {
            name: "tone".into(),
            sample_rate: rate,
            channels: 2,
            frames: n,
            duration: seconds as f64,
            sanitized_samples: 0,
        },
        samples: (0..n)
            .flat_map(|i| {
                let a = (i as f64 * std::f64::consts::TAU * 440. / rate as f64).sin() as f32 * 0.2;
                [a, -a]
            })
            .collect(),
    })
}
fn plan(p: &Project, a: Arc<AudioAsset>, rate: u32) -> Arc<PlaybackPlan> {
    PlaybackPlan::compile(
        Arc::new(p.clone()),
        HashMap::from([(p.assets[0].asset_id.clone(), a)]),
        rate,
    )
    .unwrap()
}
fn render(p: &Project, a: Arc<AudioAsset>, rate: u32) -> Vec<[f32; 2]> {
    let plan = plan(p, a, rate);
    let n = plan.frames;
    let mut r = TimelineReader::new(plan);
    r.seek(0, Arc::new(|| false)).unwrap();
    (0..n).map(|_| r.read_frame().unwrap()).collect()
}
#[test]
fn source_project_ratios_and_ten_thousand_tempo_changes_do_not_drift() {
    for (rate, source) in [(44100, 73.25), (48000, 120.), (96000, 191.37)] {
        let p = doc(rate, rate as u64 * 600);
        let p = edit(
            &p,
            json!({"command":"audio.move","anchorClipId":ids(&p)[0],"targetTick":"123456789"}),
        );
        let original = on(&p, source);
        let reference = sync::get(original.clips().next().unwrap())
            .unwrap()
            .unwrap();
        let mut p = original.clone();
        for n in 0..10000 {
            p = bpm(&p, [61.13, 137.47, 240., 95.01, 120.][n % 5]);
            let c = p.clips().next().unwrap();
            assert_eq!(
                c.position,
                Position::Ticks {
                    ticks: Signed(123456789)
                }
            );
            assert_eq!(sync::get(c).unwrap().unwrap(), reference);
            assert_eq!(
                c.source_end.0,
                (rate as f64 * 600. * source / p.musical_time.tempo_map[0].bpm).round() as u64
            );
        }
        p = bpm(&p, 120.);
        assert_eq!(p, original);
    }
}
#[test]
fn off_freezes_length_and_position_and_manual_stretch_and_pitch_coexist() {
    let p = doc(48000, 96000);
    let p = edit(
        &p,
        json!({"command":"audio.stretch","stretchFrames":"144000"}),
    );
    let p = edit(
        &p,
        json!({"command":"audio.pitch","pitchShift":{"semitones":5,"cents":37}}),
    );
    let p = on(&p, 90.);
    assert_eq!(p.clips().next().unwrap().source_end.0, 72000);
    let p = edit(&p, json!({"command":"audio.tempoSync","tempoSync":false}));
    let frozen = p.clips().next().unwrap().clone();
    let p = bpm(&p, 220.);
    assert_eq!(*p.clips().next().unwrap(), frozen);
    let p = edit(
        &p,
        json!({"command":"audio.stretch","stretchFrames":"120000"}),
    );
    assert_eq!(p.clips().next().unwrap().source_end.0, 120000);
    let p = on(&p, 110.);
    assert_eq!(p.clips().next().unwrap().source_end.0, 48000);
    assert_eq!(
        minidaw_lib::project::pitch::get(p.clips().next().unwrap())
            .unwrap()
            .total(),
        537
    );
    assert_eq!(
        p,
        migrations::decode(&migrations::encode(&p).unwrap()).unwrap()
    );
}
#[test]
fn sync_on_and_off_events_remain_independent_and_invalid_ratio_is_atomic() {
    let mut p = doc(48000, 48000);
    let mut c = p.clips().next().unwrap().clone();
    c.clip_id = id();
    p.tracks[0].clips.push(Clip::Audio(c.clone()));
    let chosen = ids(&p)[0].clone();
    let p = edit(
        &p,
        json!({"command":"audio.tempoSync","clipIds":[chosen],"tempoSync":true,"sourceBpm":120}),
    );
    let p = bpm(&p, 90.);
    assert_eq!(p.clips().nth(1).unwrap(), &c);
    assert_eq!(p.clips().next().unwrap().source_end.0, 64000);
    for bad in [0., -1., 1001.] {
        assert!(edit::apply(
            &p,
            &serde_json::from_value(
                json!({"command":"audio.tempoSync","clipIds":[chosen],"sourceBpm":bad})
            )
            .unwrap(),
            &mut Clipboard::default()
        )
        .is_err());
    }
    assert!(edit::apply(
        &p,
        &serde_json::from_value(json!({"command":"project.tempo","bpm":1})).unwrap(),
        &mut Clipboard::default()
    )
    .is_err());
}
#[test]
fn exact_tick_trim_split_move_copy_glue_and_cycle_bounds() {
    let source = source(44100, 2);
    let p = doc(44100, 88200);
    let p = edit(
        &p,
        json!({"command":"audio.fade","fadeIn":"11025","fadeOut":"5000","curve":"cosine"}),
    );
    let p = bpm(&on(&p, 120.), 137.47);
    let baseline = render(&p, source.clone(), 48000);
    let split = edit(
        &p,
        json!({"command":"audio.splitAtCursor","cursor":{"unit":"ticks","ticks":"960000"}}),
    );
    assert_eq!(
        split.clips().nth(1).unwrap().position,
        Position::Ticks {
            ticks: Signed(960000)
        }
    );
    assert_eq!(
        sync::length_ticks(split.clips().next().unwrap()).unwrap(),
        Some(960000)
    );
    assert_eq!(baseline, render(&split, source.clone(), 48000));
    let joined = edit(&split, json!({"command":"audio.glue"}));
    assert_eq!(joined.clips().count(), 1);
    assert_eq!(baseline, render(&joined, source.clone(), 48000));
    let trimmed = edit(
        &p,
        json!({"command":"audio.trim","targetTick":"240007","trimSide":"left"}),
    );
    assert_eq!(
        trimmed.clips().next().unwrap().position,
        Position::Ticks {
            ticks: Signed(240007)
        }
    );
    assert_eq!(
        sync::length_ticks(trimmed.clips().next().unwrap()).unwrap(),
        Some(3840000 - 240007)
    );
    let moved = edit(
        &trimmed,
        json!({"command":"audio.move","targetTick":"1234567","anchorClipId":ids(&trimmed)[0]}),
    );
    assert_eq!(
        moved.clips().next().unwrap().position,
        Position::Ticks {
            ticks: Signed(1234567)
        }
    );
    let mut clipboard = Clipboard::default();
    edit::apply(
        &moved,
        &serde_json::from_value(json!({"command":"edit.copy","clipIds":ids(&moved)})).unwrap(),
        &mut clipboard,
    )
    .unwrap();
    let changed = bpm(&moved, 93.31);
    let pasted = edit::apply(
        &changed,
        &serde_json::from_value(
            json!({"command":"edit.paste","cursor":{"unit":"ticks","ticks":"9600000"}}),
        )
        .unwrap(),
        &mut clipboard,
    )
    .unwrap();
    assert_eq!(
        pasted.clips().nth(1).unwrap().position,
        Position::Ticks {
            ticks: Signed(9600000)
        }
    );
    let mut cycle = p.clone();
    cycle.cycle = Some(Cycle {
        enabled: true,
        start_tick: Signed(0),
        end_tick: Signed(3840000),
    });
    let frames = CycleFrames::compile(&cycle, 48000).unwrap().unwrap();
    assert!(plan(&cycle, source, 48000).frames.abs_diff(frames.end) <= 1);
    let exact = bpm(&cycle, 180.);
    assert_eq!(
        CycleFrames::compile(&exact, 48000).unwrap().unwrap().end,
        64000
    );
    for n in 1..100000usize {
        assert_eq!(frames.position(n * frames.end), 0);
    }
}
#[test]
fn memory_streaming_pcm_pitch_and_crossfade_gain_mute_agree() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/stereo-44100.wav");
    let memory = AudioAsset::open_with_budget(&path, usize::MAX).unwrap();
    let streamed = AudioAsset::open_with_budget(&path, 0).unwrap();
    let p = doc(44100, memory.info.frames as u64);
    let p = edit(&p, json!({"command":"audio.trim","sourceEnd":"44100"}));
    let p = bpm(&on(&p, 120.), 173.3);
    let p = edit(
        &p,
        json!({"command":"audio.pitch","pitchShift":{"semitones":-3,"cents":37}}),
    );
    let p = edit(&p, json!({"command":"audio.gain","gainDb":-4.3}));
    let p = edit(
        &p,
        json!({"command":"audio.fade","fadeIn":"1000","fadeOut":"3000"}),
    );
    assert_eq!(
        render(&p, memory.clone(), 48000),
        render(&p, streamed.clone(), 48000)
    );
    let split = edit(
        &p,
        json!({"command":"audio.splitAtCursor","cursor":{"unit":"ticks","ticks":"960000"}}),
    );
    let crossfade = edit(&split, json!({"command":"audio.crossfade"}));
    assert_eq!(
        render(&crossfade, memory.clone(), 48000),
        render(&crossfade, streamed, 48000)
    );
    let mute = edit(&crossfade, json!({"command":"audio.muteEvents"}));
    assert!(render(&mute, memory, 48000).iter().all(|s| *s == [0.; 2]));
}
fn hz(samples: &[[f32; 2]], rate: u32) -> f64 {
    let lo = samples.len() / 4;
    let hi = samples.len() * 3 / 4;
    let mut crossings = vec![];
    for i in lo + 1..hi {
        let (a, b) = (samples[i - 1][0] as f64, samples[i][0] as f64);
        if a < 0. && b >= 0. {
            crossings.push(i as f64 - 1. - a / (b - a));
        }
    }
    (crossings.len() - 1) as f64 * rate as f64 / (crossings.last().unwrap() - crossings[0])
}
#[test]
fn measured_pitch_preservation_and_wide_sync_ratios() {
    for rate in [44100, 48000, 96000] {
        let asset = source(rate, 2);
        for (source, project, cents) in [
            (120., 137.47, 0),
            (60., 180., 0),
            (180., 60., 0),
            (120., 160., 537),
        ] {
            let p = doc(rate, rate as u64 * 2);
            let p = bpm(&p, project);
            let p = on(&p, source);
            let p = if cents != 0 {
                edit(
                    &p,
                    json!({"command":"audio.pitch","pitchShift":{"semitones":5,"cents":37}}),
                )
            } else {
                p
            };
            let began = std::time::Instant::now();
            let out = render(&p, asset.clone(), rate);
            let measured = hz(&out, rate);
            let expected = 440. * 2f64.powf(cents as f64 / 1200.);
            let error = 1200. * (measured / expected).log2();
            println!(
                "SYNC_QUALITY {}",
                json!({"rate":rate,"sourceBpm":source,"projectBpm":project,"pitchCents":cents,"measuredHz":measured,"errorCents":error,"frames":out.len(),"renderMs":began.elapsed().as_secs_f64()*1000.})
            );
            assert!(error.abs() < 0.5, "{error} cents");
            let expected_frames = sync::end(p.clips().next().unwrap(), &p.musical_time, rate)
                .unwrap()
                .ceil_frame(rate) as usize;
            assert_eq!(out.len(), expected_frames);
        }
    }
}

#[test]
fn short_source_window_ratio_limits_allow_only_final_sample_rounding() {
    for n in [1, 3, 9, 15, 33] {
        for (source_bpm, project_bpm) in [(30., 240.), (240., 30.)] {
            let p = on(&bpm(&doc(48000, n), project_bpm), source_bpm);
            let out = render(&p, source(48000, 1), 48000);
            assert!(!out.is_empty());
            assert!(out.iter().flatten().all(|x| x.is_finite()));
            assert_eq!(
                out.len(),
                sync::end(p.clips().next().unwrap(), &p.musical_time, 48000)
                    .unwrap()
                    .ceil_frame(48000) as usize
            );
        }
    }
}
