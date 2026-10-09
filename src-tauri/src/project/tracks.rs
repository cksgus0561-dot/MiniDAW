//! Track edits share the existing immutable transaction/history boundary.
use super::{
    edit::{self, Clipboard, EditRequest},
    schema::*,
};
use crate::error::AppResult;
use std::collections::HashSet;

pub fn transfer(p: &mut Project, ids: &[String], target: &str) -> AppResult<()> {
    let destination = p
        .tracks
        .iter()
        .position(|t| t.track_id == target)
        .ok_or_else(|| invalid("대상 Track이 없습니다."))?;
    let selected: HashSet<_> = ids.iter().map(String::as_str).collect();
    if selected.is_empty() || selected.len() != ids.len() {
        return Err(invalid("Clip 선택"));
    }
    let mut found = 0;
    for t in &p.tracks {
        for c in &t.clips {
            if selected.contains(c.id()) {
                found += 1;
                if t.kind != p.tracks[destination].kind {
                    return Err(invalid("같은 종류의 Track으로만 이동할 수 있습니다."));
                }
            }
        }
    }
    if found != selected.len() {
        return Err(invalid("이동할 Clip이 변경되었습니다."));
    }
    let mut moving = vec![];
    for (i, t) in p.tracks.iter_mut().enumerate() {
        if i == destination {
            continue;
        }
        let mut retained = Vec::with_capacity(t.clips.len());
        for clip in std::mem::take(&mut t.clips) {
            if selected.contains(clip.id()) {
                moving.push(clip);
            } else {
                retained.push(clip);
            }
        }
        t.clips = retained;
    }
    p.tracks[destination].clips.extend(moving);
    Ok(())
}
pub fn apply(p: &Project, r: &EditRequest) -> AppResult<Project> {
    let mut next = p.clone();
    match r.command.as_str() {
        "track.add" => {
            let kind = r.track_kind.clone().ok_or_else(|| invalid("Track 종류"))?;
            let label = if kind == TrackKind::Audio {
                "Audio"
            } else {
                "MIDI"
            };
            let mut number = 1;
            while next
                .tracks
                .iter()
                .any(|t| t.name == format!("{label} {number}"))
            {
                number += 1;
            }
            next.tracks.push(Track {
                synth: Default::default(),
                inserts: vec![],
                track_id: id(),
                name: format!("{label} {number}"),
                kind,
                instrument: Instrument::None,
                mix: TrackMix::default(),
                clips: vec![],
                extensions: Extensions::new(),
            });
        }
        "track.delete" => {
            if r.track_ids.is_empty()
                || r.track_ids
                    .iter()
                    .any(|id| !next.tracks.iter().any(|t| &t.track_id == id))
            {
                return Err(invalid("삭제할 Track"));
            }
            next.tracks.retain(|t| !r.track_ids.contains(&t.track_id));
        }
        "track.move" => {
            if r.track_ids.len() != 1 {
                return Err(invalid("Track 선택"));
            }
            let i = next
                .tracks
                .iter()
                .position(|t| r.track_ids.first() == Some(&t.track_id))
                .ok_or_else(|| invalid("Track 선택"))?;
            let direction = r
                .direction
                .filter(|d| matches!(d, -1 | 1))
                .ok_or_else(|| invalid("Track 이동 방향"))?;
            let target =
                (i as isize + direction as isize).clamp(0, next.tracks.len() as isize - 1) as usize;
            next.tracks.swap(i, target);
        }
        "track.mix" => {
            if r.track_ids.len() != 1 {
                return Err(invalid("Track 선택"));
            }
            let t = next
                .tracks
                .iter_mut()
                .find(|t| r.track_ids.first() == Some(&t.track_id))
                .ok_or_else(|| invalid("Track 선택"))?;
            if let Some(v) = r.mute {
                t.mix.mute = v;
            }
            if let Some(v) = r.solo {
                t.mix.solo = v;
            }
            if let Some(v) = r.volume_db {
                t.mix.volume_db = v;
            }
            if let Some(v) = r.pan {
                t.mix.pan = v;
            }
        }
        "clip.moveTrack" => {
            let target = r
                .target_track_id
                .as_deref()
                .ok_or_else(|| invalid("대상 Track"))?;
            transfer(&mut next, &r.clip_ids, target)?;
            // Pure vertical transfer never converts seconds/samples/ticks. For a
            // diagonal gesture reuse the original exact Move arithmetic once.
            if r.target_tick.is_some() {
                let kind = &next
                    .tracks
                    .iter()
                    .find(|t| t.track_id == target)
                    .unwrap()
                    .kind;
                let mut request = r.clone();
                request.command = if *kind == TrackKind::Midi {
                    "midi.clip.move"
                } else {
                    "audio.move"
                }
                .into();
                next = edit::apply(&next, &request, &mut Clipboard::default())?;
            }
        }
        _ => return Err(invalid("Track command")),
    }
    if next
        .primary_clip_id
        .as_ref()
        .is_some_and(|id| !next.clips().any(|c| &c.clip_id == id))
    {
        let first = next.clips().next().map(|c| c.clip_id.clone());
        next.primary_clip_id = first;
    }
    super::automation::prune(&mut next);
    next.validate()?;
    Ok(next)
}
