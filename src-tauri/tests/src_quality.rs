use minidaw_lib::audio::{
    decoder::{AudioData, FileInfo},
    reader::Cancel,
    resample::{output_frames, PlaybackReader},
    source::{AudioAsset, AudioSource},
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
fn asset(rate: u32, samples: Vec<f32>) -> Arc<AudioAsset> {
    AudioAsset::memory(AudioData {
        info: FileInfo {
            name: "SRC test".into(),
            sample_rate: rate,
            channels: 2,
            frames: samples.len() / 2,
            duration: samples.len() as f64 / 2. / rate as f64,
            sanitized_samples: 0,
        },
        samples,
    })
}
fn convert(a: Arc<AudioAsset>, rate: u32) -> Vec<[f32; 2]> {
    let n = output_frames(a.info.frames, a.info.sample_rate, rate);
    let mut r = PlaybackReader::new(a, rate).unwrap();
    r.seek(0, Arc::new(|| false)).unwrap();
    (0..n).map(|_| r.read_frame().unwrap()).collect()
}
fn amp(s: &[[f32; 2]], rate: u32, hz: f64) -> f64 {
    let (mut re, mut im) = (0., 0.);
    let a = rate as usize / 10;
    let b = rate as usize * 9 / 10;
    for (i, f) in s.iter().enumerate().take(b).skip(a) {
        let p = std::f64::consts::TAU * hz * i as f64 / rate as f64;
        re += f[0] as f64 * p.cos();
        im += f[0] as f64 * p.sin();
    }
    2. * re.hypot(im) / (b - a) as f64
}
#[test]
fn passband_alias_image_silence_dc_and_channel_isolation() {
    // Criteria chosen before integration: <=0.01 dB through 21 kHz,
    // alias/image <=-100 dB relative to a 0.5 input, no nonfinite output.
    for (ir, or) in [(44100, 48000), (48000, 44100)] {
        for hz in [1000., 5000., 10000., 15000., 18000., 21000., 23000.] {
            if hz >= ir as f64 / 2. {
                continue;
            }
            let input = (0..ir)
                .flat_map(|i| {
                    [
                        0.5 * (std::f64::consts::TAU * hz * i as f64 / ir as f64).sin() as f32,
                        0.,
                    ]
                })
                .collect();
            let s = convert(asset(ir, input), or);
            assert!(s.iter().all(|f| f[0].is_finite() && f[1] == 0.));
            if hz < or as f64 / 2. {
                assert!(
                    (20. * (amp(&s, or, hz) / 0.5).log10()).abs() < 0.01,
                    "{ir} {hz}"
                );
            } else {
                assert!(amp(&s, or, or as f64 - hz) < 0.5e-5);
            }
            let image = ir as f64 - hz;
            if image < or as f64 / 2. {
                assert!(amp(&s, or, image) < 0.5e-5);
            }
        }
        assert!(convert(asset(ir, vec![0.; ir as usize * 2]), or)
            .iter()
            .all(|f| *f == [0., 0.]));
        let dc = convert(asset(ir, vec![0.25; ir as usize * 2]), or);
        assert!(dc[or as usize / 10..or as usize * 9 / 10]
            .iter()
            .all(|f| (f[0] - 0.25).abs() < 1e-6 && f[0] == f[1]));
    }
}
#[test]
fn impulse_alignment_eos_short_clips_and_finite_headroom() {
    for (ir, or) in [(44100, 48000), (48000, 44100)] {
        for n in [1, 17, 589, ir as usize + 17] {
            for at in [0, n / 2, n - 1] {
                let mut input = vec![0.; n * 2];
                input[at * 2] = 0.5;
                let s = convert(asset(ir, input), or);
                assert_eq!(s.len(), output_frames(n, ir, or));
                let peak = s
                    .iter()
                    .enumerate()
                    .max_by(|a, b| a.1[0].abs().total_cmp(&b.1[0].abs()))
                    .unwrap()
                    .0;
                let expected = at as f64 * or as f64 / ir as f64;
                assert!(
                    (peak as f64 - expected).abs() <= 1.,
                    "impulse shifted {ir}/{or} n={n} at={at} peak={peak}"
                );
                assert!(s.iter().any(|f| f[0].abs() > 0.1), "EOS lost impulse");
            }
        }
        let s = convert(asset(ir, vec![f32::MAX, -f32::MAX, 1.5, -1.5]), or);
        assert!(s.iter().all(|f| f.iter().all(|x| x.is_finite())));
    }
}
#[test]
fn memory_streaming_exact_after_src_refill_wrap_and_seeks() {
    for (name, or) in [("stereo-44100.wav", 48000), ("모노-48000.WAV", 44100)] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../tests/fixtures")
            .join(name);
        let memory = AudioAsset::open_with_budget(&path, usize::MAX).unwrap();
        let file = AudioAsset::open_with_budget(&path, 0).unwrap();
        let reference = convert(memory.clone(), or);
        for a in [memory, file] {
            let src = AudioSource::for_output(a, or).unwrap();
            let check = |start: usize, count: usize| {
                let until = Instant::now() + Duration::from_secs(10);
                for (i, expected) in reference.iter().enumerate().skip(start).take(count) {
                    let frame = loop {
                        if let Some(p) = src.pair(i, i) {
                            break p[0];
                        }
                        assert!(Instant::now() < until, "SRC buffer timeout");
                        std::thread::sleep(Duration::from_micros(100));
                    };
                    assert_eq!(
                        frame.map(f32::to_bits),
                        expected.map(f32::to_bits),
                        "{name} output frame {i}"
                    );
                    if i % 512 == 511 {
                        src.consumed(i + 1);
                    }
                }
            };
            check(0, reference.len());
            for fraction in [0.5, 0.9, 0.1, 0., 0.999] {
                let pos = (reference.len() as f64 * fraction) as usize;
                src.seek(pos);
                check(pos, 4096);
            }
            for i in 0..1000 {
                src.seek(i % reference.len());
            }
            let pos = reference.len() / 3;
            src.seek(pos);
            check(pos, 4096);
            assert!(src.snapshot().error.is_none());
        }
    }
}
#[test]
fn reset_preserves_absolute_phase_and_cancels_stale_preroll() {
    for (ir, or) in [(44100, 48000), (48000, 44100)] {
        let samples = (0..ir * 2)
            .flat_map(|i| {
                [
                    (i as f64 * 0.123).sin() as f32 * 0.5,
                    (i as f64 * 0.317).cos() as f32 * 0.4,
                ]
            })
            .collect();
        let a = asset(ir, samples);
        let seq = convert(a.clone(), or);
        let mut r = PlaybackReader::new(a, or).unwrap();
        for target in [
            0,
            1,
            13,
            588,
            640,
            seq.len() / 2,
            seq.len() * 9 / 10,
            seq.len() / 10,
            seq.len() - 1,
        ] {
            r.seek(target, Arc::new(|| false)).unwrap();
            for expected in seq.iter().skip(target).take(4096) {
                assert_eq!(
                    r.read_frame().unwrap().map(f32::to_bits),
                    expected.map(f32::to_bits)
                );
            }
        }
        let cancel: Cancel = Arc::new(|| true);
        r.seek(0, cancel).unwrap();
        assert!(r.read_frame().is_err());
    }
}

