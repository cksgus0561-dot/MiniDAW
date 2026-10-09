use minidaw_lib::{
    audio::{
        cycle::CycleFrames,
        metrics::AudioMetrics,
        midi::{MidiEvent, MidiPlan, MidiScheduler, MidiSink},
        renderer::{Action, Command, Renderer, COMMAND_CAPACITY},
        source::AudioSource,
        timeline::PlaybackPlan,
        transport::{PlayState, TransportCell},
    },
    project::{
        edit::{self, Clipboard, EditRequest},
        history::History,
        migrations,
        schema::*,
        time::time,
    },
};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
fn edit(p: &Project, value: serde_json::Value) -> Project {
    let r: EditRequest = serde_json::from_value(value).unwrap();
    edit::apply(p, &r, &mut Clipboard::default()).unwrap()
}
fn project() -> Project {
    let p = edit(
        &Project::new(),
        serde_json::json!({"command":"midi.track.add"}),
    );
    edit(
        &p,
        serde_json::json!({"command":"midi.clip.add","trackIds":[p.tracks[0].track_id],"targetTick":"0","lengthTick":"3840000"}),
    )
}
fn note(p: &Project, start: i64, length: i64, pitch: u8) -> Project {
    edit(
        p,
        serde_json::json!({"command":"midi.note.add","clipIds":[p.midi_clips().next().unwrap().clip_id],"targetTick":start.to_string(),"lengthTick":length.to_string(),"pitch":pitch}),
    )
}
#[derive(Default)]
struct Events(
    Vec<MidiEvent>,
    Vec<minidaw_lib::audio::midi::MidiControlEvent>,
);
impl MidiSink for Events {
    fn control(&mut self, e: minidaw_lib::audio::midi::MidiControlEvent) {
        self.1.push(e);
    }
    fn event(&mut self, e: MidiEvent) {
        self.0.push(e);
    }
}

#[test]
fn midi_roundtrip_exact_edits_history_and_validation() {
    let p = note(&project(), 123, 240001, 60);
    let original = p.clone();
    let mut p = p;
    for i in 0..1000 {
        let c = p.midi_clips().next().unwrap();
        let n = &c.notes[0];
        p = edit(
            &p,
            serde_json::json!({"command":"midi.note.change","clipIds":[c.clip_id],"noteId":n.note_id,"targetTick":(123+i%2).to_string(),"lengthTick":(240001+i%2).to_string(),"pitch":60}),
        );
        let c = p.midi_clips().next().unwrap();
        p = edit(
            &p,
            serde_json::json!({"command":"midi.clip.move","clipIds":[c.clip_id],"anchorClipId":c.clip_id,"targetTick":(i%2).to_string()}),
        );
        p = migrations::decode(&migrations::encode(&p).unwrap()).unwrap();
    }
    let c = p.midi_clips().next().unwrap();
    assert_eq!(
        (
            c.start_tick.0,
            c.notes[0].start_tick.0,
            c.notes[0].length_tick.0
        ),
        (1, 124, 240002)
    );
    let mut h = History::default();
    h.record(original.clone(), p.clone(), "MIDI edit");
    assert_eq!(h.undo().unwrap(), original);
    assert_eq!(h.redo().unwrap(), p);
    for (start, length, pitch) in [
        (0, 0, 60),
        (-1, 100, 60),
        (3_840_000, 1, 60),
        (0, 100, 128),
        (i64::MAX, 1, 60),
    ] {
        let r = serde_json::from_value(serde_json::json!({"command":"midi.note.add","clipIds":[c.clip_id],"targetTick":start.to_string(),"lengthTick":length.to_string(),"pitch":pitch})).unwrap();
        assert!(edit::apply(&p, &r, &mut Clipboard::default()).is_err());
    }
    let deleted = edit(
        &p,
        serde_json::json!({"command":"midi.note.delete","clipIds":[c.clip_id],"noteId":c.notes[0].note_id}),
    );
    assert!(deleted.midi_clips().next().unwrap().notes.is_empty());
    let legacy = Project::new();
    assert_eq!(
        migrations::decode(&migrations::encode(&legacy).unwrap()).unwrap(),
        legacy
    );
}

