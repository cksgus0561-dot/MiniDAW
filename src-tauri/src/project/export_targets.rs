//! Export-only snapshots. Never mutate the session, realtime routing or history.
use super::{
    export::{self, cancelled, fail, Mode, Report, Request, WaveWriter},
    paths,
    schema::*,
    session::AssetState,
    time,
};
use crate::error::{AppError, AppResult};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::{atomic::AtomicBool, Arc},
    time::Instant,
};

fn invalid(detail: &str) -> AppError {
    AppError::new("export_target", "Export 대상과 시간 구간을 확인해 주세요.").detail(detail)
}

fn validate(r: &Request, p: &Project) -> AppResult<()> {
    p.validate()?;
    if !(8000..=384000).contains(&r.sample_rate) {
        return Err(invalid("sample rate"));
    }
    if let Some(v) = &r.range {
        if v.end.0 <= v.start.0
            || v.end.0 > 9_007_199_254_740_991
            || v.end.0 - v.start.0 > r.format.max_frames()
        {
            return Err(invalid("Invalid sample interval / WAV RIFF size limit"));
        }
    }
    let unique: HashSet<_> = r.track_ids.iter().collect();
    if unique.len() != r.track_ids.len()
        || r.track_ids
            .iter()
            .any(|id| !p.tracks.iter().any(|t| &t.track_id == id))
    {
        return Err(invalid("Missing or duplicate Track"));
    }
    match r.mode {
        Mode::Mixdown if !r.track_ids.is_empty() || r.range.is_some() => {
            Err(invalid("Mixdown uses the entire project"))
        }
        Mode::Selection if !r.track_ids.is_empty() || r.range.is_none() => {
            Err(invalid("Select an Arrangement time range"))
        }
        Mode::Track if r.track_ids.len() != 1 => Err(invalid("Select one Track")),
        Mode::Stems if r.track_ids.is_empty() => Err(invalid("Select at least one Track")),
        _ => Ok(()),
    }
}

fn project_end(p: &Project, rate: u32) -> AppResult<u64> {
    let mut end = 0;
    for c in p.tracks.iter().flat_map(|t| &t.clips) {
        let time = match c {
            Clip::Audio(c) => super::tempo_sync::end(
                c,
                &p.musical_time,
                p.assets
                    .iter()
                    .find(|a| a.asset_id == c.asset_id)
                    .expect("validated")
                    .metadata
                    .sample_rate,
            )?,
            Clip::Midi(c) => time::time(
                &Position::Ticks {
                    ticks: Signed(c.start_tick.0 + c.length_tick.0),
                },
                &p.musical_time,
            ),
        };
        end = end.max(u64::try_from(time.ceil_frame(rate)).map_err(fail)?);
    }
    Ok(end)
}

fn channel(p: &Project, id: &str) -> Project {
    let mut doc = p.clone();
    doc.tracks.retain(|t| t.track_id == id);
    // Explicit channel export is independent of other channels' Solo buttons.
    // The selected channel's Mute, fader, pan, inserts and Read curves remain.
    doc.tracks[0].mix.solo = false;
    doc.master = MasterMix::default();
    doc.automation.retain(|c| c.track_id == id);
    doc.primary_clip_id = None;
    doc.cycle = None;
    doc
}

fn filename(name: &str) -> String {
    let result: String = name
        .chars()
        .take(70)
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let result = result.trim().trim_end_matches(['.', ' ']);
    if result.is_empty() {
        "Untitled".into()
    } else {
        result.into()
    }
}

// Only our freshly created staging directory/files are cleaned up. No recursive
// deletion, and no existing user directory is ever used as a staging area.
struct Batch {
    directory: PathBuf,
    files: Vec<PathBuf>,
}
impl Drop for Batch {
    fn drop(&mut self) {
        for p in &self.files {
            let _ = fs::remove_file(p);
        }
        let _ = fs::remove_dir(&self.directory);
    }
}

