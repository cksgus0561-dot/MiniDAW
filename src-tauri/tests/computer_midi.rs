use minidaw_lib::{
    audio::{
        metrics::AudioMetrics,
        midi::*,
        midi_input::{LiveReader, MidiInputService},
        renderer::{Action, Command, Renderer, COMMAND_CAPACITY},
        source::AudioSource,
        timeline::PlaybackPlan,
        transport::{PlayState, TransportCell},
    },
    project::{
        edit::{self, Clipboard},
        schema::*,
    },
};
use serde_json::json;
use std::{sync::Arc, time::Instant};
fn fixture() -> Project {
    let mut p = Project::new();
    for _ in 0..2 {
        p = edit::apply(
            &p,
            &serde_json::from_value(json!({"command":"midi.track.add"})).unwrap(),
            &mut Clipboard::default(),
        )
        .unwrap();
    }
    for t in &mut p.tracks {
        t.instrument = Instrument::BasicSynth;
    }
    p
}
#[derive(Default)]
struct Sink(Vec<MidiEvent>);
impl MidiSink for Sink {
    fn event(&mut self, e: MidiEvent) {
        self.0.push(e);
    }
}
#[test]
fn held_snapshot_chords_repeat_release_route_and_sample_clock() {
    let p = fixture();
    let plan = MidiPlan::compile(&p, 48000).unwrap();
    let service = MidiInputService::default();
    let mut reader = LiveReader::computer();
    let mut sink = Sink::default();
    let t = &p.tracks[0].track_id;
    service.computer_notes(Some(t), &[60, 64, 67]).unwrap();
    service.computer_notes(Some(t), &[60, 64, 67]).unwrap();
    reader.process(&service.computer, Some(&plan), 123456, 64, &mut sink);
    assert_eq!(sink.0.len(), 3);
    assert!(sink.0.iter().all(|e| e.voice >= 0x40000000 && e.voice <= i32::MAX as u32));
    assert!(sink
        .0
        .iter()
        .all(|e| e.on && e.frame == 123456 && e.offset == 0 && e.velocity == 100));
    service.computer_notes(Some(t), &[64, 67]).unwrap();
    reader.process(&service.computer, Some(&plan), 123520, 128, &mut sink);
    assert_eq!(
        (sink.0.last().unwrap().pitch, sink.0.last().unwrap().on),
        (60, false)
    );
    service
        .computer_notes(Some(&p.tracks[1].track_id), &[72])
        .unwrap();
    reader.process(&service.computer, Some(&plan), 123648, 128, &mut sink);
    assert_eq!(sink.0.iter().filter(|e| !e.on).count(), 3);
    assert_eq!(sink.0.last().unwrap().track, 1);
    service.computer_notes(None, &[]).unwrap();
    reader.process(&service.computer, Some(&plan), 123776, 128, &mut sink);
    assert!(!sink.0.last().unwrap().on);
    assert_eq!(service.computer_status().dropped, 0);
    assert!(service.computer_notes(Some(t), &[128]).is_err());
    assert!(service.computer_notes(Some(t), &[60; 33]).is_err());
    // Rapid On/Off still enters the queue in order, even in one render buffer.
    service.computer_notes(Some(t), &[60]).unwrap();
    service.computer_notes(Some(t), &[]).unwrap();
    let before = sink.0.len();
    reader.process(&service.computer, Some(&plan), 200000, 64, &mut sink);
    assert_eq!(
        sink.0[before..].iter().map(|e| e.on).collect::<Vec<_>>(),
        vec![true, false]
    );
}
#[test]
fn real_renderer_outputs_live_chord_while_stopped_and_keeps_usb_voice() {
    let p = fixture();
    let t = p.tracks[0].track_id.clone();
    let plan = PlaybackPlan::compile(Arc::new(p), Default::default(), 48000).unwrap();
    let key = plan.midi.track_keys[0];
    let source = AudioSource::timeline(plan, 0).unwrap();
    let service = MidiInputService::default();
    let (mut tx, rx) = rtrb::RingBuffer::new(COMMAND_CAPACITY);
    let state = Arc::new(TransportCell::default());
    let mut renderer = Renderer::new(
        rx,
        state.clone(),
        Arc::new(AudioMetrics::default()),
        48000,
        2,
    );
    for action in [
        Action::Load {
            clip_id: 1,
            audio: source,
        },
        Action::MidiInput(service.hub.clone()),
        Action::ComputerMidi(service.computer.clone()),
    ] {
        tx.push(Command {
            id: 0,
            issued: Instant::now(),
            action,
        })
        .unwrap_or_else(|_| panic!("queue"));
    }
    let mut out = [0f32; 2048];
    service.computer_notes(Some(&t), &[60, 64, 67]).unwrap();
    renderer.render(&mut out);
    assert!(out.iter().any(|s| s.abs() > 0.01));
    assert_eq!(state.read().state, PlayState::Stopped);
    service.hub.route(Some(key));
    service.hub.receive(&[0x90, 60, 100]);
    renderer.render(&mut out);
    service.computer_notes(None, &[]).unwrap();
    for _ in 0..10 {
        renderer.render(&mut out);
    }
    assert!(
        out.iter().any(|s| s.abs() > 0.001),
        "USB voice must survive computer release"
    );
    service.hub.receive(&[0x80, 60, 0]);
    for _ in 0..10 {
        renderer.render(&mut out);
    }
    assert!(out.iter().all(|s| *s == 0.));
}
