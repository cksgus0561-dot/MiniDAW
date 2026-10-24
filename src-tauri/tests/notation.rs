use minidaw_lib::{
    audio::midi::MidiPlan,
    project::{
        edit::{self, Clipboard, EditRequest},
        history::History,
        migrations,
        notation::{Accidental, Spelling},
        schema::*,
        smf,
    },
};
use serde_json::{json, Value};

fn edit(p: &Project, r: Value) -> Project {
    attempt(p, r).unwrap()
}
fn attempt(p: &Project, r: Value) -> minidaw_lib::error::AppResult<Project> {
    edit::apply(
        p,
        &serde_json::from_value::<EditRequest>(r).unwrap(),
        &mut Clipboard::default(),
    )
}
fn fixture() -> Project {
    let mut p = Project::new();
    for _ in 0..2 {
        p = edit(&p, json!({"command":"midi.track.add"}));
        let t = p.tracks.last().unwrap().track_id.clone();
        for offset in [0, 3_840_000] {
            p = edit(
                &p,
                json!({"command":"midi.clip.add","trackIds":[t],"targetTick":offset.to_string(),"lengthTick":"3840000"}),
            );
            let c = p
                .tracks
                .last()
                .unwrap()
                .clips
                .last()
                .unwrap()
                .id()
                .to_owned();
            for (i, pitch) in [53, 65, 77, 71, 60, 66].into_iter().enumerate() {
                p = edit(
                    &p,
                    json!({"command":"midi.note.add","clipIds":[c],"targetTick":(123+i*300000).to_string(),"lengthTick":"234567","pitch":pitch,"velocity":87}),
                );
            }
        }
    }
    p
}
fn key(p: &Project, track: usize, k: i8) -> Project {
    edit(
        p,
        json!({"command":"midi.track.keySignature","trackIds":[p.tracks[track].track_id],"keySignature":k}),
    )
}
fn accidental(p: &Project, indices: &[usize], a: &str) -> Project {
    let c = p.tracks[0].clips[0].midi().unwrap();
    edit(
        p,
        json!({"command":"midi.note.accidental","clipIds":[c.clip_id],"noteIds":indices.iter().map(|i|c.notes[*i].note_id.clone()).collect::<Vec<_>>(),"accidental":a}),
    )
}
fn notes(p: &Project) -> &[MidiNote] {
    &p.tracks[0].clips[0].midi().unwrap().notes
}

#[test]
fn key_signature_track_scope_all_parts_octaves_no_accumulation() {
    let base = fixture();
    let p = key(&base, 0, 1);
    assert_eq!(p.tracks[1], base.tracks[1]);
    for c in p.tracks[0].clips.iter().filter_map(Clip::midi) {
        assert_eq!(
            c.notes.iter().map(|n| n.pitch).collect::<Vec<_>>(),
            vec![54, 66, 78, 71, 60, 66]
        );
        assert_eq!(c.notes[5].spelling.unwrap().accidental, Accidental::Sharp);
    }
    let mut q = p.clone();
    for _ in 0..100 {
        q = key(&q, 0, -2);
        q = key(&q, 0, 7);
        q = key(&q, 0, 0);
        q = key(&q, 0, 1);
    }
    assert_eq!(p, q);
    let off = key(&p, 0, 0);
    assert_eq!(
        notes(&off).iter().map(|n| n.pitch).collect::<Vec<_>>(),
        notes(&base).iter().map(|n| n.pitch).collect::<Vec<_>>()
    );
    for (a, b) in notes(&p).iter().zip(notes(&base)) {
        assert_eq!(
            (a.start_tick, a.length_tick, a.velocity, a.channel),
            (b.start_tick, b.length_tick, b.velocity, b.channel)
        );
    }
}

