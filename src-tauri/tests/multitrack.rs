use minidaw_lib::{
    audio::{
        decoder::{AudioData, FileInfo},
        midi::{MidiEvent, MidiPlan, MidiSink},
        source::{AssetStorage, AudioAsset},
        streaming::FrameReader,
        synth::BasicSynth,
        timeline::{PlaybackPlan, TimelineReader},
    },
    project::{
        edit::{self, Clipboard},
        history::History,
        migrations,
        schema::*,
        time::Time,
    },
};
use serde_json::{json, Value};
use std::{collections::HashMap, path::Path, sync::Arc};

fn edit(p: &Project, request: Value) -> Project {
    edit::apply(
        p,
        &serde_json::from_value(request).unwrap(),
        &mut Clipboard::default(),
    )
    .unwrap()
}
fn add(p: &Project, kind: &str) -> Project {
    edit(p, json!({"command":"track.add","trackKind":kind}))
}
fn mix(p: &Project, i: usize, fields: Value) -> Project {
    let mut r = json!({"command":"track.mix","trackIds":[p.tracks[i].track_id]});
    r.as_object_mut()
        .unwrap()
        .extend(fields.as_object().unwrap().clone());
    edit(p, r)
}
fn asset(sample: [f32; 2], rate: u32) -> Arc<AudioAsset> {
    AudioAsset::memory(AudioData {
        info: FileInfo {
            name: "tone.wav".into(),
            sample_rate: rate,
            channels: 2,
            frames: rate as usize,
            duration: 1.0,
            sanitized_samples: 0,
        },
        samples: (0..rate).flat_map(|_| sample).collect(),
    })
}
fn import(p: &mut Project, a: &AudioAsset) {
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
            file_bytes: Frames(100),
            sampled_sha256: "a".repeat(64),
        },
        extensions: Extensions::new(),
    });
}
fn two(a: &AudioAsset, b: &AudioAsset) -> Project {
    let mut p = Project::new();
    import(&mut p, a);
    p = add(&p, "audio");
    import(&mut p, b);
    let clip = p.primary_clip_id.clone().unwrap();
    let track = p.tracks[1].track_id.clone();
    edit(
        &p,
        json!({"command":"clip.moveTrack","clipIds":[clip],"targetTrackId":track}),
    )
}
fn plan(p: &Project, a: Arc<AudioAsset>, b: Arc<AudioAsset>) -> Arc<PlaybackPlan> {
    PlaybackPlan::compile(
        Arc::new(p.clone()),
        HashMap::from([
            (p.assets[0].asset_id.clone(), a),
            (p.assets[1].asset_id.clone(), b),
        ]),
        48000,
    )
    .unwrap()
}
fn sample(p: &Project, a: Arc<AudioAsset>, b: Arc<AudioAsset>) -> [f32; 2] {
    let mut r = TimelineReader::new(plan(p, a, b));
    r.seek(4000, Arc::new(|| false)).unwrap();
    r.read_frame().unwrap()
}
fn near(a: [f32; 2], b: [f32; 2]) {
    for i in 0..2 {
        assert!((a[i] - b[i]).abs() < 1e-6, "{a:?} != {b:?}");
    }
}

