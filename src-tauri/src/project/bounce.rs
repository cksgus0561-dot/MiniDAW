//! Offline Event-only bounce. All decoders, SRC and processing caches live on
//! the project command worker; the callback never sees the renderer or files.
use super::{
    edit::EditRequest,
    paths,
    schema::*,
    session::AssetState,
    time::time,
};
use crate::{
    audio::{
        source::AudioAsset,
        streaming::FrameReader,
        timeline::{PlaybackPlan, TimelineReader},
    },
    error::AppResult,
};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

pub struct Output {
    pub document: Project,
    pub path: PathBuf,
    committed: bool,
}
impl Output {
    pub fn commit(&mut self) {
        self.committed = true;
    }
}
impl Drop for Output {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Retain absolute project coordinates so envelope/SRC phase matches playback.
pub fn event_plan(
    p: &Project,
    ids: &[String],
    assets: HashMap<String, Arc<AudioAsset>>,
    rate: u32,
) -> AppResult<(Arc<PlaybackPlan>, usize, usize, Position)> {
    p.validate()?;
    let (track, selected) = super::glue::selection(p, ids)?;
    let mut isolated = p.clone();
    isolated.tracks = vec![p.tracks[track].clone()];
    let t = &mut isolated.tracks[0];
    t.clips.retain(|c| selected.contains(c.id()));
    t.mix = TrackMix::default();
    t.inserts.clear();
    isolated.automation.clear();
    isolated.master = Default::default();
    isolated.cycle = None;
    isolated.primary_clip_id = t.clips.first().map(|c| c.id().to_string());
    let start = isolated
        .clips()
        .min_by(|a, b| {
            time(&a.position, &p.musical_time).cmp_time(time(&b.position, &p.musical_time))
        })
        .expect("selection")
        .position
        .clone();
    let end = isolated
        .clips()
        .map(|c| {
            let rate = p
                .assets
                .iter()
                .find(|a| a.asset_id == c.asset_id)
                .expect("asset")
                .metadata
                .sample_rate;
            super::tempo_sync::end(c,&p.musical_time,rate).expect("validated clip")
        })
        .max_by(|a, b| a.cmp_time(*b))
        .expect("selection");
    // Preserve the exact project Start, and the existing output sample window.
    // At fractional-sample positions the WAV duration is necessarily quantized
    // (< one output sample); its first/last playback frames remain unchanged.
    let first = usize::try_from(time(&start, &p.musical_time).ceil_frame(rate)).map_err(invalid)?;
    let last = usize::try_from(end.ceil_frame(rate)).map_err(invalid)?;
    if last <= first || (last - first) as u64 > (u32::MAX as u64 - 48) / 8 {
        return Err(invalid(
            "Bounce 길이가 비어 있거나 WAV의 4 GiB 한도를 초과합니다. 구간을 나누어 주세요.",
        ));
    }
    Ok((
        PlaybackPlan::compile(Arc::new(isolated), assets, rate)?,
        first,
        last,
        start,
    ))
}
pub fn render(
    p: &Project,
    states: &[AssetState],
    ids: &[String],
    directory: &Path,
    project_path: Option<&Path>,
    rate: u32,
    replace: bool,
) -> AppResult<Output> {
    let (track, selected) = super::glue::selection(p, ids)?;
    let mut assets = HashMap::new();
    let mut identities = vec![];
    // Zero residency budget: even a large set of small files stays bounded.
    for c in p.clips().filter(|c| selected.contains(&c.clip_id)) {
        if assets.contains_key(&c.asset_id) {
            continue;
        }
        let path = states
            .iter()
            .find(|a| a.asset_id == c.asset_id && a.status == "available")
            .and_then(|a| a.resolved_path.as_deref())
            .ok_or_else(|| invalid("Bounce할 원본 파일을 다시 연결해 주세요."))?;
        let path = PathBuf::from(path);
        let fingerprint = paths::fingerprint(&path)?;
        let meta = p
            .assets
            .iter()
            .find(|a| a.asset_id == c.asset_id)
            .expect("asset");
        if fingerprint != meta.fingerprint {
            return Err(invalid(
                "Bounce 원본 파일이 변경되었습니다. 다시 연결해 주세요.",
            ));
        }
        let asset = AudioAsset::open_with_budget(&path, 0)?;
        if asset.info.sample_rate != meta.metadata.sample_rate
            || asset.info.frames as u64 != meta.metadata.source_frames.0
            || asset.info.channels as u16 != meta.metadata.channels
        {
            return Err(invalid("Bounce 원본 메타데이터가 변경되었습니다."));
        }
        identities.push((path, fingerprint));
        assets.insert(c.asset_id.clone(), asset);
    }
    let (plan, start, end, position) = event_plan(p, ids, assets, rate)?;
    fs::create_dir_all(directory).map_err(invalid)?;
    let filename = format!("Bounce-{}.wav", id());
    let path = directory.join(&filename);
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(invalid)?;
    let mut output = Output {
        document: p.clone(),
        path,
        committed: false,
    };
    write_wave(file, plan, start, end, rate)?;
    for (path, before) in identities {
        if paths::fingerprint(&path)? != before {
            return Err(invalid("Bounce 중 원본 파일이 변경되었습니다."));
        }
    }
    let asset = Asset {
        asset_id: id(),
        filename: filename.clone(),
        path: paths::reference(&output.path, project_path),
        metadata: AudioMetadata {
            sample_rate: rate,
            channels: 2,
            source_frames: Frames((end - start) as u64),
            container: "wav".into(),
            codec: None,
        },
        fingerprint: paths::fingerprint(&output.path)?,
        extensions: Default::default(),
    };
    if replace {
        let c = clip(&asset, position);
        let at = output.document.tracks[track]
            .clips
            .iter()
            .position(|c| selected.contains(c.id()))
            .expect("selected");
        output.document.tracks[track]
            .clips
            .retain(|c| !selected.contains(c.id()));
        output.document.primary_clip_id = Some(c.clip_id.clone());
        output.document.tracks[track]
            .clips
            .insert(at, Clip::Audio(c));
    }
    output.document.assets.push(asset);
    output.document.validate()?;
    Ok(output)
}
fn write_wave(
    file: File,
    plan: Arc<PlaybackPlan>,
    start: usize,
    end: usize,
    rate: u32,
) -> AppResult<()> {
    let mut writer = BufWriter::with_capacity(65536, file);
    let bytes = ((end - start) * 8) as u32;
    let mut header = Vec::with_capacity(56);
    header.extend(b"RIFF");
    header.extend((bytes + 48).to_le_bytes());
    header.extend(b"WAVEfmt ");
    header.extend(16u32.to_le_bytes());
    header.extend(3u16.to_le_bytes());
    header.extend(2u16.to_le_bytes());
    header.extend(rate.to_le_bytes());
    header.extend((rate * 8).to_le_bytes());
    header.extend(8u16.to_le_bytes());
    header.extend(32u16.to_le_bytes());
    header.extend(b"fact");
    header.extend(4u32.to_le_bytes());
    header.extend(((end - start) as u32).to_le_bytes());
    header.extend(b"data");
    header.extend(bytes.to_le_bytes());
    writer.write_all(&header).map_err(invalid)?;
    let mut reader = TimelineReader::new(plan);
    reader.seek(start, Arc::new(|| false))?;
    for _ in start..end {
        for sample in reader.read_frame()? {
            writer.write_all(&sample.to_le_bytes()).map_err(invalid)?;
        }
    }
    writer.flush().map_err(invalid)?;
    writer.get_ref().sync_all().map_err(invalid)
}
fn clip(asset: &Asset, position: Position) -> AudioClip {
    AudioClip {
        clip_id: id(),
        asset_id: asset.asset_id.clone(),
        name: asset.filename.clone(),
        position,
        source_start: Frames(0),
        source_end: asset.metadata.source_frames,
        gain: 1.,
        fade_in: Fade {
            source_frames: Frames(0),
            curve: FadeCurve::Linear,
        },
        fade_out: Fade {
            source_frames: Frames(0),
            curve: FadeCurve::Linear,
        },
        mute: false,
        extensions: Default::default(),
    }
}
pub fn place(p: &Project, r: &EditRequest) -> AppResult<Project> {
    let asset = p
        .assets
        .iter()
        .find(|a| Some(&a.asset_id) == r.asset_id.as_ref())
        .ok_or_else(|| invalid("Asset 없음"))?;
    let mut next = p.clone();
    let t = next
        .tracks
        .iter_mut()
        .find(|t| {
            t.kind == TrackKind::Audio && r.track_ids.first().is_none_or(|id| id == &t.track_id)
        })
        .ok_or_else(|| invalid("배치할 Audio Track을 선택해 주세요."))?;
    let c = clip(
        asset,
        r.cursor.clone().ok_or_else(|| invalid("배치 위치 없음"))?,
    );
    next.primary_clip_id = Some(c.clip_id.clone());
    t.clips.push(Clip::Audio(c));
    next.validate()?;
    Ok(next)
}