#[test]
fn renderer_pause_resume_seek_and_eof_match_the_resampled_timeline() {
    use minidaw_lib::audio::{
        metrics::AudioMetrics,
        renderer::{Action, Command, Renderer, COMMAND_CAPACITY},
        transport::{PlayState, TransportCell},
    };
    for (ir, or) in [(44100, 48000), (48000, 44100)] {
        let a = asset(
            ir,
            (0..ir)
                .flat_map(|i| {
                    [
                        0.2 + 0.4 * (i as f64 * 0.071).sin() as f32,
                        0.3 * (i as f64 * 0.213).cos() as f32,
                    ]
                })
                .collect(),
        );
        let expected = convert(a.clone(), or);
        let owner = AudioSource::for_output(a, or).unwrap();
        let (mut tx, rx) = rtrb::RingBuffer::new(COMMAND_CAPACITY);
        let cell = Arc::new(TransportCell::default());
        let metrics = Arc::new(AudioMetrics::default());
        let mut renderer = Renderer::new(rx, cell.clone(), metrics.clone(), or, 2);
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
            audio: owner.clone(),
        });
        send(Action::Play);
        let check = |renderer: &mut Renderer, start: usize| {
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                let before = metrics.snapshot().rendered_frames;
                let mut out = [0_f32; 960];
                renderer.render(&mut out);
                let n = (metrics.snapshot().rendered_frames - before) as usize;
                if n > 0 {
                    for (actual, reference) in out
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .take(n)
                        .zip(&expected[start..])
                    {
                        assert_eq!(
                            actual.map(f32::to_bits),
                            reference.map(|v| v.clamp(-1., 1.).to_bits())
                        );
                    }
                    return start + n;
                }
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(1));
            }
        };
        let pos = check(&mut renderer, 0);
        send(Action::Pause);
        let mut out = [1_f32; 960];
        renderer.render(&mut out);
        assert!(out.iter().all(|x| *x == 0.));
        let paused = cell.read().frame;
        renderer.render(&mut out);
        assert_eq!(cell.read().frame, paused);
        send(Action::Play);
        check(&mut renderer, pos);
        for seconds in [0.5, 0.9, 0.1, 0., 0.999] {
            send(Action::Seek(0.8));
            send(Action::Seek(seconds));
            let target = (seconds * or as f64).ceil() as usize;
            let next = check(&mut renderer, target);
            assert!(
                (cell.read().frame - (next as f64 * ir as f64 / or as f64).min(ir as f64)).abs()
                    < 1e-8
            );
        }
        assert_eq!(cell.read().state, PlayState::Stopped);
        assert_eq!(cell.read().frame, ir as f64);
        send(Action::Stop);
        renderer.render(&mut out);
        assert_eq!(cell.read().frame, 0.);
        send(Action::Play);
        check(&mut renderer, 0);
    }
}

#[test]
fn invalid_or_extreme_rate_does_not_allocate_an_unbounded_fft() {
    assert!(PlaybackReader::new(asset(44100, vec![0.; 2]), 0).is_err());
    assert!(PlaybackReader::new(asset(u32::MAX, vec![0.; 2]), 48000).is_err());
}

#[test]
fn odd_rational_block_ratios_have_no_residual_half_sample_delay() {
    let (ir, or) = (12000, 44100);
    let samples = (0..ir)
        .flat_map(|i| {
            let x = (std::f64::consts::TAU * 1000. * i as f64 / ir as f64).sin() as f32 * 0.5;
            [x, x]
        })
        .collect();
    let output = convert(asset(ir, samples), or);
    let max = output
        .iter()
        .enumerate()
        .take(or as usize * 9 / 10)
        .skip(or as usize / 10)
        .map(|(i, f)| {
            (f[0] as f64 - 0.5 * (std::f64::consts::TAU * 1000. * i as f64 / or as f64).sin()).abs()
        })
        .fold(0., f64::max);
    assert!(max < 1e-6, "residual fractional delay: {max}");
}
