use minidaw_lib::project::{
    edit::{self, Clipboard, EditRequest},
    history::History,
    migrations,
    schema::*,
};
use serde_json::json;
fn fixture() -> Project {
    let mut p = Project::new();
    p.tracks.push(Track {
        synth: Default::default(),
        inserts: vec![],
        track_id: id(),
        name: "MIDI".into(),
        kind: TrackKind::Midi,
        instrument: Instrument::None,
        mix: TrackMix::default(),
        extensions: Extensions::new(),
        clips: vec![Clip::Midi(MidiClip {
            content_offset_tick: Signed(0),
            clip_id: id(),
            name: "Part".into(),
            start_tick: Signed(960000),
            length_tick: Signed(3840000),
            notes: [
                (240000, 480000, 60),
                (960000, 240000, 64),
                (1920000, 960000, 67),
            ]
            .into_iter()
            .map(|(start, length, pitch)| MidiNote {
                note_id: id(),
                start_tick: Signed(start),
                length_tick: Signed(length),
                pitch,
                velocity: 91,
                release_velocity: 7,
                channel: 2,
            })
            .collect(),
            controls: vec![MidiControl {
                event_id: id(),
                tick: Signed(100),
                channel: 2,
                data: MidiControlData::Cc {
                    controller: 64,
                    value: 127,
                },
            }],
        })],
    });
    p
}
fn request(p: &Project, field: &str, value: i64, absolute: bool) -> EditRequest {
    let c = p.midi_clips().next().unwrap();
    serde_json::from_value(json!({"command":"midi.note.time","clipIds":[c.clip_id],"noteId":c.notes[0].note_id,"noteIds":[c.notes[0].note_id,c.notes[1].note_id],"noteTimeField":field,"targetTick":value.to_string(),"absolute":absolute})).unwrap()
}
fn change(p: &Project, field: &str, value: i64, absolute: bool) -> Project {
    edit::apply(
        p,
        &request(p, field, value, absolute),
        &mut Clipboard::default(),
    )
    .unwrap()
}
fn positions(p: &Project) -> Vec<(i64, i64)> {
    let c = p.midi_clips().next().unwrap();
    c.notes
        .iter()
        .map(|n| (c.start_tick.0 + n.start_tick.0, n.length_tick.0))
        .collect()
}
#[test]
fn start_end_length_relative_and_absolute_are_integer_atomic_edits() {
    let p = fixture();
    let q = change(&p, "start", 1200001, false);
    assert_eq!(
        positions(&q),
        vec![(1200001, 480000), (1920001, 240000), (2880000, 960000)]
    );
    let r = change(&q, "end", 1680033, false);
    assert_eq!(
        positions(&r),
        vec![(1200001, 480032), (1920001, 240032), (2880000, 960000)]
    );
    let s = change(&r, "length", 240123, false);
    assert_eq!(
        positions(&s),
        vec![(1200001, 240123), (1920001, 123), (2880000, 960000)]
    );
    let q = change(&p, "start", 960001, true);
    assert_eq!(
        positions(&q),
        vec![(960001, 480000), (960001, 240000), (2880000, 960000)]
    );
    let q = change(&p, "end", 2400001, true);
    assert_eq!(
        positions(&q),
        vec![(1200000, 1200001), (1920000, 480001), (2880000, 960000)]
    );
    let q = change(&p, "length", 1, true);
    assert_eq!(
        positions(&q),
        vec![(1200000, 1), (1920000, 1), (2880000, 960000)]
    );
    for (a, b) in p
        .midi_clips()
        .next()
        .unwrap()
        .notes
        .iter()
        .zip(q.midi_clips().next().unwrap().notes.iter())
    {
        assert_eq!(
            (a.pitch, a.velocity, a.channel, a.release_velocity),
            (b.pitch, b.velocity, b.channel, b.release_velocity)
        );
    }
}
#[test]
fn part_expansion_keeps_unselected_notes_and_controllers_absolute() {
    let p = fixture();
    let q = change(&p, "start", 1, false);
    let c = q.midi_clips().next().unwrap();
    assert_eq!(c.start_tick.0, 1);
    assert_eq!(c.start_tick.0 + c.controls[0].tick.0, 960100);
    assert_eq!(positions(&q)[2], positions(&p)[2]);
    let q = change(&p, "end", 7680001, false);
    let c = q.midi_clips().next().unwrap();
    assert_eq!(c.start_tick.0 + c.length_tick.0, 8160001);
}
#[test]
fn invalid_group_changes_reject_without_partial_edits_and_no_drift() {
    let p = fixture();
    let bytes = migrations::encode(&p).unwrap();
    for (field, value, absolute) in [
        ("end", 1, false),
        ("length", 1, false),
        ("start", -1, true),
        ("length", 0, true),
        ("start", i64::MAX, true),
    ] {
        assert!(edit::apply(
            &p,
            &request(&p, field, value, absolute),
            &mut Clipboard::default()
        )
        .is_err());
        assert_eq!(migrations::encode(&p).unwrap(), bytes);
    }
    let mut q = p.clone();
    for _ in 0..1000 {
        q = change(&q, "start", 1200001, false);
        q = change(&q, "start", 1200000, false);
    }
    assert_eq!(q, p);
    let changed = change(&p, "length", 240123, true);
    let mut h = History::default();
    h.record(p.clone(), changed.clone(), "midi.note.time");
    assert_eq!(h.undo().unwrap(), p);
    assert_eq!(h.redo().unwrap(), changed);
    assert_eq!(
        migrations::decode(&migrations::encode(&changed).unwrap()).unwrap(),
        changed
    );
}
