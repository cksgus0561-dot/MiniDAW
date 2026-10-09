//! MIDI commands share the existing project transaction and history boundary.
use super::{edit::EditRequest, schema::*};
use crate::error::AppResult;
use std::collections::HashSet;

pub fn apply(p: &Project, r: &EditRequest) -> AppResult<Project> {
    let mut next = p.clone();
    // Piano Roll commands retain their Part-relative API, including trimmed Parts.
    if r.command.starts_with("midi.note.")
        || matches!(
            r.command.as_str(),
            "midi.velocity"
                | "midi.quantize"
                | "midi.transpose"
                | "midi.control.put"
                | "midi.control.delete"
        )
    {
        for c in next.tracks.iter_mut().flat_map(|t| &mut t.clips) {
            if let Clip::Midi(c) = c {
                if r.clip_ids.first() == Some(&c.clip_id) {
                    c.rebase_content();
                }
            }
        }
    }
    match r.command.as_str() {
        "midi.track.instrument" => {
            let track = next
                .tracks
                .iter_mut()
                .find(|t| t.kind == TrackKind::Midi && r.track_ids.first() == Some(&t.track_id))
                .ok_or_else(|| invalid("MIDI Track을 선택해 주세요."))?;
            track.instrument = r.instrument.ok_or_else(|| invalid("Instrument"))?;
        }
        "midi.clip.resize" => {
            if r.clip_ids.len() != 1 {
                return Err(invalid("MIDI Resize selection"));
            }
            let c = next
                .tracks
                .iter_mut()
                .flat_map(|t| &mut t.clips)
                .find_map(|c| match c {
                    Clip::Midi(c) if r.clip_ids.first() == Some(&c.clip_id) => Some(c),
                    _ => None,
                })
                .ok_or_else(|| invalid("MIDI Resize selection"))?;
            let target = r.target_tick.ok_or_else(|| invalid("Resize tick"))?.0;
            let end = c.start_tick.0 + c.length_tick.0;
            match r.trim_side.as_ref().ok_or_else(|| invalid("Resize side"))? {
                super::edit::TrimSide::Left => {
                    if target < 0 || target >= end {
                        return Err(invalid("Part Start must precede End"));
                    }
                    c.content_offset_tick.0 = c
                        .content_offset_tick
                        .0
                        .checked_add(target - c.start_tick.0)
                        .ok_or_else(|| invalid("Resize offset overflow"))?;
                    c.start_tick = Signed(target);
                    c.length_tick = Signed(end - target);
                }
                super::edit::TrimSide::Right => {
                    if target <= c.start_tick.0 {
                        return Err(invalid("Part length must be at least one tick"));
                    }
                    c.length_tick = Signed(target - c.start_tick.0);
                }
            }
        }
        "midi.note.time" => {
            let c = next
                .tracks
                .iter_mut()
                .flat_map(|t| &mut t.clips)
                .find_map(|c| match c {
                    Clip::Midi(c) if r.clip_ids.first() == Some(&c.clip_id) => Some(c),
                    _ => None,
                })
                .ok_or_else(|| invalid("MIDI Clip selection"))?;
            edit_note_time(c, r)?;
        }
        "midi.track.add" => {
            let mut request = r.clone();
            request.command = "track.add".into();
            request.track_kind = Some(TrackKind::Midi);
            return super::tracks::apply(p, &request);
        }
        "midi.clip.add" => {
            let track = next
                .tracks
                .iter_mut()
                .find(|t| t.kind == TrackKind::Midi && r.track_ids.first() == Some(&t.track_id))
                .ok_or_else(|| invalid("MIDI Track을 선택해 주세요."))?;
            track.clips.push(Clip::Midi(MidiClip {
                content_offset_tick: Signed(0),
                clip_id: id(),
                name: format!("MIDI Part {}", track.clips.len() + 1),
                start_tick: r.target_tick.ok_or_else(|| invalid("Clip start tick"))?,
                length_tick: r.length_tick.ok_or_else(|| invalid("Clip length tick"))?,
                notes: vec![],
                controls: vec![],
            }));
        }
        "midi.clip.delete" => {
            for t in &mut next.tracks {
                t.clips
                    .retain(|c| c.midi().is_none() || !r.clip_ids.iter().any(|id| id == c.id()));
            }
        }
        "midi.clip.move" => {
            let anchor = p
                .midi_clips()
                .find(|c| {
                    r.anchor_clip_id.as_ref() == Some(&c.clip_id) && r.clip_ids.contains(&c.clip_id)
                })
                .ok_or_else(|| invalid("MIDI Move anchor"))?;
            let delta = r
                .target_tick
                .ok_or_else(|| invalid("Move tick"))?
                .0
                .checked_sub(anchor.start_tick.0)
                .ok_or_else(|| invalid("Move overflow"))?;
            for c in next.tracks.iter_mut().flat_map(|t| &mut t.clips) {
                if let Clip::Midi(c) = c {
                    if r.clip_ids.contains(&c.clip_id) {
                        c.start_tick.0 = c
                            .start_tick
                            .0
                            .checked_add(delta)
                            .ok_or_else(|| invalid("Move overflow"))?;
                    }
                }
            }
        }
        "midi.note.add" | "midi.note.change" | "midi.note.delete" => {
            let c = next
                .tracks
                .iter_mut()
                .flat_map(|t| &mut t.clips)
                .find_map(|c| match c {
                    Clip::Midi(c) if r.clip_ids.first() == Some(&c.clip_id) => Some(c),
                    _ => None,
                })
                .ok_or_else(|| invalid("MIDI Clip을 선택해 주세요."))?;
            if r.command == "midi.note.delete" {
                let ids: HashSet<&str> = r.note_ids.iter().map(String::as_str).collect();
                c.notes.retain(|n| {
                    Some(&n.note_id) != r.note_id.as_ref() && !ids.contains(n.note_id.as_str())
                });
            } else {
                let start_tick = r.target_tick.ok_or_else(|| invalid("Note start tick"))?;
                let length_tick = r.length_tick.ok_or_else(|| invalid("Note length tick"))?;
                let pitch = r.pitch.ok_or_else(|| invalid("Note pitch"))?;
                if r.command == "midi.note.add" {
                    valid_tick_range(start_tick.0, length_tick.0)?;
                    if start_tick.0 + length_tick.0 > c.length_tick.0 {
                        return Err(invalid("New Note outside Part"));
                    }
                    c.notes.push(MidiNote {
                        note_id: id(),
                        start_tick,
                        length_tick,
                        pitch,
                        velocity: r.velocity.unwrap_or(100),
                        release_velocity: 0,
                        channel: 0,
                    });
                } else {
                    let n = c
                        .notes
                        .iter_mut()
                        .find(|n| Some(&n.note_id) == r.note_id.as_ref())
                        .ok_or_else(|| invalid("MIDI Note이 변경되었습니다."))?;
                    n.start_tick = start_tick;
                    n.length_tick = length_tick;
                    n.pitch = pitch;
                }
            }
        }
        "midi.velocity" | "midi.quantize" | "midi.transpose" => {
            let c = next
                .tracks
                .iter_mut()
                .flat_map(|t| &mut t.clips)
                .find_map(|c| match c {
                    Clip::Midi(c) if r.clip_ids.first() == Some(&c.clip_id) => Some(c),
                    _ => None,
                })
                .ok_or_else(|| invalid("MIDI Clip selection"))?;
            let selected_ids: HashSet<&str> = r.note_ids.iter().map(String::as_str).collect();
            let selected = |n: &MidiNote| selected_ids.contains(n.note_id.as_str());
            if selected_ids.is_empty()
                || c.notes.iter().filter(|n| selected(n)).count() != selected_ids.len()
            {
                return Err(invalid("Note selection"));
            }
            let amount = r.semitones.unwrap_or(0);
            if r.command == "midi.transpose"
                && c.notes
                    .iter()
                    .filter(|n| selected(n))
                    .any(|n| !(0..=127).contains(&(n.pitch as i32 + amount as i32)))
            {
                return Err(invalid(
                    "선택한 노트가 MIDI 음높이 0~127 범위를 벗어납니다.",
                ));
            }
            if r.command == "midi.quantize" {
                let targets: Vec<_> = c
                    .notes
                    .iter()
                    .enumerate()
                    .filter(|(_, n)| selected(n))
                    .map(|(i, n)| {
                        Ok((
                            i,
                            quantize(
                                c.start_tick.0 + n.start_tick.0,
                                r.grid.as_deref().ok_or_else(|| invalid("Grid"))?,
                                &p.musical_time,
                            )?,
                        ))
                    })
                    .collect::<AppResult<_>>()?;
                let first = targets
                    .iter()
                    .map(|(_, t)| *t)
                    .min()
                    .unwrap()
                    .min(c.start_tick.0);
                let shift = c.start_tick.0 - first;
                c.start_tick = Signed(first);
                c.length_tick.0 += shift;
                // Extending a Part left preserves all unselected absolute events.
                for n in &mut c.notes {
                    n.start_tick.0 += shift;
                }
                for e in &mut c.controls {
                    e.tick.0 += shift;
                }
                for (i, tick) in targets {
                    let n = &mut c.notes[i];
                    n.start_tick = Signed(tick - first);
                    c.length_tick.0 = c.length_tick.0.max(n.start_tick.0 + n.length_tick.0);
                }
            } else {
                for n in c.notes.iter_mut().filter(|n| selected(n)) {
                    if r.command == "midi.velocity" {
                        n.velocity = r.velocity.ok_or_else(|| invalid("Velocity"))?;
                    } else {
                        n.pitch = (n.pitch as i32 + amount as i32) as u8;
                    }
                }
            }
        }
        "midi.control.put" | "midi.control.delete" => {
            let c = next
                .tracks
                .iter_mut()
                .flat_map(|t| &mut t.clips)
                .find_map(|c| match c {
                    Clip::Midi(c) if r.clip_ids.first() == Some(&c.clip_id) => Some(c),
                    _ => None,
                })
                .ok_or_else(|| invalid("MIDI Clip selection"))?;
            if r.command == "midi.control.delete" {
                c.controls
                    .retain(|e| Some(&e.event_id) != r.event_id.as_ref());
            } else {
                let mut e = r.event.clone().ok_or_else(|| invalid("Controller event"))?;
                if e.event_id.is_empty() {
                    e.event_id = id();
                }
                if let Some(old) = c.controls.iter_mut().find(|old| old.event_id == e.event_id) {
                    *old = e;
                } else {
                    c.controls.push(e);
                }
                c.controls.sort_by_key(|e| e.tick.0);
            }
        }
        _ => return Err(invalid("지원하지 않는 MIDI command")),
    }
    next.validate()?;
    Ok(next)
}

