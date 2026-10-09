use minidaw_lib::{
    audio::{
        automation::Automation,
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
        automation::{self, Lane, Parameter, Point, Shape},
        edit::{self, Clipboard},
        effects::{Effect, Processor},
        runtime::{AutomationClock, ProjectAudio},
        schema::*,
        session::ProjectService,
    },
};
use serde_json::json;
use std::{cell::Cell, collections::HashMap, sync::Arc, time::Instant};
fn lane(p: &mut Project, id: &str, effect: Option<&str>, name: &str, points: &[(i64, f64, Shape)]) {
    automation::channel_mut(p, id).lanes.push(Lane {
        lane_id: schema_id(),
        parameter: Parameter {
            effect_id: effect.map(String::from),
            name: name.into(),
        },
        points: points
            .iter()
            .map(|&(tick, value, shape)| Point {
                point_id: schema_id(),
                tick: Signed(tick),
                value,
                shape,
            })
            .collect(),
    });
}
fn schema_id() -> String {
    id()
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
fn integer_ppq_sample_boundaries_linear_step_seek_cycle_and_rates() {
    for rate in [44100, 48000, 96000] {
        let mut p = Project::new();
        lane(
            &mut p,
            "master",
            None,
            "volumeDb",
            &[
                (0, -24., Shape::Linear),
                (960000, 0., Shape::Step),
                (1920000, -12., Shape::Step),
            ],
        );
        p.validate().unwrap();
        let mut a = Automation::compile(&p, "master", rate);
        let mut c = Chain::from_project(&p, "master", rate);
        for f in [0, rate / 4, rate / 2, rate - 1, rate, 5, rate / 4] {
            let got = a.apply(Some(f as usize), &mut c).volume;
            let expected = if f < rate / 2 {
                -24. + 24. * f as f64 / (rate / 2) as f64
            } else if f < rate {
                0.
            } else {
                -12.
            };
            assert!((got - expected).abs() < 1e-10);
        }
        for f in [1, 31, 12345, 48001, 10000000] {
            let tick = automation::tick_at_frame(f, rate, &p.musical_time);
            let back = minidaw_lib::project::time::time(
                &Position::Ticks {
                    ticks: Signed(tick),
                },
                &p.musical_time,
            )
            .ceil_frame(rate);
            assert_eq!(back, f as i128, "frame {f} rate {rate}");
        }
        assert_eq!(a.apply(None, &mut c).volume, 0.);
    }
}
#[test]
fn audio_volume_pan_curves_are_sample_exact_after_clip_and_before_inserts() {
    let (mut p, asset) = audio_document();
    let id = p.tracks[0].track_id.clone();
    lane(
        &mut p,
        &id,
        None,
        "volumeDb",
        &[(0, -12., Shape::Linear), (960000, 0., Shape::Linear)],
    );
    lane(
        &mut p,
        &id,
        None,
        "pan",
        &[(0, -1., Shape::Linear), (1920000, 1., Shape::Linear)],
    );
    let plan = PlaybackPlan::compile(
        Arc::new(p.clone()),
        HashMap::from([(p.assets[0].asset_id.clone(), asset)]),
        48000,
    )
    .unwrap();
    let mut r = TimelineReader::new(plan);
    for f in 0..48000 {
        let out = r.read_frame().unwrap();
        let db = if f < 24000 {
            -12. + 12. * f as f64 / 24000.
        } else {
            0.
        };
        let pan = -1. + 2. * f as f64 / 48000.;
        let g = 0.5 * 10f64.powf(db / 20.);
        let expected = [g * (1. - pan).min(1.), g * (1. + pan).min(1.)];
        for i in 0..2 {
            assert!(
                (out[i] as f64 - expected[i]).abs() < 1e-6,
                "{f} {out:?} {expected:?}"
            );
        }
    }
}
#[test]
fn every_effect_parameter_reaches_same_dsp_as_static_settings_and_discrete_bypass() {
    for kind in ["eq", "compressor", "limiter", "reverb", "delay"] {
        let mut p = Project::new();
        let mut fx = Effect::new(kind).unwrap();
        match &mut fx.processor {
            Processor::External { .. } => unreachable!("This fixture covers builtin effects"),
            Processor::Eq { bands } => {
                bands[0].frequency = 400.;
                bands[0].gain_db = 6.;
                bands[0].q = 2.;
            }
            Processor::Compressor {
                threshold_db,
                ratio,
                attack_ms,
                release_ms,
                makeup_db,
            } => {
                *threshold_db = -30.;
                *ratio = 8.;
                *attack_ms = 3.;
                *release_ms = 50.;
                *makeup_db = 2.;
            }
            Processor::Limiter {
                ceiling_db,
                input_db,
            } => {
                *ceiling_db = -12.;
                *input_db = 6.;
            }
            Processor::Reverb { decay, wet } => {
                *decay = 2.5;
                *wet = 0.6;
            }
            Processor::Delay {
                time_ms,
                feedback,
                wet,
                sync_beats,
            } => {
                *time_ms = 37.;
                *feedback = 0.6;
                *wet = 0.8;
                *sync_beats = Some(0.125);
            }
        }
        let expected = fx.clone();
        let mut base = Effect::new(kind).unwrap();
        base.effect_id = fx.effect_id.clone();
        p.master.inserts.push(fx.clone());
        let values: Vec<_> = automation::parameter_names(&fx)
            .iter()
            .map(|name| {
                let param = Parameter {
                    effect_id: Some(fx.effect_id.clone()),
                    name: name.clone(),
                };
                (
                    name.clone(),
                    automation::spec(&p, "master", &param).unwrap().base,
                )
            })
            .collect();
        p.master.inserts[0] = base;
        for (name, value) in values {
            lane(
                &mut p,
                "master",
                Some(&fx.effect_id),
                &name,
                &[(0, value, Shape::Linear)],
            );
        }
        p.validate().unwrap();
        let mut a = Automation::compile(&p, "master", 48000);
        let mut c = Chain::from_project(&p, "master", 48000);
        let mut reference = Chain::new(&[expected], 48000, 120.);
        for f in 0..24000 {
            a.apply(Some(f), &mut c);
            let x = [
                (f as f64 * 0.131).sin() * 0.6,
                (f as f64 * 0.17).cos() * 0.2,
            ];
            let expected = reference.process(x);
            let got = c.process(x);
            assert!(
                (got[0] - expected[0]).abs() < 1e-10,
                "{kind} {f} {got:?} {expected:?}"
            );
        }
        let bypass = p.automation[0]
            .lanes
            .iter_mut()
            .find(|l| l.parameter.name == "bypass")
            .unwrap();
        bypass.points[0].value = 1.;
        let mut a = Automation::compile(&p, "master", 48000);
        a.apply(Some(0), &mut c);
        assert_eq!(c.process([0.21, -0.3]), [0.21, -0.3]);
    }
}
#[derive(Default)]
struct Runtime {
    clock: Cell<Option<AutomationClock>>,
    replaces: Cell<usize>,
}
impl ProjectAudio for Runtime {
    type Prepared = Arc<AudioAsset>;
    fn automation_clock(&self) -> Option<AutomationClock> {
        self.clock.get()
    }
    fn prepare_asset(&self, a: Self::Prepared) -> AppResult<Self::Prepared> {
        Ok(a)
    }
    fn install(&self, _: Option<Self::Prepared>) -> AppResult<()> {
        self.replaces.set(self.replaces.get() + 1);
        Ok(())
    }
}
#[test]
fn write_is_rust_clock_latched_grouped_undo_and_restores_future() {
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
    apply(json!({"command":"automation.lane.add","parameter":{"name":"volumeDb"}}));
    let lane = s.view().unwrap().document.automation[0].lanes[0]
        .lane_id
        .clone();
    for (t, v) in [(0, -6.), (9600000, 0.)] {
        apply(
            json!({"command":"automation.point.set","laneId":lane,"targetTick":t.to_string(),"value":v}),
        );
    }
    apply(json!({"command":"automation.channel","write":true}));
    let before = s.view().unwrap();
    for (frame, value) in [(48000, -12.), (72000, -18.), (96000, -9.)] {
        r.clock.set(Some(AutomationClock {
            frame,
            rate: 48000,
            playing: true,
            cycle_pass: 0,
        }));
        apply(json!({"command":"master.volume","volumeDb":value}));
    }
    r.clock.set(Some(AutomationClock {
        frame: 144000,
        rate: 48000,
        playing: false,
        cycle_pass: 0,
    }));
    s.finish_write(&r, r.clock.get()).unwrap();
    let after = s.view().unwrap();
    assert_eq!(after.document.master.volume_db, 0.);
    assert_eq!(after.history.undo, before.history.undo + 1);
    let points = &after.document.automation[0].lanes[0].points;
    assert!(points
        .iter()
        .any(|p| p.tick.0 == 1920000 && p.value == -12.));
    assert!(points.iter().any(|p| p.tick.0 == 5760000 && p.value == -9.));
    assert_eq!(points.last().unwrap().tick.0, 9600000);
    assert_eq!(
        r.replaces.get(),
        0,
        "Master curve does not rebuild Audio source"
    );
    apply(json!({"command":"edit.undo"}));
    assert_eq!(s.view().unwrap().document, before.document);
    apply(json!({"command":"edit.redo"}));
    assert_eq!(s.view().unwrap().document, after.document);
    let folder = std::env::temp_dir().join(format!("minidaw-automation-{}", id()));
    std::fs::create_dir(&folder).unwrap();
    let path = folder.join("saved.minidaw");
    s.save(Some(&path), s.revision().unwrap()).unwrap();
    let saved = s.view().unwrap().document;
    s.new_project(&r, s.revision().unwrap(), true).unwrap();
    s.open(&r, &path, s.revision().unwrap(), true).unwrap();
    assert_eq!(s.view().unwrap().document, saved);
    std::fs::remove_dir_all(folder).unwrap();
}
#[test]
fn stable_track_and_effect_ids_survive_reorder_and_prune_deleted_targets() {
    let mut p = Project::new();
    let mut cb = Clipboard::default();
    let edit = |p: &Project, v| {
        edit::apply(
            p,
            &serde_json::from_value(v).unwrap(),
            &mut Clipboard::default(),
        )
        .unwrap()
    };
    p = edit(&p, json!({"command":"midi.track.add"}));
    let t = p.tracks[0].track_id.clone();
    p = edit(
        &p,
        json!({"command":"effect.add","trackIds":[t],"effectKind":"eq"}),
    );
    let e = p.tracks[0].inserts[0].effect_id.clone();
    p = edit(
        &p,
        json!({"command":"effect.add","trackIds":[t],"effectKind":"delay"}),
    );
    lane(
        &mut p,
        &t,
        Some(&e),
        "band1.gainDb",
        &[(12345, 6., Shape::Linear)],
    );
    p.validate().unwrap();
    let before = p.automation.clone();
    p = edit(
        &p,
        json!({"command":"effect.move","trackIds":[t],"effectId":e,"direction":1}),
    );
    assert_eq!(p.automation, before);
    assert_eq!(Automation::compile(&p, &t, 48000).curves[0].slot, Some(1));
    p = edit(
        &p,
        json!({"command":"effect.remove","trackIds":[t],"effectId":e}),
    );
    assert!(p.automation[0].lanes.is_empty());
    p = edit(&p, json!({"command":"track.delete","trackIds":[t]}));
    assert!(p.automation.is_empty());
    let _ = &mut cb;
}
fn midi_document() -> Project {
    let mut p = edit::apply(
        &Project::new(),
        &serde_json::from_value(json!({"command":"midi.track.add"})).unwrap(),
        &mut Clipboard::default(),
    )
    .unwrap();
    p.tracks[0].instrument = Instrument::BasicSynth;
    p.tracks[0].clips.push(Clip::Midi(MidiClip {
        clip_id: id(),
        name: "part".into(),
        start_tick: Signed(0),
        length_tick: Signed(3840000),
        content_offset_tick: Signed(0),
        notes: vec![MidiNote {
            note_id: id(),
            start_tick: Signed(0),
            length_tick: Signed(3840000),
            pitch: 69,
            velocity: 100,
            release_velocity: 0,
            channel: 0,
        }],
        controls: vec![],
    }));
    p
}
fn render(p: Project, chunk: usize) -> Vec<f32> {
    let metrics = Arc::new(AudioMetrics::default());
    metrics.effects.configure(&p);
    let source = AudioSource::timeline(
        PlaybackPlan::compile(Arc::new(p), HashMap::new(), 48000).unwrap(),
        0,
    )
    .unwrap();
    let (mut tx, rx) = rtrb::RingBuffer::new(COMMAND_CAPACITY);
    let mut r = Renderer::new(rx, Arc::new(TransportCell::default()), metrics, 48000, 2);
    for action in [
        Action::Load {
            clip_id: 1,
            audio: source.clone(),
        },
        Action::Declick(false),
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
    let mut out = vec![0.; 96000];
    for block in out.chunks_mut(chunk * 2) {
        r.render(block);
    }
    out
}
#[test]
fn midi_synth_master_read_are_independent_of_callback_size_and_ui_clock() {
    let mut p = midi_document();
    let id = p.tracks[0].track_id.clone();
    let baseline = render(p.clone(), 64);
    lane(
        &mut p,
        &id,
        None,
        "volumeDb",
        &[(0, 0., Shape::Linear), (960000, -12., Shape::Linear)],
    );
    lane(
        &mut p,
        &id,
        None,
        "pan",
        &[(0, 0., Shape::Linear), (960000, 1., Shape::Linear)],
    );
    lane(
        &mut p,
        &id,
        None,
        "synth.levelDb",
        &[(0, -6., Shape::Linear)],
    );
    lane(
        &mut p,
        &id,
        None,
        "synth.attackMs",
        &[(0, 5., Shape::Linear)],
    );
    lane(
        &mut p,
        &id,
        None,
        "synth.releaseMs",
        &[(0, 40., Shape::Linear)],
    );
    lane(
        &mut p,
        "master",
        None,
        "volumeDb",
        &[(0, -3., Shape::Linear)],
    );
    let a = render(p.clone(), 64);
    let b = render(p.clone(), 127);
    let c = render(p, 1024);
    assert_eq!(a, b);
    assert_eq!(a, c);
    for f in [12001usize, 24001, 30001] {
        let g = 10f64.powf((-9. - 12. * (f as f64 / 24000.).min(1.)) / 20.);
        assert!((a[f * 2 + 1] as f64 - baseline[f * 2 + 1] as f64 * g).abs() < 1e-6);
        if f > 24000 {
            assert_eq!(a[f * 2], 0.);
        }
    }
}
#[test]
fn delay_tail_survives_curve_only_rack_swap() {
    use minidaw_lib::audio::{effect_runtime::Exchange, synth::BasicSynth};
    let mut p = Project::new();
    let mut e = Effect::new("delay").unwrap();
    e.processor = Processor::Delay {
        time_ms: 100.,
        feedback: 0.,
        wet: 1.,
        sync_beats: None,
    };
    p.master.inserts.push(e);
    lane(
        &mut p,
        "master",
        None,
        "volumeDb",
        &[(0, 0., Shape::Linear)],
    );
    let x = Exchange::default();
    x.configure(&p);
    let mut port = x.attach(48000);
    let mut synth = BasicSynth::new(48000);
    for f in 0..4801 {
        if f == 2000 {
            p.automation[0].lanes[0].points[0].value = -3.;
            x.configure(&p);
            port.update();
        }
        port.automate(Some(f), &mut synth);
        let y = port.master(if f == 0 { [1., 0.] } else { [0.; 2] }, false);
        assert_eq!(y[0], if f == 4800 { 1. } else { 0. }, "frame {f}");
    }
}

#[test]
fn saving_during_write_snapshots_latch_and_restores_future_without_stopping_capture() {
    let service = ProjectService::new(None);
    let runtime = Runtime::default();
    let edit = |v| {
        service
            .edit(
                &runtime,
                service.revision().unwrap(),
                serde_json::from_value(v).unwrap(),
            )
            .unwrap()
    };
    edit(json!({"command":"automation.lane.add","parameter":{"name":"volumeDb"}}));
    let lane = service.view().unwrap().document.automation[0].lanes[0]
        .lane_id
        .clone();
    for (tick, value) in [(0, 0.), (9600000, -6.)] {
        edit(
            json!({"command":"automation.point.set","laneId":lane,"targetTick":tick.to_string(),"value":value}),
        );
    }
    edit(json!({"command":"automation.channel","write":true}));
    runtime.clock.set(Some(AutomationClock {
        frame: 48000,
        rate: 48000,
        playing: true,
        cycle_pass: 0,
    }));
    edit(json!({"command":"master.volume","volumeDb":-12.}));
    let folder = std::env::temp_dir().join(format!("minidaw-latch-save-{}", id()));
    std::fs::create_dir(&folder).unwrap();
    let path = folder.join("live.minidaw");
    runtime.clock.set(Some(AutomationClock {
        frame: 96000,
        rate: 48000,
        playing: true,
        cycle_pass: 0,
    }));
    service
        .save_at(
            Some(&path),
            service.revision().unwrap(),
            runtime.clock.get(),
        )
        .unwrap();
    let saved: Project = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let points = &saved.automation[0].lanes[0].points;
    assert!(points
        .iter()
        .any(|p| p.tick.0 == 3840000 && p.value == -12.));
    assert_eq!(points.last().unwrap().tick.0, 9600000);
    assert!(service.has_write_pass());
    assert!(
        service.view().unwrap().document.automation[0].lanes[0]
            .points
            .last()
            .unwrap()
            .tick
            .0
            < 3840000
    );
    std::fs::remove_dir_all(folder).unwrap();
}
#[test]
fn audio_plan_automation_swap_keeps_running_midi_voice_phase() {
    fn run(swap: bool) -> Vec<f32> {
        let p = midi_document();
        let metrics = Arc::new(AudioMetrics::default());
        metrics.effects.configure(&p);
        let plan = PlaybackPlan::compile(Arc::new(p), HashMap::new(), 48000).unwrap();
        let owner = AudioSource::timeline(plan.clone(), 0).unwrap();
        let next = AudioSource::timeline(plan, 4096).unwrap();
        let (mut tx, rx) = rtrb::RingBuffer::new(COMMAND_CAPACITY);
        let mut r = Renderer::new(rx, Arc::new(TransportCell::default()), metrics, 48000, 2);
        for action in [
            Action::Load {
                clip_id: 1,
                audio: owner.clone(),
            },
            Action::Declick(false),
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
        let mut out = vec![0.; 16384];
        r.render(&mut out[..8192]);
        if swap {
            tx.push(Command {
                id: 2,
                issued: Instant::now(),
                action: Action::AutomationReplace {
                    clip_id: 2,
                    audio: next.clone(),
                },
            })
            .ok()
            .unwrap();
        }
        r.render(&mut out[8192..]);
        out
    }
    assert_eq!(run(false), run(true));
}
#[test]
fn read_cycle_repeats_curve_at_transport_wrap_without_drift() {
    let mut p = midi_document();
    p.cycle = Some(Cycle {
        enabled: true,
        start_tick: Signed(0),
        end_tick: Signed(960000),
    });
    lane(
        &mut p,
        "master",
        None,
        "volumeDb",
        &[(0, -24., Shape::Step), (480000, 0., Shape::Step)],
    );
    let a = render(p.clone(), 64);
    let b = render(p, 127);
    assert_eq!(a, b);
    let rms = |start: usize, end: usize| {
        (a[start * 2..end * 2]
            .iter()
            .map(|v| (*v as f64).powi(2))
            .sum::<f64>()
            / ((end - start) * 2) as f64)
            .sqrt()
    };
    for base in [0, 24000] {
        let quiet = rms(base + 6000, base + 11000);
        let loud = rms(base + 18000, base + 23000);
        assert!((20. * (loud / quiet).log10() - 24.).abs() < 0.1);
    }
}

#[test]
fn synth_attack_release_level_automation_matches_static_parameters() {
    let mut baseline = midi_document();
    baseline.tracks[0].synth = automation::SynthSettings {
        level_db: -9.,
        attack_ms: 70.,
        release_ms: 150.,
    };
    if let Clip::Midi(c) = &mut baseline.tracks[0].clips[0] {
        c.notes[0].length_tick = Signed(960000);
    }
    let mut automated = baseline.clone();
    automated.tracks[0].synth = Default::default();
    let id = automated.tracks[0].track_id.clone();
    for (name, value) in [
        ("synth.levelDb", -9.),
        ("synth.attackMs", 70.),
        ("synth.releaseMs", 150.),
    ] {
        lane(
            &mut automated,
            &id,
            None,
            name,
            &[(0, value, Shape::Linear)],
        );
    }
    assert_eq!(render(baseline, 64), render(automated, 127));
}

#[test]
fn latch_crosses_multiple_cycle_passes_without_ui_ticks_and_persists_last_pass() {
    let mut p = midi_document();
    p.cycle = Some(Cycle {
        enabled: true,
        start_tick: Signed(0),
        end_tick: Signed(3840000),
    });
    lane(
        &mut p,
        "master",
        None,
        "volumeDb",
        &[(0, 0., Shape::Linear), (3840000, -6., Shape::Linear)],
    );
    automation::channel_mut(&mut p, "master").write = true;
    let mut pass = None;
    let mut next = p.clone();
    next.master.volume_db = -12.;
    let r = serde_json::from_value(json!({"command":"master.volume","volumeDb":-12.})).unwrap();
    assert!(automation::capture(&p, &mut next, &r, Some((48000, 48000, 0)), &mut pass).unwrap());
    let playback = automation::playback(&next, pass.as_ref());
    playback.validate().unwrap();
    let mut a = Automation::compile(&playback, "master", 48000);
    let mut chain = Chain::from_project(&playback, "master", 48000);
    for frame in [50000, 0, 1, 95999, 0, 16000] {
        assert_eq!(a.apply(Some(frame), &mut chain).volume, -12.);
    }
    automation::finish(&mut next, &mut pass, Some((24000, 48000, 3)));
    next.validate().unwrap();
    let q = &next.automation[0].lanes[0].points;
    for tick in [0, 960000, 1920000, 3839999] {
        assert_eq!(automation::value(q, tick, 0., false), -12.);
    }
    assert_eq!(automation::value(q, 3840000, 0., false), -6.);
}
