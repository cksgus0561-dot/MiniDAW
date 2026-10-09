use minidaw_lib::audio::{
    analysis::Analysis,
    decoder,
    reader::AudioReader,
    source::{AudioAsset, AudioSource},
};
use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};
fn temp(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/fidelity-tests");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}
fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/stereo-44100.mp3")
}
#[test]
fn tagged_mp3_counts_actual_packets_and_seeks_across_embedded_info() {
    let original = std::fs::read(fixture()).unwrap();
    let marker = original
        .windows(4)
        .position(|p| p == b"Info" || p == b"Xing")
        .unwrap();
    let frames = u32::from_be_bytes(original[marker + 8..marker + 12].try_into().unwrap());
    for (name, count, copies) in [
        ("under.mp3", frames / 2, 1),
        ("over.mp3", frames + 7, 1),
        ("concat.mp3", frames * 4 + 3, 4),
        ("long-delay.mp3", frames, 1),
    ] {
        let path = temp(name);
        let mut bytes = original.repeat(copies);
        bytes[marker + 8..marker + 12].copy_from_slice(&count.to_be_bytes());
        if name == "long-delay.mp3" {
            let flags = u32::from_be_bytes(bytes[marker + 4..marker + 8].try_into().unwrap());
            let encoder = marker
                + 8
                + if flags & 1 != 0 { 4 } else { 0 }
                + if flags & 2 != 0 { 4 } else { 0 }
                + if flags & 4 != 0 { 100 } else { 0 }
                + if flags & 8 != 0 { 4 } else { 0 };
            let trim = (2000_u32 << 12)
                | (u32::from(bytes[encoder + 22] & 15) << 8)
                | u32::from(bytes[encoder + 23]);
            bytes[encoder + 21..encoder + 24].copy_from_slice(&trim.to_be_bytes()[1..]);
        }
        std::fs::write(&path, &bytes).unwrap();
        let pcm = decoder::decode(&path).unwrap();
        let asset = AudioAsset::open_with_budget(&path, 0).unwrap();
        assert_eq!(asset.info.frames, pcm.info.frames);
        if copies == 1 && name != "long-delay.mp3" {
            assert_eq!(pcm.info.frames, 264600);
        }
        let stream = AudioSource::new(asset.clone()).unwrap();
        for target in [
            1,
            pcm.info.frames / 4,
            pcm.info.frames / 2,
            pcm.info.frames * 3 / 4,
            pcm.info.frames - 1,
        ] {
            stream.seek(target);
            let now = Instant::now();
            while stream.pair(target, target).is_none() {
                assert!(!stream.failed(), "{:?}", stream.snapshot().error);
                assert!(now.elapsed() < Duration::from_secs(5));
                thread::sleep(Duration::from_millis(1));
            }
            let sample = stream.pair(target, target).unwrap()[0];
            assert_eq!(
                sample,
                [pcm.samples[target * 2], pcm.samples[target * 2 + 1]]
            );
        }
        let analysis = Analysis::new(&asset).unwrap();
        let now = Instant::now();
        while !analysis.snapshot(&asset).complete {
            assert!(now.elapsed() < Duration::from_secs(10));
            thread::sleep(Duration::from_millis(2));
        }
        let result = analysis.snapshot(&asset);
        assert!(result.error.is_none(), "{:?}", result.error);
        assert_eq!(result.progress, 1.0);
    }
}
fn wav(name: &str, bits: u16, code: u16, pcm: &[u8]) -> PathBuf {
    let mut b = Vec::new();
    b.extend(b"RIFF");
    b.extend((36 + pcm.len() as u32).to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(16_u32.to_le_bytes());
    b.extend(code.to_le_bytes());
    b.extend(1_u16.to_le_bytes());
    b.extend(48000_u32.to_le_bytes());
    b.extend((48000 * u32::from(bits) / 8).to_le_bytes());
    b.extend((bits / 8).to_le_bytes());
    b.extend(bits.to_le_bytes());
    b.extend(b"data");
    b.extend((pcm.len() as u32).to_le_bytes());
    b.extend(pcm);
    let path = temp(name);
    std::fs::write(&path, b).unwrap();
    path
}
#[test]
fn conversions_preserve_f32_headroom_and_round_wider_sources() {
    for bits in [16, 24, 32] {
        let max = (1_i64 << (bits - 1)) - 1;
        let integers = [0, 1, -1, max, -max - 1, 123, -123];
        let pcm: Vec<u8> = integers
            .iter()
            .flat_map(|n| n.to_le_bytes()[..bits / 8].to_vec())
            .collect();
        let decoded =
            decoder::decode(&wav(&format!("pcm{bits}.wav"), bits as u16, 1, &pcm)).unwrap();
        let expected: Vec<f32> = integers
            .iter()
            .map(|&n| (n as f64 / (1_u64 << (bits - 1)) as f64) as f32)
            .collect();
        assert_eq!(decoded.samples, expected);
    }
    let values = [0.0_f32, 0.5, -0.5, 1.25, -1.5, f32::MAX, -f32::MAX];
    let pcm: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
    let decoded = decoder::decode(&wav("headroom.wav", 32, 3, &pcm)).unwrap();
    assert_eq!(decoded.samples, values);
    let values = [
        0.0_f64,
        1.0 + 2.0_f64.powi(-40),
        -1.5,
        std::f64::consts::PI / 8.0,
    ];
    let pcm: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
    let decoded = decoder::decode(&wav("float64.wav", 64, 3, &pcm)).unwrap();
    assert_eq!(decoded.samples, values.map(|v| v as f32));
    let values = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1.25];
    let pcm: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
    let decoded = decoder::decode(&wav("nonfinite.wav", 32, 3, &pcm)).unwrap();
    assert_eq!(decoded.samples, [0., 0., 0., 1.25]);
    assert_eq!(decoded.info.sanitized_samples, 3);
}
#[test]
fn verified_timeline_detects_truncated_source_after_import() {
    let path = temp("changed.wav");
    let pcm: Vec<u8> = (0..48000_i32)
        .flat_map(|v| (v as i16).to_le_bytes())
        .collect();
    let original = wav("original.wav", 16, 1, &pcm);
    std::fs::copy(&original, &path).unwrap();
    let asset = AudioAsset::open_with_budget(&path, 0).unwrap();
    let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.set_len(2048).unwrap();
    drop(file);
    let analysis = Analysis::new(&asset).unwrap();
    let now = Instant::now();
    while !analysis.snapshot(&asset).complete {
        assert!(now.elapsed() < Duration::from_secs(5));
        thread::sleep(Duration::from_millis(1));
    }
    assert!(analysis.snapshot(&asset).error.is_some());
}
#[test]
fn cancellation_is_an_error_not_normal_eof() {
    let cancel: minidaw_lib::audio::reader::Cancel = std::sync::Arc::new(|| true);
    assert!(AudioReader::open(&fixture(), Some(cancel)).is_err());
}
