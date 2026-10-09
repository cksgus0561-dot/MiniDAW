//! Worker-only file/codec cursor. Each playback instance and waveform reader owns
//! its own handle and decoder; no seek state is shared between them.
use super::decoder::FileInfo;
use crate::error::{AppError, AppResult};
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    path::Path,
    sync::Arc,
};
use symphonia::core::{
    audio::SampleBuffer,
    codecs::{Decoder, DecoderOptions},
    errors::Error,
    formats::{FormatOptions, FormatReader, SeekMode, SeekTo},
    io::{MediaSource, MediaSourceStream},
    meta::MetadataOptions,
    probe::Hint,
};

pub type Cancel = Arc<dyn Fn() -> bool + Send + Sync>;
// Debug test instrumentation is thread-local; absent from release builds.
#[cfg(debug_assertions)]
thread_local! { static OPERATIONS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }
#[cfg(debug_assertions)]
pub fn operations_on_current_thread() -> u64 {
    OPERATIONS.get()
}
#[inline]
pub(super) fn operation() {
    #[cfg(debug_assertions)]
    OPERATIONS.set(OPERATIONS.get() + 1);
}
struct CancellableFile {
    file: File,
    cancel: Option<Cancel>,
}
impl CancellableFile {
    fn check(&self) -> io::Result<()> {
        if self.cancel.as_ref().is_some_and(|f| f()) {
            // Interrupted is retried by read_exact; Other really aborts a stale seek.
            Err(io::Error::other("obsolete audio request"))
        } else {
            Ok(())
        }
    }
}
impl Read for CancellableFile {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        operation();
        self.check()?;
        self.file.read(buf)
    }
}
impl Seek for CancellableFile {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        operation();
        self.check()?;
        self.file.seek(pos)
    }
}
impl MediaSource for CancellableFile {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        self.file.metadata().ok().map(|m| m.len())
    }
}

pub fn decode_error(error: impl std::fmt::Display) -> AppError {
    AppError::new("decode", "오디오 디코딩에 실패했습니다.").detail(error)
}
pub struct Chunk {
    pub start: usize,
    pub samples: Vec<f32>,
}
pub struct AudioReader {
    pub info: FileInfo,
    format: Box<dyn FormatReader>,
    decoder: Box<dyn Decoder>,
    track: u32,
    mp3: bool,
    delay: usize,
    padding: usize,
    decode_from: usize,
    discard_before: usize,
}
impl AudioReader {
    pub fn open(path: &Path, cancel: Option<Cancel>) -> AppResult<Self> {
        Self::open_inner(path, cancel, None)
    }

    /// Independent decoder using the timeline already verified during import.
    /// No repeated whole-file scan on playback/analysis reader construction.
    pub fn for_asset(path: &Path, cancel: Option<Cancel>, info: &FileInfo) -> AppResult<Self> {
        Self::open_inner(path, cancel, Some(info))
    }

    fn open_inner(
        path: &Path,
        cancel: Option<Cancel>,
        verified: Option<&FileInfo>,
    ) -> AppResult<Self> {
        let ext = path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !matches!(ext.as_str(), "wav" | "mp3" | "flac") {
            return Err(AppError::new(
                "format",
                "지원하지 않는 오디오 형식입니다. WAV, MP3, FLAC 파일을 선택해 주세요.",
            ));
        }
        let file = File::open(path)
            .map_err(|e| AppError::new("file", "파일을 읽을 수 없습니다.").detail(e))?;
        if !file.metadata().map_err(decode_error)?.is_file() {
            return Err(AppError::new("file", "일반 오디오 파일을 선택해 주세요."));
        }
        let source = MediaSourceStream::new(
            Box::new(CancellableFile { file, cancel }),
            Default::default(),
        );
        let mut hint = Hint::new();
        hint.with_extension(&ext);
        let mut format = symphonia::default::get_probe()
            .format(
                &hint,
                source,
                &FormatOptions {
                    // MP3 header counts may include embedded Info frames or be
                    // stale. Apply gapless trimming against our verified count.
                    enable_gapless: ext != "mp3",
                    ..Default::default()
                },
                &MetadataOptions::default(),
            )
            .map_err(decode_error)?
            .format;
        let track = format
            .default_track()
            .ok_or_else(|| decode_error("오디오 트랙이 없습니다."))?;
        let track_id = track.id;
        let params = track.codec_params.clone();
        let decoder = symphonia::default::get_codecs()
            .make(&params, &DecoderOptions::default())
            .map_err(decode_error)?;
        let rate = params.sample_rate.unwrap_or(0);
        let channels = params.channels.map_or(0, |c| c.count());
        if rate == 0 || !(1..=2).contains(&channels) {
            return Err(AppError::new(
                "channels",
                "모노와 스테레오 오디오를 지원합니다.",
            ));
        }
        // These three demuxers use sample-frame timestamps. Refuse an unknown
        // timebase instead of silently mapping a seek to the wrong sample.
        if params
            .time_base
            .is_some_and(|t| t.numer != 1 || t.denom != rate)
        {
            return Err(decode_error("지원하지 않는 오디오 시간 기준입니다."));
        }
        let frames = if let Some(info) = verified {
            info.frames as u64
        } else if let Some(n) = params.n_frames.filter(|n| *n > 0) {
            n
        } else {
            // Headerless files: bounded packet scan, never collect whole PCM.
            let mut frames = 0;
            loop {
                match format.next_packet() {
                    Ok(p) if p.track_id() == track_id => frames = p.ts().saturating_add(p.dur()),
                    Ok(_) => (),
                    Err(Error::IoError(e)) if e.kind() == io::ErrorKind::UnexpectedEof => break,
                    Err(e) => return Err(decode_error(e)),
                }
            }
            format
                .seek(SeekMode::Accurate, SeekTo::TimeStamp { ts: 0, track_id })
                .map_err(decode_error)?;
            frames
        };
        let frames = usize::try_from(frames).map_err(decode_error)?;
        if frames == 0 {
            return Err(decode_error("재생할 오디오 샘플이 없습니다."));
        }
        let mut reader = Self {
            info: FileInfo {
                name: path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                sample_rate: rate,
                channels,
                frames,
                duration: frames as f64 / rate as f64,
                sanitized_samples: 0,
            },
            format,
            decoder,
            track: track_id,
            mp3: ext == "mp3",
            delay: if ext == "mp3" {
                params.delay.unwrap_or(0) as usize
            } else {
                0
            },
            padding: if ext == "mp3" {
                params.padding.unwrap_or(0) as usize
            } else {
                0
            },
            decode_from: 0,
            discard_before: 0,
        };
        if let Some(info) = verified {
            if info.sample_rate != rate || info.channels != channels {
                return Err(decode_error("불러온 후 원본 오디오 형식이 변경되었습니다."));
            }
            reader.info = info.clone();
        } else {
            reader.resolve_length()?;
        }
        Ok(reader)
    }

