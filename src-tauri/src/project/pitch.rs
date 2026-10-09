//! Event-level transpose, independent of duration and musical tempo.
use super::{
    schema::*,
    stretch::{self, Recipe},
};
use crate::error::AppResult;
use serde::{Deserialize, Serialize};
pub const KEY: &str = "minidaw.pitchShift.v1";
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Pitch {
    pub semitones: i16,
    pub cents: i16,
}
impl Pitch {
    pub fn total(self) -> i16 {
        self.semitones * 100 + self.cents
    }
    pub fn validate(self) -> AppResult<()> {
        if !(-12..=12).contains(&self.semitones) || !(-100..=100).contains(&self.cents) {
            return Err(invalid(
                "Pitch: Semitone -12–12, Fine Tune -100–100 cent (정수)",
            ));
        }
        Ok(())
    }
}
pub fn get(c: &AudioClip) -> AppResult<Pitch> {
    let p: Pitch = c
        .extensions
        .get(KEY)
        .map(|v| serde_json::from_value(v.clone()).map_err(invalid))
        .transpose()?
        .unwrap_or_default();
    p.validate()?;
    Ok(p)
}
pub fn validate(c: &AudioClip) -> AppResult<()> {
    if get(c)?.total() != 0 && stretch::recipe(c)?.is_none() {
        return Err(invalid("Pitch source recipe 없음"));
    }
    Ok(())
}
fn window_shift(w: &mut EnvelopeWindow, offset: u64, add: bool) -> AppResult<()> {
    for frame in [&mut w.source_start, &mut w.source_end] {
        frame.0 = if add {
            frame.0.checked_add(offset)
        } else {
            frame.0.checked_sub(offset)
        }
        .ok_or_else(|| invalid("Pitch envelope window"))?;
    }
    Ok(())
}
pub fn set(c: &mut AudioClip, p: Pitch) -> AppResult<()> {
    p.validate()?;
    if get(c)? == p {
        return Ok(());
    }
    if p.total() != 0 && stretch::recipe(c)?.is_none() {
        // A bounded original window, also retaining a split event's fade context.
        // Unity ratio means no duration change. Subsequent Trim/Split retain this
        // context so their DSP output is sample-identical across edit boundaries.
        let mut envelope: Option<EnvelopeWindow> = c
            .extensions
            .get(ENVELOPE_WINDOW)
            .map(|v| serde_json::from_value(v.clone()).map_err(invalid))
            .transpose()?;
        let start = envelope.as_ref().map_or(c.source_start, |w| w.source_start);
        let end = envelope.as_ref().map_or(c.source_end, |w| w.source_end);
        let r = Recipe {
            source_start: start,
            source_end: end,
            output_frames: Frames(end.0 - start.0),
        };
        c.source_start.0 -= start.0;
        c.source_end.0 -= start.0;
        if let Some(w) = &mut envelope {
            window_shift(w, start.0, false)?;
            c.extensions.insert(
                ENVELOPE_WINDOW.into(),
                serde_json::to_value(w).map_err(invalid)?,
            );
        }
        c.extensions.insert(
            stretch::KEY.into(),
            serde_json::to_value(r).map_err(invalid)?,
        );
    }
    if p == Pitch::default() {
        c.extensions.remove(KEY);
    } else {
        c.extensions
            .insert(KEY.into(), serde_json::to_value(p).map_err(invalid)?);
    }
    if p.total() == 0 {
        if let Some(r) = stretch::recipe(c)? {
            if r.output_frames.0 == r.source_end.0 - r.source_start.0 && !super::tempo_sync::enabled(c) {
                // Exact original coordinates, not a second inverse pitch shift.
                c.source_start.0 += r.source_start.0;
                c.source_end.0 += r.source_start.0;
                if let Some(value) = c.extensions.get(ENVELOPE_WINDOW) {
                    let mut w: EnvelopeWindow =
                        serde_json::from_value(value.clone()).map_err(invalid)?;
                    window_shift(&mut w, r.source_start.0, true)?;
                    c.extensions.insert(
                        ENVELOPE_WINDOW.into(),
                        serde_json::to_value(w).map_err(invalid)?,
                    );
                }
                c.extensions.remove(stretch::KEY);
            }
        }
    }
    Ok(())
}
