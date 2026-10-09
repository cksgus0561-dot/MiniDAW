use minidaw_lib::{
    audio::midi::MidiPlan,
    project::{
        edit::{self, Clipboard, EditRequest},
        history::History,
        migrations,
        schema::*,
        smf,
    },
};
use serde_json::json;
fn apply(p: &Project, value: serde_json::Value) -> Project {
    edit::apply(
        p,
        &serde_json::from_value::<EditRequest>(value).unwrap(),
        &mut Clipboard::default(),
    )
    .unwrap()
}
fn fixture() -> Project {
    let p = apply(&Project::new(), json!({"command":"midi.track.add"}));
    let mut p = apply(
        &p,
        json!({"command":"midi.clip.add","trackIds":[p.tracks[0].track_id],"targetTick":"960000","lengthTick":"3840000"}),
    );
    let id = p.midi_clips().next().unwrap().clip_id.clone();
    for (start, len, pitch) in [
        (123, 240001, 60),
        (960123, 960001, 64),
        (2880123, 480001, 67),
    ] {
        p = apply(
            &p,
            json!({"command":"midi.note.add","clipIds":[id],"targetTick":start.to_string(),"lengthTick":len.to_string(),"pitch":pitch}),
        );
    }
    apply(
        &p,
        json!({"command":"midi.control.put","clipIds":[id],"event":{"eventId":"","tick":"2880123","channel":0,"data":{"kind":"cc","controller":1,"value":80}}}),
    )
}
fn resize(p: &Project, side: &str, at: i64) -> Project {
    apply(
        p,
        json!({"command":"midi.clip.resize","clipIds":[p.midi_clips().next().unwrap().clip_id],"trimSide":side,"targetTick":at.to_string()}),
    )
}
#[test]
fn resize_preserves_source_events_absolute_positions_history_and_roundtrip() {
    let original = fixture();
    let before = original.midi_clips().next().unwrap().clone();
    let mut p = original.clone();
    for _ in 0..100 {
        p = resize(&p, "left", 1920123);
        p = resize(&p, "right", 2400124);
        let c = p.midi_clips().next().unwrap();
        assert_eq!(c.notes, before.notes);
        assert_eq!(c.controls, before.controls);
        assert_eq!(c.content_origin(), before.content_origin());
        assert_eq!(c.length_tick.0, 480001);
        p = migrations::decode(&migrations::encode(&p).unwrap()).unwrap();
        p = resize(&p, "left", 123);
        p = resize(&p, "right", 6000123);
        let c = p.midi_clips().next().unwrap();
        assert_eq!(c.notes, before.notes);
        assert_eq!(c.controls, before.controls);
        assert_eq!(c.content_origin(), before.content_origin());
        p = resize(&p, "left", 960000);
        p = resize(&p, "right", 4800000);
    }
    assert_eq!(p, original);
    let changed = resize(&resize(&p, "left", 1920123), "right", 1920124);
    let mut history = History::default();
    history.record(p.clone(), changed.clone(), "MIDI Resize");
    assert_eq!(history.undo().unwrap(), p);
    assert_eq!(history.redo().unwrap(), changed);
    let decoded = migrations::decode(&migrations::encode(&changed).unwrap()).unwrap();
    assert_eq!(decoded, changed);
    // Missing optional offset is the exact legacy zero-offset document.
    let mut legacy = serde_json::to_value(&original).unwrap();
    legacy["tracks"][0]["clips"][0]
        .as_object_mut()
        .unwrap()
        .remove("contentOffsetTick");
    assert_eq!(
        migrations::decode(&serde_json::to_vec(&legacy).unwrap()).unwrap(),
        original
    );
}
#[test]
fn part_window_resolves_before_unchanged_sample_clock_and_smf_export() {
    let original = fixture();
    let p = resize(&resize(&original, "left", 2160123), "right", 2640123);
    for rate in [44100, 48000] {
        let plan = MidiPlan::compile(&p, rate).unwrap();
        assert_eq!(plan.tones.len(), 1);
        assert_eq!(plan.tones[0].pitch, 64);
        assert_eq!(
            plan.tones[0].start,
            ((2160123f64 / 960000.0 * 0.5) * rate as f64).ceil() as usize
        );
        assert_eq!(
            plan.tones[0].end,
            ((2640123f64 / 960000.0 * 0.5) * rate as f64).ceil() as usize
        );
    }
    let hidden = resize(&resize(&original, "left", 4400000), "right", 4600000);
    assert!(MidiPlan::compile(&hidden, 48000).unwrap().is_empty());
    let path = std::env::temp_dir().join(format!("minidaw-part-resize-{}.mid", id()));
    smf::write(&path, &p).unwrap();
    let imported = smf::read(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    assert_eq!(imported.tracks, p.tracks);
}
#[test]
fn resized_part_remains_editable_without_moving_unselected_content() {
    let p = resize(&fixture(), "left", 1920000);
    let c = p.midi_clips().next().unwrap();
    let id = c.clip_id.clone();
    let note = c.notes[1].note_id.clone();
    let hidden = c.content_origin() + c.notes[0].start_tick.0;
    let p = apply(
        &p,
        json!({"command":"midi.note.change","clipIds":[id],"noteId":note,"targetTick":"124","lengthTick":"960001","pitch":65}),
    );
    let c = p.midi_clips().next().unwrap();
    assert_eq!(c.content_origin() + c.notes[1].start_tick.0, 1920124);
    assert_eq!(c.notes[1].length_tick.0, 960001);
    assert_eq!(c.content_origin() + c.notes[0].start_tick.0, hidden);
    let p = apply(
        &p,
        json!({"command":"midi.note.time","clipIds":[id],"noteId":note,"noteIds":[note],"noteTimeField":"start","targetTick":"1200123"}),
    );
    let c = p.midi_clips().next().unwrap();
    assert_eq!(c.content_origin() + c.notes[1].start_tick.0, 1200123);
    assert_eq!(c.content_origin() + c.notes[0].start_tick.0, hidden);
    let p = resize(&p, "left", 0);
    let c = p.midi_clips().next().unwrap();
    let data = c.notes.clone();
    let origin = c.content_origin();
    let moved = apply(
        &p,
        json!({"command":"midi.clip.move","clipIds":[id],"anchorClipId":id,"targetTick":"240000"}),
    );
    let c = moved.midi_clips().next().unwrap();
    assert_eq!(c.notes, data);
    assert_eq!(c.content_origin(), origin + 240000);
}
#[test]
fn invalid_resize_is_rejected_and_one_tick_is_valid() {
    let p = fixture();
    let c = p.midi_clips().next().unwrap();
    for (side, at) in [
        ("left", -1),
        ("left", 4800000),
        ("right", 960000),
        ("right", 9007199254740992i64),
    ] {
        let r=serde_json::from_value(json!({"command":"midi.clip.resize","clipIds":[c.clip_id],"trimSide":side,"targetTick":at.to_string()})).unwrap();
        assert!(edit::apply(&p, &r, &mut Clipboard::default()).is_err());
    }
    assert_eq!(
        resize(&p, "right", 960001)
            .midi_clips()
            .next()
            .unwrap()
            .length_tick
            .0,
        1
    );
}

#[test]
fn partially_hidden_note_can_still_be_resized_in_piano_roll() {
    let p = resize(&fixture(), "left", 2160123);
    let c = p.midi_clips().next().unwrap();
    let p = apply(
        &p,
        json!({"command":"midi.note.change","clipIds":[c.clip_id],"noteId":c.notes[1].note_id,"targetTick":"-240000","lengthTick":"720001","pitch":64}),
    );
    let c = p.midi_clips().next().unwrap();
    assert_eq!(c.content_origin() + c.notes[1].start_tick.0, 1920123);
    assert_eq!(c.notes[1].length_tick.0, 720001);
    assert_eq!(c.notes.len(), 3);
}
