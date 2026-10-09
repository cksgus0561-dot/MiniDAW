//! Offline measurement of the production oscillator and renderer; never linked
//! into the callback. The JSON includes limits and the independent analytic reference.
use minidaw_lib::{
    audio::{
        metrics::AudioMetrics,
        midi::*,
        renderer::{Action, Command, Renderer, COMMAND_CAPACITY},
        source::AudioSource,
        synth::BasicSynth,
        timeline::PlaybackPlan,
        transport::TransportCell,
    },
    project::schema::*,
};
use rustfft::{num_complex::Complex, FftPlanner};
use serde_json::json;
use std::{sync::Arc, time::Instant};
fn project(count: usize) -> Project {
    let mut p = Project::new();
    p.tracks.push(Track {
        synth: Default::default(),
        inserts: vec![],
        track_id: id(),
        name: "Synth audit".into(),
        kind: TrackKind::Midi,
        instrument: Instrument::BasicSynth,
        mix: TrackMix::default(),
        extensions: Extensions::new(),
        clips: vec![Clip::Midi(MidiClip {
            clip_id: id(),
            name: "Test".into(),
            start_tick: Signed(0),
            length_tick: Signed(1_920_000),
            content_offset_tick: Signed(0),
            controls: vec![],
            notes: (0..count)
                .map(|i| MidiNote {
                    note_id: id(),
                    start_tick: Signed(0),
                    length_tick: Signed(1_920_000),
                    pitch: 48 + (i % 48) as u8,
                    velocity: 100,
                    release_velocity: 0,
                    channel: 0,
                })
                .collect(),
        })],
    });
    p
}
fn event(pitch: u8) -> MidiEvent {
    MidiEvent {
        frame: 0,
        offset: 0,
        voice: 1,
        track: 0,
        channel: 0,
        pitch,
        on: true,
        velocity: 127,
    }
}
fn main() {
    let p = project(0);
    let n = 131072;
    let mut planner = FftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(n);
    let window: Vec<_> = (0..n)
        .map(|i| {
            let a = std::f64::consts::TAU * i as f64 / n as f64;
            0.35875 - 0.48829 * a.cos() + 0.14128 * (2.0 * a).cos() - 0.01168 * (3.0 * a).cos()
        })
        .collect();
    let norm = window.iter().sum::<f64>();
    let mut cases = vec![];
    for rate in [44100, 48000, 96000, 192000] {
        let plan = MidiPlan::compile(&p, rate).unwrap();
        let mut synth = BasicSynth::new(rate);
        for pitch in [0, 21, 48, 60, 69, 84, 96, 108, 120, 127] {
            for bend in [-8192, 0, 8191] {
                synth.configure(Some(&plan));
                synth.control(MidiControlEvent {
                    frame: 0,
                    offset: 0,
                    track: 0,
                    channel: 0,
                    control: Control::PitchBend(bend),
                });
                synth.event(event(pitch));
                let warm = (rate / 10) as usize;
                for _ in 0..warm {
                    synth.sample();
                }
                let f = 440.0
                    * 2f64.powf(
                        (f64::from(pitch) - 69.0
                            + f64::from(bend) / if bend < 0 { 8192.0 } else { 8191.0 } * 2.0)
                            / 12.0,
                    );
                let mut signal = Vec::with_capacity(n);
                let mut residual = Vec::with_capacity(n);
                let mut error = 0.0;
                let mut power = 0.0;
                for (i, w) in window.iter().enumerate() {
                    // Include the actual device f32 conversion. Deliberate 2nd/3rd
                    // harmonics are the timbre, not THD or aliases.
                    let value = synth.sample()[0] as f32 as f64;
                    signal.push(value);
                    let mut ideal = 0.0;
                    for (h, amp) in [1.0, 0.25, 0.125].iter().enumerate() {
                        let hf = f * (h + 1) as f64;
                        let t = ((0.48 * rate as f64 - hf) / (0.08 * rate as f64)).clamp(0.0, 1.0);
                        ideal += amp
                            * t
                            * t
                            * (3.0 - 2.0 * t)
                            * (std::f64::consts::TAU * hf * (warm + i) as f64 / rate as f64).sin()
                            * 0.16
                            / 1.375;
                    }
                    let difference = value - ideal;
                    error += difference * difference;
                    power += value * value;
                    residual.push(Complex::new(difference * w, 0.0));
                }
                fft.process(&mut residual);
                let peak = residual[1..n / 2]
                    .iter()
                    .map(|c| c.norm() * 2.0 / norm)
                    .fold(0f64, f64::max);
                let crossings: Vec<_> = signal
                    .windows(2)
                    .enumerate()
                    .filter(|(_, v)| v[0] <= 0.0 && v[1] > 0.0)
                    .map(|(i, v)| i as f64 - v[0] / (v[1] - v[0]))
                    .collect();
                let measured = (crossings.len() - 1) as f64 * rate as f64
                    / (crossings.last().unwrap() - crossings[0]);
                let db = 10.0 * (error / power).log10();
                let cents = 1200.0 * (measured / f).log2();
                assert!(
                    db < -100.0 && cents.abs() < 0.5,
                    "{rate}/{pitch}/{bend}: residual {db}, cents {cents}"
                );
                cases.push(json!({"rate":rate,"pitch":pitch,"bend":bend,"expectedHz":f,"measuredHz":measured,"pitchErrorCents":cents,
                "rmsDbfs":10.0*(power/n as f64).log10(),"residualDbRelative":db,"largestResidualSpectralPeakDbfs":20.0*peak.log10()}));
            }
        }
    }
    // Real production renderer (audio + MIDI + Master conversion + metrics),
    // small varied voice counts; no long device or streaming stress benchmark.
    let mut performance = vec![];
    for rate in [44100, 48000, 96000] {
        for count in [0, 1, 16, 64, 256] {
            let p = project(count);
            let plan = PlaybackPlan::compile(Arc::new(p), Default::default(), rate).unwrap();
            let source = AudioSource::timeline(plan, 0).unwrap();
            let (mut tx, rx) = rtrb::RingBuffer::new(COMMAND_CAPACITY);
            let metrics = Arc::new(AudioMetrics::default());
            let mut renderer = Renderer::new(
                rx,
                Arc::new(TransportCell::default()),
                metrics.clone(),
                rate,
                2,
            );
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
            let mut out = [0f32; 256];
            let mut times = vec![];
            let mut peak = 0f32;
            for _ in 0..256 {
                let start = Instant::now();
                renderer.render(&mut out);
                times.push(start.elapsed().as_secs_f64() * 1000.0);
                for v in out {
                    peak = peak.max(v.abs());
                }
            }
            times.sort_by(f64::total_cmp);
            let budget = 128000.0 / rate as f64;
            performance.push(json!({"rate":rate,"voices":count,"bufferFrames":128,"budgetMs":budget,"medianMs":times[128],"p99Ms":times[253],"maxMs":times[255],"maxPeak":peak,"starvation":source.snapshot().starvation,"metrics":metrics.snapshot()}));
            assert!(times[128] < budget && source.snapshot().starvation == 0);
        }
    }
    let report = json!({"passed":true,"method":"131072 samples, 4-term Blackman-Harris residual FFT against analytic band-limited 3-partial reference, f32 output. Residual includes interpolation, quantization, unwanted distortion/aliases; intended harmonics excluded. Positive zero-crossing frequency estimate.","cases":cases,"renderer":performance});
    let path = std::env::args()
        .nth(1)
        .unwrap_or("../docs/validation/synth-quality.json".into());
    std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    println!("120 tone/bend/rate cases; 15 production-renderer scenarios passed.");
}
