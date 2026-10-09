//! Track allocation AND deallocation on the thread actually executing render().
//! The process-wide allocator ignores other test/build/runtime threads.
use minidaw_lib::audio::{
    decoder::{AudioData, FileInfo},
    metrics::AudioMetrics,
    renderer::{Action, Command, Renderer, COMMAND_CAPACITY},
    source::AudioSource,
    transport::{PlayState, TransportCell},
};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    sync::Arc,
    time::Instant,
};

thread_local! {
    static TRACK: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
    static FREES: Cell<u64> = const { Cell::new(0) };
}
struct TrackingAllocator;
unsafe impl GlobalAlloc for TrackingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if TRACK.try_with(Cell::get).unwrap_or(false) {
            let _ = ALLOCS.try_with(|n| n.set(n.get() + 1));
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if TRACK.try_with(Cell::get).unwrap_or(false) {
            let _ = FREES.try_with(|n| n.set(n.get() + 1));
        }
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: TrackingAllocator = TrackingAllocator;

#[test]
fn effects_swap_reset_and_queue_backpressure_never_free_on_callback() {
    use minidaw_lib::project::{effects::Effect, schema::Project};
    let (mut tx, rx) = rtrb::RingBuffer::new(COMMAND_CAPACITY);
    let metrics = Arc::new(AudioMetrics::default());
    let mut p = Project::new();
    p.master.inserts = ["eq", "compressor", "reverb", "delay", "limiter"]
        .into_iter()
        .map(|kind| Effect::new(kind).unwrap())
        .collect();
    // Automation changes coefficients/values per sample and swaps compiled curves
    // while retaining the existing preallocated Effect state.
    use minidaw_lib::project::{
        automation::{Channel, Lane, Parameter, Point, Shape},
        schema::{id, Signed},
    };
    p.automation.push(Channel {
        track_id: "master".into(),
        read: true,
        write: false,
        lanes: vec![Lane {
            lane_id: id(),
            parameter: Parameter {
                effect_id: Some(p.master.inserts[0].effect_id.clone()),
                name: "band1.gainDb".into(),
            },
            points: vec![
                Point {
                    point_id: id(),
                    tick: Signed(0),
                    value: -6.,
                    shape: Shape::Linear,
                },
                Point {
                    point_id: id(),
                    tick: Signed(960000),
                    value: 6.,
                    shape: Shape::Linear,
                },
            ],
        }],
    });
    metrics.effects.configure(&p);
    let mut r = Renderer::new(
        rx,
        Arc::new(TransportCell::default()),
        metrics.clone(),
        48000,
        2,
    );
    let owner = AudioSource::memory(AudioData {
        info: FileInfo {
            name: "fx".into(),
            sample_rate: 48000,
            channels: 2,
            frames: 48000,
            duration: 1.,
            sanitized_samples: 0,
        },
        samples: vec![0.4; 96000],
    });
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
    let a = ALLOCS.get();
    let f = FREES.get();
    let io = minidaw_lib::audio::reader::operations_on_current_thread();
    let mut out = [0f32; 128];
    for i in 0..150 {
        p.master.inserts[0].enabled = i % 2 == 0;
        p.automation[0].lanes[0].points[1].value = (i % 24) as f64 - 12.;
        metrics.effects.configure(&p);
        if i % 20 == 0 {
            tx.push(Command {
                id: i,
                issued: Instant::now(),
                action: Action::Seek(0.),
            })
            .ok()
            .unwrap();
        }
        TRACK.set(true);
        r.render(&mut out);
        TRACK.set(false);
        assert!(out.iter().all(|v| v.is_finite() && v.abs() <= 1.));
        // Deliberately let reclamation fill up; callback must defer, never wait/drop.
        if i % 17 == 0 {
            metrics.effects.collect();
        }
    }
    assert_eq!(ALLOCS.get(), a);
    assert_eq!(FREES.get(), f);
    assert_eq!(
        minidaw_lib::audio::reader::operations_on_current_thread(),
        io
    );
}

fn audio() -> Arc<AudioSource> {
    AudioSource::for_output(
        minidaw_lib::audio::source::AudioAsset::memory(AudioData {
            info: FileInfo {
                name: "rt-test".into(),
                sample_rate: 44100,
                channels: 2,
                frames: 44100,
                duration: 1.0,
                sanitized_samples: 0,
            },
            samples: vec![0.15; 88200],
        }),
        48000,
    )
    .unwrap()
}

#[test]
fn callback_never_allocates_or_frees_even_when_replacing_audio() {
    let (mut producer, consumer) = rtrb::RingBuffer::new(COMMAND_CAPACITY);
    let transport = Arc::new(TransportCell::default());
    let metrics = Arc::new(AudioMetrics::default());
    let mut renderer = Renderer::new(consumer, transport.clone(), metrics.clone(), 48000, 2);
    renderer.set_declick(true);
    // This models the non-real-time retained-owner list used by AudioService.
    let owners = [audio(), audio()];
    let mut output = [0.0_f32; 512];
    let mut integer_output = [0_i16; 512];
    for iteration in 0..500 {
        let action = match iteration % 7 {
            0 => Action::Load {
                clip_id: iteration as u64 + 1,
                audio: owners[(iteration / 7) % 2].clone(),
            },
            1 => Action::Play,
            2 => Action::Seek(0.25),
            3 => Action::Toggle,
            4 => Action::Toggle,
            5 => Action::Stop,
            _ => Action::Unload,
        };
        producer
            .push(Command {
                id: iteration as u64 + 1,
                issued: Instant::now(),
                action,
            })
            .ok()
            .unwrap();
        metrics
            .master
            .set_db(if iteration % 2 == 0 { -6.0 } else { 0.0 });
        TRACK.set(true);
        if iteration % 2 == 0 {
            renderer.render(&mut output);
        } else {
            renderer.render(&mut integer_output);
        }
        TRACK.set(false);
        assert!(output.iter().all(|s| s.is_finite()));
    }
    assert_eq!(ALLOCS.get(), 0, "allocation occurred inside callback");
    assert_eq!(FREES.get(), 0, "deallocation occurred inside callback");
    assert_eq!(metrics.snapshot().callbacks, 500);
}

#[test]
fn spectrum_tap_is_nonblocking_allocation_free_and_bit_identical() {
    use minidaw_lib::audio::spectrum::{Settings, Spectrum};
    let spectrum = Arc::new(Spectrum::default());
    spectrum
        .configure(Settings {
            enabled: true,
            ..Settings::default()
        })
        .unwrap();
    // Native-rate memory source isolates analyzer side effects from an asynchronous SRC cursor.
    let owners = [AudioSource::memory(AudioData {
        info: FileInfo {
            name: "spectrum-rt".into(),
            sample_rate: 48000,
            channels: 2,
            frames: 48000,
            duration: 1.0,
            sanitized_samples: 0,
        },
        samples: vec![0.15; 96000],
    })];
    let make = || {
        let (mut tx, rx) = rtrb::RingBuffer::new(COMMAND_CAPACITY);
        let renderer = Renderer::new(
            rx,
            Arc::new(TransportCell::default()),
            Arc::new(AudioMetrics::default()),
            48000,
            2,
        );
        for (id, action) in [
            Action::Load {
                clip_id: 1,
                audio: owners[0].clone(),
            },
            Action::Play,
        ]
        .into_iter()
        .enumerate()
        {
            tx.push(Command {
                id: id as u64,
                issued: Instant::now(),
                action,
            })
            .ok()
            .unwrap();
        }
        (tx, renderer)
    };
    let (_tx_a, mut a) = make();
    let (_tx_b, mut b) = make();
    b.set_spectrum(spectrum.tap(48000).unwrap());
    let mut plain = [0f32; 512];
    let mut tapped = plain;
    let allocs = ALLOCS.get();
    let frees = FREES.get();
    let io = minidaw_lib::audio::reader::operations_on_current_thread();
    // Faster than wall clock intentionally fills the analysis queue. No callback waits.
    for _ in 0..250 {
        a.render(&mut plain);
        TRACK.set(true);
        b.render(&mut tapped);
        TRACK.set(false);
        assert_eq!(plain, tapped);
    }
    assert_eq!(ALLOCS.get(), allocs);
    assert_eq!(FREES.get(), frees);
    assert_eq!(
        minidaw_lib::audio::reader::operations_on_current_thread(),
        io
    );
    assert!(spectrum.snapshot(0).unwrap().dropped_blocks > 0);
    spectrum.configure(Settings::default()).unwrap();
    let before = spectrum.snapshot(0).unwrap().dropped_blocks;
    for _ in 0..20 {
        TRACK.set(true);
        b.render(&mut tapped);
        TRACK.set(false);
    }
    assert_eq!(spectrum.snapshot(0).unwrap().dropped_blocks, before);
    assert_eq!(ALLOCS.get(), allocs);
    assert_eq!(FREES.get(), frees);
}

#[test]
fn seek_while_paused_has_no_audio_latency_measurement() {
    let (mut tx, rx) = rtrb::RingBuffer::new(COMMAND_CAPACITY);
    let transport = Arc::new(TransportCell::default());
    let metrics = Arc::new(AudioMetrics::default());
    let mut renderer = Renderer::new(rx, transport.clone(), metrics.clone(), 48000, 2);
    let owner = audio();
    let mut output = [0.0_f32; 256];
    for (id, action) in [
        Action::Load {
            clip_id: 1,
            audio: owner.clone(),
        },
        Action::Play,
        Action::Pause,
        Action::Seek(0.7),
    ]
    .into_iter()
    .enumerate()
    {
        tx.push(Command {
            id: id as u64,
            issued: Instant::now(),
            action,
        })
        .ok()
        .unwrap();
        renderer.render(&mut output);
    }
    assert_eq!(transport.read().state, PlayState::Paused);
    assert!((transport.read().frame / 44100.0 - 0.7).abs() < 1e-9);
    assert_eq!(metrics.snapshot().seek_ms, None);
    assert!(output.iter().all(|s| *s == 0.0));
}

#[test]
fn playing_timeline_plan_swaps_never_allocate_free_or_read_on_callback() {
    use minidaw_lib::{
        audio::{source::AudioAsset, timeline::PlaybackPlan},
        project::schema::*,
    };
    use std::collections::HashMap;
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/stereo-44100.mp3");
    let asset = AudioAsset::open_with_budget(&path, 0).unwrap();
    let mut p = Project::new();
    p.import(serde_json::from_value(serde_json::json!({
        "assetId":id(),"filename":"fixture.mp3","path":{"projectRelativePath":"fixture.mp3","originalAbsolutePath":null},
        "metadata":{"sampleRate":44100,"channels":2,"sourceFrames":asset.info.frames.to_string(),"container":"mp3","codec":null},
        "fingerprint":{"fileBytes":"1","sampledSha256":"a".repeat(64)},"extensions":{}
    })).unwrap());
    let mut owners = Vec::new();
    for gain in [0.5, 0.75] {
        let Clip::Audio(c) = &mut p.tracks[0].clips[0] else {
            panic!("Audio fixture")
        };
        c.gain = gain;
        // Include a worker-rendered Pitch + Stretch cache in the replacement sequence.
        // The original no-Stretch plan above remains part of this test.
        if gain == 0.75 {
            let n = c.source_end.0;
            c.extensions.insert(
                minidaw_lib::project::stretch::KEY.into(),
                serde_json::json!({
                    "sourceStart":"0", "sourceEnd":n.to_string(), "outputFrames":(n*3/2).to_string()
                }),
            );
            c.source_end = Frames(n * 3 / 2);
            minidaw_lib::project::pitch::set(c, minidaw_lib::project::pitch::Pitch {semitones:5,cents:37}).unwrap();
        }
        let plan = PlaybackPlan::compile(
            Arc::new(p.clone()),
            HashMap::from([(p.assets[0].asset_id.clone(), asset.clone())]),
            48000,
        )
        .unwrap();
        owners.push(AudioSource::timeline(plan, 0).unwrap());
    }
    let (mut tx, rx) = rtrb::RingBuffer::new(COMMAND_CAPACITY);
    let transport = Arc::new(TransportCell::default());
    let mut renderer = Renderer::new(
        rx,
        transport.clone(),
        Arc::new(AudioMetrics::default()),
        48000,
        2,
    );
    renderer.set_declick(true);
    let mut output = [0_f32; 128];
    let allocations = ALLOCS.get();
    let frees = FREES.get();
    #[cfg(debug_assertions)]
    let operations = minidaw_lib::audio::reader::operations_on_current_thread();
    for i in 0..200 {
        let action = match i {
            0 => Action::Load {
                clip_id: 1,
                audio: owners[0].clone(),
            },
            1 => Action::Play,
            _ if i % 2 == 0 => Action::AutomationReplace {
                clip_id: i + 1,
                audio: owners[i as usize % 2].clone(),
            },
            _ => Action::Replace {
                clip_id: i + 1,
                audio: owners[i as usize % 2].clone(),
            },
        };
        tx.push(Command {
            id: i + 1,
            issued: Instant::now(),
            action,
        })
        .ok()
        .unwrap();
        TRACK.set(true);
        renderer.render(&mut output);
        TRACK.set(false);
        assert!(output.iter().all(|s| s.is_finite()));
        if i > 0 {
            assert_eq!(transport.read().state, PlayState::Playing);
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(ALLOCS.get(), allocations);
    assert_eq!(FREES.get(), frees);
    #[cfg(debug_assertions)]
    assert_eq!(
        minidaw_lib::audio::reader::operations_on_current_thread(),
        operations
    );
}

#[test]
fn cycle_wraps_streaming_src_without_callback_io_or_allocation() {
    use minidaw_lib::{
        audio::{source::AudioAsset, timeline::PlaybackPlan},
        project::schema::*,
    };
    use std::collections::HashMap;
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/stereo-44100.mp3");
    let asset = AudioAsset::open_with_budget(&path, 0).unwrap();
    let mut p = Project::new();
    p.import(serde_json::from_value(serde_json::json!({
        "assetId":id(),"filename":"fixture.mp3","path":{"projectRelativePath":"fixture.mp3","originalAbsolutePath":null},
        "metadata":{"sampleRate":44100,"channels":2,"sourceFrames":asset.info.frames.to_string(),"container":"mp3","codec":null},
        "fingerprint":{"fileBytes":"1","sampledSha256":"a".repeat(64)},"extensions":{}
    })).unwrap());
    // 44.1 kHz Streaming MP3 -> 48 kHz output, .1–.3 second cycle.
    p.cycle = Some(Cycle {
        enabled: true,
        start_tick: Signed(192000),
        end_tick: Signed(576000),
    });
    let plan = PlaybackPlan::compile(
        Arc::new(p.clone()),
        HashMap::from([(p.assets[0].asset_id.clone(), asset)]),
        48000,
    )
    .unwrap();
    let source = AudioSource::timeline(plan, 0).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(150));
    let (mut tx, rx) = rtrb::RingBuffer::new(COMMAND_CAPACITY);
    let transport = Arc::new(TransportCell::default());
    let mut renderer = Renderer::new(
        rx,
        transport.clone(),
        Arc::new(AudioMetrics::default()),
        48000,
        2,
    );
    for (id, action) in [
        Action::Load {
            clip_id: 1,
            audio: source.clone(),
        },
        Action::Play,
    ]
    .into_iter()
    .enumerate()
    {
        tx.push(Command {
            id: id as u64,
            issued: Instant::now(),
            action,
        })
        .ok()
        .unwrap();
    }
    let mut output = [0f32; 512];
    let allocations = ALLOCS.get();
    let frees = FREES.get();
    #[cfg(debug_assertions)]
    let operations = minidaw_lib::audio::reader::operations_on_current_thread();
    for _ in 0..240 {
        TRACK.set(true);
        renderer.render(&mut output);
        TRACK.set(false);
        assert_eq!(transport.read().state, PlayState::Playing);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(
        transport.read().frame,
        source.timeline_frame(240 * 256) as f64
    );
    assert_eq!(source.snapshot().starvation, 0);
    assert_eq!(ALLOCS.get(), allocations);
    assert_eq!(FREES.get(), frees);
    #[cfg(debug_assertions)]
    assert_eq!(
        minidaw_lib::audio::reader::operations_on_current_thread(),
        operations
    );
}

#[test]
fn streaming_callback_never_allocates_frees_reads_seeks_or_decodes() {
    use minidaw_lib::audio::source::AudioAsset;
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/stereo-44100.mp3");
    let asset = AudioAsset::open_with_budget(&path, 0).unwrap();
    let owners = [
        AudioSource::for_output(asset.clone(), 48000).unwrap(),
        AudioSource::for_output(asset, 48000).unwrap(),
        audio(),
    ];
    let (mut tx, rx) = rtrb::RingBuffer::new(COMMAND_CAPACITY);
    let mut renderer = Renderer::new(
        rx,
        Arc::new(TransportCell::default()),
        Arc::new(AudioMetrics::default()),
        48000,
        2,
    );
    renderer.set_declick(true);
    let mut output = [0_f32; 960];
    #[cfg(debug_assertions)]
    let operations = minidaw_lib::audio::reader::operations_on_current_thread();
    let allocations = ALLOCS.get();
    let frees = FREES.get();
    for i in 0..500 {
        let action = match i % 8 {
            0 => Action::Load {
                clip_id: i + 1,
                audio: owners[(i as usize / 8) % 3].clone(),
            },
            1 => Action::Play,
            2 => Action::Seek((i % 5) as f64),
            3 | 4 => Action::Toggle,
            5 => Action::Pause,
            6 => Action::Stop,
            _ => Action::Unload,
        };
        tx.push(Command {
            id: i + 1,
            issued: Instant::now(),
            action,
        })
        .ok()
        .unwrap();
        TRACK.set(true);
        renderer.render(&mut output);
        TRACK.set(false);
        assert!(output.iter().all(|s| s.is_finite()));
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(ALLOCS.get(), allocations);
    assert_eq!(FREES.get(), frees);
    #[cfg(debug_assertions)]
    assert_eq!(
        minidaw_lib::audio::reader::operations_on_current_thread(),
        operations
    );
}

#[test]
fn midi_sample_scheduler_and_plan_changes_are_realtime_safe() {
    use minidaw_lib::{
        audio::{
            midi::{MidiEvent, MidiSink},
            timeline::PlaybackPlan,
        },
        project::schema::*,
    };
    let mut p = Project::new();
    p.tracks.push(Track {
        synth: Default::default(),
        inserts: vec![],
        track_id: id(),
        name: "MIDI".into(),
        kind: TrackKind::Midi,
        instrument: Instrument::BasicSynth,
        mix: TrackMix::default(),
        extensions: Extensions::new(),
        clips: vec![Clip::Midi(MidiClip {
            content_offset_tick: Signed(0),
            clip_id: id(),
            name: "Part".into(),
            start_tick: Signed(0),
            length_tick: Signed(960000),
            controls: vec![
                MidiControl {
                    event_id: id(),
                    tick: Signed(0),
                    channel: 0,
                    data: MidiControlData::Cc {
                        controller: 64,
                        value: 127,
                    },
                },
                MidiControl {
                    event_id: id(),
                    tick: Signed(4000),
                    channel: 0,
                    data: MidiControlData::PitchBend { value: 2000 },
                },
            ],
            notes: vec![MidiNote {
                note_id: id(),
                start_tick: Signed(0),
                length_tick: Signed(480000),
                pitch: 60,
                velocity: 100,
                release_velocity: 0,
                channel: 0,
            }],
        })],
    });
    p.cycle = Some(Cycle {
        enabled: true,
        start_tick: Signed(0),
        end_tick: Signed(9600),
    });
    let plan = PlaybackPlan::compile(Arc::new(p), Default::default(), 48000).unwrap();
    let owners = [
        AudioSource::timeline(plan.clone(), 0).unwrap(),
        AudioSource::timeline(plan, 0).unwrap(),
    ];
    std::thread::sleep(std::time::Duration::from_millis(20));
    let (mut tx, rx) = rtrb::RingBuffer::new(COMMAND_CAPACITY);
    let mut renderer = Renderer::new(
        rx,
        Arc::new(TransportCell::default()),
        Arc::new(AudioMetrics::default()),
        48000,
        2,
    );
    struct Count(usize);
    impl MidiSink for Count {
        fn control(&mut self, _: minidaw_lib::audio::midi::MidiControlEvent) {
            self.0 += 1;
        }
        fn event(&mut self, _: MidiEvent) {
            self.0 += 1;
        }
    }
    let mut sink = Count(0);
    let hub = Arc::new(minidaw_lib::audio::midi_input::LiveMidi::default());
    hub.route(Some(
        owners[0].timeline.as_ref().unwrap().midi.track_keys[0],
    ));
    tx.push(Command {
        id: 0,
        issued: Instant::now(),
        action: Action::MidiInput(hub.clone()),
    })
    .ok()
    .unwrap();
    let computer = Arc::new(minidaw_lib::audio::midi_input::LiveMidi::default());
    computer.route(Some(owners[0].timeline.as_ref().unwrap().midi.track_keys[0]));
    tx.push(Command { id: 0, issued: Instant::now(), action: Action::ComputerMidi(computer.clone()) }).ok().unwrap();
    let mut output = [0f32; 128];
    let allocations = ALLOCS.get();
    let frees = FREES.get();
    let io = minidaw_lib::audio::reader::operations_on_current_thread();
    for i in 0..80 {
        let action = match i % 8 {
            0 => Action::Replace {
                clip_id: i + 1,
                audio: owners[(i as usize / 8) % 2].clone(),
            },
            1 | 5 => Action::Play,
            2 => Action::Seek(0.001),
            3 => Action::Pause,
            4 => Action::Stop,
            6 => Action::Toggle,
            _ => Action::Toggle,
        };
        tx.push(Command {
            id: i + 1,
            issued: Instant::now(),
            action,
        })
        .ok()
        .unwrap();
        TRACK.set(true);
        hub.receive(&[0x90, 70, 100]);
        hub.receive(&[0x80, 70, 0]);
        hub.receive(&[0xb0, 64, 127]);
        computer.receive(&[0x90, 64, 100]);
        computer.receive(&[0x90, 67, 100]);
        computer.receive(&[0x80, 64, 0]);
        computer.receive(&[0x80, 67, 0]);
        renderer.render_with_midi(&mut output, &mut sink);
        TRACK.set(false);
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(sink.0 > 20);
    assert_eq!(ALLOCS.get(), allocations);
    assert_eq!(FREES.get(), frees);
    assert_eq!(
        minidaw_lib::audio::reader::operations_on_current_thread(),
        io
    );
}

#[test]
fn multitrack_streaming_and_synth_mix_never_allocates_frees_or_reads_files_in_callback() {
    use minidaw_lib::{
        audio::{source::AudioAsset, timeline::PlaybackPlan},
        project::{
            edit::{self, Clipboard},
            schema::*,
        },
    };
    let mut p = Project::new();
    let mut assets = std::collections::HashMap::new();
    for file in ["stereo-44100.wav", "모노-48000.WAV"] {
        let a = AudioAsset::open_with_budget(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../tests/fixtures")
                .join(file),
            0,
        )
        .unwrap();
        let key = id();
        p.import(Asset {
            asset_id: key.clone(),
            filename: file.into(),
            path: PathReference {
                project_relative_path: Some(file.into()),
                original_absolute_path: None,
            },
            metadata: AudioMetadata {
                sample_rate: a.info.sample_rate,
                channels: a.info.channels as u16,
                source_frames: Frames(a.info.frames as u64),
                container: "wav".into(),
                codec: None,
            },
            fingerprint: Fingerprint {
                file_bytes: Frames(100),
                sampled_sha256: "a".repeat(64),
            },
            extensions: Extensions::new(),
        });
        assets.insert(key, a);
    }
    let mut second = p.tracks[0].clone();
    second.track_id = id();
    second.name = "Audio 2".into();
    second.clips = vec![p.tracks[0].clips.pop().unwrap()];
    second.mix.pan = 1.0;
    p.tracks[0].mix.pan = -1.0;
    p.tracks.push(second);
    for pitch in [60, 69] {
        let r = serde_json::from_value(serde_json::json!({"command":"midi.track.add"})).unwrap();
        p = edit::apply(&p, &r, &mut Clipboard::default()).unwrap();
        let track = p.tracks.last_mut().unwrap();
        track.instrument = Instrument::BasicSynth;
        track.mix.volume_db = -6.0;
        track.clips.push(Clip::Midi(MidiClip {
            clip_id: id(),
            name: "Part".into(),
            start_tick: Signed(0),
            length_tick: Signed(3840000),
            content_offset_tick: Signed(0),
            controls: vec![],
            notes: vec![MidiNote {
                note_id: id(),
                start_tick: Signed(0),
                length_tick: Signed(3000000),
                pitch,
                velocity: 100,
                release_velocity: 0,
                channel: 0,
            }],
        }));
    }
    for track in &mut p.tracks {
        track.inserts = ["eq", "compressor", "reverb", "delay", "limiter"]
            .into_iter()
            .map(|kind| minidaw_lib::project::effects::Effect::new(kind).unwrap())
            .collect();
    }
    p.master.inserts = ["eq", "compressor", "reverb", "delay", "limiter"]
        .into_iter()
        .map(|kind| minidaw_lib::project::effects::Effect::new(kind).unwrap())
        .collect();
    let plan = PlaybackPlan::compile(Arc::new(p), assets, 48000).unwrap();
    assert_eq!(plan.voices.len(), 2);
    let owner = AudioSource::timeline(plan, 0).unwrap();
    let (mut tx, rx) = rtrb::RingBuffer::new(COMMAND_CAPACITY);
    let metrics = Arc::new(AudioMetrics::default());
    metrics
        .effects
        .configure(&owner.timeline.as_ref().unwrap().document);
    let mut renderer = Renderer::new(
        rx,
        Arc::new(TransportCell::default()),
        metrics.clone(),
        48000,
        2,
    );
    let spectrum = Arc::new(minidaw_lib::audio::spectrum::Spectrum::default());
    let plan = owner.timeline.as_ref().unwrap();
    spectrum
        .configure_tracks(
            minidaw_lib::audio::spectrum::Settings {
                enabled: true,
                ..Default::default()
            },
            Some(minidaw_lib::audio::spectrum::TrackCapture {
                source_id: 1,
                plan: plan.clone(),
                tracks: [0, 2]
                    .into_iter()
                    .map(|i| (plan.document.tracks[i].track_id.clone(), i))
                    .collect(),
            }),
        )
        .unwrap();
    renderer.set_spectrum(spectrum.tap(48000).unwrap());
    let mut output = [0f32; 128];
    let allocations = ALLOCS.get();
    let frees = FREES.get();
    let io = minidaw_lib::audio::reader::operations_on_current_thread();
    let mut energy = 0.0;
    for i in 0..600 {
        let action = match i {
            0 => Some(Action::Load {
                clip_id: 1,
                audio: owner.clone(),
            }),
            1 => Some(Action::Play),
            300 => Some(Action::Seek(0.2)),
            _ => None,
        };
        if let Some(action) = action {
            tx.push(Command {
                id: i,
                issued: Instant::now(),
                action,
            })
            .ok()
            .unwrap();
        }
        TRACK.set(true);
        renderer.render(&mut output);
        TRACK.set(false);
        energy += output.iter().map(|v| v.abs() as f64).sum::<f64>();
        if i % 8 == 0 {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    assert!(energy > 100.0);
    assert_eq!(ALLOCS.get(), allocations);
    assert_eq!(FREES.get(), frees);
    assert_eq!(
        minidaw_lib::audio::reader::operations_on_current_thread(),
        io
    );
}
