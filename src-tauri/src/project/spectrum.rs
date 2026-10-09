//! Read-only selection snapshot. Reuses the edited playback plan and independent
//! streaming readers; never creates waveform caches, modifies a project or seeks playback.
use super::{time::time, ProjectService};
use crate::{
    audio::{
        spectrum::{analyze_section, SpectrumFrame},
        timeline::{PlaybackPlan, TimelineReader},
    },
    error::{AppError, AppResult},
    state::AudioService,
};
use serde::Deserialize;
use std::{
    collections::HashMap,
    sync::{atomic::Ordering::*, Arc},
};

/// A source switch changes analysis state only: no transport command, ring reset,
/// project edit or Synth reconfiguration. Track IDs are resolved anew per revision.
pub fn configure_sources(
    service: &AudioService,
    projects: &ProjectService,
    settings: crate::audio::spectrum::Settings,
    sources: Vec<String>,
) -> AppResult<()> {
    if sources.is_empty() || !settings.enabled {
        return service.spectrum.configure(settings);
    }
    if sources.len() > 2 || (sources.len() == 2 && sources[0] == sources[1]) {
        return Err(AppError::new(
            "spectrum_source",
            "서로 다른 Track을 최대 두 개 선택하세요.",
        ));
    }
    let view = projects.view()?;
    let tracks: Vec<_> = sources
        .into_iter()
        .map(|id| {
            view.document
                .tracks
                .iter()
                .position(|t| t.track_id == id)
                .map(|i| (id, i))
                .ok_or_else(|| AppError::new("spectrum_source", "선택한 Track이 없습니다."))
        })
        .collect::<AppResult<_>>()?;
    let loaded = service.loaded()?;
    let source_id = loaded.as_ref().map_or(0, |l| l.id);
    let plan = if let Some(plan) = loaded.as_ref().and_then(|l| l.audio.timeline.as_ref()) {
        // MIDI inserts run after the Synth bus and can change without replacing
        // this immutable Audio/Scheduler plan. Their actual PCM is tapped in Renderer.
        let mut audio_tracks = view.document.tracks.clone();
        for track in &mut audio_tracks {
            if track.kind == super::schema::TrackKind::Midi {
                if let Some(old) = plan
                    .document
                    .tracks
                    .iter()
                    .find(|t| t.track_id == track.track_id)
                {
                    track.inserts = old.inserts.clone();
                    track.synth = old.synth.clone();
                }
            }
        }
        if plan.document.project_id != view.document.project_id
            || plan.document.tracks != audio_tracks
            || plan.document.musical_time != view.document.musical_time
            || plan.document.cycle != view.document.cycle
        {
            return Err(AppError::new(
                "stale",
                "프로젝트 변경 후 Spectrum 소스를 다시 선택하세요.",
            ));
        }
        plan.clone()
    } else {
        // Preserve the neutral single-clip fast playback path. Only the analysis
        // worker uses this plan; the asset stays shared and no PCM/cache is rebuilt.
        let mut assets = HashMap::new();
        if let (Some(l), Some(c)) = (&loaded, view.document.primary()) {
            assets.insert(c.asset_id.clone(), l.audio.asset.clone());
        }
        let rate = loaded.as_ref().map_or(48000, |l| l.audio.playback_rate);
        PlaybackPlan::compile(Arc::new(view.document.clone()), assets, rate)?
    };
    if projects.revision()? != view.revision
        || service.loaded()?.as_ref().map_or(0, |l| l.id) != source_id
    {
        return Err(AppError::new(
            "stale",
            "프로젝트가 변경되었습니다. Spectrum 소스를 다시 선택하세요.",
        ));
    }
    service.spectrum.configure_tracks(
        settings,
        Some(crate::audio::spectrum::TrackCapture {
            source_id,
            plan,
            tracks,
        }),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    pub revision: u64,
    pub clip_ids: Vec<String>,
    pub track_ids: Vec<String>,
    pub start: Option<f64>,
    pub end: Option<f64>,
    pub fft_size: usize,
}
pub fn analyze(
    service: &AudioService,
    projects: &ProjectService,
    request: Selection,
    generation: u64,
) -> AppResult<SpectrumFrame> {
    let spectrum = &service.spectrum;
    let _guard = spectrum.selection_lock.try_lock().map_err(|_| {
        AppError::new(
            "spectrum_busy",
            "진행 중인 분석을 취소한 뒤 다시 시도해 주세요.",
        )
    })?;
    let shared = spectrum.selection_generation.clone();
    let cancel: crate::audio::reader::Cancel = Arc::new(move || shared.load(Acquire) != generation);
    if cancel() {
        return Err(AppError::new(
            "cancelled",
            "Spectrum 분석이 취소되었습니다.",
        ));
    }
    spectrum.selection_progress.store(0, Relaxed);
    let view = projects.view()?;
    if view.revision != request.revision {
        return Err(AppError::new(
            "stale",
            "선택 내용이 변경되었습니다. 다시 분석해 주세요.",
        ));
    }
    let range = match (request.start, request.end) {
        (Some(a), Some(b)) if a.is_finite() && b.is_finite() && a >= 0.0 && b > a => Some((a, b)),
        (None, None) if !request.clip_ids.is_empty() => None,
        _ => {
            return Err(AppError::new(
                "selection",
                "Audio Clip 또는 Range를 먼저 선택해 주세요.",
            ))
        }
    };
    let mut document = view.document;
    for track in &mut document.tracks {
        if range.is_some() && !request.track_ids.contains(&track.track_id) {
            track.clips.clear();
        } else if range.is_none() {
            track.clips.retain(|c| {
                c.as_audio()
                    .is_some_and(|c| request.clip_ids.contains(&c.clip_id))
            });
        }
    }
    if document.clips().next().is_none() {
        return Err(AppError::new("selection", "분석할 Audio Clip이 없습니다."));
    }
    let primary = document.clips().next().map(|c| c.clip_id.clone());
    document.primary_clip_id = primary;
    let rate = service.snapshot()?.output.map_or(48000, |o| o.sample_rate);
    let mut assets = HashMap::new();
    {
        let pool = service
            .media
            .lock()
            .map_err(|_| AppError::new("spectrum", "미디어 상태 오류"))?;
        for clip in document.clips().filter(|c| !c.mute) {
            let media = pool.entries.get(&clip.asset_id).ok_or_else(|| {
                AppError::new(
                    "asset_missing",
                    "선택한 구간의 미디어를 먼저 연결해 주세요.",
                )
            })?;
            assets.insert(clip.asset_id.clone(), media.asset.clone());
        }
    }
    let first = document
        .clips()
        .map(|c| {
            time(&c.position, &document.musical_time)
                .ceil_frame(rate)
                .max(0) as usize
        })
        .min()
        .unwrap();
    let plan = PlaybackPlan::compile(Arc::new(document), assets, rate)?;
    let (start, end) = range.map_or((first, plan.frames), |(a, b)| {
        (
            (a * rate as f64).ceil() as usize,
            (b * rate as f64).ceil() as usize,
        )
    });
    let mut reader = TimelineReader::new(plan);
    analyze_section(
        &mut reader,
        start,
        end,
        rate,
        request.fft_size,
        cancel,
        &spectrum.selection_progress,
    )
}
