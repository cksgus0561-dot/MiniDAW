//! Persisted non-destructive recipe. Clip/fade windows address the rendered
//! source-rate sample grid; the recipe always refers to the original asset.
use super::{
    schema::*,
    time::{time, Time},
};
use crate::error::AppResult;
use serde::{Deserialize, Serialize};
pub const KEY: &str = "minidaw.timeStretch.v1";
pub const MIN_RATIO: f64 = 0.5;
pub const MAX_RATIO: f64 = 2.0;
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Recipe {
    pub source_start: Frames,
    pub source_end: Frames,
    pub output_frames: Frames,
}
pub fn recipe(c: &AudioClip) -> AppResult<Option<Recipe>> {
    c.extensions
        .get(KEY)
        .map(|v| serde_json::from_value(v.clone()).map_err(invalid))
        .transpose()
}
pub fn frames(c: &AudioClip, original: u64) -> AppResult<u64> {
    Ok(recipe(c)?.map_or(original, |r| r.output_frames.0))
}
pub fn validate(c: &AudioClip, original: u64) -> AppResult<()> {
    if let Some(r) = recipe(c)? {
        if r.source_start.0 >= r.source_end.0 || r.source_end.0 > original {
            return Err(invalid("Time Stretch 원본 범위"));
        }
        let ratio = r.output_frames.0 as f64 / (r.source_end.0 - r.source_start.0) as f64;
        let (min,max)=if c.extensions.contains_key(super::tempo_sync::KEY) {(super::tempo_sync::MIN_RATIO,super::tempo_sync::MAX_RATIO)}else{(MIN_RATIO,MAX_RATIO)};
        // An exact BPM ratio is rounded once onto the source sample grid.
        let epsilon = 0.5 / (r.source_end.0-r.source_start.0) as f64 + 1e-12;
        if !(min-epsilon..=max+epsilon).contains(&ratio)
            || r.output_frames.0 == 0
            || c.source_end.0 > r.output_frames.0
            || r.output_frames.0 > MAX_TIME_DENOMINATOR
        {
            return Err(invalid(format!("Time Stretch 길이 비율은 {}–{}%입니다.", min*100., max*100.)));
        }
    }
    Ok(())
}
fn scale(value: u64, numerator: u64, denominator: u64) -> u64 {
    ((value as u128 * numerator as u128 + denominator as u128 / 2) / denominator as u128) as u64
}
pub fn resize(
    c: &mut AudioClip,
    target_frames: u64,
    left: bool,
    clock: &MusicalTime,
    rate: u32,
) -> AppResult<()> {
    let old_len = c.source_end.0 - c.source_start.0;
    if target_frames == old_len {
        return Ok(());
    }
    if target_frames == 0 || target_frames > MAX_TIME_DENOMINATOR {
        return Err(invalid("Time Stretch 목표 길이"));
    }
    let existing = recipe(c)?;
    let offset = if existing.is_none() {
        c.source_start.0
    } else {
        0
    };
    // Keep the original recipe window through subsequent stretch/trim/split
    // operations. Never feed an already stretched render back into the DSP.
    let mut r = existing.unwrap_or(Recipe {
        source_start: c.source_start,
        source_end: c.source_end,
        output_frames: Frames(old_len),
    });
    let old_output = r.output_frames.0;
    let start = scale(c.source_start.0 - offset, target_frames, old_len);
    let finish = start
        .checked_add(target_frames)
        .ok_or_else(|| invalid("Time Stretch 범위"))?;
    r.output_frames = Frames(scale(old_output, target_frames, old_len).max(finish));
    let ratio = r.output_frames.0 as f64 / (r.source_end.0 - r.source_start.0) as f64;
    if !(MIN_RATIO..=MAX_RATIO).contains(&ratio) {
        return Err(invalid("Time Stretch 길이 비율은 50–200%입니다."));
    }
    if left {
        let pos = time(&c.position, clock).plus(Time::new(
            old_len as i128 - target_frames as i128,
            rate as i128,
        ));
        if pos.n < 0 {
            return Err(invalid("Time Stretch 시작 위치는 0초 이상이어야 합니다."));
        }
        c.position = pos.position()?;
    }
    c.source_start = Frames(start);
    c.source_end = Frames(finish);
    c.fade_in.source_frames =
        Frames(scale(c.fade_in.source_frames.0, target_frames, old_len).min(target_frames));
    c.fade_out.source_frames =
        Frames(scale(c.fade_out.source_frames.0, target_frames, old_len).min(target_frames));
    if let Some(v) = c.extensions.get(ENVELOPE_WINDOW) {
        let mut w: EnvelopeWindow = serde_json::from_value(v.clone()).map_err(invalid)?;
        // An unstretched split's window can extend beyond the selected source.
        // Include that original window in the first render, preserving its fade.
        if offset > w.source_start.0 || w.source_end.0 > offset + old_output {
            let n = w.source_end.0 - w.source_start.0;
            let shift = scale(offset - w.source_start.0, target_frames, old_len);
            r.source_start = w.source_start;
            r.source_end = w.source_end;
            r.output_frames = Frames(scale(n, target_frames, old_len).max(finish + shift));
            c.source_start.0 += shift;
            c.source_end.0 += shift;
            w.source_start = Frames(0);
            w.source_end = r.output_frames;
        } else {
            w.source_start = Frames(scale(w.source_start.0 - offset, target_frames, old_len));
            w.source_end =
                Frames(scale(w.source_end.0 - offset, target_frames, old_len).max(c.source_end.0));
        }
        w.fade_in.source_frames = Frames(scale(w.fade_in.source_frames.0, target_frames, old_len));
        w.fade_out.source_frames =
            Frames(scale(w.fade_out.source_frames.0, target_frames, old_len));
        c.extensions.insert(
            ENVELOPE_WINDOW.into(),
            serde_json::to_value(w).map_err(invalid)?,
        );
    }
    c.extensions
        .insert(KEY.into(), serde_json::to_value(r).map_err(invalid)?);
    Ok(())
}
