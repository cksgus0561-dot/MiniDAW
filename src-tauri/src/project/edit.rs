//! UI-independent musical commands. One call produces one validated transaction.
use super::{
    schema::*,
    time::{time, Time},
};
use crate::error::AppResult;
use serde::Deserialize;
use std::collections::HashSet;
#[derive(Clone, Default)]
pub struct Clipboard {
    pub clips: Vec<(String, AudioClip)>,
    pub origin: Option<Time>,
    pub musical_origin: Option<i64>,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EditRequest {
    pub command: String,
    pub parameter: Option<super::automation::Parameter>,
    pub lane_id: Option<String>,
    pub point_id: Option<String>,
    pub value: Option<f64>,
    pub shape: Option<super::automation::Shape>,
    pub read: Option<bool>,
    pub write: Option<bool>,
    pub synth: Option<super::automation::SynthSettings>,
    pub effect_kind: Option<String>,
    pub plugin: Option<crate::plugins::Selection>,
    pub effect_id: Option<String>,
    pub effect: Option<super::effects::Effect>,
    // Optional UI fader gesture, not musical/project data.
    pub history_group: Option<String>,
    #[serde(default)]
    pub clip_ids: Vec<String>,
    #[serde(default)]
    pub track_ids: Vec<String>,
    pub cursor: Option<Position>,
    pub range_end: Option<Position>,
    pub delta: Option<Position>,
    pub source_start: Option<Frames>,
    pub source_end: Option<Frames>,
    pub stretch_frames: Option<Frames>,
    pub tempo_sync: Option<bool>,
    pub source_bpm: Option<f64>,
    pub pitch_shift: Option<super::pitch::Pitch>,
    pub gain_db: Option<f64>,
    pub fade_in: Option<Frames>,
    pub fade_out: Option<Frames>,
    pub curve: Option<FadeCurve>,
    pub normalize_target_db: Option<f64>,
    pub bpm: Option<f64>,
    pub numerator: Option<u8>,
    pub denominator: Option<u8>,
    pub cycle: Option<Cycle>,
    // Absolute project tick destinations avoid repeated UI seconds/sample deltas.
    pub target_tick: Option<Signed>,
    pub anchor_clip_id: Option<String>,
    pub trim_side: Option<TrimSide>,
    pub note_id: Option<String>,
    pub length_tick: Option<Signed>,
    pub pitch: Option<u8>,
    #[serde(default)]
    pub note_ids: Vec<String>,
    pub velocity: Option<u8>,
    pub semitones: Option<i16>,
    pub grid: Option<String>,
    pub event: Option<MidiControl>,
    pub event_id: Option<String>,
    pub path: Option<String>,
    pub bounce_replace: Option<bool>,
    pub asset_id: Option<String>,
    pub import_tempo: Option<bool>,
    pub note_time_field: Option<NoteTimeField>,
    pub absolute: Option<bool>,
    pub instrument: Option<Instrument>,
    pub track_kind: Option<TrackKind>,
    pub target_track_id: Option<String>,
    pub direction: Option<i8>,
    pub mute: Option<bool>,
    pub solo: Option<bool>,
    pub volume_db: Option<f64>,
    pub pan: Option<f64>,
}
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NoteTimeField {
    Start,
    End,
    Length,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrimSide {
    Left,
    Right,
}
fn rate(p: &Project, c: &AudioClip) -> u32 {
    p.assets
        .iter()
        .find(|a| a.asset_id == c.asset_id)
        .expect("validated asset")
        .metadata
        .sample_rate
}
fn end(p: &Project, c: &AudioClip) -> Time {
    super::tempo_sync::end(c, &p.musical_time, rate(p,c)).expect("validated clip")
}
fn boundary(p: &Project, c: &AudioClip, at: Time) -> u64 {
    (c.source_start.0 as i128
        + at.minus(time(&c.position, &p.musical_time))
            .frame(rate(p, c)))
    .clamp(c.source_start.0 as i128, c.source_end.0 as i128) as u64
}
fn bounded_fades(c: &mut AudioClip) {
    let n = c.source_end.0 - c.source_start.0;
    c.fade_in.source_frames.0 = c.fade_in.source_frames.0.min(n);
    c.fade_out.source_frames.0 = c.fade_out.source_frames.0.min(n);
}
fn slice(p: &Project, c: &AudioClip, start: u64, end: u64, new_id: bool, boundaries: &[Time]) -> AppResult<AudioClip> {
    let mut out = c.clone();
    if (c.fade_in.source_frames.0 > 0 || c.fade_out.source_frames.0 > 0)
        && !out.extensions.contains_key(ENVELOPE_WINDOW)
    {
        out.extensions.insert(
            ENVELOPE_WINDOW.into(),
            serde_json::to_value(EnvelopeWindow {
                source_start: c.source_start,
                source_end: c.source_end,
                fade_in: c.fade_in.clone(),
                fade_out: c.fade_out.clone(),
            })
            .map_err(invalid)?,
        );
    }
    if new_id {
        out.clip_id = id();
    }
    out.position = time(&c.position, &p.musical_time)
        .plus(Time::frames(start - c.source_start.0, rate(p, c)))
        .position()?;
    out.source_start = Frames(start);
    out.source_end = Frames(end);
    if start != c.source_start.0 {
        out.fade_in.source_frames = Frames(0);
    }
    if end != c.source_end.0 {
        out.fade_out.source_frames = Frames(0);
    }
    bounded_fades(&mut out);
    super::tempo_sync::sliced(c,&mut out,&p.musical_time,rate(p,c),boundaries)?;
    Ok(out)
}
pub fn apply(p: &Project, r: &EditRequest, clipboard: &mut Clipboard) -> AppResult<Project> {
    p.validate()?;
    if matches!(r.command.as_str(), "audio.glue" | "audio.dissolvePart") { return super::glue::apply(p, &r.clip_ids, r.command == "audio.dissolvePart"); }
    if r.command == "audio.placeAsset" { return super::bounce::place(p, r); }
    if r.command.starts_with("automation.") {
        return super::automation::apply(p, r);
    }
    if r.command == "synth.set" {
        let mut next = p.clone();
        let t = next
            .tracks
            .iter_mut()
            .find(|t| r.track_ids.first() == Some(&t.track_id) && t.kind == TrackKind::Midi)
            .ok_or_else(|| invalid("Synth Track"))?;
        t.synth = r.synth.clone().ok_or_else(|| invalid("Synth settings"))?;
        next.validate()?;
        return Ok(next);
    }
    if r.command.starts_with("plugin.") { return crate::plugins::apply(p,r); }
    if r.command.starts_with("effect.") {
        return super::effects::apply(p, r);
    }
    if r.command == "master.volume" {
        let mut next = p.clone();
        next.master.volume_db = r.volume_db.ok_or_else(|| invalid("Master Volume"))?;
        next.validate()?;
        return Ok(next);
    }
    if r.command.starts_with("track.") || r.command == "clip.moveTrack" {
        return super::tracks::apply(p, r);
    }
    if r.command.starts_with("midi.") {
        return super::midi::apply(p, r);
    }
    for position in [&r.cursor, &r.range_end, &r.delta].into_iter().flatten() {
        if matches!(position, Position::Seconds { denominator, .. } if *denominator == 0 || *denominator > MAX_TIME_DENOMINATOR)
        {
            return Err(invalid("편집 시간 분모 범위"));
        }
    }
    if r.clip_ids.len() > 100_000 || r.track_ids.len() > 4096 {
        return Err(invalid("편집 선택 개수"));
    }
    let selected: HashSet<_> = r.clip_ids.iter().map(String::as_str).collect();
    if selected
        .iter()
        .any(|id| !p.clips().any(|c| c.clip_id == *id))
    {
        return Err(invalid("편집할 Clip이 변경되었습니다."));
    }
    let mut next = p.clone();
    let target = r
        .target_tick
        .map(|ticks| {
            if ticks.0 < 0 || ticks.0 > 9_007_199_254_740_991 {
                return Err(invalid("편집 tick 범위"));
            }
            Ok(time(&Position::Ticks { ticks }, &p.musical_time))
        })
        .transpose()?;
    let move_delta = if r.command == "audio.move" {
        Some(if let Some(target) = target {
            let anchor = p
                .clips()
                .find(|c| {
                    Some(&c.clip_id) == r.anchor_clip_id.as_ref()
                        && selected.contains(c.clip_id.as_str())
                })
                .ok_or_else(|| invalid("Move anchor Clip"))?;
            target.minus(time(&anchor.position, &p.musical_time))
        } else {
            time(
                r.delta.as_ref().ok_or_else(|| invalid("Move delta"))?,
                &p.musical_time,
            )
        })
    } else {
        None
    };
    let cursor = r.cursor.as_ref().map(|v| time(v, &p.musical_time));
    let range = cursor.zip(r.range_end.as_ref().map(|v| time(v, &p.musical_time)));
    if range.is_some_and(|(a, b)| a.cmp_time(b) != std::cmp::Ordering::Less) {
        return Err(invalid("시간 범위"));
    }
    let track_selected = |id: &str| r.track_ids.is_empty() || r.track_ids.iter().any(|t| t == id);
    match r.command.as_str() {
        "project.tempo" => {
            let bpm = r.bpm.ok_or_else(|| invalid("BPM 없음"))?;
            if !bpm.is_finite() || !(1.0..=1000.0).contains(&bpm) {
                return Err(invalid("BPM은 1~1000입니다."));
            }
            next.musical_time.tempo_map[0].bpm = bpm;
            super::tempo_sync::refresh(&mut next)?;
        }
        "project.signature" => {
            let signature = &mut next.musical_time.time_signatures[0];
            signature.numerator = r.numerator.ok_or_else(|| invalid("박자 분자 없음"))?;
            signature.denominator = r.denominator.ok_or_else(|| invalid("박자 분모 없음"))?;
        }
        "project.cycle" => {
            next.cycle = Some(r.cycle.clone().ok_or_else(|| invalid("Cycle 없음"))?);
        }
        "edit.copy" | "edit.cut" => {
            let mut copied = vec![];
            for t in &p.tracks {
                for c in t.clips.iter().filter_map(Clip::as_audio) {
                    if let Some((a, b)) = range {
                        if track_selected(&t.track_id) {
                            let lo = boundary(p, c, a);
                            let hi = boundary(p, c, b);
                            if lo < hi {
                                copied.push((t.track_id.clone(), slice(p, c, lo, hi, false, &[a,b])?));
                            }
                        }
                    } else if selected.contains(c.clip_id.as_str()) {
                        copied.push((t.track_id.clone(), c.clone()));
                    }
                }
            }
            if copied.is_empty() {
                return Ok(next);
            }
            let origin = range.map(|(a, _)| a).unwrap_or_else(|| {
                copied
                    .iter()
                    .map(|(_, c)| time(&c.position, &p.musical_time))
                    .min_by(|a, b| a.cmp_time(*b))
                    .unwrap()
            });
            *clipboard = Clipboard {
                musical_origin: if copied.iter().all(|(_,c)|super::tempo_sync::enabled(c)){Some(super::tempo_sync::tick_at(origin,&p.musical_time)?)}else{None},
                clips: copied,
                origin: Some(origin),
            };
            if r.command == "edit.cut" {
                let mut delete = r.clone();
                delete.command = "edit.delete".into();
                return apply(p, &delete, clipboard);
            }
        }
        "edit.paste" | "edit.duplicate" => {
            let (clips, origin, destination) = if r.command == "edit.paste" {
                (clipboard.clips.clone(), clipboard.origin, cursor)
            } else {
                let clips: Vec<_> = p
                    .tracks
                    .iter()
                    .flat_map(|t| {
                        t.clips
                            .iter()
                            .filter_map(Clip::as_audio)
                            .map(move |c| (t.track_id.clone(), c.clone()))
                    })
                    .filter(|(_, c)| selected.contains(c.clip_id.as_str()))
                    .collect();
                let origin = clips
                    .iter()
                    .map(|(_, c)| time(&c.position, &p.musical_time))
                    .min_by(|a, b| a.cmp_time(*b));
                let destination = clips
                    .iter()
                    .map(|(_, c)| end(p, c))
                    .max_by(|a, b| a.cmp_time(*b));
                (clips, origin, destination)
            };
            if let (Some(origin), Some(destination)) = (origin, destination) {
                let mut parts = std::collections::HashMap::new();
                for (track, mut c) in clips {
                    super::glue::remap(&mut c, &mut parts)?;
                    c.clip_id = id();
                    c.position = if super::tempo_sync::enabled(&c) && (r.command != "edit.paste" || clipboard.musical_origin.is_some()) {
                        let from=if r.command=="edit.paste" {clipboard.musical_origin.expect("checked")}else{super::tempo_sync::tick_at(origin,&p.musical_time)?};
                        Position::Ticks{ticks:Signed(super::tempo_sync::tick_at(time(&c.position,&p.musical_time),&p.musical_time)?-from+super::tempo_sync::tick_at(destination,&p.musical_time)?)}
                    } else { time(&c.position, &p.musical_time).minus(origin).plus(destination).position()? };
                    let t = next
                        .tracks
                        .iter_mut()
                        .find(|t| t.track_id == track)
                        .ok_or_else(|| invalid("붙여넣을 Track이 없습니다."))?;
                    t.clips.push(Clip::Audio(c));
                }
            }
        }
        "audio.splitAtCursor" | "audio.splitRange" | "edit.delete" => {
            for t in &mut next.tracks {
                let mut out = vec![];
                for clip in &t.clips {
                    let Some(c) = clip.as_audio() else {
                        out.push(clip.clone());
                        continue;
                    };
                    let use_range = range.is_some() && track_selected(&t.track_id);
                    if use_range {
                        let (a, b) = range.unwrap();
                        let lo = boundary(p, c, a);
                        let hi = boundary(p, c, b);
                        if lo >= hi {
                            out.push(clip.clone());
                            continue;
                        }
                        if c.source_start.0 < lo {
                            out.push(Clip::Audio(slice(p, c, c.source_start.0, lo, true, &[a,b])?));
                        }
                        if r.command != "edit.delete" {
                            out.push(Clip::Audio(slice(p, c, lo, hi, true, &[a,b])?));
                        }
                        if hi < c.source_end.0 {
                            out.push(Clip::Audio(slice(p, c, hi, c.source_end.0, true, &[a,b])?));
                        }
                    } else if selected.contains(c.clip_id.as_str()) && r.command == "edit.delete" {
                    } else if selected.contains(c.clip_id.as_str())
                        && r.command == "audio.splitAtCursor"
                    {
                        let at =
                            boundary(p, c, cursor.ok_or_else(|| invalid("Project Cursor 없음"))?);
                        if at > c.source_start.0 && at < c.source_end.0 {
                            out.push(Clip::Audio(slice(p, c, c.source_start.0, at, true, &[cursor.expect("cursor")])?));
                            out.push(Clip::Audio(slice(p, c, at, c.source_end.0, true, &[cursor.expect("cursor")])?));
                        } else {
                            out.push(clip.clone());
                        }
                    } else {
                        out.push(clip.clone());
                    }
                }
                t.clips = out;
            }
        }
        "audio.crossfade" => {
            let mut indices = vec![];
            for (t, track) in p.tracks.iter().enumerate() {
                for (i, c) in track.clips.iter().enumerate() {
                    if c.as_audio()
                        .is_some_and(|c| selected.contains(c.clip_id.as_str()))
                    {
                        indices.push((t, i));
                    }
                }
            }
            if indices.len() != 2 || indices[0].0 != indices[1].0 {
                return Err(invalid("같은 Track의 Clip 두 개를 선택해 주세요."));
            }
            indices.sort_by(|a, b| {
                time(&p.tracks[a.0].clips[a.1].audio().position, &p.musical_time).cmp_time(time(
                    &p.tracks[b.0].clips[b.1].audio().position,
                    &p.musical_time,
                ))
            });
            let (t, a) = indices[0];
            let (_, b) = indices[1];
            let mut left = p.tracks[t].clips[a].audio().clone();
            let mut right = p.tracks[t].clips[b].audio().clone();
            let left_end = end(p, &left);
            let right_start = time(&right.position, &p.musical_time);
            let mut overlap = left_end.minus(right_start);
            if overlap.n == 0 {
                let max = p
                    .assets
                    .iter()
                    .find(|a| a.asset_id == left.asset_id)
                    .unwrap()
                    .metadata
                    .source_frames
                    .0;
                let max = super::stretch::frames(&left, max)?;
                let handle = (rate(p, &left) as u64 / 100)
                    .max(1)
                    .min(max.saturating_sub(left.source_end.0));
                if handle == 0 {
                    let head = (rate(p, &right) as u64 / 100)
                        .max(1)
                        .min(right.source_start.0)
                        .min(right_start.frame(rate(p, &right)).max(0) as u64);
                    if head == 0 {
                        return Err(invalid("인접 Clip에 Crossfade를 만들 원본 handle이 없습니다. 먼저 겹치게 이동해 주세요."));
                    }
                    overlap = Time::frames(head, rate(p, &right));
                    right.source_start.0 -= head;
                    right.position = right_start.minus(overlap).position()?;
                } else {
                    left.source_end.0 += handle;
                    overlap = Time::frames(handle, rate(p, &left));
                }
            }
            if overlap.n <= 0
                || overlap
                    .cmp_time(Time::frames(
                        right.source_end.0 - right.source_start.0,
                        rate(p, &right),
                    ))
                    .is_gt()
            {
                return Err(invalid("Clip을 연속 또는 일부 겹치게 배치해 주세요."));
            }
            left.extensions.remove(ENVELOPE_WINDOW);
            right.extensions.remove(ENVELOPE_WINDOW);
            left.fade_out = Fade {
                source_frames: Frames(overlap.frame(rate(p, &left)) as u64),
                curve: FadeCurve::Cosine,
            };
            right.fade_in = Fade {
                source_frames: Frames(overlap.frame(rate(p, &right)) as u64),
                curve: FadeCurve::Cosine,
            };
            bounded_fades(&mut left);
            bounded_fades(&mut right);
            next.tracks[t].clips[a] = Clip::Audio(left);
            next.tracks[t].clips[b] = Clip::Audio(right);
        }
        "audio.tempoSync" | "audio.pitch" | "audio.stretch" | "audio.move" | "audio.trim" | "audio.gain" | "audio.fade"
        | "audio.muteEvents" | "audio.unmuteEvents" => {
            for c in next
                .tracks
                .iter_mut()
                .flat_map(|t| &mut t.clips)
                .filter_map(Clip::as_audio_mut)
            {
                if !selected.contains(c.clip_id.as_str()) {
                    continue;
                }
                match r.command.as_str() {
                    "audio.tempoSync" => {super::tempo_sync::set(c,r.tempo_sync,r.source_bpm,&p.musical_time,rate(p,c))?;}
                    "audio.pitch" => {
                        super::pitch::set(
                            c,
                            r.pitch_shift.ok_or_else(|| invalid("Pitch Shift 설정"))?,
                        )?;
                    }
                    "audio.move" => {
                        let delta = move_delta.expect("validated move");
                        let at = time(&c.position, &p.musical_time).plus(delta);
                        if at.n < 0 {
                            return Err(invalid("Clip은 0초 이전으로 이동할 수 없습니다."));
                        }
                        c.position = at.position()?;
                    }
                    "audio.stretch" => {
                        if super::tempo_sync::enabled(c) { return Err(invalid("수동 Time Stretch를 사용하려면 Tempo Sync를 꺼 주세요.")); }
                        let frames = r
                            .stretch_frames
                            .ok_or_else(|| invalid("Time Stretch frames"))?
                            .0;
                        let sample_rate = rate(p, c);
                        super::stretch::resize(
                            c,
                            frames,
                            matches!(r.trim_side, Some(TrimSide::Left)),
                            &p.musical_time,
                            sample_rate,
                        )?;
                    }
                    "audio.trim" => {
                        if super::tempo_sync::enabled(c) { if let Some(tick)=r.target_tick {super::tempo_sync::trim(c,tick.0,matches!(r.trim_side,Some(TrimSide::Left)),&p.musical_time,rate(p,c))?;continue;} }
                        c.extensions.remove(ENVELOPE_WINDOW);
                        let original = c.clone();
                        let mut start = r.source_start.unwrap_or(c.source_start);
                        let mut finish = r.source_end.unwrap_or(c.source_end);
                        let asset = p.assets.iter().find(|a| a.asset_id == c.asset_id).unwrap();
                        let maximum = super::stretch::frames(c, asset.metadata.source_frames.0)?;
                        if let Some(target) = target {
                            let origin = time(&c.position, &p.musical_time);
                            let rate = asset.metadata.sample_rate;
                            let frame = c.source_start.0 as i128 + target.minus(origin).frame(rate);
                            match r.trim_side.as_ref().ok_or_else(|| invalid("Trim side"))? {
                                TrimSide::Left => {
                                    let earliest = (c.source_start.0 as i128
                                        - (origin.n * rate as i128).div_euclid(origin.d))
                                    .max(0);
                                    if earliest >= c.source_end.0 as i128 {
                                        return Err(invalid(
                                            "Trim 가능한 0초 이후 원본 범위가 없습니다.",
                                        ));
                                    }
                                    start =
                                        Frames(frame.clamp(earliest, c.source_end.0 as i128 - 1)
                                            as u64);
                                }
                                TrimSide::Right => {
                                    finish = Frames(
                                        frame.clamp(c.source_start.0 as i128 + 1, maximum as i128)
                                            as u64,
                                    )
                                }
                            }
                        }
                        if start.0 >= finish.0 || finish.0 > maximum {
                            return Err(invalid("Trim 원본 범위"));
                        }
                        let at = time(&c.position, &p.musical_time).plus(Time::new(
                            start.0 as i128 - original.source_start.0 as i128,
                            asset.metadata.sample_rate as i128,
                        ));
                        if at.n < 0 {
                            return Err(invalid("Trim 위치는 0초 이상이어야 합니다."));
                        }
                        c.position = at.position()?;
                        c.source_start = start;
                        c.source_end = finish;
                        bounded_fades(c);
                    }
                    "audio.gain" => {
                        let db = r.gain_db.ok_or_else(|| invalid("Gain dB"))?;
                        if !db.is_finite() || !(-144.0..=144.0).contains(&db) {
                            return Err(invalid("Gain은 -144~144 dB 범위입니다."));
                        }
                        c.gain = 10_f64.powf(db / 20.0);
                    }
                    "audio.fade" => {
                        c.extensions.remove(ENVELOPE_WINDOW);
                        if let Some(n) = r.fade_in {
                            c.fade_in.source_frames = n;
                        }
                        if let Some(n) = r.fade_out {
                            c.fade_out.source_frames = n;
                        }
                        if let Some(curve) = &r.curve {
                            c.fade_in.curve = curve.clone();
                            c.fade_out.curve = curve.clone();
                        }
                        bounded_fades(c);
                    }
                    "audio.muteEvents" => c.mute = true,
                    "audio.unmuteEvents" => c.mute = false,
                    _ => unreachable!(),
                }
            }
        }
        _ => return Err(invalid("지원하지 않는 편집 command ID")),
    }
    if matches!(r.command.as_str(),"audio.pitch"|"audio.trim"|"audio.fade"|"audio.crossfade"|"audio.stretch") {
        for c in next.tracks.iter_mut().flat_map(|t|&mut t.clips).filter_map(Clip::as_audio_mut) {
            if selected.contains(c.clip_id.as_str()) { super::tempo_sync::edited(c,&p.musical_time,rate(p,c))?; }
        }
    } else if matches!(r.command.as_str(),"audio.move"|"edit.paste"|"edit.duplicate") { super::tempo_sync::refresh(&mut next)?; }
    if next
        .primary_clip_id
        .as_ref()
        .is_some_and(|id| !next.clips().any(|c| c.clip_id == *id))
    {
        let first = next.clips().next().map(|c| c.clip_id.clone());
        next.primary_clip_id = first;
    }
    next.validate()?;
    Ok(next)
}
