//! Read-only offline export. Dedicated worker, bounded decoding/writing,
//! independent DSP state, cancellable render, atomic destination commit.
use super::{paths, schema::*, session::AssetState};
use crate::{
    audio::{offline::OfflineMaster, source::AudioAsset, timeline::PlaybackPlan},
    error::{AppError, AppResult},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{BufWriter, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering::*},
        Arc, Mutex,
    },
    time::Instant,
};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum WaveFormat {
    Pcm16,
    Pcm24,
    Float32,
}
impl WaveFormat {
    fn bytes(self) -> u64 {
        match self {
            Self::Pcm16 => 2,
            Self::Pcm24 => 3,
            Self::Float32 => 4,
        }
    }
    fn header_bytes(self) -> u64 {
        if self == Self::Float32 {
            56
        } else {
            44
        }
    }
    pub fn max_frames(self) -> u64 {
        (u32::MAX as u64 - (self.header_bytes() - 8)) / (2 * self.bytes())
    }
}
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    #[default]
    Mixdown,
    Selection,
    Track,
    Stems,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FrameRange {
    pub start: Frames,
    pub end: Frames,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Request {
    pub job_id: String,
    pub revision: u64,
    pub path: String,
    pub format: WaveFormat,
    pub sample_rate: u32,
    #[serde(default)]
    pub overwrite: bool,
    #[serde(default)]
    pub mode: Mode,
    #[serde(default)]
    pub track_ids: Vec<String>,
    pub range: Option<FrameRange>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportFile {
    pub path: String,
    pub track_id: Option<String>,
    pub track_name: Option<String>,
    pub start_frame: u64,
    pub end_frame: u64,
    pub frames: u64,
    pub clipped_samples: u64,
    pub peak: f32,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub path: String,
    pub sample_rate: u32,
    pub format: WaveFormat,
    pub frames: u64,
    pub seconds: f64,
    pub elapsed_ms: f64,
    pub render_ms: f64,
    pub realtime_factor: f64,
    pub clipped_samples: u64,
    pub peak: f32,
    pub files: Vec<ExportFile>,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub job_id: String,
    pub stage: String,
    pub progress: f64,
    pub frames: u64,
    pub timeline_frames: u64,
    pub file_index: usize,
    pub file_count: usize,
    pub target: String,
    pub report: Option<Report>,
    pub error: Option<AppError>,
}
impl Default for Status {
    fn default() -> Self {
        Self {
            job_id: String::new(),
            stage: "idle".into(),
            progress: 0.,
            frames: 0,
            timeline_frames: 0,
            file_index: 0,
            file_count: 0,
            target: String::new(),
            report: None,
            error: None,
        }
    }
}
#[derive(Default)]
pub struct ExportService {
    state: Mutex<Status>,
    running: AtomicBool,
    cancel: Arc<AtomicBool>,
}
struct Running<'a>(&'a AtomicBool);
impl Drop for Running<'_> {
    fn drop(&mut self) {
        self.0.store(false, Release);
    }
}
impl ExportService {
    pub fn status(&self) -> Status {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    pub fn cancel(&self, id: &str) {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if s.job_id == id && self.running.load(Acquire) {
            self.cancel.store(true, Release);
        }
    }
    pub fn run(
        &self,
        r: Request,
        p: Project,
        states: Vec<AssetState>,
        project_path: Option<PathBuf>,
    ) -> AppResult<Report> {
        uuid::Uuid::parse_str(&r.job_id).map_err(fail)?;
        if self
            .running
            .compare_exchange(false, true, AcqRel, Acquire)
            .is_err()
        {
            return Err(AppError::new("export_busy", "다른 Export가 진행 중입니다."));
        }
        let _running = Running(&self.running);
        self.cancel.store(false, Release);
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = Status {
            job_id: r.job_id.clone(),
            stage: "preparing".into(),
            ..Default::default()
        };
        let result = super::export_targets::render(
            &r,
            &p,
            &states,
            project_path.as_deref(),
            self.cancel.clone(),
            |stage, frames, total, index, count, target| {
                let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
                s.stage = stage.into();
                s.frames = frames;
                s.timeline_frames = total;
                s.file_index = index;
                s.file_count = count;
                s.target = target.into();
                let part = if stage == "finalizing" {
                    0.99
                } else if total > 0 {
                    (frames as f64 / total as f64).min(1.) * 0.95
                } else {
                    0.
                };
                s.progress = s.progress.max((index.saturating_sub(1) as f64 + part) / count.max(1) as f64);
            },
        );
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match &result {
            Ok(report) => {
                s.stage = "completed".into();
                s.progress = 1.;
                s.frames = report.frames;
                s.report = Some(report.clone());
            }
            Err(e) => {
                s.stage = if e.code == "export_cancelled" {
                    "cancelled"
                } else {
                    "failed"
                }
                .into();
                s.error = Some(e.clone());
            }
        }
        result
    }
}
pub(super) fn fail(e: impl std::fmt::Display) -> AppError {
    AppError::new("export_audio", "WAV Export를 완료하지 못했습니다.").detail(e)
}
pub(super) fn cancelled(cancel: &AtomicBool) -> AppResult<()> {
    if cancel.load(Acquire) {
        Err(AppError::new("export_cancelled", "Export를 취소했습니다."))
    } else {
        Ok(())
    }
}
fn same(a: &Path, b: &Path) -> bool {
    paths::display(a).eq_ignore_ascii_case(&paths::display(b))
}
struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub fn render(
    r: &Request,
    p: &Project,
    states: &[AssetState],
    project_path: Option<&Path>,
    cancel: Arc<AtomicBool>,
    mut progress: impl FnMut(&str, u64, u64),
) -> AppResult<Report> {
    super::export_targets::render(r, p, states, project_path, cancel, |s, f, t, _, _, _| {
        progress(s, f, t)
    })
}
#[allow(clippy::too_many_arguments)]
pub(super) fn render_one(
    r: &Request,
    p: &Project,
    states: &[AssetState],
    project_path: Option<&Path>,
    cancel: Arc<AtomicBool>,
    minimum_frames: u64,
    mut progress: impl FnMut(&str, u64, u64),
) -> AppResult<Report> {
    let _plugins = crate::plugins::OfflineScope::new();
    let began = Instant::now();
    p.validate()?;
    cancelled(&cancel)?;
    if r.mode == Mode::Mixdown && p.tracks.iter().all(|t| t.clips.is_empty()) {
        return Err(AppError::new(
            "export_empty",
            "Export할 Audio/MIDI Event가 없습니다.",
        ));
    }
    let mut target = paths::absolute(Path::new(&r.path))?;
    if target.extension().is_none() {
        target.set_extension("wav");
    }
    if !target
        .extension()
        .is_some_and(|s| s.eq_ignore_ascii_case("wav"))
    {
        return Err(AppError::new(
            "export_extension",
            ".wav 파일로 저장해 주세요.",
        ));
    }
    let parent =
        fs::canonicalize(target.parent().ok_or_else(|| fail("저장 폴더 없음"))?).map_err(fail)?;
    target = Path::new(&paths::display(&parent))
        .join(target.file_name().ok_or_else(|| fail("파일 이름 없음"))?);
    let canonical = fs::canonicalize(&target).unwrap_or_else(|_| target.clone());
    if project_path.is_some_and(|path| {
        same(
            &canonical,
            &fs::canonicalize(path).unwrap_or_else(|_| path.to_owned()),
        )
    }) {
        return Err(fail("프로젝트 파일 위에 Export할 수 없습니다."));
    }
    for a in &p.assets {
        for path in states
            .iter()
            .filter(|s| s.asset_id == a.asset_id)
            .filter_map(|s| s.resolved_path.as_ref())
            .map(PathBuf::from)
            .chain(paths::candidates(&a.path, project_path))
        {
            if same(&canonical, &fs::canonicalize(&path).unwrap_or(path)) {
                return Err(AppError::new(
                    "export_source",
                    "원본 Audio 파일 위에 Export할 수 없습니다.",
                ));
            }
        }
    }
    if target.exists() && !r.overwrite {
        return Err(AppError::new(
            "export_exists",
            "같은 이름의 파일이 있습니다. 덮어쓰기를 선택하거나 다른 이름을 지정하세요.",
        ));
    }
    if target.exists() && !target.is_file() {
        return Err(fail("저장 위치가 일반 파일이 아닙니다."));
    }
    let before = fs::metadata(&target)
        .ok()
        .map(|m| (m.len(), m.modified().ok()));
    let needed: HashSet<_> = p.clips().map(|c| c.asset_id.as_str()).collect();
    let mut assets = HashMap::new();
    let mut identities = vec![];
    for a in p
        .assets
        .iter()
        .filter(|a| needed.contains(a.asset_id.as_str()))
    {
        cancelled(&cancel)?;
        let path = states
            .iter()
            .find(|s| s.asset_id == a.asset_id && s.status == "available")
            .and_then(|s| s.resolved_path.as_ref())
            .ok_or_else(|| {
                AppError::new(
                    "export_missing",
                    format!("원본을 다시 연결해 주세요: {}", a.filename),
                )
            })?;
        let path = PathBuf::from(path);
        let fingerprint = paths::fingerprint(&path)?;
        if fingerprint != a.fingerprint {
            return Err(fail(format!("원본이 변경되었습니다: {}", a.filename)));
        }
        let asset = AudioAsset::open_with_budget(&path, 0)?;
        if asset.info.sample_rate != a.metadata.sample_rate
            || asset.info.frames as u64 != a.metadata.source_frames.0
            || asset.info.channels as u16 != a.metadata.channels
        {
            return Err(fail("원본 오디오 metadata 불일치"));
        }
        identities.push((path, fingerprint));
        assets.insert(a.asset_id.clone(), asset);
    }
    let mut document = p.clone();
    document.cycle = None;
    let mut plan = PlaybackPlan::compile(Arc::new(document), assets, r.sample_rate)?;
    // Retaining one channel must not shorten its automation clock at MIDI EOF.
    // This plan is owned solely by the export worker, never by realtime playback.
    let own_plan = Arc::get_mut(&mut plan).expect("new export plan");
    own_plan.frames = own_plan
        .frames
        .max(usize::try_from(minimum_frames).map_err(fail)?);
    cancelled(&cancel)?;
    let start = r.range.as_ref().map_or(0, |v| v.start.0);
    let end = r.range.as_ref().map(|v| v.end.0);
    if end.unwrap_or(plan.frames as u64).saturating_sub(start) > r.format.max_frames() {
        return Err(fail(
            "전체 Mixdown이 비어 있거나 WAV RIFF의 4 GiB 한도를 초과합니다.",
        ));
    }
    let total = end.unwrap_or((plan.frames as u64).max(minimum_frames));
    let temp = Temporary(parent.join(format!(".minidaw-export-{}.tmp", id())));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp.0)
        .map_err(fail)?;
    let mut wave = WaveWriter::new(file, r.format, r.sample_rate)?;
    let token = cancel.clone();
    let mut mixer = OfflineMaster::new(plan, Arc::new(move || token.load(Acquire)))?;
    let rendering = Instant::now();
    let mut last = Instant::now();
    let mut position = 0u64;
    loop {
        cancelled(&cancel)?;
        for _ in 0..4096 {
            if end.is_some_and(|v| position >= v) {
                break;
            }
            let frame = mixer.next_frame()?;
            if frame.is_none() && end.is_none() && position >= minimum_frames {
                break;
            }
            // Process from project zero, including PDC warm-up and prior MIDI/DSP
            // history; only writing is gated by the exact [start,end) interval.
            if position >= start {
                wave.frame(frame.unwrap_or([0.; 2]))?;
            }
            position += 1;
        }
        crate::plugins::check_offline()?;
        let finished = end.map_or_else(
            || mixer.finished() && position >= minimum_frames,
            |v| position >= v,
        );
        if last.elapsed().as_millis() >= 40 || finished {
            progress(
                if position < start {
                    "preroll"
                } else if position >= total {
                    "tail"
                } else {
                    "rendering"
                },
                position,
                total,
            );
            last = Instant::now();
        }
        if finished {
            break;
        }
    }
    let render_ms = rendering.elapsed().as_secs_f64() * 1000.;
    progress("finalizing", wave.frames, total);
    cancelled(&cancel)?;
    for (path, before) in identities {
        if paths::fingerprint(&path)? != before {
            return Err(fail("Export 도중 원본 파일이 변경되었습니다."));
        }
    }
    let mut report = Report {
        path: paths::display(&target),
        sample_rate: r.sample_rate,
        format: r.format,
        frames: wave.frames,
        seconds: wave.frames as f64 / r.sample_rate as f64,
        elapsed_ms: 0.,
        render_ms,
        realtime_factor: 0.,
        clipped_samples: wave.clipped,
        peak: wave.peak,
        files: vec![ExportFile {
            path: paths::display(&target),
            track_id: None,
            track_name: None,
            start_frame: start,
            end_frame: start + wave.frames,
            frames: wave.frames,
            clipped_samples: wave.clipped,
            peak: wave.peak,
        }],
    };
    wave.finish()?;
    cancelled(&cancel)?;
    if before
        != fs::metadata(&target)
            .ok()
            .map(|m| (m.len(), m.modified().ok()))
    {
        return Err(AppError::new(
            "export_changed",
            "Export 도중 대상 파일이 변경되었습니다. 다른 이름으로 저장해 주세요.",
        ));
    }
    commit(&temp.0, &target, r.overwrite)?;
    report.elapsed_ms = began.elapsed().as_secs_f64() * 1000.;
    report.realtime_factor = report.seconds / (report.elapsed_ms / 1000.);
    Ok(report)
}
#[cfg(windows)]
pub(super) fn commit(from: &Path, to: &Path, replace: bool) -> AppResult<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(from: *const u16, to: *const u16, flags: u32) -> i32;
    }
    let from: Vec<_> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<_> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    if unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 8 | u32::from(replace)) } == 0 {
        return Err(fail(std::io::Error::last_os_error()));
    }
    Ok(())
}
#[cfg(not(windows))]
pub(super) fn commit(from: &Path, to: &Path, replace: bool) -> AppResult<()> {
    if replace {
        fs::rename(from, to)
    } else {
        fs::hard_link(from, to)
    }
    .map_err(fail)
}

