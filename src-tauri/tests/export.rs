use minidaw_lib::{
    audio::{
        decoder::{AudioData, FileInfo},
        metrics::AudioMetrics,
        offline::OfflineMaster,
        renderer::{Action, Command, Renderer, COMMAND_CAPACITY},
        source::{AudioAsset, AudioSource},
        timeline::PlaybackPlan,
        transport::TransportCell,
    },
    project::{
        automation::{self, Lane, Parameter, Point, Shape},
        edit::{self, Clipboard},
        effects::{Effect, Processor},
        export::{self, Request, WaveFormat, WaveWriter},
        paths,
        schema::*,
        session::AssetState,
    },
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs::{self, File},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering::*},
        Arc,
    },
    time::{Duration, Instant},
};
fn edit(p: &Project, r: Value) -> Project {
    edit::apply(
        p,
        &serde_json::from_value(r).unwrap(),
        &mut Clipboard::default(),
    )
    .unwrap()
}
fn lane(p: &mut Project, channel: &str, effect: Option<String>, name: &str, values: &[(i64, f64)]) {
    automation::channel_mut(p, channel).lanes.push(Lane {
        lane_id: id(),
        parameter: Parameter {
            effect_id: effect,
            name: name.into(),
        },
        points: values
            .iter()
            .map(|&(t, v)| Point {
                point_id: id(),
                tick: Signed(t),
                value: v,
                shape: Shape::Linear,
            })
            .collect(),
    });
}
fn effect(kind: &str) -> Effect {
    let mut e = Effect::new(kind).unwrap();
    match &mut e.processor {
        Processor::Eq { bands } => {
            bands[1].gain_db = 3.;
            bands[1].frequency = 440.;
        }
        Processor::Reverb { decay, wet } => {
            *decay = 0.3;
            *wet = 0.16;
        }
        Processor::Delay {
            time_ms,
            feedback,
            wet,
            ..
        } => {
            *time_ms = 40.;
            *feedback = 0.12;
            *wet = 0.2;
        }
        _ => {}
    }
    e
}
fn fixture() -> (Project, Arc<AudioAsset>) {
    let rate = 44100;
    let frames = rate as usize;
    let samples = (0..frames)
        .flat_map(|i| {
            let t = i as f64 / rate as f64;
            [
                (std::f64::consts::TAU * 220. * t).sin() as f32 * 0.12,
                (std::f64::consts::TAU * 443. * t).sin() as f32 * 0.07,
            ]
        })
        .collect();
    let source = AudioAsset::memory(AudioData {
        info: FileInfo {
            name: "fixture.wav".into(),
            sample_rate: rate,
            channels: 2,
            frames,
            duration: 1.,
            sanitized_samples: 0,
        },
        samples,
    });
    let mut p = Project::new();
    p.import(Asset {
        asset_id: id(),
        filename: "fixture.wav".into(),
        path: PathReference {
            project_relative_path: Some("fixture.wav".into()),
            original_absolute_path: None,
        },
        metadata: AudioMetadata {
            sample_rate: rate,
            channels: 2,
            source_frames: Frames(frames as u64),
            container: "wav".into(),
            codec: None,
        },
        fingerprint: Fingerprint {
            file_bytes: Frames(100),
            sampled_sha256: "e".repeat(64),
        },
        extensions: Default::default(),
    });
    let clip = p.clips().next().unwrap().clip_id.clone();
    p = edit(
        &p,
        json!({"command":"audio.tempoSync","clipIds":[clip],"sourceBpm":120,"tempoSync":true}),
    );
    p = edit(&p, json!({"command":"project.tempo","bpm":137.47}));
    p = edit(
        &p,
        json!({"command":"audio.pitch","clipIds":[clip],"pitchShift":{"semitones":3,"cents":17}}),
    );
    p = edit(
        &p,
        json!({"command":"audio.fade","clipIds":[clip],"fadeIn":"441","fadeOut":"882"}),
    );
    p.tracks[0].mix.volume_db = -5.;
    p.tracks[0].mix.pan = -0.17;
    p.tracks[0].inserts = vec![effect("eq"), effect("compressor")];
    let ta = p.tracks[0].track_id.clone();
    let eq = p.tracks[0].inserts[0].effect_id.clone();
    lane(
        &mut p,
        &ta,
        None,
        "volumeDb",
        &[(0, -9.), (960000, -3.), (1920000, -12.)],
    );
    lane(&mut p, &ta, None, "pan", &[(0, -0.25), (1920000, 0.7)]);
    lane(
        &mut p,
        &ta,
        Some(eq),
        "band1.gainDb",
        &[(0, 3.), (1920000, -3.)],
    );
    p = edit(&p, json!({"command":"midi.track.add"}));
    let tm = p.tracks[1].track_id.clone();
    p.tracks[1].instrument = Instrument::BasicSynth;
    p.tracks[1].mix.volume_db = -9.;
    p.tracks[1].mix.pan = 0.3;
    p.tracks[1].inserts = vec![effect("delay"), effect("reverb")];
    p.tracks[1].clips.push(Clip::Midi(MidiClip {
        clip_id: id(),
        name: "Part".into(),
        start_tick: Signed(0),
        length_tick: Signed(2400000),
        content_offset_tick: Signed(0),
        notes: vec![
            MidiNote {
                note_id: id(),
                start_tick: Signed(12345),
                length_tick: Signed(900000),
                pitch: 69,
                velocity: 85,
                release_velocity: 0,
                channel: 0,
            },
            MidiNote {
                note_id: id(),
                start_tick: Signed(540000),
                length_tick: Signed(1860000),
                pitch: 60,
                velocity: 76,
                release_velocity: 0,
                channel: 0,
            },
        ],
        controls: vec![
            MidiControl {
                event_id: id(),
                tick: Signed(600000),
                channel: 0,
                data: MidiControlData::Cc {
                    controller: 64,
                    value: 127,
                },
            },
            MidiControl {
                event_id: id(),
                tick: Signed(1500000),
                channel: 0,
                data: MidiControlData::Cc {
                    controller: 64,
                    value: 0,
                },
            },
            MidiControl {
                event_id: id(),
                tick: Signed(480000),
                channel: 0,
                data: MidiControlData::PitchBend { value: 2345 },
            },
        ],
    }));
    lane(
        &mut p,
        &tm,
        None,
        "synth.levelDb",
        &[(0, -3.), (2200000, 1.)],
    );
    lane(&mut p, &tm, None, "pan", &[(0, 0.5), (960000, -0.5)]);
    let reverb = p.tracks[1].inserts[1].effect_id.clone();
    lane(
        &mut p,
        &tm,
        Some(reverb),
        "bypass",
        &[(0, 0.), (1200000, 1.), (1920000, 0.)],
    );
    p.master.volume_db = -4.;
    p.master.inserts = vec![effect("delay"), effect("limiter")];
    lane(
        &mut p,
        "master",
        None,
        "volumeDb",
        &[(0, -4.), (960000, -9.), (2400000, -6.)],
    );
    p.validate().unwrap();
    (p, source)
}
fn plan(p: &Project, a: Arc<AudioAsset>, rate: u32) -> Arc<PlaybackPlan> {
    PlaybackPlan::compile(
        Arc::new(p.clone()),
        HashMap::from([(p.assets[0].asset_id.clone(), a)]),
        rate,
    )
    .unwrap()
}
fn offline(plan: Arc<PlaybackPlan>, block: usize) -> Vec<[f32; 2]> {
    let mut r = OfflineMaster::new(plan, Arc::new(|| false)).unwrap();
    let mut out = vec![];
    while !r.finished() {
        for _ in 0..block {
            if let Some(x) = r.next_frame().unwrap() {
                out.push(x)
            } else {
                break;
            }
        }
    }
    out
}
fn realtime(plan: Arc<PlaybackPlan>, block: usize, frames: usize) -> Vec<[f32; 2]> {
    let metrics = Arc::new(AudioMetrics::default());
    metrics.effects.configure(&plan.document);
    metrics.master.set_db(plan.document.master.volume_db);
    let source = AudioSource::timeline(plan.clone(), 0).unwrap();
    let (mut tx, rx) = rtrb::RingBuffer::new(COMMAND_CAPACITY);
    let state = Arc::new(TransportCell::default());
    let mut r = Renderer::new(rx, state, metrics, plan.rate, 2);
    r.set_declick(false);
    // A running device settles static fader changes before Play; export freezes that value.
    for _ in 0..plan.rate / 200 + 1 {
        r.render(&mut [0f32; 2]);
    }
    for action in [
        Action::Load {
            clip_id: 1,
            audio: source.clone(),
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
    let mut out = vec![];
    while out.len() < frames {
        let count = block.min(frames - out.len());
        let last = (out.len() + count).min(plan.frames).saturating_sub(1);
        if out.len() < plan.frames {
            let wait = Instant::now();
            while source.pair(last, last).is_none() {
                assert!(wait.elapsed() < Duration::from_secs(10));
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        let mut buffer = vec![0f32; count * 2];
        r.render(&mut buffer);
        out.extend(buffer.as_chunks::<2>().0.iter().copied());
    }
    assert_eq!(source.snapshot().starvation, 0);
    out
}
#[test]
fn sample_exact_realtime_master_deterministic_blocks_and_major_rates() {
    let (p, a) = fixture();
    for rate in [44100, 48000, 96000] {
        let p = plan(&p, a.clone(), rate);
        let out = offline(p.clone(), 4096);
        assert!(out.len() > p.frames);
        assert!(out.iter().flatten().any(|v| v.abs() > 0.001));
        assert!(out == offline(p.clone(), 17), "offline determinism");
        for block in [64, 511, 2048] {
            let rt = realtime(p.clone(), block, out.len());
            let error = out
                .iter()
                .zip(&rt)
                .flat_map(|(a, b)| a.iter().zip(b).map(|(a, b)| (*a - *b).abs()))
                .fold(0f32, f32::max);
            println!(
                "EXPORT_COMPARE {}",
                json!({"rate":rate,"block":block,"frames":out.len(),"maxAbsoluteError":error})
            );
            assert!(
                out == rt,
                "rate={rate} block={block} max={error} first={:?}",
                out.iter().zip(&rt).position(|(a, b)| a != b)
            );
        }
    }
}
#[test]
fn mute_solo_and_float_headroom_follow_existing_mix() {
    let (mut p, a) = fixture();
    p.automation.clear();
    p.master.inserts.clear();
    p.master.volume_db = 0.;
    p.tracks[0].mix.mute = true;
    p.tracks[1].mix.mute = true;
    assert!(offline(plan(&p, a.clone(), 48000), 128)
        .iter()
        .flatten()
        .all(|x| *x == 0.));
    p.tracks[0].mix.mute = false;
    p.tracks[0].mix.solo = true;
    p.tracks[0].mix.volume_db = 12.;
    p.tracks[0].inserts.clear();
    p.master.volume_db = 12.;
    let solo = offline(plan(&p, a.clone(), 48000), 128);
    p.tracks[1].mix.mute = false;
    assert_eq!(solo, offline(plan(&p, a, 48000), 64));
    assert!(solo.iter().flatten().any(|x| x.abs() > 1.));
}
struct Folder(PathBuf);
impl Folder {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("minidaw-export-test-{}", id()));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Folder {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn parse(bytes: &[u8]) -> (u16, u32, u16, Vec<[f32; 2]>) {
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(
        u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize + 8,
        bytes.len()
    );
    let fmt = u16::from_le_bytes(bytes[20..22].try_into().unwrap());
    let rate = u32::from_le_bytes(bytes[24..28].try_into().unwrap());
    let bits = u16::from_le_bytes(bytes[34..36].try_into().unwrap());
    let mut at = 36;
    while &bytes[at..at + 4] != b"data" {
        at += 8 + u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
    }
    let n = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
    assert_eq!(bytes.len(), at + 8 + n);
    let sample = |v: &[u8]| match bits {
        16 => i16::from_le_bytes(v.try_into().unwrap()) as f32 / 32768.,
        24 => ((i32::from_le_bytes([0, v[0], v[1], v[2]])) >> 8) as f32 / 8388608.,
        32 => f32::from_le_bytes(v.try_into().unwrap()),
        _ => panic!(),
    };
    let stride = bits as usize / 8;
    (
        fmt,
        rate,
        bits,
        bytes[at + 8..]
            .chunks_exact(stride * 2)
            .map(|p| [sample(&p[..stride]), sample(&p[stride..])])
            .collect(),
    )
}
#[test]
fn wave_formats_exact_conversion_no_dither_or_extra_limiting() {
    let folder = Folder::new();
    let input = [
        [0., -0.],
        [-1., 1.],
        [1.7, -1.7],
        [0.1234567, -0.1234567],
        [f32::MIN_POSITIVE, -f32::MIN_POSITIVE],
    ];
    for format in [WaveFormat::Pcm16, WaveFormat::Pcm24, WaveFormat::Float32] {
        let path = folder.0.join(format!("{format:?}.wav"));
        let mut wave = WaveWriter::new(File::create(&path).unwrap(), format, 96000).unwrap();
        for p in input {
            wave.frame(p).unwrap();
        }
        wave.finish().unwrap();
        let (fmt, rate, bits, out) = parse(&fs::read(&path).unwrap());
        assert_eq!(rate, 96000);
        assert_eq!(fmt, if format == WaveFormat::Float32 { 3 } else { 1 });
        for (a, b) in input.iter().flatten().zip(out.iter().flatten()) {
            if bits == 32 {
                assert_eq!(a.to_bits(), b.to_bits());
            } else {
                let scale = 2f64.powi(bits as i32 - 1);
                let expected =
                    ((*a as f64 * scale).round().clamp(-scale, scale - 1.) / scale) as f32;
                assert_eq!(expected, *b);
            }
        }
    }
}
fn disk_fixture(folder: &Folder) -> (Project, Vec<AssetState>) {
    let (mut p, a) = fixture();
    let path = folder.0.join("source.wav");
    let minidaw_lib::audio::source::AssetStorage::Memory(data) = &a.storage else {
        unreachable!()
    };
    let mut wave = WaveWriter::new(
        File::create(&path).unwrap(),
        WaveFormat::Float32,
        a.info.sample_rate,
    )
    .unwrap();
    for f in data.samples.as_chunks::<2>().0 {
        wave.frame([f[0], f[1]]).unwrap();
    }
    wave.finish().unwrap();
    p.assets[0].path = PathReference {
        project_relative_path: None,
        original_absolute_path: Some(paths::display(&path)),
    };
    p.assets[0].fingerprint = paths::fingerprint(&path).unwrap();
    let states = vec![AssetState {
        asset_id: p.assets[0].asset_id.clone(),
        status: "available",
        resolved_path: Some(paths::display(&path)),
        message: None,
    }];
    (p, states)
}
fn request(path: PathBuf, format: WaveFormat) -> Request {
    Request {
        job_id: id(),
        revision: 0,
        path: paths::display(&path),
        format,
        sample_rate: 48000,
        overwrite: false,
        mode: export::Mode::Mixdown,
        track_ids: vec![],
        range: None,
    }
}
fn selected_range(start: u64, end: u64) -> Option<export::FrameRange> {
    Some(export::FrameRange {
        start: Frames(start),
        end: Frames(end),
    })
}
#[test]
fn isolated_track_keeps_project_clock_for_automation_during_instrument_tails() {
    let folder = Folder::new();
    let (mut p, states) = disk_fixture(&folder);
    p.master = MasterMix::default();
    p.tracks[0].mix.mute = true;
    let audio = p.clips().next().unwrap().clip_id.clone();
    p = edit(
        &p,
        json!({"command":"audio.move","clipIds":[audio],"delta":{"unit":"seconds","numerator":"2","denominator":1}}),
    );
    p.automation.retain(|a| a.track_id == p.tracks[1].track_id);
    let midi = p.tracks[1].track_id.clone();
    let fx = p.tracks[1].inserts[1].effect_id.clone();
    lane(
        &mut p,
        &midi,
        Some(fx),
        "wet",
        &[(0, 0.1), (2600000, 0.1), (2900000, 0.9), (5000000, 0.2)],
    );
    let mut r = request(folder.0.join("master.wav"), WaveFormat::Float32);
    r.mode = export::Mode::Selection;
    r.range = selected_range(0, 120000);
    let full = export::render(
        &r,
        &p,
        &states,
        None,
        Arc::new(AtomicBool::new(false)),
        |_, _, _| {},
    )
    .unwrap();
    r.mode = export::Mode::Track;
    r.track_ids = vec![midi];
    r.path = paths::display(&folder.0.join("channel.wav"));
    let track = export::render(
        &r,
        &p,
        &states,
        None,
        Arc::new(AtomicBool::new(false)),
        |_, _, _| {},
    )
    .unwrap();
    let a = parse(&fs::read(full.path).unwrap()).3;
    let b = parse(&fs::read(track.path).unwrap()).3;
    let difference = a
        .iter()
        .flatten()
        .zip(b.iter().flatten())
        .map(|(a, b)| (a - b).abs())
        .fold(0f32, f32::max);
    assert_eq!(
        difference, 0.,
        "Channel automation must continue on the original project clock during MIDI tails"
    );
}
#[test]
fn selection_is_exact_full_render_slice_with_midi_fx_automation_and_silent_padding() {
    let folder = Folder::new();
    let (p, states) = disk_fixture(&folder);
    let before = p.clone();
    for rate in [44100, 48000] {
        let mut full = request(
            folder.0.join(format!("full-{rate}.wav")),
            WaveFormat::Float32,
        );
        full.sample_rate = rate;
        let report = export::render(
            &full,
            &p,
            &states,
            None,
            Arc::new(AtomicBool::new(false)),
            |_, _, _| {},
        )
        .unwrap();
        let expected = parse(&fs::read(report.path).unwrap()).3;
        for (start, end) in [
            (777, 32001),
            (31999, 32000),
            (expected.len() as u64 - 17, expected.len() as u64 + 29),
        ] {
            let mut r = request(
                folder.0.join(format!("selection-{rate}-{start}.wav")),
                WaveFormat::Float32,
            );
            r.sample_rate = rate;
            r.mode = export::Mode::Selection;
            r.range = selected_range(start, end);
            let out = export::render(
                &r,
                &p,
                &states,
                None,
                Arc::new(AtomicBool::new(false)),
                |_, _, _| {},
            )
            .unwrap();
            let actual = parse(&fs::read(out.path).unwrap()).3;
            assert_eq!(actual.len() as u64, end - start);
            assert_eq!(out.files[0].start_frame, start);
            assert_eq!(out.files[0].end_frame, end);
            for (n, f) in actual.iter().enumerate() {
                assert_eq!(
                    *f,
                    expected.get(start as usize + n).copied().unwrap_or([0.; 2])
                );
            }
        }
    }
    assert_eq!(p, before);
    println!("EXPORT_SELECTION exact PCM slice at 44100/48000 Hz; 1-frame interval and post-tail silence; project unchanged");
}
#[test]
fn channel_and_stems_are_isolated_post_fader_pre_master_aligned_and_collision_safe() {
    let folder = Folder::new();
    let (mut p, states) = disk_fixture(&folder);
    for t in &mut p.tracks {
        t.name = "Bass:/동일 이름.".into();
    }
    p.tracks[1].mix.solo = true;
    let before = p.clone();
    let cancel = Arc::new(AtomicBool::new(false));
    let mut r = request(folder.0.clone(), WaveFormat::Float32);
    r.mode = export::Mode::Stems;
    r.track_ids = p.tracks.iter().map(|t| t.track_id.clone()).rev().collect();
    let out = export::render(&r, &p, &states, None, cancel.clone(), |_, _, _| {}).unwrap();
    assert_eq!(out.files.len(), 2);
    assert!(out.files[0].path.contains("01 Bass__"));
    assert_ne!(out.files[0].path, out.files[1].path);
    let source =
        AudioAsset::open(PathBuf::from(&states[0].resolved_path.as_ref().unwrap()).as_path())
            .unwrap();
    for (i, f) in out.files.iter().enumerate() {
        assert_eq!(f.track_id.as_deref(), Some(p.tracks[i].track_id.as_str()));
        assert_eq!(f.start_frame, 0);
        assert_eq!(f.frames, out.frames);
        assert_eq!(f.end_frame, out.frames);
        let mut isolated = p.clone();
        isolated.tracks = vec![p.tracks[i].clone()];
        isolated.tracks[0].mix.solo = false;
        isolated.master = MasterMix::default();
        isolated.primary_clip_id = None;
        isolated
            .automation
            .retain(|a| a.track_id == p.tracks[i].track_id);
        let minimum = plan(&p, source.clone(), 48000).frames;
        let mut expected_plan = plan(&isolated, source.clone(), 48000);
        let isolated_plan = Arc::get_mut(&mut expected_plan).unwrap();
        isolated_plan.frames = isolated_plan.frames.max(minimum);
        let mut expected = offline(expected_plan, 333);
        expected.resize(out.frames as usize, [0.; 2]);
        let actual = parse(&fs::read(&f.path).unwrap()).3;
        assert_eq!(actual, expected, "No other Track or Master signal");
        let mut one = request(folder.0.join(format!("track-{i}.wav")), WaveFormat::Float32);
        one.mode = export::Mode::Track;
        one.track_ids = vec![p.tracks[i].track_id.clone()];
        one.range = selected_range(20001, 50009);
        let actual = export::render(&one, &p, &states, None, cancel.clone(), |_, _, _| {}).unwrap();
        assert_eq!(
            parse(&fs::read(actual.path).unwrap()).3,
            expected[20001..50009]
        );
    }
    let first = fs::read(&out.files[0].path).unwrap();
    let repeat = export::render(&r, &p, &states, None, cancel.clone(), |_, _, _| {}).unwrap();
    assert_ne!(out.path, repeat.path);
    assert_eq!(first, fs::read(&out.files[0].path).unwrap());
    assert_eq!(first, fs::read(&repeat.files[0].path).unwrap());
    let signal = cancel.clone();
    let mut prepared = 0;
    let error = export::render(&r, &p, &states, None, cancel, move |stage, _, _| {
        if stage == "preparing" {
            prepared += 1;
            if prepared == 2 {
                signal.store(true, Release);
            }
        }
    })
    .unwrap_err();
    assert_eq!(error.code, "export_cancelled");
    assert!(!fs::read_dir(&folder.0).unwrap().any(|e| e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".minidaw")));
    assert_eq!(p, before);
    println!(
        "EXPORT_STEMS {}",
        json!({"files":out.files.len(),"commonFrames":out.frames,"maxSampleError":0,"collisionSafe":true,"cancelCleanup":true})
    );
}
#[test]
fn invalid_export_targets_fail_before_creating_outputs() {
    let folder = Folder::new();
    let (p, states) = disk_fixture(&folder);
    let mut r = request(folder.0.join("invalid.wav"), WaveFormat::Float32);
    r.mode = export::Mode::Selection;
    for range in [
        None,
        selected_range(100, 100),
        selected_range(100, 99),
        selected_range(0, WaveFormat::Float32.max_frames() + 1),
    ] {
        r.range = range;
        assert_eq!(
            export::render(
                &r,
                &p,
                &states,
                None,
                Arc::new(AtomicBool::new(false)),
                |_, _, _| {}
            )
            .unwrap_err()
            .code,
            "export_target"
        );
    }
    r.mode = export::Mode::Track;
    r.range = None;
    r.track_ids = vec![id()];
    assert!(export::render(
        &r,
        &p,
        &states,
        None,
        Arc::new(AtomicBool::new(false)),
        |_, _, _| {}
    )
    .is_err());
    assert!(!PathBuf::from(&r.path).exists());
}
#[test]
fn full_disk_export_formats_streaming_cycle_readonly_and_determinism() {
    let folder = Folder::new();
    let (mut p, states) = disk_fixture(&folder);
    p.cycle = Some(Cycle {
        enabled: true,
        start_tick: Signed(0),
        end_tick: Signed(240000),
    });
    let original = p.clone();
    let mut reference = p.clone();
    reference.cycle = None;
    let source = AudioAsset::open(std::path::Path::new(
        states[0].resolved_path.as_ref().unwrap(),
    ))
    .unwrap();
    let expected = offline(plan(&reference, source, 48000), 333);
    let service = export::ExportService::default();
    for format in [WaveFormat::Pcm16, WaveFormat::Pcm24, WaveFormat::Float32] {
        let r = request(folder.0.join(format!("{format:?}.wav")), format);
        let result = service.run(r, p.clone(), states.clone(), None).unwrap();
        assert_eq!(result.frames as usize, expected.len());
        let bytes = fs::read(&result.path).unwrap();
        let (_, _, bits, decoded) = parse(&bytes);
        // Reopen with the production WAV decoder, independently of this test parser.
        let reopened = AudioAsset::open(std::path::Path::new(&result.path)).unwrap();
        assert_eq!(reopened.info.sample_rate, 48000);
        assert_eq!(reopened.info.channels, 2);
        assert_eq!(reopened.info.frames as u64, result.frames);
        let minidaw_lib::audio::source::AssetStorage::Memory(audio) = &reopened.storage else {
            panic!("small exported fixture must fit memory");
        };
        assert!(audio.samples.iter().eq(decoded.iter().flatten()));
        for (a, b) in expected.iter().flatten().zip(decoded.iter().flatten()) {
            let eps = if bits == 32 {
                0.
            } else {
                1. / 2f32.powi(bits as i32 - 1)
            };
            assert!((*a - *b).abs() <= eps);
        }
        let r = request(folder.0.join(format!("repeat-{format:?}.wav")), format);
        let repeated = service.run(r, p.clone(), states.clone(), None).unwrap();
        assert_eq!(bytes, fs::read(repeated.path).unwrap());
        assert_eq!(service.status().stage, "completed");
    }
    assert_eq!(p, original);
}
#[test]
fn cancel_error_overwrite_and_source_protection_preserve_files() {
    let folder = Folder::new();
    let (p, states) = disk_fixture(&folder);
    let target = folder.0.join("keep.wav");
    fs::write(&target, b"unchanged").unwrap();
    let mut r = request(target.clone(), WaveFormat::Float32);
    let cancel = Arc::new(AtomicBool::new(false));
    assert_eq!(
        export::render(&r, &p, &states, None, cancel.clone(), |_, _, _| {})
            .unwrap_err()
            .code,
        "export_exists"
    );
    r.overwrite = true;
    let signal = cancel.clone();
    assert_eq!(
        export::render(&r, &p, &states, None, cancel.clone(), move |_, _, _| signal
            .store(true, Release))
        .unwrap_err()
        .code,
        "export_cancelled"
    );
    assert_eq!(fs::read(&target).unwrap(), b"unchanged");
    assert!(!fs::read_dir(&folder.0).unwrap().any(|e| e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .ends_with(".tmp")));
    cancel.store(false, Release);
    r.path = states[0].resolved_path.clone().unwrap();
    let original = fs::read(&r.path).unwrap();
    assert_eq!(
        export::render(&r, &p, &states, None, cancel.clone(), |_, _, _| {})
            .unwrap_err()
            .code,
        "export_source"
    );
    assert_eq!(fs::read(&r.path).unwrap(), original);
    r.path = paths::display(&target);
    let result = export::render(&r, &p, &states, None, cancel, |_, _, _| {}).unwrap();
    assert!(result.frames > 0);
    assert_eq!(&fs::read(target).unwrap()[..4], b"RIFF");
}
