use minidaw_lib::audio::{
    analysis::Analysis,
    decoder,
    source::{AssetStorage, AudioAsset, AudioSource},
    streaming::CAPACITY,
};
use std::{
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures")
        .join(name)
}
fn ready(source: &AudioSource, frame: usize) {
    let start = Instant::now();
    while source.pair(frame, frame).is_none() {
        assert!(!source.failed(), "{:?}", source.snapshot().error);
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "read ahead timeout"
        );
        thread::sleep(Duration::from_millis(1));
    }
}
#[test]
fn compressed_and_pcm_seeks_match_full_decode_samples() {
    for name in [
        "stereo-44100.wav",
        "stereo-44100.mp3",
        "stereo-44100.flac",
        "모노-48000.WAV",
        "untagged-vbr.mp3",
    ] {
        let path = fixture(name);
        let reference = decoder::decode(&path).unwrap();
        let source = AudioSource::new(AudioAsset::open_with_budget(&path, 0).unwrap()).unwrap();
        let count = reference.info.frames;
        assert_eq!(
            source.asset.info.frames, count,
            "{name}: exact source length"
        );
        for target in [
            0,
            count * 4 / 5,
            1,
            count / 2 + 63,
            count - 200,
            127,
            count / 3,
        ] {
            source.seek(target);
            ready(&source, (target + 100).min(count - 1));
            for frame in target..(target + 100).min(count) {
                let actual = source.pair(frame, frame).unwrap()[0];
                for (ch, value) in actual.iter().enumerate() {
                    let expected = reference.samples
                        [frame * reference.info.channels + ch.min(reference.info.channels - 1)];
                    assert!(
                        value.to_bits() == expected.to_bits(),
                        "{name} target={target} frame={frame} ch={ch}: {value} vs {expected}"
                    );
                }
            }
        }
        assert_eq!(source.snapshot().buffer_bytes, CAPACITY * 8);
        assert_eq!(source.snapshot().starvation, 0);
    }
}
#[test]
fn latest_generation_wins_and_shared_asset_cursors_are_independent() {
    let asset = AudioAsset::open_with_budget(&fixture("stereo-44100.mp3"), 0).unwrap();
    let a = AudioSource::new(asset.clone()).unwrap();
    let b = AudioSource::new(asset).unwrap();
    for i in 0..1000 {
        a.seek(if i % 2 == 0 { 200000 } else { 10000 });
    }
    a.seek(12345);
    b.seek(23456);
    ready(&a, 12346);
    ready(&b, 23457);
    let reference = decoder::decode(&fixture("stereo-44100.mp3")).unwrap();
    for (source, frame) in [(a, 12345), (b, 23456)] {
        let pair = source.pair(frame, frame + 1).unwrap();
        assert_eq!(pair[0][0].to_bits(), reference.samples[frame * 2].to_bits());
        let metrics = source.snapshot();
        assert_eq!(metrics.generation, metrics.ready_generation);
        assert!(metrics.buffered_frames <= CAPACITY);
    }
}
#[test]
fn small_file_policy_and_waveform_reader_do_not_change_playback_cursor() {
    let path = fixture("stereo-44100.flac");
    let memory = AudioAsset::open(&path).unwrap();
    assert!(matches!(memory.storage, AssetStorage::Memory(_)));
    let asset = AudioAsset::open_with_budget(&path, 0).unwrap();
    let source = AudioSource::new(asset.clone()).unwrap();
    source.seek(200000);
    ready(&source, 200001);
    let generation = source.snapshot().generation;
    let analysis = Analysis::new(&asset).unwrap();
    let view = analysis.view(&asset, 1, 1.25, 1.35, 200).unwrap();
    let reference = Analysis::new(&memory)
        .unwrap()
        .view(&memory, 1, 1.25, 1.35, 200)
        .unwrap();
    assert_eq!(view.channels, reference.channels);
    assert_eq!(source.snapshot().generation, generation);
    assert!(source.pair(200000, 200001).is_some());
    let started = Instant::now();
    while !analysis.snapshot(&asset).complete {
        assert!(started.elapsed() < Duration::from_secs(5));
        thread::sleep(Duration::from_millis(2));
    }
    let state = analysis.snapshot(&asset);
    assert!(state.error.is_none());
    assert_eq!(state.progress, 1.0);
    assert!(state.bytes <= 8 * 1024 * 1024 + 512);
}
#[test]
fn deleted_source_reports_error_instead_of_panicking() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-temp");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("deleted-{}.wav", std::process::id()));
    std::fs::copy(fixture("stereo-44100.wav"), &path).unwrap();
    let source = AudioSource::new(AudioAsset::open_with_budget(&path, 0).unwrap()).unwrap();
    std::fs::remove_file(&path).unwrap();
    source.seek(20000);
    let started = Instant::now();
    while !source.failed() {
        assert!(started.elapsed() < Duration::from_secs(5));
        thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(source.snapshot().error.unwrap().code, "stream_read");
}

