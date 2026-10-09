//! Objective transition measurements; not an acoustic audibility verdict.
use minidaw_lib::audio::{
    declick::{Declick, RAMP_MS},
    decoder::{AudioData, FileInfo},
    metrics::AudioMetrics,
    renderer::{Action, Command, Renderer, COMMAND_CAPACITY},
    source::{AssetStorage, AudioSource},
    transport::TransportCell,
};
use rtrb::RingBuffer;
use serde_json::{json, Value};
use std::{sync::Arc, time::Instant};

fn stats(output: &[f64], previous: f64, baseline: &[f64]) -> Value {
    let mut last = previous;
    let mut max_jump: f64 = 0.0;
    let mut difference_energy = 0.0;
    for (&sample, &reference) in output.iter().zip(baseline) {
        max_jump = max_jump.max((sample - last).abs());
        last = sample;
        difference_energy += (sample - reference).powi(2);
    }
    // A second difference highlights impulsive/high-frequency transition energy.
    let extended: Vec<_> = [previous, previous]
        .into_iter()
        .chain(output.iter().copied())
        .collect();
    let second_difference_energy: f64 = extended
        .windows(3)
        .map(|w| (w[2] - 2.0 * w[1] + w[0]).powi(2))
        .sum();
    json!({"boundaryJump":(output[0]-previous).abs(),"windowPeakJump":max_jump,"transientDifferenceEnergy":difference_energy,"secondDifferenceEnergy":second_difference_energy})
}

fn candidate(ms: f64, dc: f64, amplitude: f64, frequency: f64, phase: f64, rate: u32) -> Value {
    let signal = |frame: usize| {
        (dc + amplitude
            * (frame as f64 * std::f64::consts::TAU * frequency / rate as f64 + phase).sin())
        .clamp(-1.0, 1.0)
    };
    let mut smooth = Declick::new(rate, ms);
    smooth.set_enabled(true);
    let mut plain = Declick::new(rate, ms);
    let mut records = Vec::new();
    let mut cursor = 0;
    let mut previous = 0.0;
    let mut plain_previous = 0.0;
    let window = (rate as usize / 100).max(1);
    for (name, active, seek) in [
        ("play", true, None),
        ("pause", false, None),
        ("resume", true, None),
        ("seek", true, Some((rate as f64 * 0.3571) as usize)),
        ("stop", false, Some(0)),
        ("restart", true, Some(0)),
    ] {
        if let Some(frame) = seek {
            cursor = frame;
        }
        smooth.transition();
        plain.transition();
        let mut output = Vec::new();
        let mut baseline = Vec::new();
        for _ in 0..window {
            let input = active.then(|| [signal(cursor); 2]);
            if active {
                cursor += 1;
            }
            output.push(smooth.process(input)[0]);
            baseline.push(plain.process(input)[0]);
        }
        records.push(json!({"action":name,"on":stats(&output,previous,&baseline),"off":stats(&baseline,plain_previous,&baseline)}));
        previous = *output.last().unwrap();
        plain_previous = *baseline.last().unwrap();
    }
    json!({"rampMs":ms,"rampFrames":(rate as f64*ms/1000.0).round(),"rate":rate,"dc":dc,"amplitude":amplitude,"frequency":frequency,"phase":phase,"addedBufferedFrames":0,"transitions":records})
}