pub(super) fn render(
    r: &Request,
    p: &Project,
    states: &[AssetState],
    project_path: Option<&Path>,
    cancel: Arc<AtomicBool>,
    mut progress: impl FnMut(&str, u64, u64, usize, usize, &str),
) -> AppResult<Report> {
    validate(r, p)?;
    cancelled(&cancel)?;
    let began = Instant::now();
    if matches!(r.mode, Mode::Mixdown | Mode::Selection) {
        return export::render_one(r, p, states, project_path, cancel, 0, |s, f, t| {
            progress(s, f, t, 1, 1, "Master")
        });
    }
    let minimum = project_end(p, r.sample_rate)?;
    if r.range.is_none() && minimum == 0 {
        return Err(invalid("No project duration to export"));
    }
    if r.range.is_none() && minimum > r.format.max_frames() {
        return Err(invalid("Project exceeds WAV RIFF size limit"));
    }
    if r.mode == Mode::Track {
        let doc = channel(p, &r.track_ids[0]);
        let mut report =
            export::render_one(r, &doc, states, project_path, cancel, minimum, |s, f, t| {
                progress(s, f, t, 1, 1, &doc.tracks[0].name)
            })?;
        report.files[0].track_id = Some(doc.tracks[0].track_id.clone());
        report.files[0].track_name = Some(doc.tracks[0].name.clone());
        return Ok(report);
    }
    let parent = fs::canonicalize(paths::absolute(Path::new(&r.path))?).map_err(fail)?;
    if !parent.is_dir() {
        return Err(invalid("Choose a destination folder for Stems"));
    }
    let staging = parent.join(format!(".minidaw-stems-{}.tmp", id()));
    fs::create_dir(&staging).map_err(fail)?;
    let mut batch = Batch {
        directory: staging,
        files: vec![],
    };
    let tracks: Vec<_> = p
        .tracks
        .iter()
        .filter(|t| r.track_ids.contains(&t.track_id))
        .collect();
    let count = tracks.len();
    let mut reports = vec![];
    for (i, track) in tracks.iter().enumerate() {
        cancelled(&cancel)?;
        let path = batch
            .directory
            .join(format!("{:02} {}.wav", i + 1, filename(&track.name)));
        batch.files.push(path.clone());
        let doc = channel(p, &track.track_id);
        let mut one = r.clone();
        one.path = paths::display(&path);
        one.overwrite = false;
        progress("preparing", 0, minimum, i + 1, count, &track.name);
        let mut report = export::render_one(
            &one,
            &doc,
            states,
            project_path,
            cancel.clone(),
            minimum,
            |s, f, t| progress(s, f, t, i + 1, count, &track.name),
        )?;
        report.files[0].track_id = Some(track.track_id.clone());
        report.files[0].track_name = Some(track.name.clone());
        reports.push(report);
    }
    let frames = reports.iter().map(|r| r.frames).max().unwrap_or(0);
    if frames == 0 {
        return Err(invalid("No project duration to export"));
    }
    for (i, report) in reports.iter_mut().enumerate() {
        progress("finalizing", frames, frames, i + 1, count, &tracks[i].name);
        WaveWriter::pad(report, frames, &cancel)?;
    }
    cancelled(&cancel)?;
    // Rename the completed batch as one unit. A collision gets a new folder;
    // no replacement flag is ever used, including races with external writers.
    let mut suffix = 1u64;
    let target = loop {
        cancelled(&cancel)?;
        let tail = if suffix == 1 {
            String::new()
        } else {
            format!(" ({suffix})")
        };
        let target = parent.join(format!("{} Stems{tail}", filename(&p.name)));
        if !target.exists() {
            #[cfg(windows)]
            let result = export::commit(&batch.directory, &target, false);
            #[cfg(not(windows))]
            let result = fs::rename(&batch.directory, &target).map_err(fail);
            match result {
                Ok(()) => break target,
                Err(_) if target.exists() => {}
                Err(e) => return Err(e),
            }
        }
        suffix += 1;
    };
    let mut result = reports[0].clone();
    result.path = paths::display(&target);
    result.files.clear();
    result.clipped_samples = 0;
    result.peak = 0.;
    result.render_ms = 0.;
    for report in reports {
        result.clipped_samples += report.clipped_samples;
        result.peak = result.peak.max(report.peak);
        result.render_ms += report.render_ms;
        for mut f in report.files {
            f.path = paths::display(
                &target.join(Path::new(&f.path).file_name().expect("generated filename")),
            );
            result.files.push(f);
        }
    }
    result.elapsed_ms = began.elapsed().as_secs_f64() * 1000.;
    result.realtime_factor = result.seconds * count as f64 / (result.elapsed_ms / 1000.);
    Ok(result)
}
