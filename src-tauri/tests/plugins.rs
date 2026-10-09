use minidaw_lib::plugins::*;
use minidaw_lib::{
    audio::{offline::OfflineMaster, timeline::PlaybackPlan},
    project::{
        edit::{self, Clipboard},
        schema::*,
    },
};
use std::path::PathBuf;
use std::{collections::HashMap, sync::Arc};
fn edit(p: &Project, r: serde_json::Value) -> Project {
    edit::apply(
        p,
        &serde_json::from_value(r).unwrap(),
        &mut Clipboard::default(),
    )
    .unwrap()
}
#[test]
fn installed_plugin_probe() {
    let folder =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/local/plugins/surge-xt-1.3.4");
    if !folder.is_dir() {
        return;
    }
    let mut paths = vec![];
    candidates(&folder, &mut paths, 0);
    for path in paths {
        let format = path.extension().unwrap().to_str().unwrap();
        let plugins = scan_one(&path, format).unwrap();
        for d in plugins {
            println!("PROBE {} {}", d.format, d.name);
            let config = prepare(d, 48000).unwrap();
            println!(
                "PARAMS {} LATENCY {} TAIL {}",
                config.parameters.len(),
                config.latency,
                config.tail
            );
            let _scope = OfflineScope::new();
            let mut p = Instance::new(&config, "probe", 48000, 120.);
            assert!(p.failure.is_none(), "{:?}", p.failure);
            p.midi(0x90, 60, 100, 1);
            let mut peak = 0f64;
            for frame in 0..48000 {
                let input = if config.descriptor.instrument {
                    [0.; 2]
                } else {
                    [(frame as f64 / 48000. * 440. * std::f64::consts::TAU).sin() * 0.1; 2]
                };
                peak = peak.max(p.process(input).iter().fold(0f64, |a, b| a.max(b.abs())));
            }
            p.midi(0x80, 60, 0, 1);
            println!("PEAK {peak}");
            assert!(peak > 0.00001);
        }
    }
}
#[test]
fn external_instrument_through_existing_offline_master_and_project_state() {
    let root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/local/plugins/surge-xt-1.3.4");
    if !root.is_dir() {
        return;
    }
    let mut files = vec![];
    candidates(&root, &mut files, 0);
    for format in ["vst3", "clap"] {
        let mut choices = vec![];
        for f in files
            .iter()
            .filter(|f| f.extension().is_some_and(|x| x == format))
        {
            choices.extend(scan_one(f, format).unwrap());
        }
        let synth = prepare(
            choices.iter().find(|d| d.instrument).unwrap().clone(),
            48000,
        )
        .unwrap();
        let effect = prepare(
            choices.iter().find(|d| !d.instrument).unwrap().clone(),
            48000,
        )
        .unwrap();
        let p = Project::default();
        let p = edit(&p, serde_json::json!({"command":"midi.track.add"}));
        let track = p.tracks.last().unwrap().track_id.clone();
        let p = edit(
            &p,
            serde_json::json!({"command":"plugin.instrument","trackIds":[track],"plugin":synth}),
        );
        let p = edit(
            &p,
            serde_json::json!({"command":"midi.clip.add","trackIds":[track],"targetTick":"0","lengthTick":"960000"}),
        );
        let clip = p.midi_clips().next().unwrap().clip_id.clone();
        let p = edit(
            &p,
            serde_json::json!({"command":"midi.note.add","clipIds":[clip],"targetTick":"12345","lengthTick":"500000","pitch":60,"velocity":80}),
        );
        let p = edit(
            &p,
            serde_json::json!({"command":"effect.add","plugin":effect}),
        );
        let encoded = serde_json::to_vec(&p).unwrap();
        let restored: Project = serde_json::from_slice(&encoded).unwrap();
        assert!(p == restored, "Plugin state round trip differs");
        restored.validate().unwrap();
        let _scope = OfflineScope::new();
        let plan = PlaybackPlan::compile(Arc::new(restored), HashMap::new(), 48000).unwrap();
        let mut out = OfflineMaster::new(plan, Arc::new(|| false)).unwrap();
        let mut peak = 0f32;
        let mut frames = 0;
        while let Some(x) = out.next_frame().unwrap() {
            peak = peak.max(x[0].abs()).max(x[1].abs());
            frames += 1;
            assert!(frames < 48000 * 40);
        }
        println!("PROJECT {format} peak={peak} frames={frames}");
        assert!(peak > 0.001);
        check_offline().unwrap();
    }
}