#[test]
fn sample_conversion_uses_shared_tempo_map_and_no_tick_drift() {
    let mut p = note(&note(&project(), 123, 240001, 60), 960123, 100001, 67);
    p.musical_time.tempo_map[0].bpm = 137.3;
    p.musical_time.tempo_map.push(Tempo {
        tick: Signed(960000),
        bpm: 91.25,
    });
    let original = p.midi_clips().next().unwrap().clone();
    for rate in [44100, 48000, 96000] {
        let plan = MidiPlan::compile(&p, rate).unwrap();
        for (n, tone) in original.notes.iter().zip(plan.tones.iter()) {
            for (tick, frame) in [
                (n.start_tick.0, tone.start),
                (n.start_tick.0 + n.length_tick.0, tone.end),
            ] {
                assert_eq!(
                    frame as i128,
                    time(
                        &Position::Ticks {
                            ticks: Signed(tick)
                        },
                        &p.musical_time
                    )
                    .ceil_frame(rate)
                );
            }
        }
    }
    assert_eq!(p.midi_clips().next().unwrap(), &original);
}

#[test]
fn mixed_audio_midi_keeps_import_edit_and_pcm_semantics() {
    use minidaw_lib::audio::{
        source::AudioAsset, streaming::FrameReader, timeline::TimelineReader,
    };
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/stereo-44100.wav");
    let asset = AudioAsset::open(&path).unwrap();
    let mut p = note(&project(), 123, 240000, 60);
    p.import(serde_json::from_value(serde_json::json!({
        "assetId":id(),"filename":"fixture.wav","path":{"projectRelativePath":"fixture.wav","originalAbsolutePath":null},
        "metadata":{"sampleRate":44100,"channels":2,"sourceFrames":asset.info.frames.to_string(),"container":"wav","codec":null},
        "fingerprint":{"fileBytes":"1","sampledSha256":"a".repeat(64)},"extensions":{}
    })).unwrap());
    assert_eq!(p.tracks[0].kind, TrackKind::Midi);
    assert_eq!(p.tracks[1].kind, TrackKind::Audio);
    let midi = p.midi_clips().next().unwrap().clone();
    let audio = p.primary().unwrap().clip_id.clone();
    for request in [
        serde_json::json!({"command":"audio.move","clipIds":[audio],"anchorClipId":audio,"targetTick":"123"}),
        serde_json::json!({"command":"audio.trim","clipIds":[audio],"targetTick":"240000","trimSide":"left"}),
        serde_json::json!({"command":"audio.splitAtCursor","clipIds":[audio],"cursor":{"unit":"ticks","ticks":"960000"}}),
    ] {
        p = edit(&p, request);
        assert_eq!(p.midi_clips().next().unwrap(), &midi);
    }
    assert_eq!(p.clips().count(), 2);
    let assets = HashMap::from([(p.assets[0].asset_id.clone(), asset)]);
    let mixed = PlaybackPlan::compile(Arc::new(p.clone()), assets.clone(), 48000).unwrap();
    p.tracks.remove(0);
    let plain = PlaybackPlan::compile(Arc::new(p), assets, 48000).unwrap();
    let (mut a, mut b) = (TimelineReader::new(mixed), TimelineReader::new(plain));
    a.seek(0, Arc::new(|| false)).unwrap();
    b.seek(0, Arc::new(|| false)).unwrap();
    for _ in 0..48000 {
        assert_eq!(a.read_frame().unwrap(), b.read_frame().unwrap());
    }
}

#[test]
fn scheduler_seek_chase_same_pitch_and_exact_off_before_on() {
    let p = note(&note(&project(), 24000, 48000, 60), 72000, 24000, 60);
    let plan = MidiPlan::compile(&p, 48000).unwrap();
    let mut scheduler = MidiScheduler::default();
    let mut e = Events::default();
    // Seek into a held note: chase exactly once, independent of any UI updates.
    scheduler.sample(&plan, 1000, 0, &mut e);
    assert_eq!((e.0[0].on, e.0[0].frame), (true, 1000));
    for frame in 1001..2401 {
        scheduler.sample(&plan, frame, frame - 1000, &mut e);
    }
    assert_eq!(
        e.0.iter().map(|e| (e.on, e.frame)).collect::<Vec<_>>(),
        vec![(true, 1000), (false, 1800), (true, 1800), (false, 2400)]
    );
    scheduler.sample(&plan, 700, 0, &mut e);
    scheduler.reset(701, 1, &mut e);
    assert_eq!(
        (e.0[e.0.len() - 1].on, e.0[e.0.len() - 1].frame),
        (false, 701)
    );
    // A one-tick note stays valid in the document, occupies one output sample.
    let p = note(&project(), 1, 1, 127);
    let plan = MidiPlan::compile(&p, 48000).unwrap();
    assert_eq!(plan.tones[0].end - plan.tones[0].start, 1);
}