fn production(enabled: bool) -> Value {
    let rate = 48000;
    let frames = 48000;
    let samples = (0..frames)
        .flat_map(|n| {
            let v =
                (0.2 + 0.7 * (n as f64 * std::f64::consts::TAU * 997.0 / rate as f64).sin()) as f32;
            [v, -v]
        })
        .collect::<Vec<_>>();
    let original = samples.clone();
    let source = AudioSource::memory(AudioData {
        info: FileInfo {
            name: "DC+sine".into(),
            sample_rate: rate,
            channels: 2,
            frames,
            duration: 1.0,
            sanitized_samples: 0,
        },
        samples,
    });
    let (mut tx, rx) = RingBuffer::new(COMMAND_CAPACITY);
    let shared = Arc::new(TransportCell::default());
    let mut renderer = Renderer::new(
        rx,
        shared.clone(),
        Arc::new(AudioMetrics::default()),
        rate,
        2,
    );
    renderer.set_declick(enabled);
    let mut records = Vec::new();
    let mut previous = [0.0f32; 2];
    tx.push(Command {
        id: 0,
        issued: Instant::now(),
        action: Action::Load {
            clip_id: 1,
            audio: source.clone(),
        },
    })
    .ok()
    .unwrap();
    for (n, (name, action)) in [
        ("play", Action::Play),
        ("pause", Action::Pause),
        ("resume", Action::Play),
        ("seek", Action::Seek(0.3571)),
        ("stop", Action::Stop),
        ("restart", Action::Play),
    ]
    .into_iter()
    .enumerate()
    {
        let before = shared.read();
        tx.push(Command {
            id: n as u64 + 1,
            issued: Instant::now(),
            action,
        })
        .ok()
        .unwrap();
        let mut block = [0.0_f32; 960];
        renderer.render(&mut block);
        let after = shared.read();
        let start = match name {
            "play" | "restart" => Some(0),
            "resume" => Some(before.frame as usize),
            "seek" => Some((0.3571_f64 * rate as f64).ceil() as usize),
            _ => None,
        };
        for frame in 0..480 {
            let expected = start.map_or([0.0; 2], |start| {
                [
                    original[(start + frame) * 2],
                    original[(start + frame) * 2 + 1],
                ]
            });
            if !enabled || frame >= (rate as f64 * RAMP_MS / 1000.0) as usize {
                assert_eq!(
                    &block[frame * 2..frame * 2 + 2],
                    &expected,
                    "{name} frame {frame}"
                );
            }
        }
        let jump = (block[0] - previous[0]).abs();
        if enabled {
            assert_eq!(jump, 0.0, "{name}");
        }
        if name == "pause" {
            assert_eq!(after.frame, before.frame);
            assert!(block[192..].iter().all(|&s| s == 0.0));
        }
        if name == "stop" {
            assert_eq!(after.frame, 0.0);
            assert!(block[192..].iter().all(|&s| s == 0.0));
        }
        records.push(json!({"action":name,"boundaryJump":jump,"beforeFrame":before.frame,"afterFrame":after.frame,"state":after.state}));
        previous.copy_from_slice(&block[958..]);
    }
    let AssetStorage::Memory(pcm) = &source.asset.storage else {
        unreachable!()
    };
    assert_eq!(pcm.samples, original);
    json!({"enabled":enabled,"rampMs":RAMP_MS,"sourcePcmUnchanged":true,"transitions":records})
}
fn main() {
    let mut candidates = Vec::new();
    for rate in [44100, 48000, 96000] {
        for ms in [1.0, 2.0, 3.0, 5.0] {
            for (dc, amp, freq) in [
                (0.5, 0.0, 0.0),
                (0.2, 0.7, 997.0),
                (0.0, 0.98, 1000.0),
                (0.0, 0.9, 15000.0),
            ] {
                for phase in [0.0, std::f64::consts::FRAC_PI_2] {
                    candidates.push(candidate(ms, dc, amp, freq, phase, rate));
                }
            }
        }
    }
    let result = json!({"curve":"raised cosine, last-output anchor to new source (or silence)","selectedMs":RAMP_MS,"candidates":candidates,"production":[production(false),production(true)],"limitations":"Digital samples only, no acoustic capture or human listening claim. Window peak includes legitimate source slope."});
    std::fs::write(
        std::env::args().nth(1).expect("report path"),
        serde_json::to_vec_pretty(&result).unwrap(),
    )
    .unwrap();
    println!("96 candidate cases and real Renderer ON/OFF validated");
}