#[test]
fn read_ahead_wraps_without_stale_frames_or_memory_growth() {
    let reference = decoder::decode(&fixture("stereo-44100.wav")).unwrap();
    let source =
        AudioSource::new(AudioAsset::open_with_budget(&fixture("stereo-44100.wav"), 0).unwrap())
            .unwrap();
    for start in (0..reference.info.frames).step_by(512) {
        let end = (start + 512).min(reference.info.frames);
        ready(&source, end - 1);
        for frame in start..end {
            assert_eq!(
                source.pair(frame, frame).unwrap()[0][0],
                reference.samples[frame * 2]
            );
        }
        source.consumed(end);
    }
    assert!(reference.info.frames > CAPACITY);
    assert_eq!(source.snapshot().buffer_bytes, CAPACITY * 8);
}

#[test]
#[ignore = "Generate ignored fixtures with node scripts/generate-large-fixtures.mjs --compressed"]
fn generated_large_sources_seek_matches_bounded_sequential_decode() {
    use minidaw_lib::audio::reader::AudioReader;
    for name in [
        "long-1200s-48000-2ch.wav",
        "long-720s-44100-2ch.mp3",
        "long-720s-44100-2ch.flac",
        "long-720s-48000-1ch.wav",
    ] {
        let path = fixture("..").join("generated").join(name);
        let asset = AudioAsset::open(&path).unwrap();
        assert!(matches!(asset.storage, AssetStorage::File(_)));
        let targets = [
            asset.info.frames * 95 / 100,
            asset.info.frames / 50,
            asset.info.frames * 7 / 10,
            63,
        ];
        let mut expected: Vec<Vec<f32>> = targets.iter().map(|_| Vec::new()).collect();
        let mut reference = AudioReader::open(&path, None).unwrap();
        while let Some(chunk) = reference.read_chunk().unwrap() {
            let end = chunk.start + chunk.samples.len() / asset.info.channels;
            for (index, &target) in targets.iter().enumerate() {
                let a = chunk.start.max(target);
                let b = end.min(target + 1000);
                if b > a {
                    expected[index].extend_from_slice(
                        &chunk.samples[(a - chunk.start) * asset.info.channels
                            ..(b - chunk.start) * asset.info.channels],
                    );
                }
            }
        }
        let source = AudioSource::new(asset.clone()).unwrap();
        for (i, &target) in targets.iter().enumerate() {
            assert_eq!(expected[i].len(), 1000 * asset.info.channels);
            source.seek(target);
            ready(&source, target + 999);
            for frame in 0..1000 {
                let actual = source.pair(target + frame, target + frame).unwrap()[0];
                for (ch, value) in actual.iter().enumerate().take(asset.info.channels) {
                    assert!(
                        value.to_bits() == expected[i][frame * asset.info.channels + ch].to_bits(),
                        "{name}: frame {} channel {ch}",
                        target + frame
                    );
                }
            }
        }
        assert_eq!(source.snapshot().buffer_bytes, CAPACITY * 8);
    }
}