// One atomic document edit; no seconds/sample conversion and no Snap rounding.
fn edit_note_time(c: &mut MidiClip, r: &EditRequest) -> AppResult<()> {
    use super::edit::NoteTimeField;
    const MAX: i128 = 9_007_199_254_740_991;
    let ids: HashSet<&str> = r.note_ids.iter().map(String::as_str).collect();
    if ids.is_empty()
        || c.notes
            .iter()
            .filter(|n| ids.contains(n.note_id.as_str()))
            .count()
            != ids.len()
    {
        return Err(invalid("Note selection"));
    }
    let anchor = c
        .notes
        .iter()
        .find(|n| Some(&n.note_id) == r.note_id.as_ref() && ids.contains(n.note_id.as_str()))
        .ok_or_else(|| invalid("Note 기준 선택"))?;
    let field = r
        .note_time_field
        .ok_or_else(|| invalid("Start / End / Length"))?;
    let target = i128::from(r.target_tick.ok_or_else(|| invalid("Note tick"))?.0);
    if !(0..=MAX).contains(&target) {
        return Err(invalid("Note tick 범위"));
    }
    let origin = i128::from(c.start_tick.0);
    let anchor_start = origin + i128::from(anchor.start_tick.0);
    let reference = match field {
        NoteTimeField::Start => anchor_start,
        NoteTimeField::End => anchor_start + i128::from(anchor.length_tick.0),
        NoteTimeField::Length => i128::from(anchor.length_tick.0),
    };
    let delta = target - reference;
    let mut changes = Vec::with_capacity(ids.len());
    let mut first = origin;
    let mut last = origin + i128::from(c.length_tick.0);
    for (index, n) in c
        .notes
        .iter()
        .enumerate()
        .filter(|(_, n)| ids.contains(n.note_id.as_str()))
    {
        let start = origin + i128::from(n.start_tick.0);
        let length = i128::from(n.length_tick.0);
        let end = start + length;
        let value = if r.absolute.unwrap_or(false) {
            target
        } else {
            (match field {
                NoteTimeField::Start => start,
                NoteTimeField::End => end,
                NoteTimeField::Length => length,
            }) + delta
        };
        let (start, end) = match field {
            NoteTimeField::Start => (value, value + length),
            NoteTimeField::End => (start, value),
            NoteTimeField::Length => (start, start + value),
        };
        if start < 0 || end <= start || end > MAX {
            return Err(invalid(
                "모든 선택 Note는 시작 ≥ 0, 길이 ≥ 1 tick이어야 합니다. 변경하지 않았습니다.",
            ));
        }
        first = first.min(start);
        last = last.max(end);
        changes.push((index, start, end));
    }
    // Like Quantize, extend the Part when needed. Other notes/CC keep their
    // absolute positions; never silently clamp the user's numeric destination.
    let shift = (origin - first) as i64;
    for n in &mut c.notes {
        n.start_tick.0 += shift;
    }
    for e in &mut c.controls {
        e.tick.0 += shift;
    }
    for (index, start, end) in changes {
        c.notes[index].start_tick = Signed((start - first) as i64);
        c.notes[index].length_tick = Signed((end - start) as i64);
    }
    c.start_tick = Signed(first as i64);
    c.length_tick = Signed((last - first) as i64);
    Ok(())
}

/// Integer project-grid quantization, independent of Snap on/off and UI FPS.
pub fn quantize(tick: i64, grid: &str, time: &MusicalTime) -> AppResult<i64> {
    let s = time
        .time_signatures
        .iter()
        .rfind(|s| s.tick.0 <= tick)
        .ok_or_else(|| invalid("Signature"))?;
    let q = i64::from(time.ticks_per_quarter);
    let step = match grid {
        "bar" => q * 4 * i64::from(s.numerator) / i64::from(s.denominator),
        "beat" => q * 4 / i64::from(s.denominator),
        _ => {
            let d: i64 = grid.parse().map_err(invalid)?;
            if ![1, 2, 4, 8, 16, 32, 64, 128].contains(&d) {
                return Err(invalid("Grid"));
            }
            q * 4 / d
        }
    };
    if step <= 0 {
        return Err(invalid("Grid tick resolution"));
    }
    let at = s.tick.0 + ((tick - s.tick.0 + step / 2) / step) * step;
    Ok(time
        .time_signatures
        .iter()
        .find(|n| n.tick.0 > s.tick.0)
        .map_or(at, |n| at.min(n.tick.0)))
}
