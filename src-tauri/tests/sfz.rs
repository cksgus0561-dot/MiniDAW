use minidaw_lib::{
    audio::{
        midi::{Control, MidiControlEvent, MidiEvent, MidiPlan, MidiSink},
        sfz,
        synth::BasicSynth,
    },
    project::{
        edit::{self, Clipboard, EditRequest},
        schema::*,
    },
};
use serde_json::{json, Value};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    path::{Path, PathBuf},
};

// Same per-thread allocation/free tracking as tests/realtime.rs.
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

const RATE: u32 = 48000;

fn wav(path: &Path, rate: u32, frames: usize, value: impl Fn(usize) -> f64) {
    let mut b = Vec::new();
    let data = (frames * 2) as u32;
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    for i in 0..frames {
        b.extend_from_slice(&((value(i) * 32767.0).round() as i16).to_le_bytes());
    }
    std::fs::write(path, b).unwrap();
}
fn dc(level: f64) -> impl Fn(usize) -> f64 {
    move |_| level
}

/// A small instrument exercising the opcodes real libraries use.
fn fixture(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("minidaw-sfz-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("smp dir")).unwrap();
    for (f, level) in [("soft", 0.2), ("loud", 0.8), ("rel", 0.5), ("down", 0.3), ("up", 0.4), ("rr1", 0.1), ("rr2", 0.6), ("ks", 0.7)] {
        wav(&dir.join(format!("smp dir/{f}.wav")), 44100, 44100, dc(level));
    }
    std::fs::write(
        dir.join("inc.sfz"),
        "<region> sample=$DIR/rr1.wav key=72 seq_length=2 seq_position=1\n<region> sample=$DIR/rr2.wav key=72 seq_length=2 seq_position=2\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("piano.sfz"),
        r#"// comment
/* block
   comment */
#define $DIR smp dir
<control> set_cc7=127
<global> ampeg_release=0.01
<group> lokey=c4 hikey=d4 pitch_keycenter=60
<region> sample=$DIR/soft.wav lovel=1 hivel=64
<region>sample=$DIR/loud.wav lovel=65 hivel=127 amp_veltrack=0
<group> trigger=release rt_decay=20 volume=-6
<region> sample=$DIR/rel.wav lokey=60 hikey=60
<group> group=1 lokey=-1 hikey=-1 on_locc64=64 on_hicc64=127 off_by=2
<region> sample=$DIR/down.wav
<group> group=2 lokey=-1 hikey=-1 on_locc64=0 on_hicc64=63
<region> sample=$DIR/up.wav
<group> lokey=-1 hikey=-1
#include "inc.sfz"
<group> sw_lokey=24 sw_hikey=25 sw_default=24
<region> sample=$DIR/ks.wav key=80 sw_last=25
<region> sample=$DIR/missing.wav key=90 fil_type=lpf_2p cutoff=500
"#,
    )
    .unwrap();
    dir.join("piano.sfz")
}

fn on(pitch: u8, velocity: u8, voice: u32) -> MidiEvent {
    MidiEvent { frame: 0, offset: 0, voice, track: 0, pitch, on: true, velocity, channel: 0 }
}
fn off(pitch: u8, voice: u32) -> MidiEvent {
    MidiEvent { on: false, velocity: 0, ..on(pitch, 1, voice) }
}
fn cc(n: u8, v: u8) -> MidiControlEvent {
    MidiControlEvent { frame: 0, offset: 0, track: 0, channel: 0, control: Control::Cc(n, v) }
}
fn attempt(p: &Project, r: Value) -> minidaw_lib::error::AppResult<Project> {
    edit::apply(p, &serde_json::from_value::<EditRequest>(r).unwrap(), &mut Clipboard::default())
}
fn project(path: &Path) -> Project {
    let mut p = attempt(&Project::new(), json!({"command":"midi.track.add"})).unwrap();
    let t = p.tracks[0].track_id.clone();
    p = attempt(&p, json!({"command":"midi.track.instrument","trackIds":[t],"instrument":"sfz","path":path.to_string_lossy()})).unwrap();
    p
}
fn synth(path: &Path) -> (BasicSynth, MidiPlan) {
    let plan = MidiPlan::compile(&project(path), RATE).unwrap();
    let mut s = BasicSynth::new(RATE);
    s.configure(Some(&plan));
    (s, plan)
}
fn run(s: &mut BasicSynth, n: usize) -> f64 {
    let mut peak: f64 = 0.0;
    for _ in 0..n {
        peak = peak.max(s.sample()[0].abs());
    }
    peak
}
fn level(s: &mut BasicSynth) -> f64 {
    run(s, 2000);
    s.sample()[0]
}

#[test]
fn parses_preprocessor_scopes_and_reports_what_it_cannot_play() {
    let path = fixture("parse");
    let b = sfz::load(&path).unwrap();
    // soft, loud, rel, down, up, rr1, rr2, ks (missing.wav skipped)
    assert_eq!(b.regions.len(), 8);
    assert_eq!(b.samples.len(), 8);
    assert_eq!(b.missing, vec!["smp dir/missing.wav".to_string()]);
    assert!(b.unsupported.iter().any(|(k, _)| k == "fil_type"));
    assert!(b.unsupported.iter().any(|(k, _)| k == "cutoff"));
    assert_eq!(b.sw, Some((24, 25)));
    assert_eq!(b.sw_default, Some(24));
    let loud = &b.regions[1];
    assert_eq!((loud.lokey, loud.hikey, loud.lovel, loud.keycenter), (60, 62, 65, 60.0));
    assert!((loud.release - 0.01).abs() < 1e-9, "global inherited");
    let rel = &b.regions[2];
    assert_eq!(rel.trigger, sfz::Trigger::Release);
    assert!((rel.gain - 10f64.powf(-6.0 / 20.0)).abs() < 1e-9);
    assert_eq!(b.regions[3].on_cc, Some((64, 64, 127)));
    assert_eq!(b.regions[3].off_by, Some(2));
    // 16-bit WAV stays 16-bit in memory: 8 files * 44100 frames * 2 bytes.
    assert_eq!(b.bytes(), 8 * 44100 * 2);
    assert!(sfz::load(&path.with_extension("txt")).is_err());
    assert!(sfz::load(&path.with_file_name("nope.sfz")).is_err());
}

#[test]
fn plays_velocity_layers_releases_and_pedal_noise() {
    let path = fixture("play");
    let (mut s, _) = synth(&path);
    assert!(!s.active());
    // Soft layer: veltrack 100% -> (64/127)^2 * 0.2
    s.event(on(60, 64, 1));
    let soft = level(&mut s);
    assert!((soft - 0.2 * (64f64 / 127.0).powi(2)).abs() < 2e-3, "{soft}");
    s.event(off(60, 1));
    // Release trigger: rel.wav 0.5, -6 dB, velocity 64 of the Note On, and
    // rt_decay 20 dB/s for the ~2001 samples the key was held.
    let after = level(&mut s);
    let expected = 0.5 * 10f64.powf(-6.0 / 20.0) * (64f64 / 127.0).powi(2) * 10f64.powf(-20.0 * 2001.0 / 48000.0 / 20.0);
    assert!((after - expected).abs() < expected * 0.03, "{after} vs {expected}");
    run(&mut s, 48000 * 2);
    assert!(!s.active(), "all voices end");
    // Loud layer ignores velocity (amp_veltrack=0), key 62 transposes by 2 semitones.
    s.event(on(62, 100, 2));
    let loud = level(&mut s);
    assert!((loud - 0.8).abs() < 2e-3, "{loud}");
    // Pedal: Note Off is held until the pedal lifts; pedal down/up noises fire.
    s.control(cc(64, 127));
    s.event(off(62, 2));
    assert!(level(&mut s) > 0.8 + 0.25, "held note + pedal-down noise");
    s.control(cc(64, 0));
    // Only the pedal-up noise remains: the note released, off_by=2 choked the down noise.
    let up = level(&mut s);
    assert!((up - 0.4).abs() < 2e-3, "{up}");
    run(&mut s, 48000 * 2);
    assert!(!s.active());
}

#[test]
fn round_robin_and_keyswitches_select_regions() {
    let path = fixture("select");
    let (mut s, _) = synth(&path);
    let mut levels = vec![];
    for v in 0..4 {
        s.event(on(72, 127, 10 + v));
        levels.push(level(&mut s));
        s.event(off(72, 10 + v));
        run(&mut s, 48000);
    }
    assert!((levels[0] - 0.1).abs() < 2e-3 && (levels[1] - 0.6).abs() < 2e-3, "{levels:?}");
    assert!((levels[2] - 0.1).abs() < 2e-3 && (levels[3] - 0.6).abs() < 2e-3, "{levels:?}");
    // sw_last=25 region is silent until key 25 is pressed (default switch is 24).
    s.event(on(80, 127, 20));
    assert_eq!(level(&mut s), 0.0);
    s.event(off(80, 20));
    s.event(on(25, 127, 21));
    s.event(off(25, 21));
    s.event(on(80, 127, 22));
    assert!((level(&mut s) - 0.7).abs() < 2e-3);
}

#[test]
fn document_keeps_path_and_missing_files_stay_silent() {
    let path = fixture("doc");
    let p = project(&path);
    assert_eq!(p.tracks[0].instrument, Instrument::Sfz);
    assert_eq!(p.tracks[0].sfz.as_deref(), Some(path.to_string_lossy().as_ref()));
    let json = serde_json::to_value(&p).unwrap();
    assert_eq!(json["tracks"][0]["instrument"], "sfz");
    let back: Project = serde_json::from_value(json).unwrap();
    back.validate().unwrap();
    // Switching away keeps the path for switching back.
    let t = p.tracks[0].track_id.clone();
    let q = attempt(&p, json!({"command":"midi.track.instrument","trackIds":[t],"instrument":"basicSynth"})).unwrap();
    assert_eq!(q.tracks[0].sfz, p.tracks[0].sfz);
    // A missing file: the plan compiles, the Track is silent, the status explains.
    let gone = path.with_file_name("gone.sfz");
    let r = attempt(&p, json!({"command":"midi.track.instrument","trackIds":[t],"instrument":"sfz","path":gone.to_string_lossy()})).unwrap();
    let plan = MidiPlan::compile(&r, RATE).unwrap();
    assert!(plan.samplers[0].is_none());
    let st = sfz::status(&gone.to_string_lossy());
    assert!(!st.loaded && st.error.is_some());
    // Invalid documents are refused.
    assert!(attempt(&p, json!({"command":"midi.track.instrument","trackIds":[t],"instrument":"sfz","path":"C:/x.wav"})).is_err());
    let mut bad = p.clone();
    bad.tracks[0].sfz = None;
    assert!(bad.validate().is_err());
}

#[test]
fn callback_work_never_allocates_or_frees() {
    let path = fixture("rt");
    let (mut s, plan) = synth(&path);
    let other = MidiPlan::compile(&project(&path), RATE).unwrap();
    TRACK.with(|t| t.set(true));
    for round in 0..3u32 {
        s.configure(Some(if round % 2 == 0 { &plan } else { &other }));
        for k in 0..300u32 {
            // More notes than voices: stealing, chords, pedal, bend, CC triggers.
            s.event(on(60 + (k % 3) as u8, 1 + (k % 127) as u8, k + 1));
            s.control(cc(64, if k % 7 == 0 { 127 } else { 0 }));
            s.control(MidiControlEvent { control: Control::PitchBend((k as i16 * 97) % 8000), ..cc(0, 0) });
            run(&mut s, 64);
            s.event(off(60 + (k % 3) as u8, k + 1));
        }
        s.control(cc(123, 0));
        run(&mut s, 4800);
        s.reset();
    }
    s.configure(None);
    run(&mut s, 64);
    TRACK.with(|t| t.set(false));
    assert_eq!((ALLOCS.with(Cell::get), FREES.with(Cell::get)), (0, 0));
    drop((s, plan, other));
}

/// The real library from the user's machine, when present.
#[test]
fn salamander_grand_piano_when_installed() {
    let path = Path::new(r"D:\Salamander Grand Piano V3\SalamanderGrandPianoV3_44.1khz16bit\SalamanderGrandPianoV3.sfz");
    if !path.exists() {
        return;
    }
    let started = std::time::Instant::now();
    let b = sfz::load(path).unwrap();
    eprintln!(
        "Salamander: {} regions, {} samples, {} MB, {:?}, unsupported {:?}, missing {}",
        b.regions.len(),
        b.samples.len(),
        b.bytes() >> 20,
        started.elapsed(),
        b.unsupported,
        b.missing.len()
    );
    assert!(b.regions.len() > 600 && b.missing.is_empty() && b.unsupported.is_empty());

    // Offline Master (the Export path): C4 (SFZ c4 = 60) for one second.
    let mut p = project(path);
    let t = p.tracks[0].track_id.clone();
    p = attempt(&p, json!({"command":"midi.clip.add","trackIds":[t],"targetTick":"0","lengthTick":"3840000"})).unwrap();
    let c = p.midi_clips().next().unwrap().clip_id.clone();
    p = attempt(&p, json!({"command":"midi.note.step","clipIds":[c],"targetTick":"0","lengthTick":"1920000","pitches":[60],"velocities":[100]})).unwrap();
    let plan = minidaw_lib::audio::timeline::PlaybackPlan::compile(std::sync::Arc::new(p), Default::default(), RATE).unwrap();
    let mut m = minidaw_lib::audio::offline::OfflineMaster::new(plan, std::sync::Arc::new(|| false)).unwrap();
    let mut out = vec![];
    while let Some(f) = m.next_frame().unwrap() {
        out.push(f[0]);
    }
    let peak = out.iter().fold(0f32, |a, x| a.max(x.abs()));
    // Fundamental by autocorrelation over 0.2..0.7 s (after the hammer attack):
    // the first lag reaching 90% of the best correlation in the 100..1000 Hz range.
    let seg = &out[RATE as usize / 5..RATE as usize * 7 / 10];
    let corr = |lag: usize| -> f64 { seg.iter().zip(&seg[lag..]).map(|(a, b)| f64::from(*a) * f64::from(*b)).sum() };
    let lags: Vec<(usize, f64)> = (RATE as usize / 1000..RATE as usize / 100).map(|l| (l, corr(l))).collect();
    let best = lags.iter().map(|x| x.1).fold(f64::MIN, f64::max);
    let first = lags.iter().position(|x| x.1 >= best * 0.9).unwrap();
    // Peak of that lobe, refined by parabolic interpolation.
    let i = (first..lags.len().min(first + 20)).max_by(|a, b| lags[*a].1.total_cmp(&lags[*b].1)).unwrap();
    let (y0, y1, y2) = (lags[i - 1].1, lags[i].1, lags[i + 1].1);
    let lag = lags[i].0 as f64 + 0.5 * (y0 - y2) / (y0 - 2.0 * y1 + y2);
    let hz = f64::from(RATE) / lag;
    eprintln!("Salamander C4 offline: {} frames, peak {peak:.3}, ~{hz:.1} Hz", out.len());
    assert!(peak > 0.05 && peak < 1.0, "{peak}");
    assert!((hz - 261.63).abs() < 1.5, "{hz}");
    // Release tail continues after the 1 s note, then ends.
    assert!(out.len() > RATE as usize * 3 / 2);
}
