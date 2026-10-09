//! Worker-only fixed-rate SRC. No resampler or decoder is reachable from render().
//! Output indices are absolute on the device-rate grid, including across seeks.
use super::{
    reader::{AudioReader, Cancel, Chunk},
    source::{AssetStorage, AudioAsset},
};
use crate::error::{AppError, AppResult};
use rubato::{
    audioadapter_buffers::direct::InterleavedSlice, Fft, FixedSync, Resampler, WindowFunction,
};
use std::sync::Arc;

pub const CHUNK_HINT: usize = 512;
// A corrupted/nonstandard coprime rate must not request an unbounded FFT plan.
const MAX_BLOCK_FRAMES: usize = 65_536;
pub fn output_frames(frames: usize, input_rate: u32, output_rate: u32) -> usize {
    (frames as u128 * output_rate as u128).div_ceil(input_rate as u128) as usize
}

struct PcmReader {
    // Close the file before releasing its last asset lease (Windows deletion).
    rendered: Option<std::io::BufReader<std::fs::File>>,
    asset: Arc<AudioAsset>,
    reader: Option<AudioReader>,
    chunk: Option<Chunk>,
    offset: usize,
    position: usize,
}
impl PcmReader {
    fn new(asset: Arc<AudioAsset>, position: usize, cancel: Cancel) -> AppResult<Self> {
        let reader = match &asset.storage {
            AssetStorage::Memory(_) | AssetStorage::Rendered(_) => None,
            AssetStorage::File(path) => {
                let mut r = AudioReader::for_asset(path, Some(cancel), &asset.info)?;
                if r.info.sample_rate != asset.info.sample_rate
                    || r.info.channels != asset.info.channels
                {
                    return Err(AppError::stream_read(
                        "불러온 후 원본 오디오 형식이 변경되었습니다.",
                    ));
                }
                if position > 0 && position < asset.info.frames {
                    r.seek(position)?;
                }
                Some(r)
            }
        };
        let rendered = if let AssetStorage::Rendered(file) = &asset.storage {
            Some(super::stretch::open(file, position)?)
        } else {
            None
        };
        Ok(Self {
            rendered,
            asset,
            reader,
            chunk: None,
            offset: 0,
            position,
        })
    }
    fn read_frame(&mut self) -> AppResult<[f64; 2]> {
        if self.position >= self.asset.info.frames {
            return Ok([0.0; 2]);
        }
        if let Some(reader) = &mut self.rendered {
            super::reader::operation();
            use std::io::Read;
            let mut bytes = [0u8; 8];
            reader
                .read_exact(&mut bytes)
                .map_err(AppError::stream_read)?;
            self.position += 1;
            return Ok([
                f32::from_le_bytes(bytes[..4].try_into().unwrap()) as f64,
                f32::from_le_bytes(bytes[4..].try_into().unwrap()) as f64,
            ]);
        }
        let ch = self.asset.info.channels;
        let samples = match &self.asset.storage {
            AssetStorage::Memory(audio) => &audio.samples[self.position * ch..],
            AssetStorage::Rendered(_) => unreachable!("handled above"),
            AssetStorage::File(_) => {
                if self
                    .chunk
                    .as_ref()
                    .is_none_or(|c| self.offset >= c.samples.len())
                {
                    self.chunk = self.reader.as_mut().expect("file reader").read_chunk()?;
                    self.offset = 0;
                    let c = self
                        .chunk
                        .as_ref()
                        .ok_or_else(|| AppError::stream_read("예상보다 일찍 파일이 끝났습니다."))?;
                    if c.start != self.position {
                        return Err(AppError::stream_read(format!(
                            "오디오 프레임 불연속: {} → {}",
                            self.position, c.start
                        )));
                    }
                }
                &self.chunk.as_ref().expect("checked chunk").samples[self.offset..]
            }
        };
        let result = [samples[0] as f64, samples[ch - 1] as f64];
        self.offset += ch;
        self.position += 1;
        Ok(result)
    }
}

