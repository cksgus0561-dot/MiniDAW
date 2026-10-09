//! Explicit full-memory decoding for small assets and reference tests only.
use crate::error::AppResult;
use serde::Serialize;
use std::path::Path;
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileInfo {
    pub name: String,
    pub sample_rate: u32,
    pub channels: usize,
    pub frames: usize,
    pub duration: f64,
    pub sanitized_samples: usize,
}

pub struct AudioData {
    pub info: FileInfo,
    pub samples: Vec<f32>,
}

pub fn decode(path: &Path) -> AppResult<AudioData> {
    let mut reader = super::reader::AudioReader::open(path, None)?;
    let mut samples = Vec::new();
    while let Some(chunk) = reader.read_chunk()? {
        samples.extend_from_slice(&chunk.samples);
    }
    if samples.is_empty() {
        return Err(super::reader::decode_error(
            "재생할 오디오 샘플이 없습니다.",
        ));
    }
    let mut info = reader.info;
    info.frames = samples.len() / info.channels;
    info.duration = info.frames as f64 / info.sample_rate as f64;
    Ok(AudioData { info, samples })
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_wav_mp3_flac_decode_to_stereo_pcm() {
        for extension in ["wav", "mp3", "flac"] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join(format!("../tests/fixtures/stereo-44100.{extension}"));
            let audio = decode(&path).unwrap();
            assert_eq!(audio.info.sample_rate, 44100);
            assert_eq!(audio.info.channels, 2);
            assert!((audio.info.duration - 6.0).abs() < 0.05);
            assert!(audio.samples.iter().all(|s| s.is_finite()));
            assert!(audio.samples.iter().any(|s| s.abs() > 0.05));
            assert!(audio
                .samples
                .as_chunks::<2>()
                .0
                .iter()
                .any(|f| (f[0] - f[1]).abs() > 0.05));
        }
    }

    #[test]
    fn reports_unsupported_missing_and_corrupt_files() {
        assert_eq!(decode(Path::new("bad.ogg")).err().unwrap().code, "format");
        assert_eq!(decode(Path::new("missing.wav")).err().unwrap().code, "file");
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/corrupt.wav");
        assert_eq!(decode(&path).err().unwrap().code, "decode");
    }

    #[test]
    fn unicode_path_uppercase_extension_mono_float_wav() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/모노-48000.WAV");
        let audio = decode(&path).unwrap();
        assert_eq!(audio.info.channels, 1);
        assert_eq!(audio.info.sample_rate, 48000);
        assert_eq!(audio.info.frames, 12000);
        assert_eq!(audio.info.duration, 0.25);
        assert!(audio.samples.iter().any(|sample| sample.abs() > 0.04));
    }
}
