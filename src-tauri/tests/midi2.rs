use minidaw_lib::{
    audio::{
        midi::*,
        midi_input::{LiveMidi, LiveReader},
    },
    project::{
        edit::{self, Clipboard, EditRequest},
        history::History,
        migrations,
        schema::*,
        smf,
        time::time,
    },
};
use serde_json::json;
fn fixture() -> Project {
    let mut p = Project::new();
    p.tracks.push(Track {
        synth: Default::default(),
        inserts: vec![],
        track_id: id(),
        name: "Keys".into(),
        kind: TrackKind::Midi,
        instrument: Instrument::None,
        mix: TrackMix::default(),
        extensions: Extensions::new(),
        clips: vec![Clip::Midi(MidiClip {
            content_offset_tick: Signed(0),
            clip_id: id(),
            name: "Part".into(),
            start_tick: Signed(123),
            length_tick: Signed(3_840_000),
            notes: vec![
                MidiNote {
                    note_id: id(),
                    start_tick: Signed(12345),
                    length_tick: Signed(234567),
                    pitch: 60,
                    velocity: 91,
                    release_velocity: 43,
                    channel: 2,
                },
                MidiNote {
                    note_id: id(),
                    start_tick: Signed(345678),
                    length_tick: Signed(456789),
                    pitch: 64,
                    velocity: 27,
                    release_velocity: 0,
                    channel: 2,
                },
            ],
            controls: vec![
                MidiControl {
                    event_id: id(),
                    tick: Signed(10),
                    channel: 2,
                    data: MidiControlData::Cc {
                        controller: 1,
                        value: 97,
                    },
                },
                MidiControl {
                    event_id: id(),
                    tick: Signed(20),
                    channel: 2,
                    data: MidiControlData::PitchBend { value: -3187 },
                },
                MidiControl {
                    event_id: id(),
                    tick: Signed(24000),
                    channel: 2,
                    data: MidiControlData::Cc {
                        controller: 64,
                        value: 127,
                    },
                },
                MidiControl {
                    event_id: id(),
                    tick: Signed(1_200_000),
                    channel: 2,
                    data: MidiControlData::Cc {
                        controller: 64,
                        value: 0,
                    },
                },
            ],
        })],
    });
    p
}
fn apply(p: &Project, mut r: serde_json::Value) -> Project {
    r["clipIds"] = json!([p.midi_clips().next().unwrap().clip_id]);
    r["noteIds"] = json!(p
        .midi_clips()
        .next()
        .unwrap()
        .notes
        .iter()
        .map(|n| &n.note_id)
        .collect::<Vec<_>>());
    edit::apply(
        p,
        &serde_json::from_value::<EditRequest>(r).unwrap(),
        &mut Clipboard::default(),
    )
    .unwrap()
}
#[test]
fn precise_quantize_idempotence_and_note_bounds() {
    let mut p = fixture();
    if let Clip::Midi(c) = &mut p.tracks[0].clips[0] {
        c.start_tick = Signed(0);
    }
    let start = p.clone();
    let mut h = History::default();
    p = apply(&p, json!({"command":"midi.velocity","velocity":127}));
    p = apply(&p, json!({"command":"midi.quantize","grid":"128"}));
    let q = p.clone();
    for _ in 0..1000 {
        p = apply(&p, json!({"command":"midi.quantize","grid":"128"}));
    }
    assert_eq!(p, q);
    for _ in 0..100 {
        p = apply(&p, json!({"command":"midi.transpose","semitones":1}));
        p = apply(&p, json!({"command":"midi.transpose","semitones":-1}));
    }
    assert_eq!(p, q);
    for (a, b) in p
        .midi_clips()
        .next()
        .unwrap()
        .notes
        .iter()
        .zip(start.midi_clips().next().unwrap().notes.iter())
    {
        assert_eq!(a.length_tick, b.length_tick);
        assert_eq!(a.start_tick.0 % 30000, 0);
        assert_eq!(a.pitch, b.pitch);
    }
    let c = p.midi_clips().next().unwrap();
    let bad:EditRequest=serde_json::from_value(json!({"command":"midi.transpose","clipIds":[c.clip_id],"noteIds":[c.notes[0].note_id,c.notes[1].note_id],"semitones":100})).unwrap();
    assert!(edit::apply(&p, &bad, &mut Clipboard::default()).is_err());
    h.record(start.clone(), p.clone(), "midi.quantize");
    assert_eq!(h.undo().unwrap(), start);
    assert_eq!(h.redo().unwrap(), p);
    assert_eq!(
        migrations::decode(&migrations::encode(&p).unwrap()).unwrap(),
        p
    );
    let mut legacy = serde_json::to_value(&start).unwrap();
    for n in legacy["tracks"][0]["clips"][0]["notes"]
        .as_array_mut()
        .unwrap()
    {
        for k in ["velocity", "releaseVelocity", "channel"] {
            n.as_object_mut().unwrap().remove(k);
        }
    }
    legacy["tracks"][0]["clips"][0]
        .as_object_mut()
        .unwrap()
        .remove("controls");
    let old = migrations::decode(&serde_json::to_vec(&legacy).unwrap()).unwrap();
    assert_eq!(old.midi_clips().next().unwrap().notes[0].velocity, 100);
    assert!(old.midi_clips().next().unwrap().controls.is_empty());
}
#[test]
fn smf_exact_and_standard_roundtrips_stale_metadata() {
    let mut p = fixture();
    p.musical_time.tempo_map = vec![
        Tempo {
            tick: Signed(0),
            bpm: 137.3,
        },
        Tempo {
            tick: Signed(960013),
            bpm: 91.25,
        },
    ];
    p.musical_time.time_signatures[0].numerator = 7;
    p.musical_time.time_signatures[0].denominator = 8;
    let bytes = smf::encode(&p).unwrap();
    let imported = smf::decode(&bytes).unwrap();
    assert_eq!(imported.musical_time, p.musical_time);
    assert_eq!(imported.tracks, p.tracks);
    let (again, _) = smf::merge(&p, imported, false).unwrap();
    assert_eq!(again.tracks.len(), 2);
    again.validate().unwrap();
    assert_ne!(again.tracks[0].track_id, again.tracks[1].track_id);
    let mut standard = midly::Smf::parse(&bytes).unwrap();
    assert_eq!(standard.header.format, midly::Format::Parallel);
    for t in &mut standard.tracks {
        t.retain(|e| {
            !matches!(
                e.kind,
                midly::TrackEventKind::Meta(midly::MetaMessage::SequencerSpecific(_))
            )
        });
    }
    let mut b = vec![];
    standard.write_std(&mut b).unwrap();
    let raw = smf::decode(&b).unwrap();
    let original = p.midi_clips().next().unwrap();
    let out = raw.tracks[0].clips[0].midi().unwrap();
    for (a, b) in original.notes.iter().zip(&out.notes) {
        assert!((a.start_tick.0 + original.start_tick.0 - b.start_tick.0).abs() <= 16);
        assert!(
            (a.start_tick.0 + original.start_tick.0 + a.length_tick.0
                - b.start_tick.0
                - b.length_tick.0)
                .abs()
                <= 16
        );
        assert_eq!(
            (a.pitch, a.velocity, a.release_velocity, a.channel),
            (b.pitch, b.velocity, b.release_velocity, b.channel)
        );
    }
    assert_eq!(
        out.controls
            .iter()
            .map(|e| (&e.data, e.channel))
            .collect::<Vec<_>>(),
        original
            .controls
            .iter()
            .map(|e| (&e.data, e.channel))
            .collect::<Vec<_>>()
    );
    // Simulate an external editor changing a standard NoteOn but leaving FF7F.
    let mut changed = midly::Smf::parse(&bytes).unwrap();
    for e in &mut changed.tracks[1] {
        if let midly::TrackEventKind::Midi {
            message: midly::MidiMessage::NoteOn { vel, .. },
            ..
        } = &mut e.kind
        {
            *vel = 55.into();
            break;
        }
    }
    let mut b = vec![];
    changed.write_std(&mut b).unwrap();
    let raw = smf::decode(&b).unwrap();
    assert_eq!(raw.tracks[0].clips[0].midi().unwrap().notes[0].velocity, 55);
    assert!(smf::decode(b"not midi").is_err());
}
#[test]
fn type_zero_ppq_480_tempo_and_zero_velocity_off() {
    use midly::*;
    let smf = Smf {
        header: Header::new(Format::SingleTrack, Timing::Metrical(480.into())),
        tracks: vec![vec![
            TrackEvent {
                delta: 0.into(),
                kind: TrackEventKind::Meta(MetaMessage::Tempo(500000.into())),
            },
            TrackEvent {
                delta: 121.into(),
                kind: TrackEventKind::Midi {
                    channel: 3.into(),
                    message: MidiMessage::NoteOn {
                        key: 69.into(),
                        vel: 96.into(),
                    },
                },
            },
            TrackEvent {
                delta: 239.into(),
                kind: TrackEventKind::Midi {
                    channel: 3.into(),
                    message: MidiMessage::NoteOn {
                        key: 69.into(),
                        vel: 0.into(),
                    },
                },
            },
            TrackEvent {
                delta: 0.into(),
                kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
            },
        ]],
    };
    let mut bytes = vec![];
    smf.write_std(&mut bytes).unwrap();
    let p = smf::decode(&bytes).unwrap();
    let n = &p.tracks[0].clips[0].midi().unwrap().notes[0];
    assert_eq!(
        (n.start_tick.0, n.length_tick.0, n.channel, n.velocity),
        (242000, 478000, 3, 96)
    );
}
#[derive(Default)]
struct Events {
    notes: Vec<MidiEvent>,
    controls: Vec<MidiControlEvent>,
}
impl MidiSink for Events {
    fn event(&mut self, e: MidiEvent) {
        self.notes.push(e);
    }
    fn control(&mut self, e: MidiControlEvent) {
        self.controls.push(e);
    }
}
#[test]
fn controller_sample_timing_chase_sustain_and_reset() {
    let p = fixture();
    for rate in [44100, 48000, 96000] {
        let plan = MidiPlan::compile(&p, rate).unwrap();
        let mut s = MidiScheduler::default();
        let mut events = Events::default();
        for f in 0..=plan.end {
            s.sample(&plan, f, f % 127, &mut events);
        }
        let c = p.midi_clips().next().unwrap();
        for e in &c.controls {
            let expected = time(
                &Position::Ticks {
                    ticks: Signed(c.start_tick.0 + e.tick.0),
                },
                &p.musical_time,
            )
            .ceil_frame(rate) as usize;
            assert!(events
                .controls
                .iter()
                .any(|v| v.frame == expected && v.channel == e.channel));
        }
        assert_eq!(events.notes[0].velocity, 91);
        assert_eq!(events.notes[1].velocity, 43);
        events = Events::default();
        let seek = plan.tones[0].end + 10;
        s.sample(&plan, seek, 0, &mut events);
        assert!(events
            .controls
            .iter()
            .any(|e| e.control == Control::Cc(64, 127)));
        assert_eq!(
            events.notes.iter().map(|e| e.on).collect::<Vec<_>>(),
            vec![true, false]
        );
        assert!(events.notes.iter().all(|e| e.frame == seek));
        s.reset(seek, 0, &mut events);
        assert!(events
            .controls
            .iter()
            .any(|e| e.control == Control::Cc(64, 0)));
        assert!(events
            .controls
            .iter()
            .any(|e| e.control == Control::PitchBend(0)));
    }
}
#[test]
fn native_input_queue_uses_rust_frame_and_handles_disconnect_overflow() {
    let p = fixture();
    let plan = MidiPlan::compile(&p, 48000).unwrap();
    let hub = LiveMidi::default();
    hub.route(Some(plan.track_keys[0]));
    let mut reader = LiveReader::default();
    let mut sink = Events::default();
    hub.receive(&[0x92, 60, 115]);
    hub.receive(&[0xb2, 64, 127]);
    hub.receive(&[0xe2, 0, 96]);
    hub.receive(&[0x82, 60, 31]);
    assert!(sink.notes.is_empty());
    reader.process(&hub, Some(&plan), 123456, 64, &mut sink);
    assert_eq!(
        sink.notes
            .iter()
            .map(|e| (e.frame, e.offset, e.on, e.velocity, e.channel))
            .collect::<Vec<_>>(),
        vec![(123456, 0, true, 115, 2), (123456, 0, false, 31, 2)]
    );
    assert_eq!(
        sink.controls.iter().map(|e| e.control).collect::<Vec<_>>(),
        vec![Control::Cc(64, 127), Control::PitchBend(4096)]
    );
    hub.receive(&[0x90, 65, 100]);
    reader.process(&hub, Some(&plan), 123520, 128, &mut sink);
    hub.route(None);
    reader.process(&hub, Some(&plan), 123648, 128, &mut sink);
    assert!(sink
        .notes
        .iter()
        .any(|e| e.pitch == 65 && !e.on && e.frame == 123648));
    assert!(sink
        .controls
        .iter()
        .any(|e| e.control == Control::Cc(64, 0)));
    hub.route(Some(plan.track_keys[0]));
    reader.process(&hub, Some(&plan), 10, 64, &mut sink);
    hub.receive(&[0x90, 60, 100]);
    reader.process(&hub, Some(&plan), 20, 64, &mut sink);
    let count = sink.notes.len();
    for _ in 0..1100 {
        hub.receive(&[0x90, 61, 100]);
    }
    reader.process(&hub, Some(&plan), 30, 64, &mut sink);
    assert_eq!(sink.notes.len(), count + 1);
    assert!(!sink.notes.last().unwrap().on);
    // Route removed from a new document: stale packets cannot target a new track.
    hub.receive(&[0x90, 70, 100]);
    reader.process(&hub, None, 40, 64, &mut sink);
    assert_eq!(sink.notes.len(), count + 1);
}