#[test]
fn renderer_midi_is_sample_exact_through_variable_buffers_and_60_cycle_wraps() {
    const RATE: u32 = 8000;
    let mut p = note(
        &note(&project(), 200 * 240, 300 * 240, 60),
        1200 * 240,
        300 * 240,
        64,
    );
    if let Clip::Midi(c) = &mut p.tracks[0].clips[0] {
        c.controls = vec![
            MidiControl {
                event_id: id(),
                tick: Signed(100 * 240),
                channel: 0,
                data: MidiControlData::Cc {
                    controller: 64,
                    value: 127,
                },
            },
            MidiControl {
                event_id: id(),
                tick: Signed(550 * 240),
                channel: 0,
                data: MidiControlData::PitchBend { value: -6000 },
            },
            MidiControl {
                event_id: id(),
                tick: Signed(850 * 240),
                channel: 0,
                data: MidiControlData::Cc {
                    controller: 64,
                    value: 0,
                },
            },
        ];
    }
    p.cycle = Some(Cycle {
        enabled: true,
        start_tick: Signed(123 * 240),
        end_tick: Signed(1360 * 240),
    });
    let cycle = CycleFrames::compile(&p, RATE).unwrap().unwrap();
    let plan = PlaybackPlan::compile(Arc::new(p), HashMap::new(), RATE).unwrap();
    let source = AudioSource::timeline(plan, 0).unwrap();
    std::thread::sleep(Duration::from_millis(40));
    let (mut tx, rx) = rtrb::RingBuffer::new(COMMAND_CAPACITY);
    let shared = Arc::new(TransportCell::default());
    let mut renderer = Renderer::new(
        rx,
        shared.clone(),
        Arc::new(AudioMetrics::default()),
        RATE,
        2,
    );
    let mut command_id = 0;
    let mut send = |action| {
        command_id += 1;
        tx.push(Command {
            id: command_id,
            issued: Instant::now(),
            action,
        })
        .ok()
        .unwrap();
    };
    send(Action::Load {
        clip_id: 1,
        audio: source.clone(),
    });
    send(Action::Play);
    let mut expected_scheduler = MidiScheduler::default();
    let mut expected = Events::default();
    let mut actual = Events::default();
    let mut total = 0;
    for size in [17, 64, 511].into_iter().cycle().take(390) {
        for offset in 0..size {
            expected_scheduler.sample(
                &source.timeline.as_ref().unwrap().midi,
                cycle.position(total + offset),
                offset,
                &mut expected,
            );
        }
        let mut output = vec![0f32; size * 2];
        renderer.render_with_midi(&mut output, &mut actual);
        assert!(output.iter().all(|s| *s == 0.));
        total += size;
        assert_eq!(shared.read().frame, cycle.position(total) as f64);
    }
    assert!(total / 1237 > 60);
    assert_eq!(actual.0, expected.0);
    assert_eq!(actual.1, expected.1);
    assert!(
        actual
            .1
            .iter()
            .filter(
                |e| e.control == minidaw_lib::audio::midi::Control::PitchBend(-6000)
                    && e.frame == 550
            )
            .count()
            > 60
    );
    assert_eq!(source.snapshot().starvation, 0);
    assert!(actual.0.iter().filter(|e| e.on && e.pitch == 60).count() > 60);
    assert!(actual
        .0
        .iter()
        .filter(|e| !e.on && e.pitch == 64)
        .all(|e| e.frame == 123));
    send(Action::Pause);
    renderer.render_with_midi(&mut [0f32; 128], &mut actual);
    let paused = shared.read().frame;
    assert_eq!(shared.read().state, PlayState::Paused);
    send(Action::Play);
    renderer.render_with_midi(&mut [0f32; 128], &mut actual);
    assert_eq!(
        shared.read().frame,
        cycle.position(paused as usize + 64) as f64
    );
    send(Action::Stop);
    renderer.render_with_midi(&mut [0f32; 128], &mut actual);
    assert_eq!(shared.read().frame, 0.);
}
