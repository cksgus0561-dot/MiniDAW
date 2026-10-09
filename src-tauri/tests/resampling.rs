use minidaw_lib::audio::{
    decoder::{AudioData, FileInfo},
    metrics::AudioMetrics,
    renderer::{Action, Command, Renderer, COMMAND_CAPACITY},
    source::AudioSource,
    transport::{PlayState, TransportCell},
};
use std::{sync::Arc, time::Instant};

fn render(
    samples: Vec<f32>,
    source_channels: usize,
    output_channels: usize,
    output_rate: u32,
    frames: usize,
) -> (Vec<f32>, PlayState) {
    let source_frames = samples.len() / source_channels;
    let owner = AudioSource::memory(AudioData {
        info: FileInfo {
            name: "resample".into(),
            sample_rate: 4,
            channels: source_channels,
            frames: source_frames,
            duration: source_frames as f64 / 4.0,
            sanitized_samples: 0,
        },
        samples,
    });
    let (mut tx, rx) = rtrb::RingBuffer::new(COMMAND_CAPACITY);
    for (id, action) in [
        Action::Load {
            clip_id: 1,
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
    let shared = Arc::new(TransportCell::default());
    let mut renderer = Renderer::new(
        rx,
        shared.clone(),
        Arc::new(AudioMetrics::default()),
        output_rate,
        output_channels,
    );
    let mut output = vec![0.0; frames * output_channels];
    renderer.render(&mut output);
    (output, shared.read().state)
}

#[test]
fn equal_rate_stereo_preserves_channels_and_speed() {
    let (output, state) = render(vec![0.1, -0.1, 0.2, -0.2, 0.3, -0.3, 0.4, -0.4], 2, 2, 4, 2);
    for (actual, expected) in output.iter().zip([0.1, -0.1, 0.2, -0.2]) {
        assert!((actual - expected).abs() < 1e-6);
    }
    assert_eq!(state, PlayState::Playing);
}

#[test]
fn mono_mapping_surround_silence_and_end() {
    let (output, state) = render(vec![0.1, 0.3], 1, 4, 4, 3);
    assert_eq!(
        output,
        [0.1, 0.1, 0.0, 0.0, 0.3, 0.3, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
    );
    assert_eq!(state, PlayState::Stopped);
}

#[test]
fn extreme_float_samples_are_clamped_without_nan() {
    let (output, _) = render(vec![f32::MAX, -f32::MAX, f32::MAX], 1, 2, 4, 6);
    assert!(output
        .iter()
        .all(|sample| sample.is_finite() && sample.abs() <= 1.0));
}

#[test]
fn clipping_occurs_at_output_after_mono_mix() {
    // Device clipping does not alter stored PCM or occur before downmix.
    let (output, _) = render(vec![1.5, 0.5], 1, 2, 4, 2);
    assert_eq!(output, [1.0, 1.0, 0.5, 0.5]);
    let (output, _) = render(vec![1.5, -0.5], 2, 1, 4, 1);
    assert_eq!(output, [0.5]);
}

#[test]
fn equal_rate_bypass_preserves_all_in_range_f32_bits() {
    let samples = vec![
        0.0,
        -0.0,
        f32::MIN_POSITIVE,
        -f32::MIN_POSITIVE,
        f32::from_bits(1),
        -f32::from_bits(1),
        0.1234567,
        -0.7654321,
        1.0,
        -1.0,
    ];
    let (output, _) = render(samples.clone(), 2, 2, 4, samples.len() / 2);
    assert_eq!(
        output.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        samples.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
    );
}
