//! Real DLL tests: deterministic VST3/CLAP Instrument + delayed Effects.
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
    plugins::{self, Selection},
    project::{
        automation::{self, Lane, Parameter, Point, Shape},
        edit::{self, Clipboard},
        effects::{Effect, Processor},
        schema::*,
    },
};
use serde_json::json;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};
thread_local! {static WATCH:Cell<bool>=const{Cell::new(false)};static ALLOCS:Cell<usize>=const{Cell::new(0)};static FREES:Cell<usize>=const{Cell::new(0)};}
struct Alloc;
unsafe impl GlobalAlloc for Alloc {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if WATCH.try_with(Cell::get).unwrap_or(false) {
            ALLOCS.set(ALLOCS.get() + 1);
        }
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        if WATCH.try_with(Cell::get).unwrap_or(false) {
            FREES.set(FREES.get() + 1);
        }
        unsafe { System.dealloc(p, l) }
    }
}
#[global_allocator]
static A: Alloc = Alloc;
fn root(format: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../tests/local/pdc-fixtures/PdcFixture.{format}"))
}
fn select(format: &str, instrument: bool, latency: usize, gain: f64) -> Selection {
    let path = root(format);
    assert!(path.exists(), "Build tests/plugin-fixture first");
    let d = plugins::scan_one(&path, format)
        .unwrap()
        .into_iter()
        .find(|d| d.instrument == instrument)
        .unwrap();
    let mut s = plugins::prepare(d, 48000).unwrap();
    for p in &mut s.parameters {
        p.value = match p.id {
            0 => {
                if format == "vst3" {
                    latency as f64 / 8192.
                } else {
                    latency as f64
                }
            }
            1 => {
                if format == "vst3" {
                    (gain + 1.) / 2.
                } else {
                    gain
                }
            }
            _ => p.value,
        };
    }
    s
}
fn fx(format: &str, latency: usize) -> Effect {
    Effect {
        effect_id: id(),
        enabled: true,
        processor: Processor::External {
            plugin: select(format, false, latency, 1.),
        },
    }
}
fn edit(p: &Project, r: serde_json::Value) -> Project {
    edit::apply(
        p,
        &serde_json::from_value(r).unwrap(),
        &mut Clipboard::default(),
    )
    .unwrap()
}
fn fixture(format: &str) -> (Project, Arc<AudioAsset>) {
    let source = AudioAsset::memory(AudioData {
        info: FileInfo {
            name: "impulse.wav".into(),
            sample_rate: 48000,
            channels: 2,
            frames: 4000,
            duration: 4000. / 48000.,
            sanitized_samples: 0,
        },
        samples: (0..4000)
            .flat_map(|i| {
                [if [500, 1500, 2500].contains(&i) {
                    0.25
                } else {
                    0.
                }; 2]
            })
            .collect(),
    });
    let mut p = Project::new();
    p.import(Asset {
        asset_id: id(),
        filename: "impulse.wav".into(),
        path: PathReference {
            project_relative_path: Some("impulse.wav".into()),
            original_absolute_path: None,
        },
        metadata: AudioMetadata {
            sample_rate: 48000,
            channels: 2,
            source_frames: Frames(4000),
            container: "wav".into(),
            codec: None,
        },
        fingerprint: Fingerprint {
            file_bytes: Frames(32044),
            sampled_sha256: "a".repeat(64),
        },
        extensions: Extensions::new(),
    });
    p.tracks[0].inserts = vec![fx(format, 13), fx(format, 257)];
    p.tracks[0].mix.pan = -1.;
    p = edit(&p, json!({"command":"midi.track.add"}));
    let t = p.tracks.last().unwrap().track_id.clone();
    p = edit(
        &p,
        json!({"command":"plugin.instrument","trackIds":[t],"plugin":select(format,true,127,0.25)}),
    );
    p = edit(
        &p,
        json!({"command":"midi.clip.add","trackIds":[t],"targetTick":"0","lengthTick":"160000"}),
    );
    let c = p.midi_clips().next().unwrap().clip_id.clone();
    for f in [500, 1500, 2500] {
        p = edit(
            &p,
            json!({"command":"midi.note.add","clipIds":[c],"targetTick":(f*40).to_string(),"lengthTick":"40","pitch":60,"velocity":127}),
        );
    }
    p.tracks[1].mix.pan = 1.;
    p.tracks[1].inserts = vec![fx(format, 257)];
    p.master.inserts = vec![fx(format, 31)];
    (p, source)
}
fn plan(p: &Project, a: Arc<AudioAsset>) -> Arc<PlaybackPlan> {
    PlaybackPlan::compile(
        Arc::new(p.clone()),
        HashMap::from([(p.assets[0].asset_id.clone(), a)]),
        48000,
    )
    .unwrap()
}
fn offline(p: Arc<PlaybackPlan>, n: usize) -> Vec<[f32; 2]> {
    let _scope = plugins::OfflineScope::new();
    let mut r = OfflineMaster::new(p, Arc::new(|| false)).unwrap();
    (0..n).map(|_| r.next_frame().unwrap().unwrap()).collect()
}
struct Live {
    r: Renderer,
    source: Arc<AudioSource>,
    tx: rtrb::Producer<Command>,
    metrics: Arc<AudioMetrics>,
    shared: Arc<TransportCell>,
}
impl Live {
    fn new(p: Arc<PlaybackPlan>) -> Self {
        let metrics = Arc::new(AudioMetrics::default());
        metrics.effects.configure(&p.document);
        let source = AudioSource::timeline(p, 0).unwrap();
        let (tx, rx) = rtrb::RingBuffer::new(COMMAND_CAPACITY);
        let shared = Arc::new(TransportCell::default());
        let mut r = Renderer::new(rx, shared.clone(), metrics.clone(), 48000, 2);
        r.set_declick(false);
        let mut s = Self {
            r,
            source,
            tx,
            metrics,
            shared,
        };
        s.command(Action::Load {
            clip_id: 1,
            audio: s.source.clone(),
        });
        s.command(Action::Play);
        s
    }
    fn command(&mut self, action: Action) {
        self.tx
            .push(Command {
                id: 1,
                issued: Instant::now(),
                action,
            })
            .ok()
            .unwrap();
        self.r.render(&mut [] as &mut [f32]);
    }
    fn frame(&mut self) -> [f32; 2] {
        let at = self.r.processing_frame();
        if at < self.source.playback_frames || self.source.cycle.is_some() {
            let start = Instant::now();
            while self.source.pair(at, at).is_none() {
                assert!(
                    start.elapsed() < Duration::from_secs(5),
                    "reader not ready at {at}"
                );
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        self.metrics.effects.collect();
        let mut x = [0f32; 2];
        WATCH.set(true);
        self.r.render(&mut x);
        WATCH.set(false);
        x
    }
    fn render(&mut self, n: usize) -> Vec<[f32; 2]> {
        (0..n).map(|_| self.frame()).collect()
    }
}
#[test]
fn native_audio_midi_master_sample_alignment_seek_loop_bypass_reorder() {
    for format in ["vst3", "clap"] {
        let (p, a) = fixture(format);
        let output = offline(plan(&p, a.clone()), 4000);
        for (i, x) in output.iter().enumerate() {
            assert_eq!(
                *x,
                if [500, 1500, 2500].contains(&i) {
                    [0.25; 2]
                } else {
                    [0.; 2]
                },
                "{format} sample {i}"
            );
        }
        let mut live = Live::new(plan(&p, a.clone()));
        let delay = live.r.pdc_latency();
        assert_eq!(delay, 415);
        let real = live.render(4000 + delay);
        assert_eq!(&real[delay..], &output);
        assert_eq!(live.shared.read().frame, 4000.);
        assert_eq!(ALLOCS.get(), 0);
        assert_eq!(FREES.get(), 0);
        live.command(Action::Seek(1200. / 48000.));
        live.command(Action::Play);
        let seek = live.render(1800 + delay);
        assert_eq!(&seek[delay..], &output[1200..3000]);
        let mut q = p.clone();
        q.tracks[0].inserts.reverse();
        q.tracks[0].inserts[0].enabled = false;
        q.tracks[1].inserts[0].enabled = false;
        q.master.inserts[0].enabled = false;
        assert_eq!(offline(plan(&q, a.clone()), 4000), output);
        q.cycle = Some(Cycle {
            enabled: true,
            start_tick: Signed(0),
            end_tick: Signed(40000),
        });
        let mut looped = Live::new(plan(&q, a));
        let loop_delay = looped.r.pdc_latency();
        let loopout = looped.render(loop_delay + 5500);
        for (i, x) in loopout[loop_delay..].iter().enumerate() {
            assert_eq!(
                *x,
                if i % 1000 == 500 { [0.25; 2] } else { [0.; 2] },
                "{format} loop {i}"
            );
        }
        assert_eq!(looped.shared.read().cycle_pass, 5);
        println!("PDC {format}: Audio 270 / MIDI 384 / Master 31; max sample error=0; seek and 5 loops exact; callback alloc/free=0");
    }
}
fn lane(p: &mut Project, track: &str, effect: Option<String>, name: &str, lo: f64, hi: f64) {
    automation::channel_mut(p, track).lanes.push(Lane {
        lane_id: id(),
        parameter: Parameter {
            effect_id: effect,
            name: name.into(),
        },
        points: vec![
            Point {
                point_id: id(),
                tick: Signed(0),
                value: lo,
                shape: Shape::Step,
            },
            Point {
                point_id: id(),
                tick: Signed(20000),
                value: hi,
                shape: Shape::Step,
            },
        ],
    });
}
#[test]
fn automation_follows_signal_arrival_through_instrument_inserts_and_master() {
    for format in ["vst3", "clap"] {
        let (mut p, a) = fixture(format);
        let track = p.tracks[1].track_id.clone();
        let fx = p.tracks[1].inserts[0].effect_id.clone();
        lane(
            &mut p,
            &track,
            Some(fx),
            "plugin.1",
            1.,
            if format == "vst3" { 0.75 } else { 0.5 },
        );
        lane(&mut p, "master", None, "volumeDb", 0., 20. * 0.5f64.log10());
        let out = offline(plan(&p, a.clone()), 4000);
        assert_eq!(out[500], [0.125, 0.0625]);
        let mut live = Live::new(plan(&p, a));
        let d = live.r.pdc_latency();
        assert_eq!(&live.render(4000 + d)[d..], &out);
    }
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut std::ffi::c_void;
    fn GetProcAddress(module: *mut std::ffi::c_void, name: *const u8) -> *mut std::ffi::c_void;
}
fn force(format: &str, instrument: bool, latency: i32) {
    use std::os::windows::ffi::OsStrExt;
    let path = root(format).canonicalize().unwrap();
    let wide: Vec<_> = path.as_os_str().encode_wide().chain([0]).collect();
    unsafe {
        let h = GetModuleHandleW(wide.as_ptr());
        assert!(!h.is_null());
        let p = GetProcAddress(h, c"pdc_test_latency".as_ptr().cast());
        assert!(!p.is_null());
        let f: unsafe extern "C" fn(i32, i32) = std::mem::transmute(p);
        f(instrument as i32, latency);
    }
}

#[test]
fn export_selection_and_stems_preserve_native_plugin_sample_alignment() {
    use minidaw_lib::project::{
        export::{self, WaveFormat, WaveWriter},
        paths,
        session::AssetState,
    };
    use std::{fs, sync::atomic::AtomicBool};
    for format in ["vst3", "clap"] {
        let (mut p, source) = fixture(format);
        let folder = std::env::temp_dir().join(format!("minidaw-pdc-export-{}", id()));
        fs::create_dir(&folder).unwrap();
        let path = folder.join("source.wav");
        let mut wave =
            WaveWriter::new(fs::File::create(&path).unwrap(), WaveFormat::Float32, 48000).unwrap();
        let minidaw_lib::audio::source::AssetStorage::Memory(data) = &source.storage else {
            panic!()
        };
        for f in data.samples.as_chunks::<2>().0 {
            wave.frame(*f).unwrap();
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
        let before = p.clone();
        let expected = offline(plan(&p, source), 4000);
        let render = |mode, ids: Vec<String>, dest: PathBuf| {
            let r:export::Request=serde_json::from_value(json!({"jobId":id(),"revision":0,"path":paths::display(&dest),"format":"float32","sampleRate":48000,"mode":mode,"trackIds":ids,"range":{"start":"333","end":"3333"}})).unwrap();
            export::render(
                &r,
                &p,
                &states,
                None,
                Arc::new(AtomicBool::new(false)),
                |_, _, _| {},
            )
            .unwrap()
        };
        let read = |path: &str| {
            let source = AudioAsset::open(std::path::Path::new(path)).unwrap();
            let minidaw_lib::audio::source::AssetStorage::Memory(data) = &source.storage else {
                panic!()
            };
            data.samples.as_chunks::<2>().0.to_vec()
        };
        let selection = render("selection", vec![], folder.join("selection.wav"));
        assert_eq!(selection.frames, 3000);
        assert_eq!(read(&selection.path), expected[333..3333]);
        let stems = render(
            "stems",
            p.tracks.iter().map(|t| t.track_id.clone()).collect(),
            folder.clone(),
        );
        assert_eq!(stems.files.len(), 2);
        let a = read(&stems.files[0].path);
        let b = read(&stems.files[1].path);
        assert_eq!(a.len(), 3000);
        assert_eq!(b.len(), 3000);
        for i in 0..3000 {
            assert_eq!(a[i][1], 0.);
            assert_eq!(b[i][0], 0.);
            assert_eq!([a[i][0], b[i][1]], expected[333 + i], "{format} sample {i}");
        }
        let single = render(
            "track",
            vec![p.tracks[1].track_id.clone()],
            folder.join("track.wav"),
        );
        assert_eq!(read(&single.path), b);
        assert_eq!(p, before);
        // Clean only this test's known outputs and private directories.
        for f in &stems.files {
            fs::remove_file(&f.path).unwrap();
        }
        fs::remove_dir(&stems.path).unwrap();
        for path in [&selection.path, &single.path, &paths::display(&path)] {
            fs::remove_file(path).unwrap();
        }
        fs::remove_dir(folder).unwrap();
        println!("PDC_EXPORT {format}: Audio 270 / MIDI 384 / Master 31 samples; selection 333..3333 and stems aligned; max sample error=0");
    }
}
#[test]
fn plugin_runtime_latency_notification_grows_buffers_off_callback() {
    for format in ["vst3", "clap"] {
        let (mut p, a) = fixture(format);
        p.cycle = Some(Cycle {
            enabled: true,
            start_tick: Signed(0),
            end_tick: Signed(40000),
        });
        let mut live = Live::new(plan(&p, a));
        live.render(900);
        force(format, true, 6000);
        for _ in 0..30 {
            live.frame();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(live.r.pdc_latency(), 6288);
        let warm = live.render(7500);
        let tail = &warm[warm.len() - 1000..];
        let peaks: Vec<_> = tail
            .iter()
            .enumerate()
            .filter(|(_, x)| x[0] != 0. || x[1] != 0.)
            .collect();
        assert_eq!(peaks.len(), 1);
        assert_eq!(*peaks[0].1, [0.25; 2]);
        assert_eq!(ALLOCS.get(), 0);
        assert_eq!(FREES.get(), 0);
        force(format, true, -1);
        println!("PDC {format} dynamic 127 -> 6000 samples, buffer grow off callback, L/R impulse aligned");
    }
}

#[test]
fn unequal_audio_chains_cancel_and_streaming_matches_memory() {
    use minidaw_lib::audio::source::AssetStorage;
    use minidaw_lib::project::export::{WaveFormat, WaveWriter};
    for format in ["vst3", "clap"] {
        let (mut p, a) = fixture(format);
        let mut t = p.tracks[0].clone();
        t.track_id = id();
        for c in &mut t.clips {
            if let Clip::Audio(c) = c {
                c.clip_id = id();
            }
        }
        t.inserts = vec![fx(format, 43)];
        if let Processor::External { plugin } = &mut t.inserts[0].processor {
            plugin
                .parameters
                .iter_mut()
                .find(|p| p.id == 1)
                .unwrap()
                .value = if format == "vst3" { 0. } else { -1. };
        }
        p.tracks.push(t);
        let expected = offline(plan(&p, a.clone()), 4000);
        for (i, x) in expected.iter().enumerate() {
            assert_eq!(
                *x,
                if [500, 1500, 2500].contains(&i) {
                    [0., 0.25]
                } else {
                    [0.; 2]
                },
                "Audio latency cancellation sample {i}"
            );
        }
        let path = root(format).with_file_name(format!("pdc-source-{format}.wav"));
        let mut w = WaveWriter::new(
            std::fs::File::create(&path).unwrap(),
            WaveFormat::Float32,
            48000,
        )
        .unwrap();
        if let AssetStorage::Memory(data) = &a.storage {
            for pair in data.samples.as_chunks::<2>().0 {
                w.frame([pair[0], pair[1]]).unwrap();
            }
        }
        w.finish().unwrap();
        let streaming = AudioAsset::open_with_budget(&path, 0).unwrap();
        assert_eq!(offline(plan(&p, streaming), 4000), expected);
    }
}
#[test]
fn silent_and_final_sample_exports_keep_exact_length_with_master_latency() {
    for format in ["vst3", "clap"] {
        let (mut p, a) = fixture(format);
        p.tracks.retain(|t| t.kind == TrackKind::Audio);
        for impulse in [false, true] {
            let mut data = AudioData {
                info: a.info.clone(),
                samples: vec![0.; 8000],
            };
            if impulse {
                data.samples[7998] = 0.25;
                data.samples[7999] = 0.25;
            }
            let asset = AudioAsset::memory(data);
            let plan = plan(&p, asset);
            let _scope = plugins::OfflineScope::new();
            let mut out = OfflineMaster::new(plan, Arc::new(|| false)).unwrap();
            let mut samples = vec![];
            while let Some(x) = out.next_frame().unwrap() {
                samples.push(x);
                assert!(samples.len() < 5000);
            }
            assert_eq!(samples.len(), 4000);
            assert_eq!(samples[3999], if impulse { [0.25, 0.] } else { [0.; 2] });
        }
    }
}

#[test]
fn master_zero_gain_transition_is_delayed_with_audio_not_applied_early() {
    for format in ["vst3", "clap"] {
        let (mut p, a) = fixture(format);
        p.tracks.retain(|t| t.kind == TrackKind::Audio);
        let mut data = AudioData {
            info: a.info.clone(),
            samples: vec![0.; 8000],
        };
        for i in [499, 500, 501] {
            data.samples[i * 2] = 0.25;
            data.samples[i * 2 + 1] = 0.25;
        }
        lane(&mut p, "master", None, "volumeDb", 0., -96.);
        let out = offline(plan(&p, AudioAsset::memory(data)), 4000);
        assert_eq!(out[499], [0.25, 0.]);
        assert_eq!(out[500], [0.; 2]);
        assert_eq!(out[501], [0.; 2]);
    }
}
#[test]
fn live_audio_insert_and_master_latency_notifications_realign_both_directions() {
    for format in ["vst3", "clap"] {
        let (mut p, a) = fixture(format);
        p.cycle = Some(Cycle {
            enabled: true,
            start_tick: Signed(0),
            end_tick: Signed(40000),
        });
        let restored: Project = serde_json::from_slice(&serde_json::to_vec(&p).unwrap()).unwrap();
        let mut live = Live::new(plan(&restored, a));
        live.render(1200);
        for latency in [371, 7] {
            force(format, false, latency);
            for _ in 0..30 {
                live.frame();
                std::thread::sleep(Duration::from_millis(1));
            }
            assert_eq!(live.r.pdc_latency(), 127 + latency as usize * 2);
            let out = live.render(4000);
            let tail = &out[3000..];
            let peaks: Vec<_> = tail.iter().filter(|x| x[0] != 0. || x[1] != 0.).collect();
            assert_eq!(peaks.len(), 1);
            assert_eq!(*peaks[0], [0.25; 2]);
        }
        force(format, false, -1);
        println!("PDC {format} live Audio/Master latency 371 -> 7, restored state and both channels exact");
    }
}
