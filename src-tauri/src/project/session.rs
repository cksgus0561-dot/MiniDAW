//! Document transactions / runtime staging / user recent-project preference.
//! This module is only called by blocking workers. Audio callbacks never access it.
use super::{paths, runtime::ProjectAudio, schema::*, storage};
use crate::{
    audio::source::AudioAsset,
    error::{AppError, AppResult},
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
};

fn busy() -> AppError {
    AppError::new("project_busy", "다른 프로젝트 작업이 진행 중입니다.")
}
fn lock<T>(m: &Mutex<T>) -> AppResult<MutexGuard<'_, T>> {
    m.lock()
        .map_err(|_| AppError::new("project_state", "프로젝트 상태를 확인할 수 없습니다."))
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetState {
    pub asset_id: String,
    pub status: &'static str,
    pub resolved_path: Option<String>,
    pub message: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentProject {
    pub path: String,
    pub name: String,
    pub missing: bool,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectView {
    pub automation_writing: Vec<super::automation::WritingValue>,
    pub history: super::history::HistoryView,
    pub clipboard_count: usize,
    pub edit_metrics: EditMetrics,
    pub document: Project,
    pub path: Option<String>,
    pub dirty: bool,
    pub revision: u64,
    pub assets: Vec<AssetState>,
    pub notice: Option<String>,
    pub recent: Vec<RecentProject>,
    pub preference_warning: Option<String>,
}
#[derive(Clone)]
struct Session {
    write_pass: Option<super::automation::WritePass>,
    history: super::history::History,
    saved_hash: Option<String>,
    edit_metrics: EditMetrics,
    document: Project,
    path: Option<PathBuf>,
    revision: u64,
    saved_revision: u64,
    digest: Option<String>,
    assets: Vec<AssetState>,
    notice: Option<String>,
}
impl Session {
    fn dirty(&self) -> bool {
        self.saved_hash
            .as_ref()
            .map_or(self.revision != self.saved_revision, |h| {
                *h != super::history::musical_hash(&self.document)
            })
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelinkResult {
    pub view: Option<ProjectView>,
    pub confirmation: Option<String>,
    pub differences: Vec<String>,
}
pub struct ProjectService {
    clipboard: Mutex<super::edit::Clipboard>,
    operation: Mutex<()>,
    session: Mutex<Session>,
    recent: Mutex<Vec<String>>,
    recent_path: Option<PathBuf>,
    preference_warning: Mutex<Option<String>>,
}
impl ProjectService {
    /// Freeze persisted edits plus a live Automation Write pass at the Rust clock.
    /// No history/document mutation and no lock is held during offline rendering.
    pub fn export_snapshot(&self, revision:u64, clock:Option<super::runtime::AutomationClock>) -> AppResult<(Project,Vec<AssetState>,Option<PathBuf>)> {
        let _operation=self.operation.try_lock().map_err(|_|busy())?;
        let mut s=self.check(revision,true)?;
        crate::plugins::capture_snapshot(&mut s.document)?;
        super::automation::finish(&mut s.document,&mut s.write_pass,clock.map(|c|(c.frame,c.rate,c.cycle_pass)));
        Ok((s.document,s.assets,s.path))
    }
    pub fn revision(&self) -> AppResult<u64> {
        Ok(lock(&self.session)?.revision)
    }
    pub fn new(recent_path: Option<PathBuf>) -> Self {
        let mut warning = None;
        let recent = recent_path
            .as_ref()
            .map(|p| {
                read_recent(p).unwrap_or_else(|e| {
                    warning = Some(e.to_string());
                    vec![]
                })
            })
            .unwrap_or_default();
        Self {
            clipboard: Mutex::new(super::edit::Clipboard::default()),
            operation: Mutex::new(()),
            session: Mutex::new(Session {
                write_pass: None,
                history: super::history::History::default(),
                saved_hash: None,
                edit_metrics: EditMetrics::default(),
                document: Project::new(),
                path: None,
                revision: 0,
                saved_revision: 0,
                digest: None,
                assets: vec![],
                notice: None,
            }),
            recent: Mutex::new(recent),
            recent_path,
            preference_warning: Mutex::new(warning),
        }
    }
    pub fn view(&self) -> AppResult<ProjectView> {
        let s = lock(&self.session)?.clone();
        let recent = lock(&self.recent)?
            .iter()
            .map(|p| RecentProject {
                path: p.clone(),
                name: Path::new(p)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                missing: !Path::new(p).is_file(),
            })
            .collect();
        Ok(ProjectView {
            automation_writing: super::automation::writing(s.write_pass.as_ref()),
            history: s.history.view(),
            clipboard_count: lock(&self.clipboard)?.clips.len(),
            edit_metrics: s.edit_metrics.clone(),
            dirty: s.dirty() || crate::plugins::dirty(&s.document),
            document: s.document,
            path: s.path.map(|p| paths::display(&p)),
            revision: s.revision,
            assets: s.assets,
            notice: s.notice,
            recent,
            preference_warning: lock(&self.preference_warning)?.clone(),
        })
    }
    fn check(&self, revision: u64, discard: bool) -> AppResult<Session> {
        let s = lock(&self.session)?.clone();
        if s.revision != revision {
            return Err(AppError::new(
                "project_changed",
                "프로젝트가 변경되었습니다. 현재 상태에서 다시 시도해 주세요.",
            ));
        }
        if s.dirty() && !discard {
            return Err(AppError::new(
                "project_unsaved",
                "변경 사항을 저장하거나 저장하지 않음을 선택해 주세요.",
            ));
        }
        Ok(s)
    }
    pub fn new_project(
        &self,
        audio: &impl ProjectAudio,
        revision: u64,
        discard: bool,
    ) -> AppResult<ProjectView> {
        let _operation = self.operation.try_lock().map_err(|_| busy())?;
        let previous = self.check(revision, discard)?;
        audio.install(None)?;
        audio.master_volume(0.0);
        audio.effects(&Project::new());
        let revision = previous.revision + 1;
        *lock(&self.session)? = Session {
            write_pass: None,
            history: super::history::History::default(),
            saved_hash: None,
            edit_metrics: EditMetrics::default(),
            document: Project::new(),
            path: None,
            revision,
            saved_revision: revision,
            digest: None,
            assets: vec![],
            notice: None,
        };
        *lock(&self.clipboard)? = super::edit::Clipboard::default();
        self.view()
    }
    pub fn import(&self, audio: &impl ProjectAudio, path: &Path) -> AppResult<ProjectView> {
        self.import_to(audio, path, None)
    }
    pub fn import_to(
        &self,
        audio: &impl ProjectAudio,
        path: &Path,
        track: Option<&str>,
    ) -> AppResult<ProjectView> {
        let _operation = self.operation.try_lock().map_err(|_| busy())?;
        let mut s = lock(&self.session)?.clone();
        if track.is_some_and(|id| {
            !s.document
                .tracks
                .iter()
                .any(|t| t.track_id == id && t.kind == TrackKind::Audio)
        }) {
            return Err(invalid("오디오를 가져올 Track이 변경되었습니다."));
        }
        let before_document = s.document.clone();
        if s.saved_hash.is_none() {
            s.saved_hash = Some(super::history::musical_hash(&s.document));
        }
        let path = canonical_media(path)?;
        let before = paths::fingerprint(&path)?;
        let source = AudioAsset::open(&path)?;
        let metadata = metadata(&path, &source);
        if paths::fingerprint(&path)? != before {
            return Err(AppError::new(
                "asset_changed",
                "파일을 읽는 동안 내용이 변경되었습니다. 다시 시도해 주세요.",
            ));
        }
        let asset = Asset {
            asset_id: id(),
            filename: source.info.name.clone(),
            path: paths::reference(&path, s.path.as_deref()),
            metadata,
            fingerprint: before,
            extensions: Extensions::new(),
        };
        let state = AssetState {
            asset_id: asset.asset_id.clone(),
            status: "available",
            resolved_path: Some(paths::display(&path)),
            message: None,
        };
        s.document.import(asset);
        if let Some(track) = track {
            let clip = s.document.primary_clip_id.clone().expect("imported clip");
            super::tracks::transfer(&mut s.document, &[clip], track)?;
        }
        s.document.validate()?;
        s.assets.push(state);
        let prepared = if audio.arrangement() {
            audio.prepare_project(&s.document, &s.assets)?
        } else if s
            .document
            .primary()
            .is_some_and(|c| s.document.preview_supported(c))
        {
            Some(audio.prepare_asset(source)?)
        } else {
            None
        };
        audio.install(prepared)?;
        s.notice = if audio.arrangement() {
            arrangement_notice(&s.assets)
        } else {
            preview_notice(&s.document, &s.assets)
        };
        s.history
            .record(before_document, s.document.clone(), "오디오 가져오기");
        s.revision += 1;
        *lock(&self.session)? = s;
        self.view()
    }
    pub fn open(
        &self,
        audio: &impl ProjectAudio,
        path: &Path,
        revision: u64,
        discard: bool,
    ) -> AppResult<ProjectView> {
        let _operation = self.operation.try_lock().map_err(|_| busy())?;
        let previous = self.check(revision, discard)?;
        let path = paths::absolute(path)?;
        // Validation and staging complete before any old audio/document is replaced.
        let (document, digest) = read_document(&path)?;
        let (assets, prepared, notice) = stage(&document, Some(&path), audio);
        audio.install(prepared)?;
        audio.master_volume(document.master.volume_db);
        audio.effects(&document);
        let revision = previous.revision + 1;
        *lock(&self.session)? = Session {
            write_pass: None,
            history: super::history::History::default(),
            saved_hash: Some(super::history::musical_hash(&document)),
            edit_metrics: EditMetrics::default(),
            document,
            path: Some(path.clone()),
            revision,
            saved_revision: revision,
            digest: Some(digest),
            assets,
            notice,
        };
        self.remember(&path);
        *lock(&self.clipboard)? = super::edit::Clipboard::default();
        self.view()
    }
    pub fn save(&self, path: Option<&Path>, revision: u64) -> AppResult<ProjectView> {
        self.save_at(path, revision, None)
    }
    /// Saving snapshots the latch through the Rust sample clock, without ending
    /// the ongoing live Write pass or losing untouched future curve data on disk.
    pub fn save_at(
        &self,
        path: Option<&Path>,
        revision: u64,
        clock: Option<super::runtime::AutomationClock>,
    ) -> AppResult<ProjectView> {
        let _operation = self.operation.try_lock().map_err(|_| busy())?;
        let mut s = self.check(revision, true)?;
        let destination =
            paths::project_destination(path.or(s.path.as_deref()).ok_or_else(|| {
                AppError::new(
                    "project_save_path",
                    "프로젝트를 저장할 위치를 선택해 주세요.",
                )
            })?)?;
        if s.path.as_ref().is_some_and(|p| same_path(p, &destination)) {
            let (_,digest)=read_document(&destination).map_err(|e|AppError::new("project_conflict","기존 프로젝트 파일이 외부에서 변경되었거나 읽을 수 없습니다. 다른 이름으로 저장해 주세요.").detail(e))?;
            if s.digest.as_ref() != Some(&digest) {
                return Err(AppError::new(
                    "project_conflict",
                    "프로젝트 파일이 외부에서 변경되었습니다. 다른 이름으로 저장해 주세요.",
                ));
            }
        }
        let mut document = s.document.clone();
        crate::plugins::capture_snapshot(&mut document)?;
        for asset in &mut document.assets {
            let resolved = s
                .assets
                .iter()
                .find(|a| a.asset_id == asset.asset_id)
                .and_then(|a| a.resolved_path.as_ref())
                .map(PathBuf::from)
                .or_else(|| {
                    paths::candidates(&asset.path, s.path.as_deref())
                        .into_iter()
                        .next()
                });
            if let Some(media) = resolved {
                if same_path(&media, &destination) {
                    return Err(AppError::new(
                        "project_media_destination",
                        "원본 오디오 파일 위에 프로젝트를 저장할 수 없습니다.",
                    ));
                }
                asset.path = paths::reference(&media, Some(&destination));
            }
        }
        document.name = destination
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let live_automation = document.automation.clone();
        super::automation::finish(
            &mut document,
            &mut s.write_pass.clone(),
            clock.map(|c| (c.frame, c.rate, c.cycle_pass)),
        );
        let saved_hash = super::history::musical_hash(&document);
        let bytes = super::migrations::encode(&document)?;
        storage::save(&destination, &document)?;
        // Only this successful commit updates path, document or saved revision.
        if s.write_pass.is_some() {
            document.automation = live_automation;
        }
        s.document = document;
        s.path = Some(destination.clone());
        s.digest = Some(hash(&bytes));
        s.revision += 1;
        s.saved_revision = s.revision;
        s.saved_hash = Some(saved_hash);
        *lock(&self.session)? = s;
        self.remember(&destination);
        self.view()
    }
    pub fn select_clip(
        &self,
        audio: &impl ProjectAudio,
        clip_id: &str,
        revision: u64,
    ) -> AppResult<ProjectView> {
        let _operation = self.operation.try_lock().map_err(|_| busy())?;
        let mut s = self.check(revision, true)?;
        if !s.document.clips().any(|c| c.clip_id == clip_id) {
            return Err(invalid("선택할 clipId가 없습니다."));
        }
        if s.document.primary_clip_id.as_deref() == Some(clip_id) {
            return self.view();
        }
        s.document.primary_clip_id = Some(clip_id.into());
        let (assets, prepared, notice) = stage(&s.document, s.path.as_deref(), audio);
        audio.install(prepared)?;
        s.assets = assets;
        s.notice = notice;
        s.revision += 1;
        *lock(&self.session)? = s;
        self.view()
    }
    pub fn relink(
        &self,
        audio: &impl ProjectAudio,
        asset_id: &str,
        path: &Path,
        revision: u64,
        confirmation: Option<&str>,
    ) -> AppResult<RelinkResult> {
        let _operation = self.operation.try_lock().map_err(|_| busy())?;
        let mut s = self.check(revision, true)?;
        let index = s
            .document
            .assets
            .iter()
            .position(|a| a.asset_id == asset_id)
            .ok_or_else(|| invalid("assetId"))?;
        let path = canonical_media(path)?;
        let fingerprint = paths::fingerprint(&path)?;
        let source = AudioAsset::open(&path)?;
        let metadata = metadata(&path, &source);
        if paths::fingerprint(&path)? != fingerprint {
            return Err(AppError::new(
                "asset_changed",
                "재연결 파일이 읽는 동안 변경되었습니다.",
            ));
        }
        let old = &s.document.assets[index];
        let mut differences = vec![];
        if old.metadata.sample_rate != metadata.sample_rate {
            differences.push(format!(
                "샘플레이트: {} → {} Hz",
                old.metadata.sample_rate, metadata.sample_rate
            ));
        }
        if old.metadata.channels != metadata.channels {
            differences.push(format!(
                "채널: {} → {}",
                old.metadata.channels, metadata.channels
            ));
        }
        if old.metadata.source_frames != metadata.source_frames {
            differences.push(format!(
                "원본 프레임: {} → {} (기존 Clip 범위 유지)",
                old.metadata.source_frames.0, metadata.source_frames.0
            ));
        }
        if old.fingerprint != fingerprint {
            differences.push("파일 크기 또는 샘플 fingerprint가 다릅니다.".into());
        }
        let token = hash(
            format!(
                "{revision}:{asset_id}:{}:{fingerprint:?}:{metadata:?}",
                paths::display(&path)
            )
            .as_bytes(),
        );
        if !differences.is_empty() && confirmation != Some(token.as_str()) {
            return Ok(RelinkResult {
                view: None,
                confirmation: Some(token),
                differences,
            });
        }
        let asset = &mut s.document.assets[index];
        asset.path = paths::reference(&path, s.path.as_deref());
        asset.filename = source.info.name.clone();
        asset.metadata = metadata;
        asset.fingerprint = fingerprint;
        s.document.validate()?;
        // Relinking an inactive asset must not interrupt the playing primary clip.
        let mut prepared_notice = None;
        if audio.arrangement() || s.document.primary().is_some_and(|c| c.asset_id == asset_id) {
            let prepared = if audio.arrangement() {
                match audio
                    .prepare_project(&s.document, &resolve_all(&s.document, s.path.as_deref()))
                {
                    Ok(p) => p,
                    Err(e) => {
                        prepared_notice = Some(e.to_string());
                        None
                    }
                }
            } else if s
                .document
                .primary()
                .is_some_and(|c| s.document.preview_supported(c))
            {
                Some(audio.prepare_asset(source)?)
            } else {
                None
            };
            audio.install(prepared)?;
        }
        s.assets = resolve_all(&s.document, s.path.as_deref());
        s.notice = if audio.arrangement() {
            prepared_notice.or_else(|| arrangement_notice(&s.assets))
        } else {
            preview_notice(&s.document, &s.assets)
        };
        s.revision += 1;
        *lock(&self.session)? = s;
        Ok(RelinkResult {
            view: Some(self.view()?),
            confirmation: None,
            differences,
        })
    }
    fn remember(&self, path: &Path) {
        let result = (|| -> AppResult<()> {
            let path = paths::display(path);
            let mut recent = lock(&self.recent)?;
            recent.retain(|p| !same_path(Path::new(p), Path::new(&path)));
            recent.insert(0, path);
            recent.truncate(10);
            self.persist_recent(&recent)
        })();
        if let Ok(mut warning) = self.preference_warning.lock() {
            *warning = result
                .err()
                .map(|e| format!("프로젝트 작업은 완료했지만 최근 목록 저장에 실패했습니다. {e}"));
        }
    }
    fn persist_recent(&self, recent: &[String]) -> AppResult<()> {
        if let Some(path) = &self.recent_path {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(invalid)?;
            }
            storage::atomic_write(path, &serde_json::to_vec_pretty(recent).map_err(invalid)?)?;
        }
        Ok(())
    }
    pub fn remove_recent(&self, path: &str) -> AppResult<ProjectView> {
        {
            let mut recent = lock(&self.recent)?;
            let mut updated = recent.clone();
            updated.retain(|p| p != path);
            self.persist_recent(&updated)?;
            *recent = updated;
        }
        *lock(&self.preference_warning)? = None;
        self.view()
    }
}
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditMetrics {
    pub command: String,
    pub mutation_ms: f64,
    pub plan_ms: f64,
    pub commit_ms: f64,
}
impl ProjectService {
    pub fn has_write_pass(&self) -> bool {
        self.session.lock().is_ok_and(|s| s.write_pass.is_some())
    }
    pub fn finish_write(
        &self,
        audio: &impl ProjectAudio,
        clock: Option<super::runtime::AutomationClock>,
    ) -> AppResult<()> {
        // An engine snapshot and an explicit Pause/Seek can complete the same
        // pass concurrently. Retry only transaction contention, off the callback.
        let start = std::time::Instant::now();
        loop {
            if !self.has_write_pass() {
                return Ok(());
            }
            let result = self.edit_clock(
                audio,
                self.revision()?,
                serde_json::from_value(serde_json::json!({"command":"automation.finish"}))
                    .map_err(invalid)?,
                clock,
            );
            match result {
                Ok(_) => return Ok(()),
                Err(_) if !self.has_write_pass() => return Ok(()),
                Err(e)
                    if matches!(e.code, "project_busy" | "project_changed")
                        && start.elapsed() < std::time::Duration::from_secs(2) =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(2))
                }
                Err(e) => return Err(e),
            }
        }
    }

    pub fn edit(
        &self,
        audio: &impl ProjectAudio,
        revision: u64,
        request: super::edit::EditRequest,
    ) -> AppResult<ProjectView> {
        self.edit_clock(audio, revision, request, None)
    }
    fn edit_clock(
        &self,
        audio: &impl ProjectAudio,
        revision: u64,
        request: super::edit::EditRequest,
        override_clock: Option<super::runtime::AutomationClock>,
    ) -> AppResult<ProjectView> {
        use super::normalize::NormalizeStrategy;
        let began = std::time::Instant::now();
        let _operation = self.operation.try_lock().map_err(|_| busy())?;
        let mut s = self.check(revision, true)?;
        // Snapshot pending native editor changes before an edit rebuilds any DSP.
        let before_plugin=s.document.clone();
        let plugin_changed=crate::plugins::capture_project(&mut s.document)?;
        if plugin_changed {s.document.validate()?;s.history.record_group(before_plugin,s.document.clone(),"plugin.capture",None);}
        if request.command=="plugin.capture" {
            if plugin_changed {s.revision+=1;*lock(&self.session)?=s;}
            return self.view();
        }
        let clock = override_clock.or_else(|| audio.automation_clock());
        let position = clock.map(|c| (c.frame, c.rate, c.cycle_pass));
        let writing_command = matches!(
            request.command.as_str(),
            "track.mix" | "master.volume" | "effect.set" | "synth.set" | "plugin.parameter"
        );
        let original = s.document.clone();
        let had_pass = s.write_pass.is_some();
        if !writing_command {
            let previous = s.document.clone();
            if let Some(group) =
                super::automation::finish(&mut s.document, &mut s.write_pass, position)
            {
                s.history.record_group(
                    previous,
                    s.document.clone(),
                    "automation.write",
                    Some(&group),
                );
            }
        }
        let before = s.document.clone();
        let mut clipboard = lock(&self.clipboard)?.clone();
        let mut import_notice = None;
        let mut bounce_output = None;
        let undo = request.command == "edit.undo";
        let redo = request.command == "edit.redo";
        let mut next = if request.command == "automation.finish" {
            before.clone()
        } else if undo {
            s.history
                .undo()
                .ok_or_else(|| invalid("실행 취소할 편집이 없습니다."))?
        } else if redo {
            s.history
                .redo()
                .ok_or_else(|| invalid("다시 실행할 편집이 없습니다."))?
        } else if request.command == "midi.import" {
            let input = super::smf::read(Path::new(
                request
                    .path
                    .as_deref()
                    .ok_or_else(|| invalid("MIDI path"))?,
            ))?;
            let (next, notice) =
                super::smf::merge(&before, input, request.import_tempo.unwrap_or(false))?;
            import_notice = Some(notice);
            next
        } else if request.command == "audio.consolidate" {
            let directory = if let Some(path) = &s.path {
                path.parent().expect("project parent").join(format!("{}.media", path.file_stem().unwrap_or_default().to_string_lossy()))
            } else {
                self.recent_path.as_ref().and_then(|p| p.parent()).ok_or_else(|| invalid("Bounce 전에 프로젝트를 저장해 주세요."))?.join("GeneratedAudio").join(&before.project_id)
            };
            let rate = clock.map(|c|c.rate).unwrap_or(48000);
            let output = super::bounce::render(&before, &s.assets, &request.clip_ids, &directory, s.path.as_deref(), rate, request.bounce_replace.unwrap_or(true))?;
            let next = output.document.clone();
            import_notice = Some(if request.bounce_replace.unwrap_or(true) { "Bounce Selection: 선택 Event를 새 Audio로 교체했습니다." } else { "Bounce Selection: 원본을 유지하고 새 Audio를 Media에 추가했습니다." }.into());
            bounce_output = Some(output);
            next
        } else if request.command == "audio.normalize" {
            let target = request.normalize_target_db.unwrap_or(0.0);
            if !target.is_finite() || !(-144.0..=0.0).contains(&target) {
                return Err(invalid("Normalize target은 -144~0 dBFS입니다."));
            }
            let mut next = before.clone();
            for c in next
                .tracks
                .iter_mut()
                .flat_map(|t| &mut t.clips)
                .filter_map(Clip::as_audio_mut)
            {
                if !request.clip_ids.contains(&c.clip_id) {
                    continue;
                }
                let path = s
                    .assets
                    .iter()
                    .find(|a| a.asset_id == c.asset_id)
                    .and_then(|a| a.resolved_path.as_deref())
                    .ok_or_else(|| invalid("Normalize할 원본을 다시 연결해 주세요."))?;
                let mut peak = super::normalize::Peak::default();
                let original =
                    crate::audio::source::AudioAsset::open_with_budget(Path::new(path), 0)?;
                let asset = if let Some(r) = super::stretch::recipe(c)? {
                    let meta = before
                        .assets
                        .iter()
                        .find(|a| a.asset_id == c.asset_id)
                        .expect("asset");
                    crate::audio::stretch::prepare_pitched(
                        original,
                        &r,
                        super::pitch::get(c)?.total(),
                        &meta.fingerprint.sampled_sha256,
                    )?
                } else {
                    original
                };
                super::normalize::analyze_asset(
                    asset,
                    c.source_start.0,
                    c.source_end.0,
                    &mut peak,
                )?;
                if let Some(gain) = peak.gain(target) {
                    c.gain = gain;
                }
            }
            next
        } else {
            super::edit::apply(&before, &request, &mut clipboard)?
        };
        if next.musical_time.tempo_map != before.musical_time.tempo_map { super::tempo_sync::refresh(&mut next)?; }
        let written = super::automation::capture(
            &before,
            &mut next,
            &request,
            clock
                .filter(|c| c.playing)
                .map(|c| (c.frame, c.rate, c.cycle_pass)),
            &mut s.write_pass,
        )?;
        // File identity/location belongs to the session, not the undoable arrangement.
        next.name = before.name.clone();
        for asset in &mut next.assets {
            if let Some(path) = asset.path.original_absolute_path.as_ref() {
                asset.path = paths::reference(Path::new(path), s.path.as_deref());
            }
        }
        next.validate()?;
        if next == before && request.command != "automation.finish" && !had_pass {
            *lock(&self.clipboard)? = clipboard;
            return self.view();
        }
        let mutation_ms = began.elapsed().as_secs_f64() * 1000.0;
        let playback = super::automation::playback(&next, s.write_pass.as_ref());
        let states = resolve_all(&next, s.path.as_deref());
        let preparing = std::time::Instant::now();
        // A Master-only edit (including Undo/Redo) never rebuilds read-ahead,
        // re-primes streaming or resets the MIDI scheduler.
        let mut without_master = next.clone();
        without_master.master = before.master.clone();
        without_master.automation.retain(|a| {
            next.tracks
                .iter()
                .any(|t| t.track_id == a.track_id && t.kind == TrackKind::Audio)
        });
        without_master.automation.extend(
            before
                .automation
                .iter()
                .filter(|a| {
                    !before
                        .tracks
                        .iter()
                        .any(|t| t.track_id == a.track_id && t.kind == TrackKind::Audio)
                })
                .cloned(),
        );
        without_master
            .automation
            .sort_by(|a, b| a.track_id.cmp(&b.track_id));
        let mut compare = before.clone();
        compare
            .automation
            .sort_by(|a, b| a.track_id.cmp(&b.track_id));
        for track in &mut without_master.tracks {
            if track.kind == TrackKind::Midi {
                if let Some(old) = before.tracks.iter().find(|t| t.track_id == track.track_id) {
                    track.inserts = old.inserts.clone();
                    track.synth = old.synth.clone();
                }
            }
        }
        // Pool-only Bounce/Undo changes no audible Event. Part membership is
        // also presentation metadata; neither should re-prime playback.
        for document in [&mut without_master, &mut compare] {
            let used: std::collections::HashSet<_> = document.clips().map(|c|c.asset_id.clone()).collect();
            document.assets.retain(|a| used.contains(&a.asset_id));
            for c in document.tracks.iter_mut().flat_map(|t|&mut t.clips).filter_map(Clip::as_audio_mut) { c.extensions.remove(super::glue::KEY); }
        }
        if without_master != compare
            || (had_pass
                && original.automation != next.automation
                && original.automation.iter().any(|c| {
                    original
                        .tracks
                        .iter()
                        .any(|t| t.track_id == c.track_id && t.kind == TrackKind::Audio)
                }))
        {
            let prepared = if audio.arrangement() {
                audio.prepare_project(&playback, &states)?
            } else {
                stage(&playback, s.path.as_deref(), audio).1
            };
            let mut no_automation = next.clone();
            no_automation.automation = original.automation.clone();
            if written || request.command.starts_with("automation.") || no_automation == original {
                audio.replace_automation(prepared)?;
            } else {
                audio.replace(prepared)?;
            }
        }
        let plan_ms = preparing.elapsed().as_secs_f64() * 1000.0;
        audio.master_volume(next.master.volume_db);
        audio.effects(&playback);
        if !undo && !redo && request.command != "automation.finish" {
            let group = if written {
                s.write_pass.as_ref().map(|p| p.group.as_str())
            } else {
                request.history_group.as_deref().filter(|g| {
                    g.len() <= 128
                        && matches!(request.command.as_str(), "track.mix" | "master.volume")
                })
            };
            s.history.record_group(
                before,
                next.clone(),
                if written {
                    "automation.write"
                } else {
                    &request.command
                },
                group,
            );
        }
        s.document = next;
        s.assets = states;
        s.notice = if audio.arrangement() {
            arrangement_notice(&s.assets)
        } else {
            preview_notice(&s.document, &s.assets)
        };
        s.notice = import_notice.or(s.notice);
        s.revision += 1;
        s.edit_metrics = EditMetrics {
            command: request.command,
            mutation_ms,
            plan_ms,
            commit_ms: began.elapsed().as_secs_f64() * 1000.0,
        };
        *lock(&self.session)? = s;
        if let Some(output) = &mut bounce_output { output.commit(); }
        *lock(&self.clipboard)? = clipboard;
        self.view()
    }
}
fn arrangement_notice(assets: &[AssetState]) -> Option<String> {
    assets.iter().any(|a|a.status!="available").then(||"연결되지 않은 미디어의 Clip은 무음으로 재생합니다. 미디어에서 파일을 다시 연결해 주세요.".into())
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn same_path(a: &Path, b: &Path) -> bool {
    #[cfg(windows)]
    {
        paths::display(a).eq_ignore_ascii_case(&paths::display(b))
    }
    #[cfg(not(windows))]
    {
        a == b
    }
}
fn canonical_media(path: &Path) -> AppResult<PathBuf> {
    let path = paths::absolute(path)?;
    let canonical = fs::canonicalize(path)
        .map_err(|e| AppError::new("asset_read", "오디오 파일을 찾을 수 없습니다.").detail(e))?;
    Ok(PathBuf::from(paths::display(&canonical)))
}
fn metadata(path: &Path, source: &AudioAsset) -> AudioMetadata {
    let container = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_lowercase();
    let codec = match container.as_str() {
        "mp3" => Some("MPEG Layer III".into()),
        "flac" => Some("FLAC".into()),
        _ => None,
    };
    AudioMetadata {
        sample_rate: source.info.sample_rate,
        channels: source.info.channels as u16,
        source_frames: Frames(source.info.frames as u64),
        container,
        codec,
    }
}
pub fn resolve_all(document: &Project, path: Option<&Path>) -> Vec<AssetState> {
    document
        .assets
        .iter()
        .map(|asset| {
            let mut state = AssetState {
                asset_id: asset.asset_id.clone(),
                status: "missing",
                resolved_path: None,
                message: Some("원본 파일을 찾을 수 없습니다. 파일을 다시 연결해 주세요.".into()),
            };
            for candidate in paths::candidates(&asset.path, path) {
                if !candidate.is_file() {
                    continue;
                }
                match paths::fingerprint(&candidate) {
                    Ok(f) if f == asset.fingerprint => {
                        return AssetState {
                            asset_id: asset.asset_id.clone(),
                            status: "available",
                            resolved_path: Some(paths::display(&candidate)),
                            message: None,
                        }
                    }
                    Ok(_) => {
                        state.status = "changed";
                        state.message = Some(
                            "저장 당시와 다른 파일입니다. 파일 다시 연결에서 확인해 주세요.".into(),
                        );
                    }
                    Err(e) => {
                        state.status = "unreadable";
                        state.message = Some(e.to_string());
                    }
                }
            }
            state
        })
        .collect()
}
fn single_clip_notice(document: &Project) -> Option<String> {
    (document.clips().count() > 1).then(|| {
        "현재는 선택한 클립 하나만 미리듣기합니다. 다른 클립과 트랙도 프로젝트에 보존됩니다.".into()
    })
}
fn preview_notice(document: &Project, assets: &[AssetState]) -> Option<String> {
    if let Some(clip) = document.primary() {
        if !document.preview_supported(clip) {
            return Some("클립 편집/확장 데이터는 보존되었습니다. 현재 버전은 전체 원본·0초 위치·기본 gain의 단일 클립만 재생합니다.".into());
        }
        if assets
            .iter()
            .any(|a| a.asset_id == clip.asset_id && a.status != "available")
        {
            return Some("프로젝트를 열었습니다. 재생할 원본 파일을 다시 연결해 주세요.".into());
        }
    }
    single_clip_notice(document)
}
fn stage<R: ProjectAudio>(
    document: &Project,
    path: Option<&Path>,
    audio: &R,
) -> (Vec<AssetState>, Option<R::Prepared>, Option<String>) {
    let mut assets = resolve_all(document, path);
    if audio.arrangement() {
        return match audio.prepare_project(document, &assets) {
            Ok(prepared) => {
                let notice = arrangement_notice(&assets);
                (assets, prepared, notice)
            }
            Err(e) => (assets, None, Some(e.to_string())),
        };
    }
    let mut prepared = None;
    if let Some(clip) = document.primary().filter(|c| document.preview_supported(c)) {
        if let Some(state) = assets
            .iter_mut()
            .find(|a| a.asset_id == clip.asset_id && a.status == "available")
        {
            let asset = document
                .assets
                .iter()
                .find(|a| a.asset_id == clip.asset_id)
                .expect("validated reference");
            let attempt = (|| -> AppResult<R::Prepared> {
                let p = Path::new(state.resolved_path.as_ref().expect("resolved path"));
                let source = AudioAsset::open(p)?;
                if metadata(p, &source) != asset.metadata {
                    return Err(AppError::new("asset_metadata","원본의 sample rate / 채널 / 프레임 정보가 변경되었습니다. 파일을 다시 연결해 주세요."));
                }
                audio.prepare_asset(source)
            })();
            match attempt {
                Ok(value) => prepared = Some(value),
                Err(error) => {
                    state.status = "unreadable";
                    state.message = Some(error.to_string());
                }
            }
        }
    }
    let notice = preview_notice(document, &assets);
    (assets, prepared, notice)
}
fn read_document(path: &Path) -> AppResult<(Project, String)> {
    let file = fs::File::open(path).map_err(|e| {
        AppError::new("project_read", "프로젝트 파일을 읽을 수 없습니다.").detail(e)
    })?;
    if !file.metadata().map_err(invalid)?.is_file() {
        return Err(invalid("일반 파일이 아닙니다."));
    }
    let mut bytes = vec![];
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(invalid)?;
    Ok((super::migrations::decode(&bytes)?, hash(&bytes)))
}
fn read_recent(path: &Path) -> AppResult<Vec<String>> {
    if !path.exists() {
        return Ok(vec![]);
    }
    let file = fs::File::open(path).map_err(invalid)?;
    let mut bytes = vec![];
    file.take(512 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(invalid)?;
    if bytes.len() > 512 * 1024 {
        return Err(invalid("최근 목록 파일 크기"));
    }
    let paths: Vec<String> = serde_json::from_slice(&bytes).map_err(invalid)?;
    if paths.len() > 10 {
        return Err(invalid("최근 목록 개수"));
    }
    for p in &paths {
        paths::validate_path(p)?;
        if !Path::new(p).is_absolute() {
            return Err(invalid("최근 경로"));
        }
    }
    Ok(paths)
}