pub struct PlaybackReader {
    asset: Arc<AudioAsset>,
    pcm: Option<PcmReader>,
    dsp: Option<Fft<f64>>,
    input: Vec<f64>,
    output: Vec<f64>,
    available: usize,
    offset: usize,
    skip: usize,
    cancel: Option<Cancel>,
}
impl PlaybackReader {
    pub fn new(asset: Arc<AudioAsset>, rate: u32) -> AppResult<Self> {
        let input_rate = asset.info.sample_rate as usize;
        let output_rate = rate as usize;
        if input_rate == 0 || output_rate == 0 {
            return Err(AppError::new("src", "sample rate는 0보다 커야 합니다."));
        }
        let dsp = if rate == asset.info.sample_rate {
            None
        } else {
            let (mut a, mut b) = (input_rate, output_rate);
            while b != 0 {
                (a, b) = (b, a % b);
            }
            // Even input AND output blocks make the FIR centre and output_delay
            // integral; otherwise Rubato's integer delay can leave a half-sample
            // phase offset for rate pairs such as 12 kHz -> 44.1 kHz.
            let blocks = CHUNK_HINT.div_ceil(input_rate / a).next_multiple_of(2);
            if blocks * (input_rate / a).max(output_rate / a) > MAX_BLOCK_FRAMES {
                return Err(AppError::new(
                    "src",
                    "이 sample rate 조합은 SRC 버퍼 한도를 초과합니다.",
                )
                .detail(format!("{input_rate} → {output_rate} Hz")));
            }
            Some(
                Fft::new_custom(
                    asset.info.sample_rate as usize,
                    rate as usize,
                    blocks * (input_rate / a),
                    1,
                    2,
                    WindowFunction::BlackmanHarris2,
                    FixedSync::Both,
                )
                .map_err(|e| {
                    AppError::new("src", "출력 sample rate 변환을 준비할 수 없습니다.").detail(e)
                })?,
            )
        };
        let input = vec![0.0; dsp.as_ref().map_or(0, |r| r.input_frames_max() * 2)];
        let output = vec![0.0; dsp.as_ref().map_or(0, |r| r.output_frames_max() * 2)];
        Ok(Self {
            asset,
            pcm: None,
            dsp,
            input,
            output,
            available: 0,
            offset: 0,
            skip: 0,
            cancel: None,
        })
    }
    pub fn seek(&mut self, target: usize, cancel: Cancel) -> AppResult<()> {
        let start = if let Some(dsp) = &mut self.dsp {
            dsp.reset();
            let ni = dsp.input_frames_next();
            let no = dsp.output_frames_next();
            // Two prior globally aligned blocks exceed the finite overlap/filter
            // history. Reset phase and output are identical to sequential rendering.
            let block = (target / no).saturating_sub(2);
            self.skip = target - block * no + dsp.output_delay();
            block * ni
        } else {
            self.skip = 0;
            target
        };
        self.pcm = Some(PcmReader::new(self.asset.clone(), start, cancel.clone())?);
        self.cancel = Some(cancel);
        self.available = 0;
        self.offset = 0;
        Ok(())
    }
    pub fn read_frame(&mut self) -> AppResult<[f32; 2]> {
        let pcm = self.pcm.as_mut().expect("seek before reading");
        let Some(dsp) = &mut self.dsp else {
            return pcm.read_frame().map(|f| [f[0] as f32, f[1] as f32]);
        };
        loop {
            if self.offset >= self.available {
                if self.cancel.as_ref().is_some_and(|c| c()) {
                    return Err(AppError::stream_read("SRC 요청 취소"));
                }
                for frame in self.input.as_chunks_mut::<2>().0 {
                    frame.copy_from_slice(&pcm.read_frame()?);
                }
                let input = InterleavedSlice::new(&self.input, 2, self.input.len() / 2)
                    .expect("fixed input");
                let n = self.output.len() / 2;
                let mut output =
                    InterleavedSlice::new_mut(&mut self.output, 2, n).expect("fixed output");
                self.available = dsp
                    .process_into_buffer(&input, &mut output, None)
                    .map_err(AppError::stream_read)?
                    .1;
                self.offset = 0;
            }
            let skipped = self.skip.min(self.available - self.offset);
            self.offset += skipped;
            self.skip -= skipped;
            if self.offset < self.available {
                let i = self.offset * 2;
                self.offset += 1;
                // Preserve headroom. f64 DSP prevents overflow on finite f32 input;
                // only bound to representable f32 here, device clipping is later.
                let convert = |x: f64| x.clamp(-(f32::MAX as f64), f32::MAX as f64) as f32;
                return Ok([convert(self.output[i]), convert(self.output[i + 1])]);
            }
        }
    }
}
