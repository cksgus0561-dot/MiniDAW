use minidaw_lib::{
    audio::midi_input::StepCapture,
    project::{
        edit::{self, Clipboard, EditRequest},
        schema::*,
    },
};
use serde_json::{json, Value};

fn attempt(p: &Project, r: Value) -> minidaw_lib::error::AppResult<Project> {
    edit::apply(
        p,
        &serde_json::from_value::<EditRequest>(r).unwrap(),
        &mut Clipboard::default(),
    )
}
fn edit(p: &Project, r: Value) -> Project {
    attempt(p, r).unwrap()
}
fn fixture() -> (Project, String, String) {
    let mut p = edit(&Project::new(), json!({"command":"midi.track.add"}));
    let t = p.tracks[0].track_id.clone();
    p = edit(
        &p,
        json!({"command":"midi.clip.add","trackIds":[t],"targetTick":"3840000","lengthTick":"3840000"}),
    );
    let c = p.midi_clips().next().unwrap().clip_id.clone();
    (p, t, c)
}
fn clip(p: &Project) -> &MidiClip {
    p.midi_clips().next().unwrap()
}
fn step(c: &str, start: i64, length: i64, pitches: &[u8], velocities: &[u8]) -> Value {
    json!({"command":"midi.note.step","clipIds":[c],"targetTick":start.to_string(),"lengthTick":length.to_string(),"pitches":pitches,"velocities":velocities})
}

#[test]
fn chord_is_one_transaction_with_played_velocities() {
    let (p, _, c) = fixture();
    let q = edit(&p, step(&c, 0, 240_000, &[64, 60, 67], &[90, 100, 110]));
    let notes = &clip(&q).notes;
    assert_eq!(notes.len(), 3);
    for (pitch, velocity) in [(64, 90), (60, 100), (67, 110)] {
        let n = notes.iter().find(|n| n.pitch == pitch).unwrap();
        assert_eq!((n.start_tick.0, n.length_tick.0, n.velocity, n.channel), (0, 240_000, velocity, 0));
    }
    // Velocities are optional (computer keys use the default).
    let r = edit(&q, step(&c, 240_000, 240_000, &[72], &[]));
    assert_eq!(clip(&r).notes.last().unwrap().velocity, 100);
    assert_eq!(clip(&r).start_tick.0, 3_840_000);
}

#[test]
fn entering_past_the_part_end_extends_it_to_the_right() {
    let (p, _, c) = fixture();
    let q = edit(&p, step(&c, 3_720_000, 240_000, &[60], &[]));
    assert_eq!(clip(&q).start_tick.0, 3_840_000);
    assert_eq!(clip(&q).length_tick.0, 3_960_000);
    // Inside the Part the length is unchanged.
    let r = edit(&q, step(&c, 0, 240_000, &[62], &[]));
    assert_eq!(clip(&r).length_tick.0, 3_960_000);
}

#[test]
fn reentering_a_step_overwrites_instead_of_stacking() {
    let (p, _, c) = fixture();
    let q = edit(&p, step(&c, 480_000, 240_000, &[60, 64], &[80, 80]));
    let r = edit(&q, step(&c, 480_000, 120_000, &[64, 67], &[120, 120]));
    let notes = &clip(&r).notes;
    assert_eq!(notes.len(), 3);
    let e = notes.iter().find(|n| n.pitch == 64).unwrap();
    assert_eq!((e.length_tick.0, e.velocity), (120_000, 120));
    let ids: std::collections::HashSet<_> = notes.iter().map(|n| &n.note_id).collect();
    assert_eq!(ids.len(), 3);
}

#[test]
fn key_signature_spells_but_never_changes_played_pitch() {
    let (p, t, c) = fixture();
    let p = edit(&p, json!({"command":"midi.track.keySignature","trackIds":[t],"keySignature":1}));
    // Mouse drawing writes F on the staff, which sounds F# in G major.
    let drawn = edit(
        &p,
        json!({"command":"midi.note.add","clipIds":[c],"targetTick":"0","lengthTick":"1000","pitch":65}),
    );
    assert_eq!(clip(&drawn).notes[0].pitch, 66);
    // A played F natural stays F natural (explicit accidental); F# follows the key.
    let q = edit(&p, step(&c, 0, 1000, &[65, 66], &[]));
    let notes = &clip(&q).notes;
    assert_eq!(notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), [65, 66]);
    assert!(notes.iter().all(|n| n.spelling.is_some()));
    // Keyless tracks keep the existing compact document form.
    let (p, _, c) = fixture();
    assert!(clip(&edit(&p, step(&c, 0, 1000, &[61], &[]))).notes[0].spelling.is_none());
}

#[test]
fn invalid_steps_change_nothing() {
    let (p, _, c) = fixture();
    for r in [
        step(&c, 0, 1000, &[], &[]),
        step(&c, 0, 1000, &[60, 60], &[]),
        step(&c, 0, 1000, &[128], &[]),
        step(&c, 0, 1000, &[60, 62], &[100]),
        step(&c, 0, 1000, &[60], &[0]),
        step(&c, 0, 1000, &[60], &[128]),
        step(&c, -1, 1000, &[60], &[]),
        step(&c, 0, 0, &[60], &[]),
        step(&c, i64::MAX, 1, &[60], &[]),
        json!({"command":"midi.note.step","clipIds":["00000000-0000-4000-8000-000000000000"],"targetTick":"0","lengthTick":"1","pitches":[60]}),
    ] {
        assert!(attempt(&p, r).is_err());
    }
    assert!(clip(&p).notes.is_empty());
}

#[test]
fn usb_capture_is_armed_only_while_step_mode_polls() {
    let s = StepCapture::default();
    s.capture(&[0x90, 60, 100]);
    assert!(s.take(true).is_empty(), "disarmed input is never queued");
    s.capture(&[0x91, 61, 90]);
    s.capture(&[0xb0, 64, 127]); // CC: not a step event
    s.capture(&[0x90, 62]); // malformed
    s.capture(&[0x81, 61, 0]);
    s.capture(&[0x90, 62, 0]); // Note On velocity 0 = Off, interpreted by the UI
    assert_eq!(s.take(true), vec![[0x91, 61, 90], [0x81, 61, 0], [0x90, 62, 0]]);
    assert!(s.take(true).is_empty());
    for _ in 0..1000 {
        s.capture(&[0x90, 60, 100]);
    }
    assert_eq!(s.take(true).len(), 256, "bounded queue drops instead of blocking");
    s.capture(&[0x90, 60, 100]);
    assert!(s.take(false).is_empty(), "disarming discards stale input");
    s.capture(&[0x90, 60, 100]);
    assert!(s.take(true).is_empty());
}
