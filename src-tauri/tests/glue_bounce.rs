use minidaw_lib::{
    audio::{
        source::AudioAsset,
        streaming::FrameReader,
        timeline::{PlaybackPlan, TimelineReader},
    },
    error::{AppError, AppResult},
    project::{
        bounce,
        edit::{self, Clipboard},
        glue, migrations, paths,
        runtime::ProjectAudio,
        schema::*,
        session::{resolve_all, ProjectService},
        time::{time, Time},
    },
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
struct Folder(PathBuf);
impl Folder {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("minidaw-bounce-test-{}", id()));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Folder {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/fixtures/stereo-44100.wav")
}
fn edit(p: &Project, r: Value) -> Project {
    edit::apply(
        p,
        &serde_json::from_value(r).unwrap(),
        &mut Clipboard::default(),
    )
    .unwrap()
}
fn ids(p: &Project) -> Vec<String> {
    p.clips().map(|c| c.clip_id.clone()).collect()
}
fn document(path: &Path) -> Project {
    let a = AudioAsset::open(path).unwrap();
    let mut p = Project::new();
    p.import(Asset {
        asset_id: id(),
        filename: "source.wav".into(),
        path: paths::reference(path, None),
        metadata: AudioMetadata {
            sample_rate: a.info.sample_rate,
            channels: a.info.channels as u16,
            source_frames: Frames(a.info.frames as u64),
            container: "wav".into(),
            codec: None,
        },
        fingerprint: paths::fingerprint(path).unwrap(),
        extensions: Default::default(),
    });
    p
}
fn assets(p: &Project, budget: usize) -> HashMap<String, Arc<AudioAsset>> {
    p.assets
        .iter()
        .map(|a| {
            (
                a.asset_id.clone(),
                AudioAsset::open_with_budget(
                    Path::new(a.path.original_absolute_path.as_ref().unwrap()),
                    budget,
                )
                .unwrap(),
            )
        })
        .collect()
}
fn samples(plan: Arc<PlaybackPlan>, start: usize, end: usize) -> Vec<[f32; 2]> {
    let mut r = TimelineReader::new(plan);
    r.seek(start, Arc::new(|| false)).unwrap();
    (start..end).map(|_| r.read_frame().unwrap()).collect()
}
fn event_samples(p: &Project, budget: usize, rate: u32) -> (usize, Vec<[f32; 2]>) {
    let (plan, start, end, _) = bounce::event_plan(p, &ids(p), assets(p, budget), rate).unwrap();
    (start, samples(plan, start, end))
}
#[test]
fn split_glue_restores_source_window_envelope_processing_and_pcm_exactly() {
    for processed in [false, true] {
        let mut p = document(&fixture());
        let id = ids(&p)[0].clone();
        p = edit(
            &p,
            json!({"command":"audio.trim","clipIds":[id],"sourceStart":"441","sourceEnd":"22050"}),
        );
        p = edit(
            &p,
            json!({"command":"audio.gain","clipIds":[id],"gainDb":-3.7}),
        );
        p = edit(
            &p,
            json!({"command":"audio.fade","clipIds":[id],"fadeIn":"5000","fadeOut":"7000","curve":"cosine"}),
        );
        if processed {
            p = edit(
                &p,
                json!({"command":"audio.stretch","clipIds":[id],"stretchFrames":"30000"}),
            );
            p = edit(
                &p,
                json!({"command":"audio.pitch","clipIds":[id],"pitchShift":{"semitones":3,"cents":17}}),
            );
        }
        p = edit(
            &p,
            json!({"command":"audio.move","clipIds":[id],"anchorClipId":id,"targetTick":"1212345"}),
        );
        let original = p.clips().next().unwrap().clone();
        let expected = event_samples(&p, usize::MAX, 48000);
        let split = time(&original.position, &p.musical_time)
            .plus(Time::frames(9000, 44100))
            .position()
            .unwrap();
        let split = edit(
            &p,
            json!({"command":"audio.splitAtCursor","clipIds":[id],"cursor":split}),
        );
        assert_eq!(ids(&split).len(), 2);
        let joined = glue::apply(&split, &ids(&split), false).unwrap();
        let c = joined.clips().next().unwrap();
        assert_eq!(ids(&joined).len(), 1);
        assert_eq!(c.source_start, original.source_start);
        assert_eq!(c.source_end, original.source_end);
        assert_eq!(c.fade_in, original.fade_in);
        assert_eq!(c.fade_out, original.fade_out);
        assert_eq!(c.extensions, original.extensions);
        assert_eq!(
            time(&c.position, &p.musical_time),
            time(&original.position, &p.musical_time)
        );
        assert_eq!(expected, event_samples(&joined, 0, 48000));
    }
}
#[test]
fn incompatible_events_form_persistent_part_and_duplicate_gets_new_group() {
    let mut p = document(&fixture());
    let mut c = p.clips().next().unwrap().clone();
    c.clip_id = id();
    c.position = Time::new(2, 1).position().unwrap();
    c.gain = 0.25;
    p.tracks[0].clips.push(Clip::Audio(c));
    let expected = event_samples(&p, usize::MAX, 44100);
    let grouped = glue::apply(&p, &ids(&p), false).unwrap();
    let part = glue::part(grouped.clips().next().unwrap())
        .unwrap()
        .unwrap();
    assert!(grouped
        .clips()
        .all(|c| glue::part(c).unwrap().unwrap() == part));
    assert_eq!(expected, event_samples(&grouped, 0, 44100));
    assert_eq!(
        grouped,
        migrations::decode(&migrations::encode(&grouped).unwrap()).unwrap()
    );
    let copy = edit(
        &grouped,
        json!({"command":"edit.duplicate","clipIds":ids(&grouped)}),
    );
    let parts: Vec<_> = copy
        .clips()
        .map(|c| glue::part(c).unwrap().unwrap().part_id)
        .collect();
    assert_eq!(parts[0], parts[1]);
    assert_eq!(parts[2], parts[3]);
    assert_ne!(parts[0], parts[2]);
    let plain = glue::apply(&grouped, &[ids(&grouped)[0].clone()], true).unwrap();
    assert_eq!(p, plain);
    let mut other = grouped.clone();
    let mut track = other.tracks[0].clone();
    track.track_id = id();
    for c in &mut track.clips {
        c.as_audio_mut().unwrap().clip_id = id();
    }
    other.tracks.push(track);
    assert!(glue::apply(&other, &ids(&other), false).is_err());
}
fn arrangement() -> Project {
    let mut p = document(&fixture());
    let original = p.clips().next().unwrap().clone();
    p.tracks[0].clips.clear();
    for (start, len, gain, mute) in [
        (0., 17640, 0.5, false),
        (0.2, 17640, 20., false),
        (1.2, 13230, 0.7, false),
        (1.8, 8820, 1., true),
    ] {
        let mut c = original.clone();
        c.clip_id = id();
        c.position = Time::new((start * 1000.) as i128, 1000).position().unwrap();
        c.source_end = Frames(len);
        c.gain = gain;
        c.mute = mute;
        c.fade_in.source_frames = Frames(1000);
        c.fade_out.source_frames = Frames(2000);
        p.tracks[0].clips.push(Clip::Audio(c));
    }
    p.primary_clip_id = Some(ids(&p)[0].clone());
    let third = ids(&p)[2].clone();
    p = edit(
        &p,
        json!({"command":"audio.stretch","clipIds":[third],"stretchFrames":"19845"}),
    );
    p = edit(
        &p,
        json!({"command":"audio.pitch","clipIds":[third],"pitchShift":{"semitones":-5,"cents":37}}),
    );
    p
}
#[test]
fn bounce_is_exact_event_mix_including_gaps_overlaps_mute_gain_fades_stretch_pitch() {
    let f = Folder::new();
    let mut p = arrangement();
    let original = fs::read(fixture()).unwrap();
    // Mixer state must never leak into the file, even a completely muted Track.
    let expected = event_samples(&p, usize::MAX, 48000);
    assert!(expected.1.iter().flatten().any(|s| s.abs() > 1.));
    assert!(expected.1[40000..55000].iter().all(|s| *s == [0.; 2]));
    assert!(expected.1[86400..].iter().all(|s| *s == [0.; 2]));
    p.tracks[0].mix.mute = true;
    p.tracks[0].mix.volume_db = -25.;
    p.tracks[0].mix.pan = 0.75;
    p.master.volume_db = -96.;
    for kind in ["eq", "compressor", "limiter", "reverb", "delay"] {
        p = edit(
            &p,
            json!({"command":"effect.add","trackIds":[p.tracks[0].track_id],"effectKind":kind}),
        );
    }
    p = edit(&p, json!({"command":"effect.add","effectKind":"reverb"}));
    p = edit(
        &p,
        json!({"command":"automation.lane.add","trackIds":[p.tracks[0].track_id],"parameter":{"name":"volumeDb"}}),
    );
    let lane = p.automation[0].lanes[0].lane_id.clone();
    p = edit(
        &p,
        json!({"command":"automation.point.set","trackIds":[p.tracks[0].track_id],"laneId":lane,"parameter":{"name":"volumeDb"},"targetTick":"0","value":-96}),
    );
    p = edit(
        &p,
        json!({"command":"automation.channel","trackIds":[p.tracks[0].track_id],"read":true}),
    );
    assert_eq!(expected, event_samples(&p, 0, 48000));
    let mut output = bounce::render(
        &p,
        &resolve_all(&p, None),
        &ids(&p),
        &f.0,
        None,
        48000,
        true,
    )
    .unwrap();
    let a = AudioAsset::open_with_budget(&output.path, usize::MAX).unwrap();
    assert_eq!(a.info.frames, 96000);
    assert_eq!(ids(&output.document).len(), 1);
    assert_eq!(output.document.tracks[0].mix, p.tracks[0].mix);
    assert_eq!(output.document.master, p.master);
    assert_eq!(output.document.automation, p.automation);
    assert_eq!(event_samples(&output.document, usize::MAX, 48000), expected);
    assert_eq!(event_samples(&output.document, 0, 48000), expected);
    assert_eq!(fs::read(fixture()).unwrap(), original);
    assert_eq!(output.document.assets.len(), 2);
    output.commit();
    let kept = bounce::render(
        &p,
        &resolve_all(&p, None),
        &ids(&p),
        &f.0,
        None,
        48000,
        false,
    )
    .unwrap();
    assert_eq!(kept.document.tracks, p.tracks);
    assert_eq!(kept.document.assets.len(), 2);
    let placed = edit(
        &kept.document,
        json!({"command":"audio.placeAsset","assetId":kept.document.assets.last().unwrap().asset_id,"cursor":{"unit":"ticks","ticks":"3840000"}}),
    );
    assert_eq!(ids(&placed).len(), ids(&p).len() + 1);
}
#[test]
fn fractional_tick_start_and_mixed_rates_preserve_device_sample_coverage() {
    let f = Folder::new();
    let mut p = arrangement();
    let chosen = ids(&p);
    p = edit(
        &p,
        json!({"command":"audio.move","clipIds":chosen,"anchorClipId":chosen[0],"targetTick":"1920137"}),
    );
    for rate in [44100, 48000, 96000] {
        let expected = event_samples(&p, 0, rate);
        let output =
            bounce::render(&p, &resolve_all(&p, None), &ids(&p), &f.0, None, rate, true).unwrap();
        let c = output.document.clips().next().unwrap();
        assert_eq!(c.position, p.clips().next().unwrap().position);
        assert_eq!(event_samples(&output.document, 0, rate), expected);
    }
}
#[derive(Default)]
struct Runtime {
    fail: AtomicBool,
}
impl ProjectAudio for Runtime {
    type Prepared = Arc<AudioAsset>;
    fn prepare_asset(&self, a: Self::Prepared) -> AppResult<Self::Prepared> {
        Ok(a)
    }
    fn install(&self, _: Option<Self::Prepared>) -> AppResult<()> {
        if self.fail.load(Ordering::Relaxed) {
            Err(AppError::new("test", "commit failure"))
        } else {
            Ok(())
        }
    }
    fn arrangement(&self) -> bool {
        true
    }
}
fn service_edit(
    s: &ProjectService,
    r: &Runtime,
    request: Value,
) -> AppResult<minidaw_lib::project::session::ProjectView> {
    s.edit(
        r,
        s.view().unwrap().revision,
        serde_json::from_value(request).unwrap(),
    )
}
#[test]
fn bounce_assets_history_save_reopen_and_failed_commit_cleanup() {
    let f = Folder::new();
    let s = ProjectService::new(Some(f.0.join("recent.json")));
    let r = Runtime::default();
    s.import(&r, &fixture()).unwrap();
    let path = f.0.join("Song.minidaw");
    s.save(Some(&path), s.view().unwrap().revision).unwrap();
    let before = s.view().unwrap().document;
    let request =
        json!({"command":"audio.consolidate","clipIds":ids(&before),"bounceReplace":true});
    r.fail.store(true, Ordering::Relaxed);
    assert!(service_edit(&s, &r, request.clone()).is_err());
    assert_eq!(before, s.view().unwrap().document);
    assert_eq!(fs::read_dir(f.0.join("Song.media")).unwrap().count(), 0);
    r.fail.store(false, Ordering::Relaxed);
    let after = service_edit(&s, &r, request).unwrap().document;
    let file = PathBuf::from(
        after
            .assets
            .last()
            .unwrap()
            .path
            .original_absolute_path
            .as_ref()
            .unwrap(),
    );
    assert!(file.exists());
    service_edit(&s, &r, json!({"command":"edit.undo"})).unwrap();
    assert_eq!(before, s.view().unwrap().document);
    assert!(file.exists());
    service_edit(&s, &r, json!({"command":"edit.redo"})).unwrap();
    assert_eq!(after, s.view().unwrap().document);
    s.save(None, s.view().unwrap().revision).unwrap();
    let after = s.view().unwrap().document;
    let reopened = ProjectService::new(None);
    reopened
        .open(&r, &path, reopened.view().unwrap().revision, true)
        .unwrap();
    let mut restored = reopened.view().unwrap().document;
    for (a, b) in restored.assets.iter_mut().zip(&after.assets) {
        assert_eq!(
            fs::canonicalize(a.path.original_absolute_path.as_ref().unwrap()).unwrap(),
            fs::canonicalize(b.path.original_absolute_path.as_ref().unwrap()).unwrap()
        );
        a.path.original_absolute_path = b.path.original_absolute_path.clone();
    }
    assert_eq!(restored, after);
    assert!(reopened
        .view()
        .unwrap()
        .assets
        .iter()
        .all(|a| a.status == "available"));
    let kept = service_edit(
        &s,
        &r,
        json!({"command":"audio.consolidate","clipIds":ids(&after),"bounceReplace":false}),
    )
    .unwrap()
    .document;
    assert_eq!(kept.tracks, after.tracks);
    service_edit(&s, &r, json!({"command":"edit.undo"})).unwrap();
    assert_eq!(s.view().unwrap().document, after);
    service_edit(&s, &r, json!({"command":"edit.redo"})).unwrap();
    assert_eq!(s.view().unwrap().document, kept);
}

#[test]
fn crossfade_and_mixed_rate_assets_bounce_without_source_changes() {
    let f = Folder::new();
    let path = fixture();
    let original = fs::read(&path).unwrap();
    let mut p = document(&path);
    let mut c = p.clips().next().unwrap().clone();
    p.tracks[0].clips[0].as_audio_mut().unwrap().source_end = Frames(22050);
    c.clip_id = id();
    c.source_start = Frames(22050);
    c.source_end = Frames(44100);
    c.position = Time::new(1, 2).position().unwrap();
    p.tracks[0].clips.push(Clip::Audio(c));
    p = edit(&p, json!({"command":"audio.crossfade","clipIds":ids(&p)}));
    let mixed = bounce::render(
        &p,
        &resolve_all(&p, None),
        &ids(&p),
        &f.0,
        None,
        48000,
        false,
    )
    .unwrap();
    let extra = mixed.document.assets.last().unwrap().clone();
    let mut p = mixed.document.clone();
    p = edit(
        &p,
        json!({"command":"audio.placeAsset","assetId":extra.asset_id,"cursor":{"unit":"ticks","ticks":"960000"}}),
    );
    for rate in [44100, 48000, 96000] {
        let expected = event_samples(&p, usize::MAX, rate);
        assert_eq!(expected, event_samples(&p, 0, rate));
        let output =
            bounce::render(&p, &resolve_all(&p, None), &ids(&p), &f.0, None, rate, true).unwrap();
        assert_eq!(expected, event_samples(&output.document, 0, rate));
    }
    assert_eq!(fs::read(&path).unwrap(), original);
}