#[test]
fn track_transactions_preserve_vertical_clip_data_history_and_legacy_files() {
    let a = asset([0.1, 0.2], 48000);
    let mut p = two(&a, &a);
    p.tracks[0].clips[0].as_audio_mut().unwrap().position =
        Time::frames(1234, 44100).position().unwrap();
    let original = p.clone();
    let clip = p.tracks[0].clips[0].clone();
    for i in 0..100 {
        p = edit(
            &p,
            json!({"command":"clip.moveTrack","clipIds":[clip.id()],"targetTrackId":p.tracks[if i%2==0 {1}else{0}].track_id}),
        );
    }
    assert_eq!(p, original);
    assert_eq!(p.tracks[0].clips[0], clip);
    p = add(&p, "midi");
    let midi = p.tracks[2].track_id.clone();
    assert!(edit::apply(
        &p,
        &serde_json::from_value(
            json!({"command":"clip.moveTrack","clipIds":[clip.id()],"targetTrackId":midi})
        )
        .unwrap(),
        &mut Clipboard::default()
    )
    .is_err());
    p = mix(&p, 0, json!({"volumeDb":-8.5,"pan":0.4,"solo":true}));
    let before = p.clone();
    p = edit(
        &p,
        json!({"command":"track.move","trackIds":[p.tracks[0].track_id],"direction":1}),
    );
    assert_eq!(p.tracks[1], before.tracks[0]);
    let mut h = History::default();
    h.record(before.clone(), p.clone(), "Track reorder");
    assert_eq!(h.undo().unwrap(), before);
    assert_eq!(h.redo().unwrap(), p);
    let keep = p.clone();
    p = edit(
        &p,
        json!({"command":"track.delete","trackIds":[p.tracks[1].track_id]}),
    );
    assert_eq!(p.tracks.len(), 2);
    assert!(!p.clips().any(|c| c.clip_id == clip.id()));
    h.record(keep.clone(), p.clone(), "Delete");
    assert_eq!(h.undo().unwrap(), keep);
    assert_eq!(h.redo().unwrap(), p);
    assert_eq!(
        migrations::decode(&migrations::encode(&keep).unwrap()).unwrap(),
        keep
    );
    let raw = serde_json::to_value(&original).unwrap();
    assert!(raw["tracks"][0].get("mix").is_none());
    assert_eq!(
        migrations::decode(&serde_json::to_vec(&raw).unwrap()).unwrap(),
        original
    );
}

#[test]
fn audio_tracks_mix_at_master_with_global_solo_balance_and_gain() {
    let a = asset([0.1, 0.2], 48000);
    let b = asset([0.3, 0.4], 48000);
    let p = two(&a, &b);
    near(sample(&p, a.clone(), b.clone()), [0.4, 0.6]);
    let gain = 10f32.powf(-6.0 / 20.0);
    let q = mix(&p, 0, json!({"volumeDb":-6,"pan":-1}));
    near(sample(&q, a.clone(), b.clone()), [0.1 * gain + 0.3, 0.4]);
    let q = mix(&q, 1, json!({"pan":1}));
    near(sample(&q, a.clone(), b.clone()), [0.1 * gain, 0.4]);
    let q = mix(&p, 0, json!({"solo":true}));
    near(sample(&q, a.clone(), b.clone()), [0.1, 0.2]);
    let q = mix(&q, 1, json!({"solo":true}));
    near(sample(&q, a.clone(), b.clone()), [0.4, 0.6]);
    let q = mix(&q, 0, json!({"mute":true}));
    near(sample(&q, a.clone(), b.clone()), [0.3, 0.4]);
    let q = add(&p, "midi");
    let q = mix(&q, 2, json!({"solo":true}));
    near(sample(&q, a.clone(), b.clone()), [0.0; 2]);
    assert_eq!(
        plan(&q, a, b).frames,
        48000,
        "muting cannot shorten transport"
    );
    let one = edit(
        &p,
        json!({"command":"track.delete","trackIds":[p.tracks[1].track_id]}),
    );
    assert!(one.preview_supported(one.primary().unwrap()));
    let one = mix(&one, 0, json!({"pan":1}));
    assert!(
        !one.preview_supported(one.primary().unwrap()),
        "fader must not use neutral direct-source shortcut"
    );
}

