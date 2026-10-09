use minidaw_lib::{
    audio::{
        cycle::CycleFrames,
        decoder::{AudioData, FileInfo},
        metrics::AudioMetrics,
        renderer::{Action, Command, Renderer, COMMAND_CAPACITY},
        source::{AudioAsset, AudioSource},
        timeline::PlaybackPlan,
        transport::{PlayState, TransportCell},
    },
    project::{
        edit::{self, Clipboard, EditRequest},
        history::History,
        migrations,
        schema::*,
        time::time,
    },
};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
fn edit(p: &Project, value: serde_json::Value) -> Project {
    let request: EditRequest = serde_json::from_value(value).unwrap();
    edit::apply(p, &request, &mut Clipboard::default()).unwrap()
}
#[test]
fn tempo_signature_cycle_roundtrip_legacy_and_undo() {
    let original = Project::new();
    let legacy = migrations::encode(&original).unwrap();
    assert!(!String::from_utf8_lossy(&legacy).contains("cycle"));
    assert_eq!(migrations::decode(&legacy).unwrap(), original);
    let p = edit(
        &original,
        serde_json::json!({"command":"project.tempo","bpm":137.5}),
    );
    let p = edit(
        &p,
        serde_json::json!({"command":"project.signature","numerator":7,"denominator":8}),
    );
    let p = edit(
        &p,
        serde_json::json!({"command":"project.cycle","cycle":{"enabled":true,"startTick":"960000","endTick":"4320000"}}),
    );
    assert_eq!(p.musical_time.ticks_per_quarter, 960000);
    assert_eq!(
        migrations::decode(&migrations::encode(&p).unwrap()).unwrap(),
        p
    );
    let mut history = History::default();
    history.record(original.clone(), p.clone(), "Music settings");
    assert_eq!(history.undo().unwrap(), original);
    assert_eq!(history.redo().unwrap(), p);
    let mut invalid = p.clone();
    invalid.cycle.as_mut().unwrap().end_tick = Signed(0);
    assert!(invalid.validate().is_err());
    let mut invalid = p.clone();
    invalid.musical_time.time_signatures[0].denominator = 3;
    assert!(invalid.validate().is_err());
    assert_eq!(
        time(
            &Position::Seconds {
                numerator: Signed(3),
                denominator: 2
            },
            &p.musical_time
        )
        .seconds(),
        1.5
    );
    assert!(
        (time(
            &Position::Ticks {
                ticks: Signed(960000)
            },
            &p.musical_time
        )
        .seconds()
            - 60.0 / 137.5)
            .abs()
            < 1e-9
    );
}

#[test]
fn cycle_audio_and_transport_are_sample_exact_across_many_wraps() {
    const RATE: u32 = 8000;
    let frames = RATE as usize * 2;
    let asset = AudioAsset::memory(AudioData {
        info: FileInfo {
            name: "cycle.wav".into(),
            sample_rate: RATE,
            channels: 2,
            frames,
            duration: 2.0,
            sanitized_samples: 0,
        },
        samples: (0..frames)
            .flat_map(|i| {
                let x = (i % 1000) as f32 / 10000.0;
                [x, -x]
            })
            .collect(),
    });
    let mut p = Project::new();
    p.import(Asset {
        asset_id: id(),
        filename: "cycle.wav".into(),
        path: PathReference {
            project_relative_path: Some("cycle.wav".into()),
            original_absolute_path: None,
        },
        metadata: AudioMetadata {
            sample_rate: RATE,
            channels: 2,
            source_frames: Frames(frames as u64),
            container: "wav".into(),
            codec: None,
        },
        fingerprint: Fingerprint {
            file_bytes: Frames(100),
            sampled_sha256: "a".repeat(64),
        },
        extensions: Extensions::new(),
    });
    // At 120 BPM: 240 ticks per output sample. Length 1237 is not buffer aligned.
    p.cycle = Some(Cycle {
        enabled: true,
        start_tick: Signed(123 * 240),
        end_tick: Signed(1360 * 240),
    });
    let cycle = CycleFrames::compile(&p, RATE).unwrap().unwrap();
    let plan = PlaybackPlan::compile(
        Arc::new(p.clone()),
        HashMap::from([(p.assets[0].asset_id.clone(), asset)]),
        RATE,
    )
    .unwrap();
    let source = AudioSource::timeline(plan, 0).unwrap();
    std::thread::sleep(Duration::from_millis(50)); // Fill bounded read-ahead before offline consumption.
    let (mut tx, rx) = rtrb::RingBuffer::new(COMMAND_CAPACITY);
    let shared = Arc::new(TransportCell::default());
    let mut renderer = Renderer::new(
        rx,
        shared.clone(),
        Arc::new(AudioMetrics::default()),
        RATE,
        2,
    );
    renderer.set_declick(false);
    let mut id = 0;
    let mut send = |action| {
        id += 1;
        tx.push(Command {
            id,
            issued: Instant::now(),
            action,
        })
        .ok()
        .unwrap();
    };
    send(Action::Load {
        clip_id: 1,
        audio: source.clone(),
    });
    send(Action::Play);
    let mut total = 0;
    for size in [17, 64, 511].into_iter().cycle().take(360) {
        let mut output = vec![0.0f32; size * 2];
        renderer.render(&mut output);
        for (i, pair) in output.as_chunks::<2>().0.iter().enumerate() {
            let expected = (cycle.position(total + i) % 1000) as f32 / 10000.0;
            assert_eq!(pair, &[expected, -expected]);
        }
        total += size;
        assert_eq!(shared.read().frame, cycle.position(total) as f64);
        assert_eq!(shared.read().state, PlayState::Playing);
    }
    assert!(total / (cycle.end - cycle.start) > 50);
    assert_eq!(source.snapshot().starvation, 0);
    send(Action::Pause);
    renderer.render(&mut [0.0f32; 128]);
    let paused = shared.read().frame;
    assert_eq!(shared.read().state, PlayState::Paused);
    send(Action::Play);
    renderer.render(&mut [0.0f32; 128]);
    assert_eq!(
        shared.read().frame,
        cycle.position(paused as usize + 64) as f64
    );
    send(Action::Stop);
    renderer.render(&mut [0.0f32; 128]);
    assert_eq!(shared.read().frame, 0.0);
    assert_eq!(shared.read().state, PlayState::Stopped);
}