#[test]
fn external_midi_uses_exact_existing_sample_clock_and_preserves_controller_bytes() {
    use minidaw_lib::audio::{
        midi::{MidiPlan, MidiScheduler},
        synth::BasicSynth,
    };
    let mut p = edit(
        &Project::default(),
        serde_json::json!({"command":"midi.track.add"}),
    );
    let t = p.tracks.last().unwrap().track_id.clone();
    let config = Selection {
        descriptor: Descriptor {
            path: "C:/Missing/test.clap".into(),
            format: "clap".into(),
            id: "test.instrument".into(),
            name: "Missing Test".into(),
            vendor: "test".into(),
            instrument: true,
        },
        state: "deadbeef".into(),
        parameters: vec![],
        latency: 0,
        tail: 0,
        sample_rate: 48000,
    };
    p = edit(
        &p,
        serde_json::json!({"command":"plugin.instrument","trackIds":[t],"plugin":config}),
    );
    p = edit(
        &p,
        serde_json::json!({"command":"midi.clip.add","trackIds":[t],"targetTick":"0","lengthTick":"960000"}),
    );
    let c = p.midi_clips().next().unwrap().clip_id.clone();
    p = edit(
        &p,
        serde_json::json!({"command":"midi.note.add","clipIds":[c],"targetTick":"12345","lengthTick":"500000","pitch":69,"velocity":83}),
    );
    for (tick, data) in [
        (
            20000,
            serde_json::json!({"kind":"cc","controller":64,"value":127}),
        ),
        (40000, serde_json::json!({"kind":"pitchBend","value":4096})),
        (
            600000,
            serde_json::json!({"kind":"cc","controller":64,"value":0}),
        ),
    ] {
        p = edit(
            &p,
            serde_json::json!({"command":"midi.control.put","clipIds":[c],"event":{"eventId":"","tick":tick.to_string(),"channel":0,"data":data}}),
        );
    }
    let plan = MidiPlan::compile(&p, 48000).unwrap();
    let mut synth = BasicSynth::new(48000);
    synth.configure(Some(&plan));
    let mut midi = MidiScheduler::default();
    let mut events = vec![];
    for frame in 0..24001 {
        midi.sample(&plan, frame, 0, &mut synth);
        for e in synth.external_events() {
            events.push((frame, e.status, e.a, e.b));
        }
        synth.clear_external();
    }
    for expected in [
        (309, 0x90, 69, 83),
        (12809, 0x80, 69, 0),
        (500, 0xb0, 64, 127),
        (1000, 0xe0, 0, 96),
        (15000, 0xb0, 64, 0),
    ] {
        assert!(
            events.contains(&expected),
            "missing {expected:?}, events={events:?}"
        );
    }
    let json = serde_json::to_string(&p).unwrap();
    let mut restored: Project = serde_json::from_str(&json).unwrap();
    assert!(!capture_project(&mut restored).unwrap());
    assert!(p == restored, "Missing plugin must preserve state");
}

#[test]
fn real_external_effect_bypass_and_parameter_automation() {
    use minidaw_lib::{
        audio::{automation::Automation, effects::Chain},
        project::{
            automation::{Channel, Lane, Parameter, Point, Shape},
            effects::{Effect, Processor},
        },
    };
    let root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/local/plugins/surge-xt-1.3.4");
    if !root.is_dir() {
        return;
    }
    let mut files = vec![];
    candidates(&root, &mut files, 0);
    for format in ["vst3", "clap"] {
        let d = files
            .iter()
            .filter(|f| f.extension().is_some_and(|x| x == format))
            .flat_map(|f| scan_one(f, format).unwrap())
            .find(|d| !d.instrument)
            .unwrap();
        let mut cfg = prepare(d, 48000).unwrap();
        let mix = cfg
            .parameters
            .iter_mut()
            .find(|p| p.id == 849359077)
            .expect("Surge FX Mix ID");
        let pid = mix.id;
        mix.value = 0.;
        let mut p = Project::default();
        let effect = Effect {
            effect_id: id(),
            enabled: true,
            processor: Processor::External {
                plugin: cfg.clone(),
            },
        };
        let eid = effect.effect_id.clone();
        p.master.inserts.push(effect.clone());
        let mut auto = p.clone();
        auto.automation.push(Channel {
            track_id: "master".into(),
            read: true,
            write: false,
            lanes: vec![Lane {
                lane_id: id(),
                parameter: Parameter {
                    effect_id: Some(eid),
                    name: format!("plugin.{pid}"),
                },
                points: vec![Point {
                    point_id: id(),
                    tick: Signed(0),
                    value: 1.,
                    shape: Shape::Step,
                }],
            }],
        });
        let _scope = OfflineScope::new();
        let mut dry = Chain::from_project(&p, "master", 48000);
        let mut wet = Chain::from_project(&auto, "master", 48000);
        let mut automation = Automation::compile(&auto, "master", 48000);
        let mut bypassed = effect.clone();
        bypassed.enabled = false;
        let mut bypass = Chain::new(&[bypassed], 48000, 120.);
        let mut difference = 0.;
        for frame in 0..24000 {
            let x = [if frame < 1000 { 0.1 } else { 0. }; 2];
            automation.apply(Some(frame), &mut wet);
            let a = dry.process(x);
            let b = wet.process(x);
            let bypass_output = bypass.process(x);
            let latency = bypass.latency();
            assert_eq!(
                bypass_output,
                if frame >= latency && frame < 1000 + latency {
                    [0.1; 2]
                } else {
                    [0.; 2]
                }
            );
            difference += (a[0] - b[0]).abs();
        }
        assert!(
            difference > 10.,
            "Automation must change plugin DSP output: {difference}"
        );
        check_offline().unwrap();
        println!("PLUGIN_AUTOMATION {format} accumulated_difference={difference}");
    }
}
