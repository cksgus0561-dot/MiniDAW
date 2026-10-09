use minidaw_lib::{
    audio::source::AudioAsset,
    error::{AppError, AppResult},
    project::{
        migrations, paths,
        runtime::ProjectAudio,
        schema::*,
        session::{resolve_all, ProjectService},
        storage,
    },
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

struct Folder(PathBuf);
impl Folder {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("minidaw-project-test-{}", id()));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn file(&self, s: &str) -> PathBuf {
        self.0.join(s)
    }
}
impl Drop for Folder {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[derive(Default)]
struct Runtime {
    current: Mutex<Option<Arc<AudioAsset>>>,
    commits: AtomicUsize,
    fail: AtomicBool,
}
impl ProjectAudio for Runtime {
    type Prepared = Arc<AudioAsset>;
    fn prepare_asset(&self, a: Self::Prepared) -> AppResult<Self::Prepared> {
        Ok(a)
    }
    fn install(&self, a: Option<Self::Prepared>) -> AppResult<()> {
        if self.fail.load(Ordering::Relaxed) {
            return Err(AppError::new("test_commit", "실행 상태 적용 실패"));
        }
        *self.current.lock().unwrap() = a;
        self.commits.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures")
        .join(name)
}
fn setup(folder: &Folder) -> (ProjectService, Runtime, PathBuf) {
    let path = folder.file("한글 원본 with spaces.wav");
    fs::copy(fixture("stereo-44100.wav"), &path).unwrap();
    let s = ProjectService::new(Some(folder.file("recent.json")));
    let r = Runtime::default();
    s.import(&r, &path).unwrap();
    (s, r, path)
}
fn revision(s: &ProjectService) -> u64 {
    s.view().unwrap().revision
}

#[test]
fn editing_history_saved_state_failure_and_branch_are_transactional() {
    let f = Folder::new();
    let (s, r, _) = setup(&f);
    let edit = |command: &str| {
        let v = s.view().unwrap();
        s.edit(&r,v.revision,serde_json::from_value(serde_json::json!({"command":command,"clipIds":v.document.clips().map(|c|&c.clip_id).collect::<Vec<_>>(),"gainDb":-6})).unwrap())
    };
    s.save(Some(&f.file("first.minidaw")), revision(&s))
        .unwrap();
    let saved = s.view().unwrap().document;
    edit("audio.gain").unwrap();
    assert!(s.view().unwrap().dirty);
    edit("edit.undo").unwrap();
    assert!(!s.view().unwrap().dirty);
    assert_eq!(s.view().unwrap().document, saved);
    edit("edit.redo").unwrap();
    assert!(s.view().unwrap().dirty);
    s.save(Some(&f.file("second.minidaw")), revision(&s))
        .unwrap();
    assert!(!s.view().unwrap().dirty);
    edit("edit.undo").unwrap();
    assert!(s.view().unwrap().dirty);
    edit("edit.redo").unwrap();
    assert!(!s.view().unwrap().dirty);
    let before = s.view().unwrap();
    r.fail.store(true, Ordering::Relaxed);
    assert!(edit("edit.undo").is_err());
    let after = s.view().unwrap();
    assert_eq!(after.document, before.document);
    assert_eq!(after.history.undo, before.history.undo);
    assert_eq!(after.revision, before.revision);
    r.fail.store(false, Ordering::Relaxed);
    edit("edit.undo").unwrap();
    edit("audio.muteEvents").unwrap();
    assert_eq!(s.view().unwrap().history.redo, 0);
    edit("edit.undo").unwrap();
    edit("edit.undo").unwrap();
    assert_eq!(s.view().unwrap().document.clips().count(), 0);
    edit("edit.redo").unwrap();
    assert_eq!(s.view().unwrap().document.clips().count(), 1);
}

#[test]
fn exact_schema_round_trip_multiple_assets_paths_and_edit_metadata() {
    let f = Folder::new();
    let (s, r, _) = setup(&f);
    s.import(&r, &fixture("stereo-44100.flac")).unwrap();
    let mut p = s.view().unwrap().document;
    p.assets[0].path.project_relative_path = Some("미디어/한글 원본 with spaces.wav".into());
    p.assets[1].path.original_absolute_path = Some(paths::display(
        &f.file(&format!("{}/missing.flac", vec!["긴 경로"; 100].join("/"))),
    ));
    p.assets[0].metadata.source_frames = Frames(u64::MAX);
    let Clip::Audio(c) = &mut p.tracks[0].clips[0] else {
        panic!("Audio fixture")
    };
    c.source_start = Frames(9_007_199_254_740_993);
    c.source_end = Frames(18_446_744_073_709_551_614);
    c.position = Position::Seconds {
        numerator: Signed(9_007_199_254_740_993),
        denominator: 44100,
    };
    c.gain = 0.75;
    c.fade_in.source_frames = Frames(100);
    c.fade_out.source_frames = Frames(300);
    c.mute = true;
    c.extensions.insert(
        "org.minidaw.future.pitch".into(),
        serde_json::json!({"schemaVersion":1,"semitones":2.5}),
    );
    let original = migrations::encode(&p).unwrap();
    let decoded = migrations::decode(&original).unwrap();
    assert_eq!(p, decoded);
    assert_eq!(original, migrations::encode(&decoded).unwrap());
    assert!(!p.preview_supported(p.clips().next().unwrap()));
    let mut p2 = p.clone();
    let Clip::Audio(c) = &mut p2.tracks[0].clips[0] else {
        panic!("Audio fixture")
    };
    c.position = Position::Ticks {
        ticks: Signed(i64::MAX),
    };
    assert_eq!(
        p2,
        migrations::decode(&migrations::encode(&p2).unwrap()).unwrap()
    );
}
#[test]
fn versions_validation_and_corruption_are_explicit() {
    let f = Folder::new();
    let (s, _, _) = setup(&f);
    let value = serde_json::to_value(s.view().unwrap().document).unwrap();
    let check = |v: serde_json::Value, code: &str| {
        assert_eq!(
            migrations::decode(&serde_json::to_vec(&v).unwrap())
                .err()
                .unwrap()
                .code,
            code
        );
    };
    let mut v = value.clone();
    v["schemaVersion"] = 999.into();
    check(v, "project_version_newer");
    let mut v = value.clone();
    v["schemaVersion"] = 0.into();
    check(v, "project_version_unsupported");
    let mut v = value.clone();
    v.as_object_mut().unwrap().remove("projectId");
    check(v, "project_invalid");
    let mut v = value.clone();
    v["tracks"][0]["clips"][0]["assetId"] = id().into();
    check(v, "project_invalid");
    let mut v = value.clone();
    v["assets"][0]["assetId"] = v["projectId"].clone();
    check(v, "project_invalid");
    let mut v = value.clone();
    v["tracks"][0]["clips"][0]["position"]["denominator"] = 0.into();
    check(v, "project_invalid");
    let mut v = value.clone();
    v["tracks"][0]["clips"][0]["sourceStart"] = 100.into();
    check(v, "project_invalid");
    let mut v = value.clone();
    v["audioPreferences"] = serde_json::json!({"driverType":"asio"});
    check(v, "project_invalid");
    let mut v = value.clone();
    v["assets"][0]["path"]["projectRelativePath"] = "../escape.wav".into();
    check(v, "project_invalid");
    let mut v = value;
    v["tracks"][0]["clips"][0]["unknownEdit"] = true.into();
    check(v, "project_invalid");
    assert!(migrations::decode(b"{broken").is_err());
    assert!(migrations::decode(&vec![b' '; MAX_BYTES as usize + 1]).is_err());
}
#[test]
fn count_and_id_bounds_do_not_panic() {
    let mut p = Project::new();
    p.tracks = vec![
        Track {
            synth: Default::default(),
            inserts: vec![],
            track_id: id(),
            name: "Audio".into(),
            kind: TrackKind::Audio,
            instrument: Instrument::None,
            mix: TrackMix::default(),
            clips: vec![],
            extensions: Extensions::new()
        };
        4097
    ];
    assert!(p.validate().is_err());
    p.tracks.clear();
    p.project_id = "not-an-id".into();
    assert!(p.validate().is_err());
    for path in [
        "",
        "bad\0path",
        "../escape.wav",
        "/absolute.wav",
        "C:relative.wav",
    ] {
        assert!(paths::validate_reference(&PathReference {
            project_relative_path: Some(path.into()),
            original_absolute_path: None
        })
        .is_err());
    }
}
#[test]
fn import_save_save_as_is_small_keeps_ids_and_never_touches_audio() {
    let f = Folder::new();
    let (s, r, media) = setup(&f);
    let before = fs::read(&media).unwrap();
    let first = s.view().unwrap();
    assert!(first.dirty);
    assert!(first.path.is_none());
    assert_eq!(first.document.tracks.len(), 1);
    let saved = s
        .save(Some(&f.file("노래.minidaw")), first.revision)
        .unwrap();
    assert!(!saved.dirty);
    assert_eq!(
        saved.document.assets[0]
            .path
            .project_relative_path
            .as_deref(),
        Some("한글 원본 with spaces.wav")
    );
    let dest = f.file("다른 폴더");
    fs::create_dir(&dest).unwrap();
    let commits = r.commits.load(Ordering::Relaxed);
    let copied = s
        .save(Some(&dest.join("다른 이름.minidaw")), saved.revision)
        .unwrap();
    assert!(copied.document.assets[0]
        .path
        .project_relative_path
        .is_none());
    assert_eq!(copied.document.project_id, first.document.project_id);
    assert_eq!(copied.document.tracks, first.document.tracks);
    assert_eq!(
        r.commits.load(Ordering::Relaxed),
        commits,
        "save must not touch audio runtime"
    );
    assert_eq!(before, fs::read(&media).unwrap());
    assert!(fs::metadata(copied.path.unwrap()).unwrap().len() < 8192);
}
#[test]
fn relative_priority_absolute_fallback_and_project_folder_move() {
    let f = Folder::new();
    let (s, _, media) = setup(&f);
    let saved = s.save(Some(&f.file("song.minidaw")), revision(&s)).unwrap();
    let p = saved.document;
    let relocated = f.file("relocated");
    fs::create_dir(&relocated).unwrap();
    let second = relocated.join(media.file_name().unwrap());
    fs::copy(&media, &second).unwrap();
    let project = relocated.join("song.minidaw");
    assert_eq!(
        resolve_all(&p, Some(&project))[0].resolved_path.as_deref(),
        Some(paths::display(&second).as_str())
    );
    fs::remove_file(&second).unwrap();
    assert_eq!(
        resolve_all(&p, Some(&project))[0].resolved_path.as_deref(),
        Some(paths::display(&media).as_str())
    );
    fs::rename(&media, &second).unwrap();
    assert_eq!(resolve_all(&p, Some(&project))[0].status, "available");
}
#[test]
fn missing_open_relink_preserves_ids_and_marks_dirty() {
    let f = Folder::new();
    let (s, r, media) = setup(&f);
    let path = f.file("missing.minidaw");
    let saved = s.save(Some(&path), revision(&s)).unwrap();
    let moved = f.file("옮긴 음악.wav");
    fs::rename(&media, &moved).unwrap();
    let loaded = s.open(&r, &path, revision(&s), false).unwrap();
    assert_eq!(loaded.assets[0].status, "missing");
    assert!(!loaded.dirty);
    assert!(r.current.lock().unwrap().is_none());
    let asset_id = loaded.document.assets[0].asset_id.clone();
    let result = s
        .relink(&r, &asset_id, &moved, loaded.revision, None)
        .unwrap();
    assert!(result.confirmation.is_none());
    let fixed = result.view.unwrap();
    assert!(fixed.dirty);
    assert_eq!(fixed.document.tracks, saved.document.tracks);
    assert_eq!(fixed.document.assets[0].asset_id, asset_id);
    assert_eq!(fixed.assets[0].status, "available");
    assert!(r.current.lock().unwrap().is_some());
}
#[test]
fn relink_mismatch_requires_confirmation_bound_to_candidate_and_revision() {
    let f = Folder::new();
    let (s, r, _) = setup(&f);
    let before = s.view().unwrap();
    let id = &before.document.assets[0].asset_id;
    let replacement = fixture("모노-48000.WAV");
    let proposal = s
        .relink(&r, id, &replacement, before.revision, None)
        .unwrap();
    assert!(proposal.view.is_none());
    assert!(proposal.differences.len() >= 3);
    assert_eq!(s.view().unwrap().document, before.document);
    let again = s
        .relink(&r, id, &replacement, before.revision, Some("wrong token"))
        .unwrap();
    assert!(again.view.is_none());
    let fixed = s
        .relink(
            &r,
            id,
            &replacement,
            before.revision,
            proposal.confirmation.as_deref(),
        )
        .unwrap()
        .view
        .unwrap();
    assert_eq!(fixed.document.tracks, before.document.tracks);
    assert!(fixed.notice.is_some());
    assert!(
        r.current.lock().unwrap().is_none(),
        "unsupported trim/replacement must not be silently played as full source"
    );
    assert_eq!(
        s.relink(&r, id, &replacement, before.revision, None)
            .err()
            .unwrap()
            .code,
        "project_changed"
    );
}
#[test]
fn missing_relative_reference_survives_save_as() {
    let f = Folder::new();
    let (s, r, media) = setup(&f);
    let path = f.file("original.minidaw");
    s.save(Some(&path), revision(&s)).unwrap();
    fs::remove_file(&media).unwrap();
    s.open(&r, &path, revision(&s), false).unwrap();
    let sub = f.file("sub");
    fs::create_dir(&sub).unwrap();
    let saved = s
        .save(Some(&sub.join("copy.minidaw")), revision(&s))
        .unwrap();
    assert!(saved.document.assets[0]
        .path
        .project_relative_path
        .is_none());
    assert_eq!(
        saved.document.assets[0]
            .path
            .original_absolute_path
            .as_deref(),
        Some(paths::display(&media).as_str())
    );
}
#[test]
fn save_failures_keep_file_dirty_and_current_path() {
    let f = Folder::new();
    let (s, r, _) = setup(&f);
    let path = f.file("original.minidaw");
    s.save(Some(&path), revision(&s)).unwrap();
    let bytes = fs::read(&path).unwrap();
    s.import(&r, &fixture("stereo-44100.mp3")).unwrap();
    let before = s.view().unwrap();
    for bad in [
        f.file("missing-directory/song.minidaw"),
        f.file("wrong.wav"),
    ] {
        assert!(s.save(Some(&bad), revision(&s)).is_err());
        let after = s.view().unwrap();
        assert!(after.dirty);
        assert_eq!(after.path, before.path);
        assert_eq!(after.document, before.document);
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
    let permissions = fs::metadata(&path).unwrap().permissions();
    let mut readonly = permissions.clone();
    readonly.set_readonly(true);
    fs::set_permissions(&path, readonly).unwrap();
    #[cfg(windows)]
    {
        assert!(s.save(None, revision(&s)).is_err());
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert!(s.view().unwrap().dirty);
    }
    fs::set_permissions(&path, permissions).unwrap();
    fs::write(&path, b"{corrupt external edit").unwrap();
    assert_eq!(
        s.save(None, revision(&s)).err().unwrap().code,
        "project_conflict"
    );
    assert_eq!(fs::read(&path).unwrap(), b"{corrupt external edit");
    assert!(s.view().unwrap().dirty);
}
#[test]
fn atomic_replace_does_not_truncate_on_replace_or_temp_failure() {
    let f = Folder::new();
    let p = Project::new();
    let path = f.file("song.minidaw");
    storage::save(&path, &p).unwrap();
    let mut q = p.clone();
    q.name = "new".into();
    storage::save(&path, &q).unwrap();
    assert_eq!(storage::read(&path).unwrap(), q);
    let dir = f.file("directory.minidaw");
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("keep"), b"keep").unwrap();
    assert!(storage::save(&dir, &q).is_err());
    assert_eq!(fs::read(dir.join("keep")).unwrap(), b"keep");
    assert!(storage::save(&path.join("child.minidaw"), &q).is_err());
    assert_eq!(storage::read(&path).unwrap(), q);
    assert!(!fs::read_dir(&f.0).unwrap().any(|e| e
        .unwrap()
        .path()
        .extension()
        .is_some_and(|s| s == "tmp")));
}
#[test]
fn unsaved_guard_failed_load_and_failed_runtime_commit_preserve_session() {
    let f = Folder::new();
    let (s, r, _) = setup(&f);
    let before = s.view().unwrap();
    let commits = r.commits.load(Ordering::Relaxed);
    assert_eq!(
        s.new_project(&r, before.revision, false)
            .err()
            .unwrap()
            .code,
        "project_unsaved"
    );
    assert_eq!(
        s.new_project(&r, before.revision + 1, true)
            .err()
            .unwrap()
            .code,
        "project_changed"
    );
    let path = f.file("bad.minidaw");
    fs::write(&path, b"{broken").unwrap();
    assert!(s.open(&r, &path, before.revision, true).is_err());
    assert_eq!(r.commits.load(Ordering::Relaxed), commits);
    r.fail.store(true, Ordering::Relaxed);
    assert!(s.new_project(&r, before.revision, true).is_err());
    assert!(s.import(&r, &fixture("stereo-44100.flac")).is_err());
    assert_eq!(s.view().unwrap().document, before.document);
    r.fail.store(false, Ordering::Relaxed);
    let fresh = s.new_project(&r, before.revision, true).unwrap();
    assert!(!fresh.dirty);
    assert!(fresh.path.is_none());
    assert_ne!(fresh.document.project_id, before.document.project_id);
}
#[test]
fn recents_persist_bound_missing_and_remove_without_project_data() {
    let f = Folder::new();
    let recent = f.file("recent.json");
    let s = ProjectService::new(Some(recent.clone()));
    for n in 0..12 {
        s.save(Some(&f.file(&format!("song{n}.minidaw"))), revision(&s))
            .unwrap();
    }
    let restored = ProjectService::new(Some(recent));
    assert_eq!(restored.view().unwrap().recent.len(), 10);
    assert!(restored.view().unwrap().document.assets.is_empty());
    let first = restored.view().unwrap().recent[0].path.clone();
    fs::remove_file(&first).unwrap();
    assert!(restored.view().unwrap().recent[0].missing);
    restored.remove_recent(&first).unwrap();
    assert_eq!(restored.view().unwrap().recent.len(), 9);
    let data = fs::read_to_string(f.file("song10.minidaw")).unwrap();
    for forbidden in [
        "recent",
        "driverType",
        "transportDeclick",
        "fontScale",
        "waveform",
        "samples",
        "metrics",
        "buffer",
    ] {
        assert!(!data.contains(forbidden), "{forbidden}");
    }
}
#[test]
fn large_sparse_media_metadata_never_serializes_payload() {
    let f = Folder::new();
    let (s, _, _) = setup(&f);
    let mut p = s.view().unwrap().document;
    let asset = f.file("huge.wav");
    fs::File::create(&asset)
        .unwrap()
        .set_len(3 * 1024 * 1024 * 1024)
        .unwrap();
    p.assets[0].path = paths::reference(&asset, Some(&f.file("large.minidaw")));
    p.assets[0].fingerprint = paths::fingerprint(&asset).unwrap();
    p.assets[0].metadata.source_frames = Frames(500_000_000);
    storage::save(&f.file("large.minidaw"), &p).unwrap();
    assert!(fs::metadata(f.file("large.minidaw")).unwrap().len() < 8192);
}
#[test]
fn new_source_id_is_path_independent_and_old_clips_are_retained() {
    let f = Folder::new();
    let (s, r, media) = setup(&f);
    let first = s.view().unwrap();
    let second = s.import(&r, &media).unwrap();
    assert_ne!(
        second.document.assets[0].asset_id,
        second.document.assets[1].asset_id
    );
    assert_eq!(
        second.document.tracks[0].clips[0],
        first.document.tracks[0].clips[0]
    );
    assert_eq!(second.document.clips().count(), 2);
    let selected = s
        .select_clip(
            &r,
            &first.document.primary_clip_id.unwrap(),
            second.revision,
        )
        .unwrap();
    assert_eq!(
        selected.document.primary_clip_id,
        first.document.tracks[0]
            .clips
            .first()
            .map(|c| c.audio().clip_id.clone())
    );
}