#[test]
fn per_note_absolute_accidentals_natural_and_reset_do_not_cascade() {
    let p = key(&fixture(), 0, 1);
    let natural = accidental(&p, &[1], "natural");
    assert_eq!(notes(&natural)[1].pitch, 65);
    assert_eq!(notes(&natural)[0].pitch, 54);
    let sharp = accidental(&natural, &[1], "sharp");
    assert_eq!(notes(&sharp)[1].pitch, 66);
    assert_eq!(sharp, accidental(&sharp, &[1], "sharp"));
    let flat = accidental(&sharp, &[1], "flat");
    assert_eq!(notes(&flat)[1].pitch, 64);
    assert_eq!(notes(&key(&flat, 0, -2))[1].pitch, 64);
    let reset = accidental(&flat, &[1], "key");
    assert_eq!(notes(&reset)[1], notes(&p)[1]);
    let group = accidental(&p, &[0, 1, 2], "natural");
    assert_eq!(notes(&group)[0].pitch, 53);
    assert_eq!(notes(&group)[2].pitch, 77);
    assert_eq!(group.tracks[0].clips[1], p.tracks[0].clips[1]);
}

#[test]
fn written_input_mouse_edits_quantize_transpose_and_track_transfer() {
    let p = key(&fixture(), 0, 1);
    let c = p.tracks[0].clips[0].midi().unwrap();
    let mut q = edit(
        &p,
        json!({"command":"midi.note.add","clipIds":[c.clip_id],"targetTick":"2000000","lengthTick":"123456","pitch":65}),
    );
    assert_eq!(notes(&q).last().unwrap().pitch, 66);
    let n = notes(&q)[1].clone();
    let c = c.clip_id.clone();
    q = edit(
        &q,
        json!({"command":"midi.note.change","clipIds":[c],"noteId":n.note_id,"targetTick":"333333","lengthTick":"456789","pitch":n.pitch}),
    );
    assert_eq!(notes(&q)[1].spelling, n.spelling);
    q = edit(
        &q,
        json!({"command":"midi.note.change","clipIds":[c],"noteId":n.note_id,"targetTick":"333333","lengthTick":"456789","pitch":65}),
    );
    assert_eq!(
        notes(&q)[1].spelling.unwrap().accidental,
        Accidental::Natural
    );
    q = edit(
        &q,
        json!({"command":"midi.transpose","clipIds":[c],"noteIds":[n.note_id],"semitones":1}),
    );
    assert_eq!(notes(&q)[1].pitch, 66);
    q = edit(
        &q,
        json!({"command":"midi.quantize","clipIds":[c],"noteIds":[n.note_id],"grid":"16"}),
    );
    assert_eq!(notes(&q)[1].start_tick.0 % 240000, 0);
    q = key(&q, 1, -2);
    q = edit(
        &q,
        json!({"command":"clip.moveTrack","clipIds":[c],"targetTrackId":q.tracks[1].track_id}),
    );
    let moved = q.tracks[1]
        .clips
        .iter()
        .find(|x| x.id() == c)
        .unwrap()
        .midi()
        .unwrap();
    assert_eq!(moved.notes[1].pitch, 65);
    assert_eq!(moved.notes[1].length_tick.0, 456789);
    q.validate().unwrap();
    let p = key(&fixture(), 1, 1);
    let clip = p.tracks[0].clips[0].id().to_owned();
    let q = edit(
        &p,
        json!({"command":"clip.moveTrack","clipIds":[clip],"targetTrackId":p.tracks[1].track_id}),
    );
    let moved = q.tracks[1].clips.last().unwrap().midi().unwrap();
    assert_eq!(
        moved.notes[1].pitch, 66,
        "Previously unmarked F follows destination key"
    );
    assert_eq!(
        moved.notes[5].pitch, 66,
        "Existing chromatic F sharp remains explicit"
    );
}

