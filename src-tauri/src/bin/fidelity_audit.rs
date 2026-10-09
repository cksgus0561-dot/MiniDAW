//! Offline objective audit. Never linked into the callback or invoked by the UI.
use minidaw_lib::audio::{
    analysis::Analysis,
    decoder::{AudioData, FileInfo},
    metrics::AudioMetrics,
    reader::AudioReader,
    renderer::{Action, Command, Renderer, COMMAND_CAPACITY},
    source::{AssetStorage, AudioAsset, AudioSource},
    transport::TransportCell,
};
use serde_json::{json, Value};
use std::{
    fs,
    path::Path,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

#[derive(Default)]
struct ErrorStats {
    count: u64,
    mismatch: u64,
    first: Option<u64>,
    max: f64,
    squares: f64,
    nan: u64,
    inf: u64,
}
impl ErrorStats {
    fn add(&mut self, actual: f64, expected: f64, index: u64) {
        self.count += 1;
        if actual.is_nan() {
            self.nan += 1;
        }
        if actual.is_infinite() {
            self.inf += 1;
        }
        if actual.to_bits() != expected.to_bits() {
            self.mismatch += 1;
            if self.first.is_none() {
                self.first = Some(index);
            }
        }
        let e = (actual - expected).abs();
        self.max = self.max.max(e);
        self.squares += e * e;
    }
    fn json(&self) -> Value {
        json!({"samples":self.count,"exactMismatch":self.mismatch,"firstMismatch":self.first,"maxAbs":self.max,"rms":(self.squares/self.count.max(1) as f64).sqrt(),"nan":self.nan,"inf":self.inf})
    }
}
fn ready(source: &AudioSource, frame: usize) {
    let t = Instant::now();
    while source.pair(frame, frame).is_none() {
        assert!(!source.failed(), "{:?}", source.snapshot().error);
        assert!(
            t.elapsed() < Duration::from_secs(20),
            "timeout frame {frame}"
        );
        thread::sleep(Duration::from_millis(1));
    }
}
fn compare(path: &Path) -> Value {
    let start = Instant::now();
    let memory = AudioAsset::open_with_budget(path, usize::MAX).unwrap();
    let AssetStorage::Memory(ref pcm) = memory.storage else {
        panic!("force memory failed")
    };
    let mut sequential = AudioReader::open(path, None).unwrap();
    let mut mem_stats: [ErrorStats; 2] = Default::default();
    let mut position = 0;
    while let Some(chunk) = sequential.read_chunk().unwrap() {
        assert_eq!(chunk.start, position);
        for (i, &v) in chunk.samples.iter().enumerate() {
            let ch = i % pcm.info.channels;
            mem_stats[ch].add(
                v as f64,
                pcm.samples[position * pcm.info.channels + i] as f64,
                (position * pcm.info.channels + i) as u64,
            );
        }
        position += chunk.samples.len() / pcm.info.channels;
    }
    assert_eq!(position, pcm.info.frames);
    let asset = AudioAsset::open_with_budget(path, 0).unwrap();
    assert_eq!(asset.info.frames, pcm.info.frames);
    let source = AudioSource::new(asset.clone()).unwrap();
    let mut stream_stats: [ErrorStats; 2] = Default::default();
    for a in (0..position).step_by(4096) {
        let b = (a + 4096).min(position);
        ready(&source, b - 1);
        for f in a..b {
            let pair = source.pair(f, f).unwrap();
            for (ch, stats) in stream_stats.iter_mut().enumerate().take(pcm.info.channels) {
                stats.add(
                    pair[0][ch] as f64,
                    pcm.samples[f * pcm.info.channels + ch] as f64,
                    (f * pcm.info.channels + ch) as u64,
                );
            }
        }
        source.consumed(b);
    }
    let stream_snapshot = source.snapshot();
    let mut targets = vec![
        0,
        1,
        position / 10,
        position / 4,
        position / 2,
        position * 3 / 4,
        position * 9 / 10,
        position - 1,
        position.saturating_sub(1000),
    ];
    let mut seed = 1234567_u64;
    for _ in 0..8 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        targets.push(seed as usize % position);
    }
    let mut seeks = Vec::new();
    for target in targets {
        let now = Instant::now();
        source.seek(target);
        let end = (target + 4096).min(position);
        ready(&source, end - 1);
        let mut stats: [ErrorStats; 2] = Default::default();
        for f in target..end {
            let pair = source.pair(f, f).unwrap();
            for (ch, s) in stats.iter_mut().enumerate().take(pcm.info.channels) {
                s.add(
                    pair[0][ch] as f64,
                    pcm.samples[f * pcm.info.channels + ch] as f64,
                    (f * pcm.info.channels + ch) as u64,
                );
            }
        }
        seeks.push(json!({"targetFrame":target,"frames":end-target,"readyMs":now.elapsed().as_secs_f64()*1000.0,"channels":stats.iter().take(pcm.info.channels).map(ErrorStats::json).collect::<Vec<_>>()}));
    }
    // Rapid generations and independently owned cursors must not mix samples.
    for i in 0..1000 {
        source.seek(i * 197 % position);
    }
    source.seek(position / 2);
    ready(&source, position / 2);
    assert_eq!(
        source.pair(position / 2, position / 2).unwrap()[0][0].to_bits(),
        pcm.samples[(position / 2) * pcm.info.channels].to_bits()
    );
    let analysis = Analysis::new(&asset).unwrap();
    while !analysis.snapshot(&asset).complete {
        assert!(start.elapsed() < Duration::from_secs(240));
        thread::sleep(Duration::from_millis(5));
    }
    let wave = analysis.snapshot(&asset);
    assert!(wave.error.is_none(), "{:?}", wave.error);
    assert_eq!(wave.progress, 1.0);
    let first = pcm.samples.iter().position(|v| *v != 0.0);
    let last = pcm.samples.iter().rposition(|v| *v != 0.0);
    json!({"path":path,"file":pcm.info,"sequentialVsMemory":mem_stats.iter().take(pcm.info.channels).map(ErrorStats::json).collect::<Vec<_>>(),"memoryVsStreaming":stream_stats.iter().take(pcm.info.channels).map(ErrorStats::json).collect::<Vec<_>>(),"streamAfterFullPass":stream_snapshot,"seeks":seeks,"waveform":wave,"firstNonzeroSample":first,"lastNonzeroSample":last,"firstFrame":&pcm.samples[..pcm.info.channels],"lastFrame":&pcm.samples[pcm.samples.len()-pcm.info.channels..],"elapsedMs":start.elapsed().as_secs_f64()*1000.0})
}
fn conversion(dir: &Path) -> Value {
    let mut reports = Vec::new();
    for name in ["pcm16", "pcm24", "pcm32", "float32", "float64", "mono16"] {
        let path = dir.join(format!("{name}.wav"));
        let result = minidaw_lib::audio::decoder::decode(&path);
        let audio = match result {
            Ok(a) => a,
            Err(e) => {
                reports.push(json!({"name":name,"unsupported":e}));
                continue;
            }
        };
        let expected = fs::read(dir.join(format!("{name}.f64"))).unwrap();
        assert_eq!(expected.len() / 8, audio.samples.len());
        let mut stats: [ErrorStats; 2] = Default::default();
        let mut correctly_rounded = 0;
        let mut over = 0;
        for (i, b) in expected.as_chunks::<8>().0.iter().enumerate() {
            let reference = f64::from_le_bytes(*b);
            let value = audio.samples[i];
            stats[i % audio.info.channels].add(value as f64, reference, i as u64);
            if value.to_bits() == (reference as f32).to_bits() {
                correctly_rounded += 1;
            }
            if value.abs() > 1.0 {
                over += 1;
            }
        }
        let mut record = json!({"name":name,"frames":audio.info.frames,"channels":stats.iter().take(audio.info.channels).map(ErrorStats::json).collect::<Vec<_>>(),"correctlyRoundedSamples":correctly_rounded,"overFullScalePreserved":over,"firstSamples":&audio.samples[..8]});
        if name == "pcm16" || name == "pcm24" {
            let flac =
                minidaw_lib::audio::decoder::decode(&dir.join(format!("{name}.flac"))).unwrap();
            let independent = fs::read(dir.join(format!("{name}-flac-reference.f64"))).unwrap();
            assert_eq!(flac.samples.len(), audio.samples.len());
            let mut st = ErrorStats::default();
            for (i, b) in independent.as_chunks::<8>().0.iter().enumerate() {
                st.add(flac.samples[i] as f64, f64::from_le_bytes(*b), i as u64);
            }
            record["flacVsFfmpeg"] = st.json();
        }
        reports.push(record);
    }
    json!(reports)
}
fn render_signal(
    rate: u32,
    out_rate: u32,
    samples: Vec<f32>,
    channels: usize,
) -> (Vec<f32>, Value) {
    let frames = samples.len() / channels;
    let audio = AudioSource::memory(AudioData {
        info: FileInfo {
            name: "synthetic".into(),
            sample_rate: rate,
            channels,
            frames,
            duration: frames as f64 / rate as f64,
            sanitized_samples: 0,
        },
        samples,
    });
    let audio = AudioSource::for_output(audio.asset.clone(), out_rate).unwrap();
    let (mut tx, rx) = rtrb::RingBuffer::new(COMMAND_CAPACITY);
    let metrics = Arc::new(AudioMetrics::default());
    let mut renderer = Renderer::new(
        rx,
        Arc::new(TransportCell::default()),
        metrics.clone(),
        out_rate,
        2,
    );
    for (id, action) in [
        Action::Load {
            clip_id: 1,
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
    let mut out = vec![0_f32; (frames as f64 * out_rate as f64 / rate as f64).ceil() as usize * 2];
    let now = Instant::now();
    let mut at = 0;
    while at < out.len() {
        let count = 960.min(out.len() - at);
        let before = metrics.snapshot().rendered_frames;
        renderer.render(&mut out[at..at + count]);
        let written = (metrics.snapshot().rendered_frames - before) as usize * 2;
        at += written;
        if written == 0 {
            assert!(!audio.failed());
            assert!(now.elapsed() < Duration::from_secs(10));
            thread::sleep(Duration::from_millis(1));
        }
    }
    let cpu = now.elapsed().as_secs_f64() * 1000.0;
    (
        out,
        json!({"renderWallMs":cpu,"metrics":metrics.snapshot()}),
    )
}
fn amplitude(samples: &[f32], rate: u32, hz: f64, ch: usize) -> f64 {
    let n = samples.len() / 2;
    let mut re = 0.0;
    let mut im = 0.0;
    for (i, f) in samples.as_chunks::<2>().0.iter().enumerate() {
        let p = std::f64::consts::TAU * hz * i as f64 / rate as f64;
        re += f[ch] as f64 * p.cos();
        im += f[ch] as f64 * p.sin();
    }
    2.0 * (re * re + im * im).sqrt() / n as f64
}
fn src_audit() -> Value {
    fs::create_dir_all("tests/generated/fidelity/src").unwrap();
    let mut reports = Vec::new();
    for (rate, out_rate) in [(44100, 48000), (48000, 44100), (48000, 48000)] {
        for (name, freqs) in [
            ("silence", vec![]),
            ("dc", vec![]),
            ("impulse", vec![]),
            ("1k", vec![1000.]),
            ("mid", vec![5000.]),
            ("high", vec![15000.]),
            (
                "nearNyquist",
                vec![if rate == 44100 { 21000. } else { 23000. }],
            ),
            ("multitone", vec![1000., 5000., 15000.]),
        ] {
            let mut input = vec![0.0_f32; rate as usize * 2];
            for (i, f) in input.as_chunks_mut::<2>().0.iter_mut().enumerate() {
                let v = match name {
                    "dc" => 0.25,
                    "impulse" => {
                        if i == rate as usize / 4 {
                            0.5
                        } else {
                            0.0
                        }
                    }
                    _ => freqs
                        .iter()
                        .map(|hz| {
                            0.5 / freqs.len() as f64
                                * (std::f64::consts::TAU * hz * i as f64 / rate as f64).sin()
                        })
                        .sum(),
                };
                f[0] = v as f32;
                f[1] = f[0];
            }
            fs::write(
                format!("tests/generated/fidelity/src/{rate}-{out_rate}-{name}-input.f32"),
                input
                    .iter()
                    .flat_map(|s| s.to_le_bytes())
                    .collect::<Vec<_>>(),
            )
            .unwrap();
            let (output, cpu) = render_signal(rate, out_rate, input, 2);
            fs::write(
                format!("tests/generated/fidelity/src/{rate}-{out_rate}-{name}-output.f32"),
                output
                    .iter()
                    .flat_map(|s| s.to_le_bytes())
                    .collect::<Vec<_>>(),
            )
            .unwrap();
            let n = output.len() / 2;
            let max = output.iter().map(|v| v.abs()).fold(0.0_f32, f32::max);
            let dc = output.iter().step_by(2).map(|v| *v as f64).sum::<f64>() / n as f64;
            let mut tones = Vec::new();
            for &hz in &freqs {
                let folded = if hz > out_rate as f64 / 2.0 {
                    out_rate as f64 - hz
                } else {
                    hz
                };
                let a = amplitude(&output, out_rate, folded, 0);
                let image = (rate as f64 - hz).abs();
                tones.push(json!({"inputHz":hz,"measuredHz":folded,"amplitude":a,"gainDb":20.0*(a/(0.5/freqs.len()as f64)).log10(),"imageHz":image,"imageAmplitude":if image<out_rate as f64/2.0{Some(amplitude(&output,out_rate,image,0))}else{None}}));
            }
            let impulse_peak = output
                .as_chunks::<2>()
                .0
                .iter()
                .enumerate()
                .max_by(|a, b| a.1[0].abs().total_cmp(&b.1[0].abs()))
                .map(|x| x.0);
            reports.push(json!({"inputRate":rate,"outputRate":out_rate,"signal":name,"frames":n,"dc":dc,"peak":max,"tones":tones,"nonfinite":output.iter().filter(|v|!v.is_finite()).count(),"channelMismatch":output.as_chunks::<2>().0.iter().filter(|f|f[0].to_bits()!=f[1].to_bits()).count(),"impulsePeakFrame":if name=="impulse"{impulse_peak}else{None},"cpu":cpu}));
        }
    }
    let mut channel_tests = Vec::new();
    for ch in 0..2 {
        let mut input = vec![0.0; 96000];
        input[1234 * 2 + ch] = 0.5;
        let (out, _) = render_signal(48000, 48000, input.clone(), 2);
        assert_eq!(out, input);
        channel_tests.push(json!({"impulseChannel":ch,"exact":true}));
    }
    let input: Vec<f32> = (0..48000)
        .flat_map(|i| {
            [
                (std::f64::consts::TAU * 1000. * i as f64 / 48000.).sin() as f32 * 0.25,
                (std::f64::consts::TAU * 3300. * i as f64 / 48000.).sin() as f32 * 0.25,
            ]
        })
        .collect();
    let (out, _) = render_signal(48000, 48000, input.clone(), 2);
    assert_eq!(out, input);
    let mono: Vec<f32> = (0..48000).map(|i| i as f32 / 96000.).collect();
    let (out, _) = render_signal(48000, 48000, mono.clone(), 1);
    assert!(out
        .as_chunks::<2>()
        .0
        .iter()
        .zip(mono)
        .all(|(a, b)| *a == [b, b]));
    json!({"measurements":reports,"channelTests":channel_tests,"differentLRSinesExact":true,"monoDuplicatedExactly":true,"transitions":transitions()})
}
fn transitions() -> Value {
    let samples: Vec<f32> = (0..48000)
        .map(|i| 0.2 + 0.4 * (std::f64::consts::TAU * 997. * i as f64 / 48000.).sin() as f32)
        .collect();
    let audio = AudioSource::memory(AudioData {
        info: FileInfo {
            name: "transition".into(),
            sample_rate: 48000,
            channels: 1,
            frames: 48000,
            duration: 1.,
            sanitized_samples: 0,
        },
        samples,
    });
    let (mut tx, rx) = rtrb::RingBuffer::new(COMMAND_CAPACITY);
    let mut renderer = Renderer::new(
        rx,
        Arc::new(TransportCell::default()),
        Arc::new(AudioMetrics::default()),
        48000,
        2,
    );
    tx.push(Command {
        id: 0,
        issued: Instant::now(),
        action: Action::Load {
            clip_id: 1,
            audio: audio.clone(),
        },
    })
    .ok()
    .unwrap();
    let mut previous = 0.;
    let mut records = Vec::new();
    for (id, (name, action)) in [
        ("play", Action::Play),
        ("pause", Action::Pause),
        ("resume", Action::Play),
        ("seek", Action::Seek(0.357)),
        ("stop", Action::Stop),
        ("restart", Action::Play),
    ]
    .into_iter()
    .enumerate()
    {
        tx.push(Command {
            id: id as u64 + 1,
            issued: Instant::now(),
            action,
        })
        .ok()
        .unwrap();
        let before = renderer.transport.frame;
        let mut out = [0_f32; 960];
        renderer.render(&mut out);
        records.push(json!({"action":name,"previousLast":previous,"nextFirst":out[0],"jump":out[0]-previous,"frameBefore":before,"frameAfter":renderer.transport.frame}));
        previous = out[958];
    }
    json!(records)
}
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let report = match args[0].as_str() {
        "compare" => Value::Array(
            args[2..]
                .iter()
                .map(|p| {
                    eprintln!("Comparing {p}");
                    compare(Path::new(p))
                })
                .collect(),
        ),
        "conversion" => conversion(Path::new(&args[2])),
        "src" => src_audit(),
        "raw-mp3" => raw_mp3(Path::new(&args[2])),
        _ => panic!("compare|conversion|src output.json [paths]"),
    };
    fs::write(&args[1], serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    println!("Report: {}", args[1]);
}

// Direct Symphonia gapless sequential decoding, bypassing AudioReader, checks
// that valid tagged sources retain the original decoder's complete PCM range.
fn raw_mp3(path: &Path) -> Value {
    use symphonia::core::{
        audio::SampleBuffer, codecs::DecoderOptions, formats::FormatOptions, io::MediaSourceStream,
        meta::MetadataOptions, probe::Hint,
    };
    let pcm = minidaw_lib::audio::decoder::decode(path).unwrap();
    let mut hint = Hint::new();
    hint.with_extension("mp3");
    let mut format = symphonia::default::get_probe()
        .format(
            &hint,
            MediaSourceStream::new(Box::new(fs::File::open(path).unwrap()), Default::default()),
            &FormatOptions {
                enable_gapless: true,
                ..Default::default()
            },
            &MetadataOptions::default(),
        )
        .unwrap()
        .format;
    let track = format.default_track().unwrap();
    let params = track.codec_params.clone();
    let id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&params, &DecoderOptions::default())
        .unwrap();
    let mut stats: [ErrorStats; 2] = Default::default();
    let mut samples = 0;
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break
            }
            Err(e) => panic!("{e}"),
        };
        if packet.track_id() != id {
            continue;
        }
        let decoded = decoder.decode(&packet).unwrap();
        let mut converted = SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
        converted.copy_interleaved_ref(decoded);
        for &v in converted.samples() {
            assert!(samples < pcm.samples.len());
            stats[samples % pcm.info.channels].add(
                pcm.samples[samples] as f64,
                v as f64,
                samples as u64,
            );
            samples += 1;
        }
    }
    assert_eq!(samples, pcm.samples.len());
    for s in &stats {
        assert_eq!(s.mismatch, 0);
    }
    json!({"path":path,"reference":"Symphonia 0.5.5 direct sequential enable_gapless=true, without MiniDAW AudioReader","codecParameters":format!("{params:?}"),"frames":pcm.info.frames,"channels":stats.iter().take(pcm.info.channels).map(ErrorStats::json).collect::<Vec<_>>()})
}
