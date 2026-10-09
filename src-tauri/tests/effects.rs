use minidaw_lib::{
    audio::{
        decoder::{AudioData, FileInfo},
        effects::Chain,
        metrics::AudioMetrics,
        renderer::{Action, Command, Renderer, COMMAND_CAPACITY},
        source::{AudioAsset, AudioSource},
        streaming::FrameReader,
        timeline::{PlaybackPlan, TimelineReader},
        transport::TransportCell,
    },
    error::AppResult,
    project::{
        edit::{self, Clipboard},
        effects::{Effect, Processor},
        runtime::ProjectAudio,
        schema::*,
        session::ProjectService,
    },
};
use serde_json::json;
use std::{cell::Cell, collections::HashMap, sync::Arc, time::Instant};
fn effect(kind: &str) -> Effect {
    Effect::new(kind).unwrap()
}
fn gain_eq(db: f64) -> Effect {
    let mut e = effect("eq");
    if let Processor::Eq { bands } = &mut e.processor {
        bands[1].frequency = 1000.;
        bands[1].gain_db = db;
        bands[1].q = 2.;
    }
    e
}
fn tone_gain(e: Effect, rate: u32, hz: f64) -> f64 {
    let mut c = Chain::new(&[e], rate, 120.);
    let (mut before, mut after) = (0., 0.);
    for i in 0..rate * 2 {
        let v = (std::f64::consts::TAU * hz * i as f64 / rate as f64).sin() * 0.1;
        let y = c.process([v, v]);
        if i >= rate {
            before += v * v;
            after += y[0] * y[0];
        }
    }
    10. * (after / before).log10()
}
#[test]
fn eq_gain_frequency_q_and_exact_bypass_at_major_rates() {
    for rate in [44100, 48000, 96000] {
        for gain in [-12., 6., 12.] {
            assert!((tone_gain(gain_eq(gain), rate, 1000.) - gain).abs() < 0.01);
        }
        assert!(tone_gain(gain_eq(12.), rate, 100.) < 0.1);
        let mut e = gain_eq(24.);
        e.enabled = false;
        let mut c = Chain::new(&[e], rate, 120.);
        for i in 0..1000 {
            let x = [(i as f64).sin(), (i as f64 * 0.173).cos()];
            assert_eq!(c.process(x), x);
        }
    }
}
#[test]
fn compressor_steady_ratio_makeup_stereo_and_gain_reduction() {
    let mut e = effect("compressor");
    e.processor = Processor::Compressor {
        threshold_db: -24.,
        ratio: 4.,
        attack_ms: 5.,
        release_ms: 100.,
        makeup_db: 3.,
    };
    let mut c = Chain::new(&[e], 48000, 120.);
    let mut y = [0.; 2];
    for _ in 0..48000 {
        y = c.process([0.5, -0.25]);
    }
    let expected = -24. + (20. * 0.5f64.log10() + 24.) / 4. + 3.;
    assert!((20. * y[0].log10() - expected).abs() < 0.001);
    assert_eq!(y[1], -y[0] / 2.);
    let gr = c.reduction[0];
    assert!((gr as f64 - (20. * 0.5f64.log10() - expected + 3.)).abs() < 0.001);
    c.process([0.; 2]);
    assert!(c.reduction[0] > gr * 0.99);
    for _ in 0..96000 {
        c.process([0.; 2]);
    }
    assert_eq!(c.reduction[0], 0.);
}
#[test]
fn limiter_stereo_peak_ceiling_and_order_are_real() {
    for rate in [44100, 48000, 96000] {
        let mut e = effect("limiter");
        e.processor = Processor::Limiter {
            ceiling_db: -3.,
            input_db: 12.,
        };
        let mut c = Chain::new(&[e], rate, 120.);
        let ceiling = 10f64.powf(-3. / 20.);
        for i in 0..rate {
            let v = if i % 127 == 0 {
                40.
            } else {
                (i as f64 * 0.137).sin() * 2.
            };
            let y = c.process([v, -v * 0.5]);
            assert!(y.iter().all(|v| v.abs() <= ceiling));
            assert!((y[0] * 0.5 + y[1]).abs() < 1e-12);
        }
        assert!(c.reduction[0] > 0.);
    }
    let eq = gain_eq(18.);
    let mut limiter = effect("limiter");
    limiter.processor = Processor::Limiter {
        ceiling_db: -12.,
        input_db: 0.,
    };
    let mut a = Chain::new(&[eq.clone(), limiter.clone()], 48000, 120.);
    let mut b = Chain::new(&[limiter, eq], 48000, 120.);
    let (mut peak_a, mut peak_b) = (0f64, 0f64);
    for i in 0..48000 {
        let x = [(std::f64::consts::TAU * 1000. * i as f64 / 48000.).sin() * 0.5; 2];
        peak_a = peak_a.max(a.process(x)[0].abs());
        peak_b = peak_b.max(b.process(x)[0].abs());
    }
    assert!(peak_a <= 10f64.powf(-12. / 20.));
    assert!(peak_b > peak_a * 3.);
}
#[test]
fn delay_impulses_feedback_wet_tempo_and_reset() {
    for rate in [44100, 48000, 96000] {
        let mut e = effect("delay");
        e.processor = Processor::Delay {
            time_ms: 123.,
            feedback: 0.5,
            wet: 0.4,
            sync_beats: Some(0.5),
        };
        for bpm in [120., 150.] {
            let n = (0.5 * 60. / bpm * rate as f64).round() as usize;
            let mut c = Chain::new(&[e.clone()], rate, bpm);
            for i in 0..n * 3 + 1 {
                let x = if i == 0 { [1., -0.5] } else { [0.; 2] };
                let y = c.process(x);
                let expected = if i == 0 {
                    0.6
                } else if i == n {
                    0.4
                } else if i == n * 2 {
                    0.2
                } else if i == n * 3 {
                    0.1
                } else {
                    0.
                };
                assert!((y[0] - expected).abs() < 1e-7, "{i}/{n}");
                assert!((y[1] + y[0] * 0.5).abs() < 1e-7);
            }
            c.reset();
            for _ in 0..n * 2 {
                assert_eq!(c.process([0.; 2]), [0.; 2]);
            }
        }
    }
}
#[test]
fn reverb_stereo_decay_tail_is_stable_and_reset_is_clean() {
    for rate in [44100, 48000, 96000] {
        let mut e = effect("reverb");
        e.processor = Processor::Reverb { decay: 1., wet: 1. };
        let mut c = Chain::new(&[e], rate, 120.);
        let (mut early, mut late, mut difference) = (0., 0., 0.);
        for i in 0..rate * 4 {
            let y = c.process(if i == 0 { [1., 0.] } else { [0.; 2] });
            assert!(y.iter().all(|v| v.is_finite() && v.abs() < 2.));
            let energy = y[0] * y[0] + y[1] * y[1];
            if i < rate / 2 {
                early += energy;
            }
            if i >= rate * 2 {
                late += energy;
            }
            difference += (y[0] - y[1]).abs();
        }
        assert!(early > 0.01 && late < early * 0.0001 && difference > 0.01);
        c.reset();
        for _ in 0..rate / 2 {
            assert_eq!(c.process([0.; 2]), [0.; 2]);
        }
    }
}
fn audio_document() -> (Project, Arc<AudioAsset>) {
    let a = AudioAsset::memory(AudioData {
        info: FileInfo {
            name: "constant".into(),
            sample_rate: 48000,
            channels: 2,
            frames: 48000,
            duration: 1.,
            sanitized_samples: 0,
        },
        samples: vec![0.5; 96000],
    });
    let mut p = Project::new();
    p.import(Asset {
        asset_id: id(),
        filename: "constant.wav".into(),
        path: PathReference {
            project_relative_path: Some("constant.wav".into()),
            original_absolute_path: None,
        },
        metadata: AudioMetadata {
            sample_rate: 48000,
            channels: 2,
            source_frames: Frames(48000),
            container: "wav".into(),
            codec: None,
        },
        fingerprint: Fingerprint {
            file_bytes: Frames(100),
            sampled_sha256: "a".repeat(64),
        },
        extensions: Default::default(),
    });
    (p, a)
}
#[test]
fn audio_track_chain_processes_sum_once_and_drains_delay_after_clip() {
    let (mut p, a) = audio_document();
    let mut second = p.tracks[0].clips[0].clone();
    if let Clip::Audio(c) = &mut second {
        c.clip_id = id();
    }
    p.tracks[0].clips.push(second);
    let comp = effect("compressor");
    p.tracks[0].inserts = vec![comp.clone()];
    let plan = PlaybackPlan::compile(
        Arc::new(p.clone()),
        HashMap::from([(p.assets[0].asset_id.clone(), a.clone())]),
        48000,
    )
    .unwrap();
    let mut r = TimelineReader::new(plan);
    let mut c = Chain::new(&[comp], 48000, 120.);
    for _ in 0..48000 {
        assert!((r.read_frame().unwrap()[0] as f64 - c.process([1.; 2])[0]).abs() < 1e-7);
    }
    p.tracks[0].inserts = vec![effect("delay")];
    let plan = PlaybackPlan::compile(
        Arc::new(p.clone()),
        HashMap::from([(p.assets[0].asset_id.clone(), a)]),
        48000,
    )
    .unwrap();
    assert!(plan.frames > 48000);
    let mut r = TimelineReader::new(plan);
    for _ in 0..48000 {
        r.read_frame().unwrap();
    }
    assert!(r.read_frame().unwrap()[0] > 0.1);
    r.seek(60000, Arc::new(|| false)).unwrap();
    assert_eq!(r.read_frame().unwrap(), [0.; 2]);
}
#[derive(Default)]
struct Runtime {
    installs: Cell<usize>,
    updates: Cell<usize>,
}
impl ProjectAudio for Runtime {
    type Prepared = Arc<AudioAsset>;
    fn prepare_asset(&self, a: Self::Prepared) -> AppResult<Self::Prepared> {
        Ok(a)
    }
    fn install(&self, _: Option<Self::Prepared>) -> AppResult<()> {
        self.installs.set(self.installs.get() + 1);
        Ok(())
    }
    fn effects(&self, _: &Project) {
        self.updates.set(self.updates.get() + 1);
    }
}
#[test]
fn insert_transactions_reorder_bypass_history_and_storage() {
    let s = ProjectService::new(None);
    let r = Runtime::default();
    let apply = |v| {
        s.edit(
            &r,
            s.revision().unwrap(),
            serde_json::from_value(v).unwrap(),
        )
        .unwrap()
    };
    for kind in ["eq", "compressor", "limiter", "reverb", "delay"] {
        apply(json!({"command":"effect.add","effectKind":kind}));
    }
    let p = s.view().unwrap().document;
    let id = &p.master.inserts[2].effect_id;
    apply(json!({"command":"effect.move","effectId":id,"direction":1}));
    assert_eq!(s.view().unwrap().document.master.inserts[3].effect_id, *id);
    let mut e = p.master.inserts[2].clone();
    e.enabled = false;
    apply(json!({"command":"effect.set","effectId":id,"effect":e}));
    assert!(!s.view().unwrap().document.master.inserts[3].enabled);
    apply(json!({"command":"edit.undo"}));
    assert!(s.view().unwrap().document.master.inserts[3].enabled);
    apply(json!({"command":"edit.redo"}));
    assert_eq!(r.installs.get(), 0);
    assert!(r.updates.get() > 5);
    let folder =
        std::env::temp_dir().join(format!("minidaw-fx-{}", minidaw_lib::project::schema::id()));
    std::fs::create_dir(&folder).unwrap();
    let path = folder.join("fx.minidaw");
    s.save(Some(&path), s.revision().unwrap()).unwrap();
    let saved = s.view().unwrap().document;
    s.new_project(&r, s.revision().unwrap(), true).unwrap();
    s.open(&r, &path, s.revision().unwrap(), true).unwrap();
    assert_eq!(s.view().unwrap().document, saved);
    std::fs::remove_dir_all(folder).unwrap();
    let legacy = serde_json::to_value(Project::new()).unwrap();
    assert_eq!(
        serde_json::from_value::<Project>(legacy)
            .unwrap()
            .master
            .inserts
            .len(),
        0
    );
}
#[test]
fn midi_and_master_effects_do_not_change_sample_clock_or_output_when_bypassed() {
    let mut p = Project::new();
    let request = serde_json::from_value(json!({"command":"midi.track.add"})).unwrap();
    p = edit::apply(&p, &request, &mut Clipboard::default()).unwrap();
    p.tracks[0].instrument = Instrument::BasicSynth;
    p.tracks[0].clips.push(Clip::Midi(MidiClip {
        clip_id: id(),
        name: "Part".into(),
        start_tick: Signed(0),
        length_tick: Signed(1920000),
        content_offset_tick: Signed(0),
        notes: vec![MidiNote {
            note_id: id(),
            start_tick: Signed(12345),
            length_tick: Signed(960000),
            pitch: 69,
            velocity: 100,
            release_velocity: 0,
            channel: 0,
        }],
        controls: vec![],
    }));
    fn render(p: Project) -> (Vec<f32>, usize) {
        let metrics = Arc::new(AudioMetrics::default());
        metrics.effects.configure(&p);
        let plan = PlaybackPlan::compile(Arc::new(p), Default::default(), 48000).unwrap();
        let owner = AudioSource::timeline(plan, 0).unwrap();
        let (mut tx, rx) = rtrb::RingBuffer::new(COMMAND_CAPACITY);
        let state = Arc::new(TransportCell::default());
        let mut r = Renderer::new(rx, state.clone(), metrics, 48000, 2);
        for action in [
            Action::Load {
                clip_id: 1,
                audio: owner.clone(),
            },
            Action::Play,
        ] {
            tx.push(Command {
                id: 1,
                issued: Instant::now(),
                action,
            })
            .ok()
            .unwrap();
        }
        let mut out = vec![0.; 48000];
        r.render(&mut out);
        (out, state.read().frame as usize)
    }
    let baseline = render(p.clone());
    let mut eq = gain_eq(12.);
    eq.enabled = false;
    p.tracks[0].inserts = vec![eq.clone()];
    p.master.inserts = vec![effect("delay")];
    p.master.inserts[0].enabled = false;
    let bypass = render(p.clone());
    assert_eq!(baseline, bypass);
    p.tracks[0].inserts[0].enabled = true;
    let wet = render(p);
    assert_eq!(baseline.1, wet.1);
    assert_eq!(
        baseline.0.iter().position(|v| v.abs() > 1e-7),
        wet.0.iter().position(|v| v.abs() > 1e-7)
    );
    assert!(baseline
        .0
        .iter()
        .zip(wet.0)
        .any(|(a, b)| (a - b).abs() > 0.001));
}