#[test]
fn persistence_history_backward_compatibility_and_actual_scheduled_export_pitch() {
    let old = fixture();
    let p = accidental(&key(&old, 0, 1), &[1], "flat");
    let mut history = History::default();
    history.record(old.clone(), p.clone(), "notation");
    assert_eq!(history.undo().unwrap(), old);
    assert_eq!(history.redo().unwrap(), p);
    assert_eq!(
        migrations::decode(&migrations::encode(&p).unwrap()).unwrap(),
        p
    );
    let legacy = migrations::encode(&old).unwrap();
    assert!(!String::from_utf8_lossy(&legacy).contains("keySignature"));
    assert!(!String::from_utf8_lossy(&legacy).contains("spelling"));
    assert_eq!(migrations::decode(&legacy).unwrap(), old);
    for rate in [44100, 48000, 96000] {
        let plan = MidiPlan::compile(&p, rate).unwrap();
        let original = MidiPlan::compile(&old, rate).unwrap();
        assert_eq!(plan.tones[1].pitch, 64);
        for (a, b) in plan.tones.iter().zip(&original.tones) {
            assert_eq!(
                (a.start, a.end, a.velocity, a.channel),
                (b.start, b.end, b.velocity, b.channel)
            );
        }
    }
    let imported = smf::decode(&smf::encode(&p).unwrap()).unwrap();
    let pitches = imported
        .tracks
        .iter()
        .flat_map(|t| t.clips.iter().filter_map(Clip::midi))
        .flat_map(|c| c.notes.iter().map(|n| n.pitch))
        .collect::<Vec<_>>();
    assert_eq!(
        pitches,
        p.midi_clips()
            .flat_map(|c| c.notes.iter().map(|n| n.pitch))
            .collect::<Vec<_>>()
    );
}

#[test]
fn bounds_and_invalid_inputs_are_atomic() {
    let mut p = fixture();
    let c = p.tracks[0].clips[0].midi().unwrap().clip_id.clone();
    p = edit(
        &p,
        json!({"command":"midi.note.add","clipIds":[c],"targetTick":"2000000","lengthTick":"1000","pitch":127}),
    );
    let before = p.clone();
    assert!(attempt(&p,json!({"command":"midi.track.keySignature","trackIds":[p.tracks[0].track_id],"keySignature":7})).is_err());
    assert_eq!(p, before);
    assert!(attempt(&p,json!({"command":"midi.track.keySignature","trackIds":[p.tracks[0].track_id],"keySignature":8})).is_err());
    assert!(attempt(&p,json!({"command":"midi.note.accidental","clipIds":[c],"noteIds":["missing"],"accidental":"sharp"})).is_err());
    assert!(Spelling {
        natural_pitch: 0,
        accidental: Accidental::Flat
    }
    .resolve(0)
    .is_err());
    // Enharmonic spellings across octave boundaries stay exact.
    assert_eq!(
        Spelling {
            natural_pitch: 59,
            accidental: Accidental::Sharp
        }
        .resolve(0)
        .unwrap(),
        60
    );
    assert_eq!(
        Spelling {
            natural_pitch: 60,
            accidental: Accidental::Flat
        }
        .resolve(0)
        .unwrap(),
        59
    );
}

#[test]
fn all_fifteen_signatures_follow_standard_sharp_flat_order() {
    use minidaw_lib::project::notation::key_offset;
    for key in -7i8..=7 {
        let order = if key > 0 {
            [5, 0, 7, 2, 9, 4, 11]
        } else {
            [11, 4, 9, 2, 7, 0, 5]
        };
        for octave in [2, 4, 7] {
            for (i, pc) in order.into_iter().enumerate() {
                let natural_pitch = octave * 12 + pc;
                let expected = if i < key.unsigned_abs() as usize {
                    i16::from(key.signum())
                } else {
                    0
                };
                assert_eq!(key_offset(natural_pitch, key), expected);
                assert_eq!(
                    Spelling {
                        natural_pitch,
                        accidental: Accidental::Key
                    }
                    .resolve(key)
                    .unwrap() as i16,
                    natural_pitch + expected
                );
            }
        }
    }
}