pub struct WaveWriter {
    writer: BufWriter<File>,
    format: WaveFormat,
    rate: u32,
    pub frames: u64,
    pub clipped: u64,
    pub peak: f32,
}
impl WaveWriter {
    pub(super) fn pad(report: &mut Report, frames: u64, cancel: &AtomicBool) -> AppResult<()> {
        if frames <= report.frames {
            return Ok(());
        }
        let file = OpenOptions::new()
            .write(true)
            .open(&report.path)
            .map_err(fail)?;
        let mut wave = Self {
            writer: BufWriter::with_capacity(65536, file),
            format: report.format,
            rate: report.sample_rate,
            frames: report.frames,
            clipped: report.clipped_samples,
            peak: report.peak,
        };
        wave.writer.seek(SeekFrom::End(0)).map_err(fail)?;
        while wave.frames < frames {
            if wave.frames.is_multiple_of(4096) {
                cancelled(cancel)?;
            }
            wave.frame([0.; 2])?;
        }
        wave.finish()?;
        report.frames = frames;
        report.seconds = frames as f64 / report.sample_rate as f64;
        for f in &mut report.files {
            f.frames = frames;
            f.end_frame = f.start_frame + frames;
        }
        Ok(())
    }
    pub fn new(file: File, format: WaveFormat, rate: u32) -> AppResult<Self> {
        if !(8000..=384000).contains(&rate) {
            return Err(fail("sample rate 범위"));
        }
        let mut s = Self {
            writer: BufWriter::with_capacity(65536, file),
            format,
            rate,
            frames: 0,
            clipped: 0,
            peak: 0.,
        };
        s.header()?;
        Ok(s)
    }
    pub fn frame(&mut self, frame: [f32; 2]) -> AppResult<()> {
        if self.frames >= self.format.max_frames() {
            return Err(fail("WAV RIFF의 4 GiB 한도를 초과했습니다."));
        }
        for v in frame {
            if !v.is_finite() {
                return Err(fail("Mixdown 출력에 유효하지 않은 sample이 있습니다."));
            }
            self.peak = self.peak.max(v.abs());
            let (scale, maximum) = match self.format {
                WaveFormat::Pcm16 => (32768., 32767.),
                WaveFormat::Pcm24 => (8388608., 8388607.),
                WaveFormat::Float32 => {
                    self.writer.write_all(&v.to_le_bytes()).map_err(fail)?;
                    continue;
                }
            };
            let raw = (v as f64 * scale).round();
            self.clipped += u64::from(raw < -scale || raw > maximum);
            let v = raw.clamp(-scale, maximum) as i32;
            match self.format {
                WaveFormat::Pcm16 => self.writer.write_all(&(v as i16).to_le_bytes()),
                WaveFormat::Pcm24 => self.writer.write_all(&v.to_le_bytes()[..3]),
                _ => unreachable!(),
            }
            .map_err(fail)?;
        }
        self.frames += 1;
        Ok(())
    }
    fn header(&mut self) -> AppResult<()> {
        let bytes = (self.frames * 2 * self.format.bytes()) as u32;
        let mut h = Vec::with_capacity(56);
        h.extend(b"RIFF");
        h.extend((bytes + (self.format.header_bytes() - 8) as u32).to_le_bytes());
        h.extend(b"WAVEfmt ");
        h.extend(16u32.to_le_bytes());
        h.extend(
            (if self.format == WaveFormat::Float32 {
                3u16
            } else {
                1u16
            })
            .to_le_bytes(),
        );
        h.extend(2u16.to_le_bytes());
        h.extend(self.rate.to_le_bytes());
        h.extend((self.rate * 2 * self.format.bytes() as u32).to_le_bytes());
        h.extend((2 * self.format.bytes() as u16).to_le_bytes());
        h.extend((self.format.bytes() as u16 * 8).to_le_bytes());
        if self.format == WaveFormat::Float32 {
            h.extend(b"fact");
            h.extend(4u32.to_le_bytes());
            h.extend((self.frames as u32).to_le_bytes());
        }
        h.extend(b"data");
        h.extend(bytes.to_le_bytes());
        self.writer.write_all(&h).map_err(fail)
    }
    pub fn finish(mut self) -> AppResult<()> {
        self.writer.seek(SeekFrom::Start(0)).map_err(fail)?;
        self.header()?;
        self.writer.flush().map_err(fail)?;
        self.writer.get_ref().sync_all().map_err(fail)
    }
}
