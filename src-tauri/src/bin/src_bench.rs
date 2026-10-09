//! Offline candidate comparison; no decoder, device, normalization or fades.
use rubato::{
    audioadapter_buffers::direct::InterleavedSlice, Async, Fft, FixedAsync, FixedSync, Resampler,
    SincInterpolationParameters, WindowFunction,
};
use serde_json::{json, Value};
use std::{fs, time::Instant};

fn amplitude(s: &[f32], rate: usize, hz: f64) -> f64 {
    let (mut re, mut im) = (0.0, 0.0);
    let a = rate / 10;
    let b = rate * 9 / 10;
    for i in a..b {
        let p = std::f64::consts::TAU * hz * i as f64 / rate as f64;
        re += s[2 * i] as f64 * p.cos();
        im += s[2 * i] as f64 * p.sin();
    }
    2.0 * re.hypot(im) / (b - a) as f64
}
fn write(path: &str, s: &[f32]) {
    fs::write(
        path,
        s.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<_>>(),
    )
    .unwrap();
}
fn main() {
    let folder = "tests/generated/src-upgrade";
    fs::create_dir_all(folder).unwrap();
    let mut rows = Vec::<Value>::new();
    for (ir, or) in [(44100, 48000), (48000, 44100)] {
        for (name, freqs) in [
            ("silence", vec![]),
            ("dc", vec![]),
            ("impulse", vec![]),
            ("1k", vec![1000.]),
            ("5k", vec![5000.]),
            ("10k", vec![10000.]),
            ("15k", vec![15000.]),
            ("18k", vec![18000.]),
            ("20k", vec![20000.]),
            ("20k5", vec![20500.]),
            ("nearNyquist", vec![21000.]),
            ("21k5", vec![21500.]),
            ("21k75", vec![21750.]),
            ("edge", vec![22000.]),
            ("22k5", vec![22500.]),
            ("23k5", vec![23500.]),
            ("23k9", vec![23900.]),
            ("alias", vec![if ir == 48000 { 23000. } else { 21500. }]),
            ("multitone", vec![1000., 5000., 10000., 15000., 18000.]),
        ] {
            if freqs.iter().any(|&f| f >= ir as f64 / 2.0) {
                continue;
            }
            let input: Vec<f32> = (0..ir)
                .flat_map(|i| {
                    let x = match name {
                        "dc" => 0.25,
                        "impulse" => {
                            if i == ir / 5 {
                                0.5
                            } else {
                                0.
                            }
                        }
                        _ => freqs
                            .iter()
                            .map(|f| {
                                0.5 / freqs.len() as f64
                                    * (std::f64::consts::TAU * f * i as f64 / ir as f64).sin()
                            })
                            .sum(),
                    } as f32;
                    [x, x]
                })
                .collect();
            write(&format!("{folder}/{ir}-{or}-{name}-input.f32"), &input);
            for candidate in [
                "linear", "fft256", "fft512", "fft1024", "sinc256", "sinc512",
            ] {
                let construct = Instant::now();
                let mut dsp: Option<Box<dyn Resampler<f64>>> = match candidate {
                    "linear" => None,
                    x if x.starts_with("fft") => Some(Box::new(
                        Fft::new_custom(
                            ir,
                            or,
                            x[3..].parse().unwrap(),
                            1,
                            2,
                            WindowFunction::BlackmanHarris2,
                            FixedSync::Both,
                        )
                        .unwrap(),
                    )),
                    x => Some(Box::new(
                        Async::new_sinc(
                            or as f64 / ir as f64,
                            1.0,
                            &SincInterpolationParameters::new(
                                x[4..].parse().unwrap(),
                                WindowFunction::BlackmanHarris2,
                            ),
                            512,
                            2,
                            FixedAsync::Input,
                        )
                        .unwrap(),
                    )),
                };
                let construct_ms = construct.elapsed().as_secs_f64() * 1000.;
                let (mut output, mut times) = (Vec::with_capacity(or * 2), Vec::<f64>::new());
                let (mut delay, mut block_in, mut block_out) = (0, 0, 0);
                let mut reset_prime_us = Vec::new();
                let start = Instant::now();
                if let Some(dsp) = &mut dsp {
                    delay = dsp.output_delay();
                    block_in = dsp.input_frames_max();
                    block_out = dsp.output_frames_max();
                    let mut a = vec![0.0; block_in * 2];
                    let mut b = vec![0.0; block_out * 2];
                    let (mut pos, mut produced) = (0, 0);
                    while output.len() < or * 2 {
                        let n = dsp.input_frames_next();
                        a.fill(0.0);
                        for i in 0..n.min(ir.saturating_sub(pos)) {
                            a[i * 2] = input[(pos + i) * 2] as f64;
                            a[i * 2 + 1] = input[(pos + i) * 2 + 1] as f64;
                        }
                        let ia = InterleavedSlice::new(&a, 2, block_in).unwrap();
                        let mut oa = InterleavedSlice::new_mut(&mut b, 2, block_out).unwrap();
                        let t = Instant::now();
                        let (_, count) = dsp.process_into_buffer(&ia, &mut oa, None).unwrap();
                        times.push(t.elapsed().as_secs_f64() * 1e6);
                        for i in 0..count {
                            if produced + i >= delay && output.len() < or * 2 {
                                output.extend([b[i * 2] as f32, b[i * 2 + 1] as f32]);
                            }
                        }
                        pos += n;
                        produced += count;
                    }
                    // Processing only: reset and prime enough to supply a 480-frame
                    // device buffer. Input decoding / scheduling is excluded.
                    for _ in 0..32 {
                        let ia = InterleavedSlice::new(&a, 2, block_in).unwrap();
                        let mut oa = InterleavedSlice::new_mut(&mut b, 2, block_out).unwrap();
                        let t = Instant::now();
                        dsp.reset();
                        let mut count = 0;
                        while count < delay + 480 {
                            count += dsp.process_into_buffer(&ia, &mut oa, None).unwrap().1;
                        }
                        reset_prime_us.push(t.elapsed().as_secs_f64() * 1e6);
                    }
                } else {
                    // Same two-point f64 arithmetic and accumulated phase as old Renderer.
                    let mut pos = 0.0_f64;
                    for _ in 0..or {
                        let i = (pos as usize).min(ir - 1);
                        let j = (i + 1).min(ir - 1);
                        let a = input[2 * i] as f64;
                        let b = input[2 * j] as f64;
                        let x = (a + (b - a) * (pos - i as f64)) as f32;
                        output.extend([x, x]);
                        pos += ir as f64 / or as f64;
                    }
                }
                let wall_ms = start.elapsed().as_secs_f64() * 1000.;
                // Keep reset/priming trials separately from full-clip cost.
                let wall_ms = wall_ms - reset_prime_us.iter().sum::<f64>() / 1000.;
                let tones: Vec<_>=freqs.iter().map(|&hz|{let folded=if hz>or as f64/2. {or as f64-hz}else{hz};let a=amplitude(&output,or,folded);let image=ir as f64-hz;json!({"hz":hz,"measuredHz":folded,"gainDb":20.*(a/(0.5/freqs.len()as f64)).log10(),"amplitude":a,"imageHz":image,"imageDb":if image<or as f64/2. {Some(20.*(amplitude(&output,or,image)/(0.5/freqs.len()as f64)).log10())}else{None}})}).collect();
                let peak = output
                    .iter()
                    .step_by(2)
                    .enumerate()
                    .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
                    .unwrap()
                    .0;
                write(
                    &format!("{folder}/{ir}-{or}-{name}-{candidate}.f32"),
                    &output,
                );
                rows.push(json!({"inputRate":ir,"outputRate":or,"signal":name,"candidate":candidate,"tones":tones,"frames":output.len()/2,"delayFrames":delay,"blockIn":block_in,"blockOut":block_out,"constructMs":construct_ms,"wallMs":wall_ms,"dspTotalMs":times.iter().sum::<f64>()/1000.,"blockMaxUs":times.iter().copied().fold(0.,f64::max),"blockAvgUs":if times.is_empty(){0.}else{times.iter().sum::<f64>()/times.len() as f64},"impulsePeak":if name=="impulse"{Some(peak)}else{None},"nonfinite":output.iter().filter(|v|!v.is_finite()).count(),"channelMismatch":output.as_chunks::<2>().0.iter().filter(|f|f[0].to_bits()!=f[1].to_bits()).count(),"dcInterior":output[or/10*2..or*9/10*2].iter().map(|v|*v as f64).sum::<f64>()/((or*9/10-or/10)*2) as f64}));
                rows.last_mut().unwrap()["resetPrime480AvgUs"] =
                    json!(reset_prime_us.iter().sum::<f64>() / reset_prime_us.len().max(1) as f64);
                rows.last_mut().unwrap()["resetPrime480MaxUs"] =
                    json!(reset_prime_us.iter().copied().fold(0., f64::max));
            }
        }
    }
    fs::write(
        "docs/validation/src-candidates.json",
        serde_json::to_vec_pretty(&rows).unwrap(),
    )
    .unwrap();
    println!("{} candidate measurements saved", rows.len());
}
