//! Audio Parts retain their constituent Events; no source or DSP is rewritten.
use super::{
    schema::*,
    time::time,
};
use crate::error::AppResult;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub const KEY: &str = "minidaw.audioPart.v1";
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Part {
    pub part_id: String,
    pub name: String,
}
pub fn part(c: &AudioClip) -> AppResult<Option<Part>> {
    c.extensions
        .get(KEY)
        .map(|v| {
            let p: Part = serde_json::from_value(v.clone()).map_err(invalid)?;
            uuid::Uuid::parse_str(&p.part_id).map_err(invalid)?;
            if p.name.len() > 4096 {
                return Err(invalid("Audio Part 이름 길이"));
            }
            Ok(p)
        })
        .transpose()
}
pub fn remap(c: &mut AudioClip, ids: &mut HashMap<String, String>) -> AppResult<()> {
    if let Some(mut p) = part(c)? {
        p.part_id = ids.entry(p.part_id).or_insert_with(id).clone();
        c.extensions
            .insert(KEY.into(), serde_json::to_value(p).map_err(invalid)?);
    }
    Ok(())
}
/// Resolve one Track and expand selected Parts, including for Bounce Selection.
pub fn selection(p: &Project, ids: &[String]) -> AppResult<(usize, HashSet<String>)> {
    let mut selected: HashSet<String> = ids.iter().cloned().collect();
    if selected.is_empty() {
        return Err(invalid("같은 Audio Track의 Event를 선택해 주세요."));
    }
    let tracks: Vec<_> = p
        .tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.clips.iter().any(|c| selected.contains(c.id())))
        .collect();
    if tracks.len() != 1
        || tracks[0].1.kind != TrackKind::Audio
        || selected.iter().any(|id| {
            !tracks[0]
                .1
                .clips
                .iter()
                .any(|c| c.as_audio().is_some_and(|c| &c.clip_id == id))
        })
    {
        return Err(invalid("같은 Audio Track의 Event만 선택해 주세요."));
    }
    let (index, track) = tracks[0];
    let parts: HashSet<_> = track
        .clips
        .iter()
        .filter_map(Clip::as_audio)
        .filter(|c| selected.contains(&c.clip_id))
        .filter_map(|c| part(c).ok().flatten().map(|p| p.part_id))
        .collect();
    for c in track.clips.iter().filter_map(Clip::as_audio) {
        if part(c)?.is_some_and(|p| parts.contains(&p.part_id)) {
            selected.insert(c.clip_id.clone());
        }
    }
    Ok((index, selected))
}
pub fn apply(p: &Project, ids: &[String], dissolve: bool) -> AppResult<Project> {
    let (track, selected) = selection(p, ids)?;
    let mut next = p.clone();
    if dissolve {
        for c in next.tracks[track]
            .clips
            .iter_mut()
            .filter_map(Clip::as_audio_mut)
            .filter(|c| selected.contains(&c.clip_id))
        {
            c.extensions.remove(KEY);
        }
    } else {
        let mut clips: Vec<_> = p.tracks[track]
            .clips
            .iter()
            .filter_map(Clip::as_audio)
            .filter(|c| selected.contains(&c.clip_id))
            .collect();
        if clips.len() < 2 {
            return Err(invalid("Glue할 Event 두 개 이상을 선택해 주세요."));
        }
        clips.sort_by(|a, b| {
            time(&a.position, &p.musical_time).cmp_time(time(&b.position, &p.musical_time))
        });
        if let Some(c) = joined(p, &clips)? {
            let at = next.tracks[track]
                .clips
                .iter()
                .position(|c| selected.contains(c.id()))
                .expect("selected");
            next.tracks[track]
                .clips
                .retain(|c| !selected.contains(c.id()));
            next.primary_clip_id = Some(c.clip_id.clone());
            next.tracks[track].clips.insert(at, Clip::Audio(c));
        } else {
            let part = serde_json::to_value(Part {
                part_id: id(),
                name: clips[0].name.clone(),
            })
            .map_err(invalid)?;
            for c in next.tracks[track]
                .clips
                .iter_mut()
                .filter_map(Clip::as_audio_mut)
                .filter(|c| selected.contains(&c.clip_id))
            {
                c.extensions.insert(KEY.into(), part.clone());
            }
        }
    }
    next.validate()?;
    Ok(next)
}
fn joined(p: &Project, clips: &[&AudioClip]) -> AppResult<Option<AudioClip>> {
    let first = clips[0];
    let rate = p
        .assets
        .iter()
        .find(|a| a.asset_id == first.asset_id)
        .expect("asset")
        .metadata
        .sample_rate;
    let clean = |c: &AudioClip| {
        let mut e = c.extensions.clone();
        e.remove(KEY);
        e.remove(super::tempo_sync::KEY);
        e
    };
    for pair in clips.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let continuous = super::tempo_sync::end(a,&p.musical_time,rate)?
            .cmp_time(time(&b.position, &p.musical_time))
            .is_eq();
        if a.asset_id != b.asset_id
            || a.source_end != b.source_start
            || !continuous
            || a.gain != b.gain
            || a.mute != b.mute
            || clean(a) != clean(b)
            || !super::tempo_sync::compatible(a,b)?
        {
            return Ok(None);
        }
        if !a.extensions.contains_key(ENVELOPE_WINDOW)
            && (a.fade_out.source_frames.0 != 0 || b.fade_in.source_frames.0 != 0)
        {
            return Ok(None);
        }
    }
    let mut c = first.clone();
    c.clip_id = id();
    c.extensions.remove(KEY);
    c.source_end = clips.last().expect("clips").source_end;
    c.fade_out = clips.last().expect("clips").fade_out.clone();
    if let Some(w) = c.extensions.get(ENVELOPE_WINDOW) {
        let w: EnvelopeWindow = serde_json::from_value(w.clone()).map_err(invalid)?;
        if w.source_start == c.source_start && w.source_end == c.source_end {
            c.fade_in = w.fade_in;
            c.fade_out = w.fade_out;
            c.extensions.remove(ENVELOPE_WINDOW);
        }
    }
    super::tempo_sync::joined(first,clips.last().expect("clips"),&mut c,&p.musical_time,rate)?;
    Ok(Some(c))
}