#[test]
fn quantize_extends_part_without_shifting_other_absolute_events() {
    let p = fixture();
    let c = p.midi_clips().next().unwrap();
    let r:EditRequest=serde_json::from_value(json!({"command":"midi.quantize","clipIds":[c.clip_id],"noteIds":[c.notes[0].note_id],"grid":"16"})).unwrap();
    let q = edit::apply(&p, &r, &mut Clipboard::default()).unwrap();
    let after = q.midi_clips().next().unwrap();
    assert_eq!(after.start_tick.0, 0);
    assert_eq!(after.notes[0].start_tick.0, 0);
    assert_eq!(after.notes[0].length_tick, c.notes[0].length_tick);
    assert_eq!(
        after.notes[1].start_tick.0,
        c.start_tick.0 + c.notes[1].start_tick.0
    );
    for (a, b) in after.controls.iter().zip(&c.controls) {
        assert_eq!(a.tick.0, c.start_tick.0 + b.tick.0);
    }
}
#[test]
fn legacy_ppq_import_preserves_destination_timebase_and_tempo_limits() {
    let p = fixture();
    let mut destination = Project::new();
    destination.musical_time.ticks_per_quarter = 480;
    let (merged, _) = smf::merge(
        &destination,
        smf::decode(&smf::encode(&p).unwrap()).unwrap(),
        true,
    )
    .unwrap();
    assert_eq!(merged.musical_time.ticks_per_quarter, 480);
    let c = merged.midi_clips().next().unwrap();
    assert!((c.notes[0].start_tick.0 * 2000 - 12468).abs() <= 1000);
    let mut slow = p;
    slow.musical_time.tempo_map[0].bpm = 1.;
    assert!(smf::encode(&slow).is_err());
}
