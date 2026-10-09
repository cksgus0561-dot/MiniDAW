//! Worker-only duration-compensated pitch output. Signalsmith first produces
//! target_frames * pitch_factor frames without changing pitch; this bandlimited
//! stage then shifts pitch and returns exactly target_frames. No intermediate
//! PCM file or whole-source buffer, and no change to device-rate SRC.
use crate::error::{AppError, AppResult};
use rubato::{
    audioadapter_buffers::direct::InterleavedSlice, Async, FixedAsync, Resampler,
    SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use std::{
    fs::File,
    io::{BufWriter, Write},
};
const BLOCK: usize = 2048;
fn error(e: impl std::fmt::Display) -> AppError {
    AppError::new("pitch_shift", "Pitch/Time 처리를 완료할 수 없습니다.").detail(e)
}
pub(super) struct PitchWriter {
    file: BufWriter<File>,
    dsp: Option<Async<f64>>,
    input: Vec<f64>,
    output: Vec<f64>,
    fill: usize,
    skip: usize,
    written: u64,
    target: u64,
    bytes: Vec<u8>,
}
impl PitchWriter {
    pub fn new(file: File, target: u64, factor: f64) -> AppResult<Self> {
        let dsp = if factor == 1. {
            None
        } else {
            Some(
                Async::new_sinc(
                    1. / factor,
                    1.,
                    &SincInterpolationParameters {
                        sinc_len: 1024,
                        f_cutoff: None,
                        oversampling_factor: 256,
                        interpolation: SincInterpolationType::Cubic,
                        window: WindowFunction::BlackmanHarris2,
                    },
                    BLOCK,
                    2,
                    FixedAsync::Input,
                )
                .map_err(error)?,
            )
        };
        let skip = dsp.as_ref().map_or(0, Resampler::output_delay);
        let output = vec![0.; dsp.as_ref().map_or(0, |d| d.output_frames_max() * 2)];
        Ok(Self {
            file: BufWriter::with_capacity(128 * 1024, file),
            dsp,
            input: vec![0.; BLOCK * 2],
            output,
            fill: 0,
            skip,
            written: 0,
            target,
            bytes: Vec::with_capacity(BLOCK * 16),
        })
    }
    fn flush_bytes(&mut self) -> AppResult<()> {
        self.file.write_all(&self.bytes).map_err(error)?;
        self.bytes.clear();
        Ok(())
    }
    fn sample(&mut self, x: f32) -> AppResult<()> {
        if !x.is_finite() {
            return Err(error("non-finite PCM"));
        }
        self.bytes.extend_from_slice(&x.to_le_bytes());
        Ok(())
    }
    fn block(&mut self) -> AppResult<()> {
        let input = InterleavedSlice::new(&self.input, 2, BLOCK).map_err(error)?;
        let n = self.output.len() / 2;
        let mut output = InterleavedSlice::new_mut(&mut self.output, 2, n).map_err(error)?;
        let n = self
            .dsp
            .as_mut()
            .expect("pitch DSP")
            .process_into_buffer(&input, &mut output, None)
            .map_err(error)?
            .1;
        let skip = self.skip.min(n);
        self.skip -= skip;
        let count = (n - skip).min((self.target - self.written) as usize);
        for i in skip * 2..(skip + count) * 2 {
            self.sample(self.output[i] as f32)?;
        }
        self.written += count as u64;
        self.fill = 0;
        self.flush_bytes()
    }
    pub fn write(&mut self, samples: &[f32]) -> AppResult<()> {
        if self.dsp.is_none() {
            for &s in samples {
                self.sample(s)?;
            }
            self.written += samples.len() as u64 / 2;
            return self.flush_bytes();
        }
        for frame in samples.as_chunks::<2>().0 {
            self.input[self.fill * 2] = frame[0] as f64;
            self.input[self.fill * 2 + 1] = frame[1] as f64;
            self.fill += 1;
            if self.fill == BLOCK {
                self.block()?;
            }
        }
        Ok(())
    }
    pub fn finish(mut self) -> AppResult<()> {
        // Drain the finite FIR latency with silence, bounded independently of
        // event length. Only the exact requested output sample count is stored.
        if self.dsp.is_some() {
            for _ in 0..4 {
                if self.written >= self.target {
                    break;
                }
                self.input[self.fill * 2..].fill(0.);
                self.block()?;
            }
        }
        if self.written != self.target {
            return Err(error("pitch output length"));
        }
        self.file.flush().map_err(error)?;
        self.file.get_ref().sync_all().map_err(error)
    }
}
