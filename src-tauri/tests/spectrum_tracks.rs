use minidaw_lib::{
    audio::{
        decoder::{AudioData, FileInfo},
        metrics::AudioMetrics,
        renderer::{Action, Command, Renderer, COMMAND_CAPACITY},
        source::{AudioAsset, AudioSource},
        spectrum::{Settings, Spectrum, SpectrumFrame, TrackCapture},
        streaming::FrameReader,
        timeline::{PlaybackPlan, TimelineReader},
        transport::TransportCell,
    },
    project::schema::*,
};
use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};
fn scene() -> Arc<PlaybackPlan> {
    let mut p = Project::new();
    let mut assets = HashMap::new();
    for (n, hz) in [1000., 5000.].into_iter().enumerate() {
        let key = id();
        let rate = if n == 0 { 44100 } else { 48000 };
        let a = AudioAsset::memory(AudioData {
            info: FileInfo {
                name: "tone.wav".into(),
                sample_rate: rate,
                channels: 2,
                frames: rate as usize * 2,
                duration: 2.,
                sanitized_samples: 0,
            },
            samples: (0..rate * 2)
                .flat_map(|i| {
                    let s =
                        (std::f64::consts::TAU * hz * i as f64 / rate as f64).sin() as f32 * 0.5;
                    [s, s]
                })
                .collect(),
        });
        p.import(Asset {
            asset_id: key.clone(),
            filename: "tone.wav".into(),
            path: PathReference {
                project_relative_path: Some("tone.wav".into()),
                original_absolute_path: None,
            },
            metadata: AudioMetadata {
                sample_rate: rate,
                channels: 2,
                source_frames: Frames(rate as u64 * 2),
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
    let mut t = p.tracks[0].clone();
    t.track_id = id();
    t.name = "Second".into();
    t.clips = vec![p.tracks[0].clips.pop().unwrap()];
    p.tracks.push(t);
    p.tracks[0].mix.volume_db = -6.;
    p.tracks[0].mix.pan = -1.;
    p.tracks[0].clips[0].as_audio_mut().unwrap().gain = 0.5;
    PlaybackPlan::compile(Arc::new(p), assets, 48000).unwrap()
}
fn config(s: &Spectrum, plan: &Arc<PlaybackPlan>, indices: &[usize]) {
    s.configure_tracks(
        Settings {
            enabled: true,
            fft_size: 4096,
            smoothing_ms: 0,
        },
        Some(TrackCapture {
            source_id: 7,
            plan: plan.clone(),
            tracks: indices
                .iter()
                .map(|i| (plan.document.tracks[*i].track_id.clone(), *i))
                .collect(),
        }),
    )
    .unwrap();
}
fn peak(f: &SpectrumFrame, hz: f32, ch: usize) -> f32 {
    f.frequencies
        .iter()
        .zip(&f.curves[ch])
        .filter(|(h, _)| (**h / hz).ln().abs() < 0.06)
        .map(|(_, d)| *d)
        .fold(-120., f32::max)
}
#[test]
fn isolated_audio_reader_matches_master_sum_with_src_gain_pan_and_seek() {
    let p = scene();
    let mut master = TimelineReader::new(p.clone());
    let mut a = TimelineReader::for_track(p.clone(), 0);
    let mut b = TimelineReader::for_track(p, 1);
    for at in [0, 20000, 5000, 60000] {
        for r in [&mut master, &mut a, &mut b] {
            r.seek(at, Arc::new(|| false)).unwrap();
        }
        for _ in 0..4096 {
            let m = master.read_frame().unwrap();
            let x = a.read_frame().unwrap();
            let y = b.read_frame().unwrap();
            for ch in 0..2 {
                assert!((m[ch] - x[ch] - y[ch]).abs() < 1e-7);
            }
            assert_eq!(x[1], 0.);
        }
    }
}
#[test]
fn dual_worker_curves_are_isolated_and_stale_source_blocks_cannot_leak() {
    let p = scene();
    let s = Arc::new(Spectrum::default());
    config(&s, &p, &[0, 1]);
    let mut tap = s.tap(48000).unwrap();
    // Actual playback clock discontinuities (Seek/Cycle) are carried with each sample.
    for at in (0..48).map(|i| 10000 + i * 256) {
        assert!(tap.begin_source(7));
        for i in 0..256 {
            tap.push_tracks([[0.; 2]; 2], Some(at + i));
        }
        std::thread::sleep(Duration::from_millis(6));
    }
    let v = s.snapshot(0).unwrap();
    assert!(v.error.is_none());
    let a = v.frame.unwrap();
    let b = v.comparison.unwrap();
    assert_eq!(a.sequence, b.sequence);
    assert_eq!(v.sources.len(), 2);
    assert!((peak(&a, 1000., 0) + 18.0412).abs() < 0.22);
    assert!(peak(&a, 5000., 0) < -80.);
    assert_eq!(peak(&a, 1000., 1), -120.);
    assert!((peak(&b, 5000., 0) + 6.0206).abs() < 0.22);
    assert!(peak(&b, 1000., 0) < -80.);
    config(&s, &p, &[1]);
    for _ in 0..24 {
        tap.begin_source(8);
        for i in 0..256 {
            tap.push_tracks([[1.; 2]; 2], Some(i));
        }
        std::thread::sleep(Duration::from_millis(6));
    }
    assert!(s.snapshot(0).unwrap().frame.is_none());
    for at in (0..32).map(|i| 30000 + i * 256) {
        tap.begin_source(7);
        for i in 0..256 {
            tap.push_tracks([[0.; 2]; 2], Some(at + i));
        }
        std::thread::sleep(Duration::from_millis(6));
    }
    let v = s.snapshot(0).unwrap();
    assert!(v.comparison.is_none());
    assert!((peak(&v.frame.unwrap(), 5000., 0) + 6.0206).abs() < 0.22);
    s.configure(Settings::default()).unwrap();
    assert!(!tap.begin_source(7));
    assert!(s.snapshot(0).unwrap().frame.is_none());
}
#[test]
fn comparison_capture_does_not_change_a_single_master_pcm_sample() {
    let plan = scene();
    let s = Arc::new(Spectrum::default());
    config(&s, &plan, &[0, 1]);
    let make = || {
        let (mut tx, rx) = rtrb::RingBuffer::new(COMMAND_CAPACITY);
        let audio = AudioSource::timeline(plan.clone(), 0).unwrap();
        let renderer = Renderer::new(
            rx,
            Arc::new(TransportCell::default()),
            Arc::new(AudioMetrics::default()),
            48000,
            2,
        );
        for (id, action) in [
            Action::Load {
                clip_id: 7,
                audio: audio.clone(),
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
        (tx, renderer, audio)
    };
    let (_ta, mut a, _oa) = make();
    let (_tb, mut b, _ob) = make();
    // Consume much faster than real time below: first prime both independent
    // playback workers so their scheduling cannot masquerade as a PCM change.
    let deadline = Instant::now() + Duration::from_secs(5);
    while _oa.snapshot().buffered_frames < 25600 || _ob.snapshot().buffered_frames < 25600 {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    b.set_spectrum(s.tap(48000).unwrap());
    let mut x = [0f32; 512];
    let mut y = x;
    for _ in 0..100 {
        a.render(&mut x);
        b.render(&mut y);
        assert_eq!(x, y);
    }
}

#[test]
fn actual_synth_capture_resolves_midi_bus_ids_after_audio_tracks() {
    let audio = scene();
    let mut p = (*audio.document).clone();
    for pitch in [69, 76] {
        p.tracks.push(Track {
            synth: Default::default(),
            inserts: vec![],
            track_id: id(),
            name: format!("Synth {pitch}"),
            kind: TrackKind::Midi,
            instrument: Instrument::BasicSynth,
            mix: TrackMix::default(),
            extensions: Extensions::new(),
            clips: vec![Clip::Midi(MidiClip {
                clip_id: id(),
                name: "Part".into(),
                start_tick: Signed(0),
                length_tick: Signed(3840000),
                content_offset_tick: Signed(0),
                controls: vec![],
                notes: vec![MidiNote {
                    note_id: id(),
                    start_tick: Signed(0),
                    length_tick: Signed(3840000),
                    pitch,
                    velocity: 100,
                    release_velocity: 0,
                    channel: 0,
                }],
            })],
        });
    }
    let plan = PlaybackPlan::compile(Arc::new(p), audio.assets.clone(), 48000).unwrap();
    let s = Arc::new(Spectrum::default());
    config(&s, &plan, &[2, 3]);
    let tap = s.tap(48000).unwrap();
    assert_eq!(tap.tracks(7), [Some(0), Some(1)]);
    assert_eq!(tap.tracks(8), [None; 2]);
    let owner = AudioSource::timeline(plan.clone(), 0).unwrap();
    let (mut tx, rx) = rtrb::RingBuffer::new(COMMAND_CAPACITY);
    let mut renderer = Renderer::new(
        rx,
        Arc::new(TransportCell::default()),
        Arc::new(AudioMetrics::default()),
        48000,
        2,
    );
    renderer.set_spectrum(tap);
    for (id, action) in [
        Action::Load {
            clip_id: 7,
            audio: owner.clone(),
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
    let mut out = [0f32; 512];
    for _ in 0..64 {
        renderer.render(&mut out);
        std::thread::sleep(Duration::from_millis(6));
    }
    let v = s.snapshot(0).unwrap();
    let a = v.frame.unwrap();
    let b = v.comparison.unwrap();
    assert!(peak(&a, 440., 0) > -30.);
    assert!(peak(&b, 659.25, 0) > -30.);
    assert!(peak(&a, 5000., 0) < -70.);
    assert!(peak(&b, 1000., 0) < -70.);
    config(&s, &plan, &[0, 3]);
    let mut tap = s.tap(48000).unwrap();
    assert!(tap.begin_source(7));
    assert!(!tap.is_master());
    assert_eq!(tap.tracks(7), [None, Some(1)]);
}