    /// Count actual audio packets, including tagged MP3s. next_packet excludes
    /// embedded ID3/Info headers; container estimates must not define PCM length.
    fn resolve_length(&mut self) -> AppResult<()> {
        if !self.mp3 {
            return Ok(());
        }
        let mut frames = 0_u64;
        loop {
            match self.format.next_packet() {
                Ok(p) if p.track_id() == self.track => {
                    frames = p.ts().saturating_add(p.block_dur())
                }
                Ok(_) => (),
                Err(Error::IoError(e)) if e.kind() == io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(decode_error(e)),
            }
        }
        let frames = frames
            .checked_sub((self.delay + self.padding) as u64)
            .ok_or_else(|| decode_error("MP3 delay/padding이 실제 프레임 수를 초과합니다."))?;
        if frames == 0 {
            return Err(decode_error("재생할 오디오 샘플이 없습니다."));
        }
        self.info.frames = usize::try_from(frames).map_err(decode_error)?;
        self.info.duration = frames as f64 / self.info.sample_rate as f64;
        self.seek(0)
    }

    pub fn seek(&mut self, frame: usize) -> AppResult<()> {
        // One second bounds reservoir + synthesis priming even for low bitrate
        // MP3. Packet scanning (not accurate seek's header counter) maintains
        // the same timestamps as sequential decoding across embedded Info tags.
        let preroll = if self.mp3 {
            self.info.sample_rate as usize
        } else {
            0
        };
        self.format
            .seek(
                SeekMode::Accurate,
                SeekTo::TimeStamp {
                    // Symphonia 0.5.5 accurate MP3 seek counts embedded Info
                    // headers unlike next_packet. Rewind, then scan packets
                    // without decoding until the bounded preroll window.
                    ts: if self.mp3 { 0 } else { frame as u64 },
                    track_id: self.track,
                },
            )
            .map_err(decode_error)?;
        self.decoder.reset();
        self.decode_from = if self.mp3 {
            frame.saturating_add(self.delay).saturating_sub(preroll)
        } else {
            0
        };
        self.discard_before = frame;
        Ok(())
    }

    pub fn read_chunk(&mut self) -> AppResult<Option<Chunk>> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(p) => p,
                Err(Error::IoError(e)) if e.kind() == io::ErrorKind::UnexpectedEof => {
                    return Ok(None)
                }
                Err(e) => return Err(decode_error(e)),
            };
            if packet.track_id() != self.track {
                continue;
            }
            if packet.ts().saturating_add(packet.block_dur()) <= self.decode_from as u64 {
                continue;
            }
            operation();
            let decoded = self.decoder.decode(&packet).map_err(|e| {
                decode_error(format!(
                    "packet decode ts={} dur={}: {e}",
                    packet.ts(),
                    packet.dur()
                ))
            })?;
            let spec = *decoded.spec();
            if spec.rate != self.info.sample_rate || spec.channels.count() != self.info.channels {
                return Err(decode_error("파일 중간에 오디오 형식이 바뀌었습니다."));
            }
            let mut converted = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
            converted.copy_interleaved_ref(decoded);
            let raw_start = usize::try_from(packet.ts()).map_err(decode_error)?;
            let decoded_frames = converted.samples().len() / self.info.channels;
            let skip = self
                .discard_before
                .saturating_add(self.delay)
                .saturating_sub(raw_start)
                .min(decoded_frames);
            let keep_end = if self.mp3 {
                self.info
                    .frames
                    .saturating_add(self.delay)
                    .saturating_sub(raw_start)
                    .min(decoded_frames)
            } else {
                decoded_frames
            };
            if keep_end <= skip {
                continue;
            }
            let samples: Vec<f32> = converted.samples()
                [skip * self.info.channels..keep_end * self.info.channels]
                .iter()
                .map(|&s| {
                    if s.is_finite() {
                        s
                    } else {
                        self.info.sanitized_samples += 1;
                        0.0
                    }
                })
                .collect();
            if !samples.is_empty() {
                return Ok(Some(Chunk {
                    start: raw_start + skip - self.delay,
                    samples,
                }));
            }
        }
    }
}
