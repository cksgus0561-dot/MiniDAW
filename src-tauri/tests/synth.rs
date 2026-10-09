use minidaw_lib::{
    audio::{
        metrics::AudioMetrics,
        midi::*,
        midi_input::LiveMidi,
        renderer::{Action, Command, Renderer, COMMAND_CAPACITY},
        source::AudioSource,
        synth::BasicSynth,
        timeline::PlaybackPlan,
        transport::{PlayState, TransportCell},
    },
    project::{
        edit::{self, Clipboard},
        history::History,
        migrations,
        schema::*,
    },
};
use serde_json::json;
use std::{sync::Arc, time::Instant};
fn apply(p: &Project, r: serde_json::Value) -> Project {
    edit::apply(
        p,
        &serde_json::from_value(r).unwrap(),
        &mut Clipboard::default(),
    )
    .unwrap()
}
fn project() -> Project {
    let p = apply(&Project::new(), json!({"command":"midi.track.add"}));
    apply(
        &p,
        json!({"command":"midi.track.instrument","trackIds":[p.tracks[0].track_id],"instrument":"basicSynth"}),
    )
}
fn note(id: u32, pitch: u8, velocity: u8, on: bool) -> MidiEvent {
    MidiEvent {
        frame: 0,
        offset: 0,
        voice: id,
        track: 0,
        channel: 0,
        pitch,
        velocity,
        on,
    }
}
fn control(control: Control) -> MidiControlEvent {
    MidiControlEvent {
        frame: 0,
        offset: 0,
        track: 0,
        channel: 0,
        control,
    }
}
fn synth(p: &Project, rate: u32) -> BasicSynth {
    let mut s = BasicSynth::new(rate);
    s.configure(Some(&MidiPlan::compile(p, rate).unwrap()));
    s
}
fn energy(s: &mut BasicSynth, frames: usize) -> f64 {
    (0..frames).map(|_| s.sample()[0].powi(2)).sum::<f64>() / frames as f64
}

