//! Offline stereo pitch/time processing. Bounded PCM blocks, worker-only decoding/DSP/I/O.
//! Disposable raw f32 files avoid RIFF's 4 GiB limit. No PCM/cache path is saved
//! in the project. Active assets keep their files alive across plan replacement.
use super::{
    resample::PlaybackReader,
    source::{AssetStorage, AudioAsset},
};
use crate::{
    error::{AppError, AppResult},
    project::stretch::Recipe,
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    ffi::c_void,
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
    time::Instant,
};
unsafe extern "C" {
    fn minidaw_stretch_new(rate: u32, precise: bool) -> *mut c_void;
    fn minidaw_stretch_delete(p: *mut c_void);
    fn minidaw_stretch_prime_length(p: *mut c_void, rate: f64) -> i32;
    fn minidaw_stretch_seek(p: *mut c_void, input: *const f32, n: i32, rate: f64) -> bool;
    fn minidaw_stretch_process(
        p: *mut c_void,
        input: *const f32,
        ni: i32,
        output: *mut f32,
        no: i32,
    ) -> bool;
    fn minidaw_stretch_flush(p: *mut c_void, output: *mut f32, n: i32, rate: f32) -> bool;
}
struct Dsp(*mut c_void);
impl Drop for Dsp {
    fn drop(&mut self) {
        unsafe { minidaw_stretch_delete(self.0) }
    }
}
pub struct RenderedFile {
    pub path: PathBuf,
    // Windows delete-on-close also removes caches after process termination.
    // Readers use the default FILE_SHARE_DELETE and hold the asset lease.
    _lease: File,
}
impl Drop for RenderedFile {
    fn drop(&mut self) {
        super::reader::operation();
        let _ = std::fs::remove_file(&self.path);
    }
}
fn error(e: impl std::fmt::Display) -> AppError {
    AppError::new("time_stretch", "Pitch/Time 처리를 준비할 수 없습니다.").detail(e)
}
fn checked(ok: bool) -> AppResult<()> {
    if ok {
        Ok(())
    } else {
        Err(error("Signalsmith 처리 실패"))
    }
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderStats {
    pub frames: u64,
    pub elapsed_ms: f64,
    pub block_frames: usize,
}
/// Public for offline audits. The caller owns the destination; a partial file
/// must never be installed into a playback plan.
pub fn render_to(asset: Arc<AudioAsset>, recipe: &Recipe, path: &Path) -> AppResult<RenderStats> {
    render_pitched_to(asset, recipe, 0, path)
}
pub fn render_pitched_to(
    asset: Arc<AudioAsset>,
    recipe: &Recipe,
    cents: i16,
    path: &Path,
) -> AppResult<RenderStats> {
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(error)?;
    render_file(asset, recipe, cents, file)
}
fn render_file(
    asset: Arc<AudioAsset>,
    recipe: &Recipe,
    cents: i16,
    file: File,
) -> AppResult<RenderStats> {
    super::reader::operation();
    let began = Instant::now();
    let n = recipe
        .source_end
        .0
        .checked_sub(recipe.source_start.0)
        .filter(|n| *n > 0)
        .ok_or_else(|| error("source window"))?;
    let target = recipe.output_frames.0;
    let ratio = target as f64 / n as f64;
    if !(-1300..=1300).contains(&cents) {
        return Err(error("Pitch range ±1300 cent"));
    }
    let epsilon = 0.5 / n as f64 + 1e-12;
    if target == 0 || !(0.125-epsilon..=8.+epsilon).contains(&ratio) || recipe.source_end.0 > asset.info.frames as u64 {
        return Err(error("12.5–800% / source window"));
    }
    let factor = 2f64.powf(cents as f64 / 1200.);
    let m = ((target as f64 * factor).round() as u64).max(1);
    let compensated = m as f64 / n as f64;
    if !(0.5..=2.).contains(&compensated) {
        // Large combined settings are cascaded through the already validated
        // clean TSM range, rather than asking one phase-vocoder to stretch 4x.
        // The intermediate is a bounded-memory disposable file, not project data.
        let middle = ((n as f64 * compensated.sqrt().clamp(0.5, 2.)).round() as u64)
            .clamp(n.div_ceil(2), n * 2);
        let stage = Recipe {
            source_start: recipe.source_start,
            source_end: recipe.source_end,
            output_frames: crate::project::schema::Frames(middle),
        };
        let intermediate = render_asset(asset.clone(), &stage, 0)?;
        let next = Recipe {
            source_start: crate::project::schema::Frames(0),
            source_end: crate::project::schema::Frames(middle),
            output_frames: recipe.output_frames,
        };
        let mut stats = render_file(intermediate, &next, cents, file)?;
        stats.elapsed_ms = began.elapsed().as_secs_f64() * 1000.;
        return Ok(stats);
    }
    let mut source = PlaybackReader::new(asset.clone(), asset.info.sample_rate)?;
    source.seek(recipe.source_start.0 as usize, Arc::new(|| false))?;
    let mut remaining = n;
    let mut input = Vec::new();
    let mut read = |count: usize, input: &mut Vec<f32>| -> AppResult<()> {
        input.clear();
        for _ in 0..count {
            let f = if remaining > 0 {
                remaining -= 1;
                source.read_frame()?
            } else {
                [0.; 2]
            };
            input.extend_from_slice(&f);
        }
        Ok(())
    };
    let mut writer = super::pitch::PitchWriter::new(file, target, factor)?;
    let mut written = 0u64;
    let mut write = |samples: &[f32]| -> AppResult<()> {
        writer.write(samples)?;
        written += samples.len() as u64 / 2;
        Ok(())
    };
    const BLOCK: usize = 2048;
    if n == m {
        // Unity is transparent, including transients and very short selections.
        for at in (0..n).step_by(BLOCK) {
            read((n - at).min(BLOCK as u64) as usize, &mut input)?;
            write(&input)?;
        }
    } else {
        let dsp = Dsp(unsafe { minidaw_stretch_new(asset.info.sample_rate, cents != 0) });
        if dsp.0.is_null() {
            return Err(error("FFT memory"));
        }
        let rate = n as f64 / m as f64;
        let prime = unsafe { minidaw_stretch_prime_length(dsp.0, rate) } as usize;
        read(prime, &mut input)?;
        checked(unsafe { minidaw_stretch_seek(dsp.0, input.as_ptr(), prime as i32, rate) })?;
        // Streaming equivalent of the library's exact() API, including its
        // mirrored boundary correction. Never materialize the complete input.
        let body_input = n.saturating_sub(prime as u64);
        let body_output = ((body_input as u128 * m as u128) / n as u128) as u64;
        let tail = (m - body_output) as usize;
        let mut output = vec![0.; 2 * BLOCK.max(tail)];
        let mut previous = 0u64;
        for at in (0..body_output).step_by(BLOCK) {
            let count = (body_output - at).min(BLOCK as u64) as usize;
            let endpoint =
                ((at + count as u64) as u128 * body_input as u128 / body_output as u128) as u64;
            let ni = (endpoint - previous) as usize;
            previous = endpoint;
            read(ni, &mut input)?;
            checked(unsafe {
                minidaw_stretch_process(
                    dsp.0,
                    input.as_ptr(),
                    ni as i32,
                    output.as_mut_ptr(),
                    count as i32,
                )
            })?;
            write(&output[..count * 2])?;
        }
        checked(unsafe {
            minidaw_stretch_flush(dsp.0, output.as_mut_ptr(), tail as i32, rate as f32)
        })?;
        write(&output[..tail * 2])?;
    }
    if written != m {
        return Err(error("output length"));
    }
    writer.finish()?;
    Ok(RenderStats {
        frames: target,
        elapsed_ms: began.elapsed().as_secs_f64() * 1000.,
        block_frames: BLOCK,
    })
}
type Cache = Mutex<HashMap<String, Weak<AudioAsset>>>;
static CACHE: OnceLock<Cache> = OnceLock::new();
pub fn prepare(
    asset: Arc<AudioAsset>,
    recipe: &Recipe,
    identity: &str,
) -> AppResult<Arc<AudioAsset>> {
    prepare_pitched(asset, recipe, 0, identity)
}
pub fn prepare_pitched(
    asset: Arc<AudioAsset>,
    recipe: &Recipe,
    cents: i16,
    identity: &str,
) -> AppResult<Arc<AudioAsset>> {
    let mut hash = Sha256::new();
    hash.update(
        b"signalsmith-a670068-linear-146b26f-f64-160ms-20ms-precise-cascade-sinc1024-cubic256-v4",
    );
    hash.update(identity);
    hash.update(cents.to_le_bytes());
    hash.update((Arc::as_ptr(&asset) as usize).to_le_bytes());
    hash.update(serde_json::to_vec(recipe).map_err(error)?);
    hash.update(asset.info.sample_rate.to_le_bytes());
    hash.update(asset.info.channels.to_le_bytes());
    if let AssetStorage::File(path) = &asset.storage {
        hash.update(path.to_string_lossy().as_bytes());
        let meta = std::fs::metadata(path).map_err(error)?;
        hash.update(meta.len().to_le_bytes());
        if let Ok(t) = meta.modified() {
            hash.update(format!("{t:?}"));
        }
    }
    let key = format!("{:x}", hash.finalize());
    // This lock is never reachable from the callback. Serializing preparation
    // avoids duplicate multi-gigabyte renders; only live plans retain PCM files.
    let mut cache = CACHE.get_or_init(Default::default).lock().map_err(error)?;
    if let Some(cached) = cache.get(&key).and_then(Weak::upgrade) {
        return Ok(cached);
    }
    cache.retain(|_, v| v.strong_count() > 0);
    let result = render_asset(asset, recipe, cents)?;
    cache.insert(key, Arc::downgrade(&result));
    Ok(result)
}
fn render_asset(asset: Arc<AudioAsset>, recipe: &Recipe, cents: i16) -> AppResult<Arc<AudioAsset>> {
    let path = std::env::temp_dir().join(format!(
        "minidaw-stretch-{}-{}.f32",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let mut options = OpenOptions::new();
    options.create_new(true).read(true).write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x04000000); // FILE_FLAG_DELETE_ON_CLOSE
    }
    let lease = options.open(&path).map_err(error)?;
    let file = Arc::new(RenderedFile {
        path,
        _lease: lease,
    });
    render_file(
        asset.clone(),
        recipe,
        cents,
        file._lease.try_clone().map_err(error)?,
    )?;
    let mut info = asset.info.clone();
    info.frames = usize::try_from(recipe.output_frames.0).map_err(error)?;
    info.channels = 2;
    info.duration = info.frames as f64 / info.sample_rate as f64;
    Ok(Arc::new(AudioAsset {
        info,
        storage: AssetStorage::Rendered(file),
    }))
}
// A separate handle is opened on a decoding worker, never on the callback.
pub fn open(file: &RenderedFile, frame: usize) -> AppResult<std::io::BufReader<File>> {
    super::reader::operation();
    use std::io::{Seek, SeekFrom};
    let mut reader =
        std::io::BufReader::with_capacity(64 * 1024, File::open(&file.path).map_err(error)?);
    reader
        .seek(SeekFrom::Start(frame as u64 * 8))
        .map_err(error)?;
    Ok(reader)
}
