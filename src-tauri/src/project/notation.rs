//! Written pitches are resolved during document edits, never in the audio callback.
//! `MidiNote::pitch` remains the sounding MIDI pitch used by playback and SMF export.
use super::{edit::EditRequest, schema::*};
use crate::error::AppResult;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Accidental {
    Key,
    Sharp,
    Flat,
    Natural,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Spelling {
    pub natural_pitch: i16,
    pub accidental: Accidental,
}

pub fn natural(pitch: i16) -> bool {
    matches!(pitch.rem_euclid(12), 0 | 2 | 4 | 5 | 7 | 9 | 11)
}
pub fn key_offset(pitch: i16, key: i8) -> i16 {
    let order = if key > 0 {
        [5, 0, 7, 2, 9, 4, 11]
    } else {
        [11, 4, 9, 2, 7, 0, 5]
    };
    if order
        .iter()
        .take(key.unsigned_abs() as usize)
        .any(|p| *p == pitch.rem_euclid(12))
    {
        key.signum() as i16
    } else {
        0
    }
}
impl Spelling {
    pub fn resolve(self, key: i8) -> AppResult<u8> {
        if !(-7..=7).contains(&key)
            || !(-1..=128).contains(&self.natural_pitch)
            || !natural(self.natural_pitch)
        {
            return Err(invalid("조표 또는 음이름이 올바르지 않습니다."));
        }
        let offset = match self.accidental {
            Accidental::Key => key_offset(self.natural_pitch, key),
            Accidental::Sharp => 1,
            Accidental::Flat => -1,
            Accidental::Natural => 0,
        };
        let pitch = self.natural_pitch + offset;
        if !(0..=127).contains(&pitch) {
            return Err(invalid(
                "선택한 노트가 MIDI 음높이 0~127 범위를 벗어납니다.",
            ));
        }
        Ok(pitch as u8)
    }
}

/// Infer spelling without changing a pre-existing/imported sounding pitch.
pub fn infer(pitch: u8, key: i8) -> Spelling {
    let pitch = i16::from(pitch);
    if natural(pitch) {
        return Spelling {
            natural_pitch: pitch,
            accidental: if key_offset(pitch, key) == 0 {
                Accidental::Key
            } else {
                Accidental::Natural
            },
        };
    }
    let base = pitch + if key < 0 { 1 } else { -1 };
    let offset = pitch - base;
    Spelling {
        natural_pitch: base,
        accidental: if key_offset(base, key) == offset {
            Accidental::Key
        } else if offset > 0 {
            Accidental::Sharp
        } else {
            Accidental::Flat
        },
    }
}

pub fn written(pitch: u8, key: i8) -> AppResult<(u8, Option<Spelling>)> {
    if key == 0 {
        return Ok((pitch, None));
    }
    let spelling = if natural(pitch.into()) {
        Spelling {
            natural_pitch: pitch.into(),
            accidental: Accidental::Key,
        }
    } else {
        infer(pitch, key)
    };
    Ok((spelling.resolve(key)?, Some(spelling)))
}

/// Mouse pitch moves and chromatic transpose retain their exact existing semantics.
pub fn set_sounding(note: &mut MidiNote, pitch: u8, key: i8) {
    if note.pitch != pitch {
        if note.spelling.is_some() || key != 0 {
            note.spelling = Some(infer(pitch, key));
        }
        note.pitch = pitch;
    }
}

pub fn apply(p: &Project, r: &EditRequest) -> AppResult<Project> {
    let mut next = p.clone();
    if r.command == "midi.track.keySignature" {
        if r.track_ids.len() != 1 {
            return Err(invalid("MIDI Track을 선택해 주세요."));
        }
        let key = r
            .key_signature
            .filter(|k| (-7..=7).contains(k))
            .ok_or_else(|| invalid("조표 또는 음이름이 올바르지 않습니다."))?;
        let t = next
            .tracks
            .iter_mut()
            .find(|t| t.kind == TrackKind::Midi && r.track_ids.contains(&t.track_id))
            .ok_or_else(|| invalid("MIDI Track을 선택해 주세요."))?;
        if key == t.key_signature {
            return Ok(next);
        }
        for c in &mut t.clips {
            if let Clip::Midi(c) = c {
                for n in &mut c.notes {
                    let spelling = n
                        .spelling
                        .unwrap_or_else(|| infer(n.pitch, t.key_signature));
                    n.pitch = spelling.resolve(key)?;
                    n.spelling = Some(spelling);
                }
            }
        }
        t.key_signature = key;
    } else {
        let accidental = r
            .accidental
            .ok_or_else(|| invalid("임시표를 선택해 주세요."))?;
        if r.clip_ids.len() != 1 {
            return Err(invalid("MIDI Clip을 선택해 주세요."));
        }
        let t = next
            .tracks
            .iter_mut()
            .find(|t| {
                t.kind == TrackKind::Midi
                    && t.clips
                        .iter()
                        .any(|c| r.clip_ids.iter().any(|id| id == c.id()))
            })
            .ok_or_else(|| invalid("MIDI Clip을 선택해 주세요."))?;
        let key = t.key_signature;
        let c = t
            .clips
            .iter_mut()
            .find_map(|c| match c {
                Clip::Midi(c) if r.clip_ids.contains(&c.clip_id) => Some(c),
                _ => None,
            })
            .unwrap();
        let ids: HashSet<_> = r.note_ids.iter().collect();
        if ids.is_empty()
            || c.notes.iter().filter(|n| ids.contains(&n.note_id)).count() != ids.len()
        {
            return Err(invalid("Note selection"));
        }
        for n in c.notes.iter_mut().filter(|n| ids.contains(&n.note_id)) {
            let mut spelling = n.spelling.unwrap_or_else(|| infer(n.pitch, key));
            spelling.accidental = accidental;
            n.pitch = spelling.resolve(key)?;
            n.spelling = Some(spelling);
        }
    }
    next.validate()?;
    Ok(next)
}