#[test]
fn midi_transfer_keeps_hidden_notes_controllers_and_exact_ticks() {
    let mut p = add(&add(&Project::new(), "midi"), "midi");
    p = edit(
        &p,
        json!({"command":"midi.clip.add","trackIds":[p.tracks[0].track_id],"targetTick":"1234567","lengthTick":"3840000"}),
    );
    let id = p.midi_clips().next().unwrap().clip_id.clone();
    p = edit(
        &p,
        json!({"command":"midi.note.add","clipIds":[id],"targetTick":"100001","lengthTick":"200003","pitch":64}),
    );
    p = edit(
        &p,
        json!({"command":"midi.control.put","clipIds":[id],"event":{"eventId":"","tick":"101","channel":0,"data":{"kind":"cc","controller":64,"value":127}}}),
    );
    let before = p.midi_clips().next().unwrap().clone();
    for i in 0..100 {
        p = edit(
            &p,
            json!({"command":"clip.moveTrack","clipIds":[id],"targetTrackId":p.tracks[if i%2==0 {1}else{0}].track_id}),
        );
    }
    assert_eq!(*p.midi_clips().next().unwrap(), before);
    p = edit(
        &p,
        json!({"command":"clip.moveTrack","clipIds":[id],"targetTrackId":p.tracks[1].track_id,"targetTick":"1234568","anchorClipId":id}),
    );
    let after = p.midi_clips().next().unwrap();
    assert_eq!(after.start_tick.0, 1234568);
    assert_eq!(after.notes, before.notes);
    assert_eq!(after.controls, before.controls);
    assert_eq!(
        migrations::decode(&migrations::encode(&p).unwrap()).unwrap(),
        p
    );
}

fn synth(p: &Project, second: bool) -> BasicSynth {
    let mut s = BasicSynth::new(48000);
    s.configure(Some(&MidiPlan::compile(p, 48000).unwrap()));
    for t in 0..if second { 2 } else { 1 } {
        s.event(MidiEvent {
            frame: 0,
            offset: 0,
            voice: t as u32,
            track: t,
            channel: 0,
            pitch: 69,
            velocity: 100,
            on: true,
        });
    }
    s
}
#[test]
fn synth_buses_are_independent_and_audio_solo_gates_instruments() {
    let mut p = add(&add(&Project::new(), "midi"), "midi");
    for t in &mut p.tracks {
        t.instrument = Instrument::BasicSynth;
    }
    let mut one = synth(&p, false);
    let mut two = synth(&p, true);
    let q = mix(&p, 1, json!({"mute":true}));
    let mut muted = synth(&q, true);
    let q = mix(&p, 0, json!({"volumeDb":-6,"pan":-1}));
    let q = mix(&q, 1, json!({"pan":1}));
    let mut balanced = synth(&q, true);
    let q = add(&p, "audio");
    let q = mix(&q, 2, json!({"solo":true}));
    let mut audio_solo = synth(&q, true);
    for _ in 0..10000 {
        let a = one.sample();
        let b = two.sample();
        let c = muted.sample();
        let d = balanced.sample();
        for ch in 0..2 {
            assert!((b[ch] - 2.0 * a[ch]).abs() < 1e-12);
            assert!((c[ch] - a[ch]).abs() < 1e-12);
        }
        assert!((d[0] - a[0] * 10f64.powf(-6.0 / 20.0)).abs() < 1e-12);
        assert!((d[1] - a[1]).abs() < 1e-12);
        assert_eq!(audio_solo.sample(), [0.0; 2]);
    }
}

#[test]
fn independent_streaming_readers_sum_different_rates_and_seek_exactly() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures");
    let a = AudioAsset::open_with_budget(&root.join("stereo-44100.wav"), 0).unwrap();
    let b = AudioAsset::open_with_budget(&root.join("모노-48000.WAV"), 0).unwrap();
    assert!(matches!(a.storage, AssetStorage::File(_)));
    assert!(matches!(b.storage, AssetStorage::File(_)));
    let p = two(&a, &b);
    let q = mix(&p, 1, json!({"mute":true}));
    let z = mix(&p, 0, json!({"mute":true}));
    let mut both = TimelineReader::new(plan(&p, a.clone(), b.clone()));
    let mut left = TimelineReader::new(plan(&q, a.clone(), b.clone()));
    let mut right = TimelineReader::new(plan(&z, a, b));
    for frame in [0, 17000, 2200, 34000] {
        for r in [&mut both, &mut left, &mut right] {
            r.seek(frame, Arc::new(|| false)).unwrap();
        }
        for _ in 0..1024 {
            let a = left.read_frame().unwrap();
            let b = right.read_frame().unwrap();
            near(both.read_frame().unwrap(), [a[0] + b[0], a[1] + b[1]]);
        }
    }
}