#[test]
fn routing_roundtrip_history_and_legacy_silence() {
    let plain = apply(&Project::new(), json!({"command":"midi.track.add"}));
    assert_eq!(plain.tracks[0].instrument, Instrument::None);
    let mut s = synth(&plain, 48000);
    s.event(note(0, 69, 127, true));
    assert_eq!(energy(&mut s, 2048), 0.0);
    let p = apply(
        &plain,
        json!({"command":"midi.track.instrument","trackIds":[plain.tracks[0].track_id],"instrument":"basicSynth"}),
    );
    assert_eq!(
        migrations::decode(&migrations::encode(&p).unwrap()).unwrap(),
        p
    );
    let mut h = History::default();
    h.record(plain.clone(), p.clone(), "Instrument");
    assert_eq!(h.undo().unwrap(), plain);
    assert_eq!(h.redo().unwrap(), p);
    let mut legacy = serde_json::to_value(&p).unwrap();
    legacy["tracks"][0]
        .as_object_mut()
        .unwrap()
        .remove("instrument");
    assert_eq!(
        migrations::decode(&serde_json::to_vec(&legacy).unwrap()).unwrap(),
        plain
    );
    let mut invalid = p;
    invalid.tracks[0].kind = TrackKind::Audio;
    assert!(invalid.validate().is_err());
}
#[test]
fn velocity_pedal_bend_polyphony_and_routing_are_independent() {
    let p = project();
    let mut loud = synth(&p, 48000);
    let mut quiet = synth(&p, 48000);
    loud.event(note(1, 69, 127, true));
    quiet.event(note(1, 69, 64, true));
    let a = energy(&mut loud, 10000);
    let b = energy(&mut quiet, 10000);
    assert!((b / a - (64f64 / 127.0).powi(4)).abs() < 1e-12);
    loud.control(control(Control::Cc(64, 127)));
    loud.event(note(1, 69, 0, false));
    assert!(energy(&mut loud, 4800) > 0.001);
    loud.control(control(Control::Cc(64, 0)));
    energy(&mut loud, 2400);
    assert!(!loud.active());
    assert_eq!(energy(&mut loud, 256), 0.0);
    let mut bent = synth(&p, 48000);
    bent.control(control(Control::PitchBend(8191)));
    bent.event(note(2, 69, 127, true));
    let mut pitched = synth(&p, 48000);
    pitched.event(note(2, 71, 127, true));
    for _ in 0..48000 {
        assert!((bent.sample()[0] - pitched.sample()[0]).abs() < 1e-10);
    }
    let mut poly = synth(&p, 48000);
    for id in 0..256 {
        poly.event(note(id, 69, 127, true));
    }
    assert_eq!(poly.voice_count(), 256);
    for _ in 0..4800 {
        assert!(poly
            .sample()
            .iter()
            .all(|s| s.is_finite() && s.abs() <= 1.0));
    }
    poly.event(note(300, 60, 127, true));
    assert_eq!(poly.voice_count(), 256); // bounded, release-first/oldest stealing
    poly.control(control(Control::Cc(123, 0)));
    energy(&mut poly, 2400);
    assert!(!poly.active());
    let mut routed = synth(&p, 48000);
    let mut event = note(1, 69, 127, true);
    event.track = 1;
    routed.event(event);
    assert!(!routed.active());
}
fn send(tx: &mut rtrb::Producer<Command>, action: Action) {
    tx.push(Command {
        id: 1,
        issued: Instant::now(),
        action,
    })
    .ok()
    .unwrap();
}
#[test]
fn renderer_matches_sample_exact_synth_through_eof_and_release_tail() {
    const RATE: u32 = 48000;
    let p = project();
    let p = apply(
        &p,
        json!({"command":"midi.clip.add","trackIds":[p.tracks[0].track_id],"targetTick":"0","lengthTick":"144000"}),
    );
    let clip = p.midi_clips().next().unwrap().clip_id.clone();
    let p = apply(
        &p,
        json!({"command":"midi.note.add","clipIds":[clip],"targetTick":"123","lengthTick":"84001","pitch":69,"velocity":100}),
    );
    let p = apply(
        &p,
        json!({"command":"midi.control.put","clipIds":[clip],"event":{"eventId":"","tick":"40000","channel":0,"data":{"kind":"cc","controller":64,"value":127}}}),
    );
    let p = apply(
        &p,
        json!({"command":"midi.control.put","clipIds":[clip],"event":{"eventId":"","tick":"88000","channel":0,"data":{"kind":"pitchBend","value":4096}}}),
    );
    let plan = PlaybackPlan::compile(Arc::new(p.clone()), Default::default(), RATE).unwrap();
    let source = AudioSource::timeline(plan.clone(), 0).unwrap();
    let (mut tx, rx) = rtrb::RingBuffer::new(COMMAND_CAPACITY);
    let state = Arc::new(TransportCell::default());
    let mut r = Renderer::new(
        rx,
        state.clone(),
        Arc::new(AudioMetrics::default()),
        RATE,
        2,
    );
    send(
        &mut tx,
        Action::Load {
            clip_id: 1,
            audio: source.clone(),
        },
    );
    send(&mut tx, Action::Play);
    let mut reference = synth(&p, RATE);
    let mut scheduler = MidiScheduler::default();
    let mut at = 0;
    for count in [1, 17, 64, 257].into_iter().cycle().take(90) {
        let mut out = vec![0f32; count * 2];
        r.render(&mut out);
        for pair in out.as_chunks::<2>().0 {
            if at < plan.frames {
                scheduler.sample(&plan.midi, at, 0, &mut reference);
            }
            let expected = reference.sample();
            assert_eq!(
                *pair,
                [expected[0] as f32, expected[1] as f32],
                "frame {at}"
            );
            at += 1;
            if at == plan.frames {
                scheduler.sample(&plan.midi, at, 0, &mut reference);
                scheduler.reset(at, 0, &mut reference);
            }
        }
    }
    assert_eq!(state.read().state, PlayState::Stopped);
    assert!(!reference.active());
    assert_eq!(source.snapshot().starvation, 0);
}
#[test]
fn live_keyboard_reaches_master_when_stopped_and_transport_clears_voices() {
    let p = project();
    let plan = PlaybackPlan::compile(Arc::new(p), Default::default(), 48000).unwrap();
    let key = plan.midi.track_keys[0];
    let source = AudioSource::timeline(plan, 0).unwrap();
    let hub = Arc::new(LiveMidi::default());
    hub.route(Some(key));
    let (mut tx, rx) = rtrb::RingBuffer::new(COMMAND_CAPACITY);
    let state = Arc::new(TransportCell::default());
    let mut r = Renderer::new(
        rx,
        state.clone(),
        Arc::new(AudioMetrics::default()),
        48000,
        2,
    );
    send(
        &mut tx,
        Action::Load {
            clip_id: 1,
            audio: source.clone(),
        },
    );
    send(&mut tx, Action::MidiInput(hub.clone()));
    hub.receive(&[0x90, 69, 100]);
    let mut out = [0f32; 2048];
    r.render(&mut out);
    assert!(out.iter().any(|x| x.abs() > 0.01));
    assert_eq!(state.read().state, PlayState::Stopped);
    hub.receive(&[0xb0, 64, 127]);
    hub.receive(&[0x80, 69, 0]);
    r.render(&mut out);
    assert!(out.iter().any(|x| x.abs() > 0.01));
    hub.receive(&[0xb0, 64, 0]);
    for _ in 0..3 {
        r.render(&mut out);
    }
    assert!(out.iter().all(|x| *x == 0.0));
    for action in [
        Action::Stop,
        Action::Pause,
        Action::Seek(0.0),
        Action::Unload,
    ] {
        hub.receive(&[0x90, 69, 100]);
        r.render(&mut out);
        assert!(out.iter().any(|x| x.abs() > 0.01));
        send(&mut tx, action);
        r.render(&mut out);
        assert!(out.iter().all(|x| *x == 0.0));
    }
}
