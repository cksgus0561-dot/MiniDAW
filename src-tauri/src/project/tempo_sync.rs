//! Event musical definition, independent of rendered PCM and project BPM.
//! Integer fractions retain original source/trim/fade coordinates across tempo
//! changes. Only render preparation rounds to the source-rate sample grid.
use super::{
    schema::*,
    stretch::{self, Recipe},
    time::{time, Time},
};
use crate::error::AppResult;
use serde::{Deserialize, Serialize};
pub const KEY: &str = "minidaw.tempoSync.v1";
pub const MIN_RATIO: f64 = 0.125;
pub const MAX_RATIO: f64 = 8.;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Fraction {
    pub numerator: Frames,
    pub denominator: Frames,
}
impl Fraction {
    fn new(n: u64, d: u64) -> Self {
        let (mut a, mut b) = (n, d);
        while b != 0 {
            (a, b) = (b, a % b);
        }
        let g = a.max(1);
        Self {
            numerator: Frames(n / g),
            denominator: Frames(d / g),
        }
    }
    fn at(&self, n: u64) -> u64 {
        ((self.numerator.0 as u128 * n as u128 + self.denominator.0 as u128 / 2)
            / self.denominator.0 as u128) as u64
    }
    fn update(&mut self, value: u64, output: u64) {
        if self.at(output) != value {
            *self = Self::new(value, output);
        }
    }
    fn valid(&self) -> bool {
        self.denominator.0 > 0
            && self.denominator.0 <= MAX_TIME_DENOMINATOR
            && self.numerator.0 <= self.denominator.0
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Window {
    pub start: Fraction,
    pub end: Fraction,
    pub fade_in: Fraction,
    pub fade_out: Fraction,
}
impl Window {
    fn new(start: u64, end: u64, fi: u64, fo: u64, n: u64) -> Self {
        Self {
            start: Fraction::new(start, n),
            end: Fraction::new(end, n),
            fade_in: Fraction::new(fi, n),
            fade_out: Fraction::new(fo, n),
        }
    }
    fn update(&mut self, start: u64, end: u64, fi: u64, fo: u64, n: u64) {
        self.start.update(start, n);
        self.end.update(end, n);
        self.fade_in.update(fi, n);
        self.fade_out.update(fo, n);
    }
    fn valid(&self) -> bool {
        [&self.start, &self.end, &self.fade_in, &self.fade_out]
            .iter()
            .all(|f| f.valid())
            && (self.start.numerator.0 as u128 * self.end.denominator.0 as u128)
                < self.end.numerator.0 as u128 * self.start.denominator.0 as u128
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Definition {
    pub source_start: Frames,
    pub source_end: Frames,
    pub ticks: Signed,
    pub window: Window,
    pub envelope: Option<Window>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Sync {
    pub enabled: bool,
    pub source_bpm: f64,
    pub definition: Option<Definition>,
}
pub fn get(c: &AudioClip) -> AppResult<Option<Sync>> {
    c.extensions
        .get(KEY)
        .map(|v| serde_json::from_value(v.clone()).map_err(invalid))
        .transpose()
}
pub fn enabled(c: &AudioClip) -> bool {
    c.extensions
        .get(KEY)
        .and_then(|v| v.get("enabled"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}
fn put(c: &mut AudioClip, s: Sync) -> AppResult<()> {
    c.extensions
        .insert(KEY.into(), serde_json::to_value(s).map_err(invalid)?);
    Ok(())
}
fn fixed_bpm(clock: &MusicalTime) -> AppResult<f64> {
    let bpm = clock.tempo_map[0].bpm;
    if clock.tempo_map.iter().any(|p| p.bpm != bpm) {
        return Err(invalid(
            "Audio Tempo Sync는 현재 단일 Project BPM을 지원합니다.",
        ));
    }
    Ok(bpm)
}
/// Invert the shared musical clock with a nearest-integer PPQ tick result.
pub fn tick_at(at: Time, clock: &MusicalTime) -> AppResult<i64> {
    let (mut lo, mut hi) = (0i64, MAX_TIME_DENOMINATOR as i64);
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if time(&Position::Ticks { ticks: Signed(mid) }, clock)
            .cmp_time(at)
            .is_lt()
        {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    if lo > 0 {
        let a = time(
            &Position::Ticks {
                ticks: Signed(lo - 1),
            },
            clock,
        );
        let b = time(&Position::Ticks { ticks: Signed(lo) }, clock);
        if at.minus(a).cmp_time(b.minus(at)).is_le() {
            lo -= 1;
        }
    }
    if at.n < 0 {
        return Err(invalid("Tempo Sync 위치는 0 이상이어야 합니다."));
    }
    Ok(lo)
}
pub fn length_ticks(c: &AudioClip) -> AppResult<Option<i64>> {
    Ok(get(c)?
        .filter(|s| s.enabled)
        .and_then(|s| s.definition)
        .map(|d| {
            d.window.end.at(d.ticks.0 as u64) as i64 - d.window.start.at(d.ticks.0 as u64) as i64
        }))
}
pub fn end(c: &AudioClip, clock: &MusicalTime, rate: u32) -> AppResult<Time> {
    if let Some(n) = length_ticks(c)? {
        let start = match c.position {
            Position::Ticks { ticks } => ticks.0,
            _ => tick_at(time(&c.position, clock), clock)?,
        };
        return Ok(time(
            &Position::Ticks {
                ticks: Signed(
                    start
                        .checked_add(n)
                        .ok_or_else(|| invalid("Tempo Sync tick overflow"))?,
                ),
            },
            clock,
        ));
    }
    Ok(time(&c.position, clock).plus(Time::frames(c.source_end.0 - c.source_start.0, rate)))
}
fn ticks_for(n: u64, rate: u32, bpm: f64, ppq: u32) -> AppResult<i64> {
    let ticks = (n as f64 / rate as f64 * bpm / 60. * ppq as f64).round();
    if !ticks.is_finite() || !(1.0..=MAX_TIME_DENOMINATOR as f64).contains(&ticks) {
        return Err(invalid("Tempo Sync musical 길이 범위"));
    }
    Ok(ticks as i64)
}
/// Normalize into the existing recipe coordinates without rendering anything.
fn ensure_recipe(c: &mut AudioClip) -> AppResult<Recipe> {
    if let Some(r) = stretch::recipe(c)? {
        return Ok(r);
    }
    let mut w: Option<EnvelopeWindow> = c
        .extensions
        .get(ENVELOPE_WINDOW)
        .map(|v| serde_json::from_value(v.clone()).map_err(invalid))
        .transpose()?;
    let start = w.as_ref().map_or(c.source_start, |w| w.source_start);
    let end = w.as_ref().map_or(c.source_end, |w| w.source_end);
    let r = Recipe {
        source_start: start,
        source_end: end,
        output_frames: Frames(end.0 - start.0),
    };
    c.source_start.0 -= start.0;
    c.source_end.0 -= start.0;
    if let Some(w) = &mut w {
        w.source_start.0 -= start.0;
        w.source_end.0 -= start.0;
        c.extensions.insert(
            ENVELOPE_WINDOW.into(),
            serde_json::to_value(w).map_err(invalid)?,
        );
    }
    c.extensions.insert(
        stretch::KEY.into(),
        serde_json::to_value(&r).map_err(invalid)?,
    );
    Ok(r)
}
fn capture(
    c: &AudioClip,
    r: &Recipe,
    s: &mut Sync,
    rate: u32,
    clock: &MusicalTime,
) -> AppResult<()> {
    let n = r.output_frames.0;
    let ticks = Signed(ticks_for(
        r.source_end.0 - r.source_start.0,
        rate,
        s.source_bpm,
        clock.ticks_per_quarter,
    )?);
    let d = s.definition.get_or_insert_with(|| Definition {
        source_start: r.source_start,
        source_end: r.source_end,
        ticks,
        window: Window::new(
            c.source_start.0,
            c.source_end.0,
            c.fade_in.source_frames.0,
            c.fade_out.source_frames.0,
            n,
        ),
        envelope: None,
    });
    if d.source_start != r.source_start || d.source_end != r.source_end {
        s.definition = None;
        return capture(c, r, s, rate, clock);
    }
    d.ticks = ticks;
    d.window.update(
        c.source_start.0,
        c.source_end.0,
        c.fade_in.source_frames.0,
        c.fade_out.source_frames.0,
        n,
    );
    if let Some(v) = c.extensions.get(ENVELOPE_WINDOW) {
        let w: EnvelopeWindow = serde_json::from_value(v.clone()).map_err(invalid)?;
        let e = d.envelope.get_or_insert_with(|| {
            Window::new(
                w.source_start.0,
                w.source_end.0,
                w.fade_in.source_frames.0,
                w.fade_out.source_frames.0,
                n,
            )
        });
        e.update(
            w.source_start.0,
            w.source_end.0,
            w.fade_in.source_frames.0,
            w.fade_out.source_frames.0,
            n,
        );
    } else {
        d.envelope = None;
    }
    Ok(())
}
pub fn materialize(c: &mut AudioClip, clock: &MusicalTime, _rate: u32) -> AppResult<()> {
    let Some(s) = get(c)?.filter(|s| s.enabled) else {
        return Ok(());
    };
    let d = s
        .definition
        .as_ref()
        .ok_or_else(|| invalid("Tempo Sync definition 없음"))?;
    let ratio = s.source_bpm / fixed_bpm(clock)?;
    if !(MIN_RATIO..=MAX_RATIO).contains(&ratio) {
        return Err(invalid(
            "Tempo Sync 비율 범위: 12.5–800% (Source BPM / Project BPM)",
        ));
    }
    let n = ((d.source_end.0 - d.source_start.0) as f64 * ratio)
        .round()
        .max(1.) as u64;
    c.source_start = Frames(d.window.start.at(n));
    c.source_end = Frames(d.window.end.at(n));
    let length = c.source_end.0.saturating_sub(c.source_start.0);
    if length == 0 || n > MAX_TIME_DENOMINATOR {
        return Err(invalid("Tempo Sync 결과는 1 sample 이상이어야 합니다."));
    }
    c.fade_in.source_frames = Frames(d.window.fade_in.at(n).min(length));
    c.fade_out.source_frames = Frames(d.window.fade_out.at(n).min(length));
    if let Some(e) = &d.envelope {
        let mut w: EnvelopeWindow = serde_json::from_value(
            c.extensions
                .get(ENVELOPE_WINDOW)
                .ok_or_else(|| invalid("Tempo Sync fade context"))?
                .clone(),
        )
        .map_err(invalid)?;
        w.source_start = Frames(e.start.at(n).min(c.source_start.0));
        w.source_end = Frames(e.end.at(n).max(c.source_end.0));
        let size = w.source_end.0 - w.source_start.0;
        w.fade_in.source_frames = Frames(e.fade_in.at(n).min(size));
        w.fade_out.source_frames = Frames(e.fade_out.at(n).min(size));
        c.extensions.insert(
            ENVELOPE_WINDOW.into(),
            serde_json::to_value(w).map_err(invalid)?,
        );
    }
    c.extensions.insert(
        stretch::KEY.into(),
        serde_json::to_value(Recipe {
            source_start: d.source_start,
            source_end: d.source_end,
            output_frames: Frames(n),
        })
        .map_err(invalid)?,
    );
    if !matches!(c.position, Position::Ticks { .. }) {
        c.position = Position::Ticks {
            ticks: Signed(tick_at(time(&c.position, clock), clock)?),
        };
    }
    Ok(())
}
pub fn set(
    c: &mut AudioClip,
    on: Option<bool>,
    bpm: Option<f64>,
    clock: &MusicalTime,
    rate: u32,
) -> AppResult<()> {
    let mut s = get(c)?.unwrap_or(Sync {
        enabled: false,
        source_bpm: clock.tempo_map[0].bpm,
        definition: None,
    });
    if let Some(bpm) = bpm {
        if !bpm.is_finite() || !(1.0..=1000.).contains(&bpm) {
            return Err(invalid("Source BPM은 1–1000입니다."));
        }
        s.source_bpm = bpm;
    }
    if let Some(on) = on {
        if !on && s.enabled {
            c.position = time(&c.position, clock).position()?;
        }
        s.enabled = on;
    }
    if s.enabled {
        let r = ensure_recipe(c)?;
        capture(c, &r, &mut s, rate, clock)?;
    }
    put(c, s)?;
    materialize(c, clock, rate)
}
/// Called for edits, never for a BPM change: unchanged reference fractions stay
/// untouched, so Move/Gain/Pitch cannot re-quantize the musical definition.
pub fn edited(c: &mut AudioClip, clock: &MusicalTime, rate: u32) -> AppResult<()> {
    let Some(mut s) = get(c)? else {
        return Ok(());
    };
    if !s.enabled {
        s.definition = None;
        return put(c, s);
    }
    let r = ensure_recipe(c)?;
    capture(c, &r, &mut s, rate, clock)?;
    put(c, s)?;
    materialize(c, clock, rate)
}
pub fn refresh(p: &mut Project) -> AppResult<()> {
    let clock = p.musical_time.clone();
    let rates: std::collections::HashMap<_, _> = p
        .assets
        .iter()
        .map(|a| (a.asset_id.clone(), a.metadata.sample_rate))
        .collect();
    for c in p
        .tracks
        .iter_mut()
        .flat_map(|t| &mut t.clips)
        .filter_map(Clip::as_audio_mut)
    {
        materialize(c, &clock, rates[&c.asset_id])?;
    }
    Ok(())
}
/// Split/Range boundaries stay on the user's exact PPQ grid, with PCM rounding
/// deferred to materialize(). Adjacent parts share the same source definition.
pub fn sliced(
    original: &AudioClip,
    out: &mut AudioClip,
    clock: &MusicalTime,
    rate: u32,
    boundaries: &[Time],
) -> AppResult<()> {
    if !enabled(original) {
        return Ok(());
    }
    let mut s = get(original)?.expect("enabled");
    let d = s.definition.as_ref().expect("definition");
    let total = d.ticks.0 as u64;
    let original_start = tick_at(time(&original.position, clock), clock)?;
    let source_tick = d.window.start.at(total) as i64;
    let r = stretch::recipe(original)?.expect("sync recipe");
    let at = |frame: u64, unchanged: u64, reference: &Fraction| -> AppResult<Fraction> {
        if frame == unchanged {
            return Ok(reference.clone());
        }
        for &boundary in boundaries {
            let offset = boundary.minus(time(&original.position, clock));
            let mapped = original.source_start.0 as i128 + offset.frame(rate);
            if mapped == frame as i128 {
                let t = source_tick + tick_at(boundary, clock)? - original_start;
                if t >= 0 && t <= total as i64 {
                    return Ok(Fraction::new(t as u64, total));
                }
            }
        }
        Ok(Fraction::new(frame, r.output_frames.0))
    };
    let start = at(out.source_start.0, original.source_start.0, &d.window.start)?;
    let end = at(out.source_end.0, original.source_end.0, &d.window.end)?;
    capture(out, &r, &mut s, rate, clock)?;
    let d = s.definition.as_mut().expect("definition");
    d.window.start = start;
    d.window.end = end;
    out.position = Position::Ticks {
        ticks: Signed(original_start + d.window.start.at(total) as i64 - source_tick),
    };
    put(out, s)?;
    materialize(out, clock, rate)
}
pub fn trim(
    c: &mut AudioClip,
    target: i64,
    left: bool,
    clock: &MusicalTime,
    rate: u32,
) -> AppResult<()> {
    let mut s = get(c)?.ok_or_else(|| invalid("Tempo Sync"))?;
    let d = s.definition.as_mut().expect("definition");
    let total = d.ticks.0 as u64;
    let start = tick_at(time(&c.position, clock), clock)?;
    let a = d.window.start.at(total) as i64;
    let b = d.window.end.at(total) as i64;
    let n = stretch::recipe(c)?.expect("recipe").output_frames.0;
    let minimum = total.div_ceil(n).max(1) as i64;
    let edge = a + target - start;
    if left {
        if (a - start).max(0) > b - minimum {
            return Err(invalid("Tempo Sync Trim 가능한 구간 없음"));
        }
        let edge = edge.clamp((a - start).max(0), b - minimum);
        d.window.start = Fraction::new(edge as u64, total);
        c.position = Position::Ticks {
            ticks: Signed(start + edge - a),
        };
    } else {
        if a + minimum > total as i64 {
            return Err(invalid("Tempo Sync Trim 가능한 구간 없음"));
        }
        d.window.end = Fraction::new(edge.clamp(a + minimum, total as i64) as u64, total);
    }
    d.envelope = None;
    c.extensions.remove(ENVELOPE_WINDOW);
    put(c, s)?;
    materialize(c, clock, rate)
}
pub fn validate(c: &AudioClip, original: u64, rate: u32, clock: &MusicalTime) -> AppResult<()> {
    let Some(s) = get(c)? else {
        return Ok(());
    };
    if !s.source_bpm.is_finite() || !(1.0..=1000.).contains(&s.source_bpm) {
        return Err(invalid("Source BPM 범위"));
    }
    if let Some(d) = &s.definition {
        if d.source_start.0 >= d.source_end.0
            || d.source_end.0 > original
            || d.ticks.0 < 1
            || d.ticks.0 > MAX_TIME_DENOMINATOR as i64
            || !d.window.valid()
            || d.envelope.as_ref().is_some_and(|w| !w.valid())
        {
            return Err(invalid("Tempo Sync source definition"));
        }
        if s.enabled
            && (d.ticks.0
                != ticks_for(
                    d.source_end.0 - d.source_start.0,
                    rate,
                    s.source_bpm,
                    clock.ticks_per_quarter,
                )?
                || !matches!(stretch::recipe(c)?,Some(r) if r.source_start==d.source_start && r.source_end==d.source_end))
        {
            return Err(invalid("Tempo Sync 기준 BPM / 원본 recipe 불일치"));
        }
        if s.enabled && length_ticks(c)?.is_none_or(|n| n <= 0) {
            return Err(invalid("Tempo Sync musical 범위"));
        }
    }
    if s.enabled
        && (s.definition.is_none()
            || !matches!(c.position, Position::Ticks { .. })
            || stretch::recipe(c)?.is_none())
    {
        return Err(invalid("Tempo Sync musical 위치 / recipe 없음"));
    }
    Ok(())
}

/// One source-clock origin for every Split sibling. Never derive this from
/// each sibling's rounded PCM window, which can offset SRC by one sample.
pub fn anchor(c: &AudioClip, clock: &MusicalTime) -> AppResult<Option<Time>> {
    let Some(s) = get(c)?.filter(|s| s.enabled) else {
        return Ok(None);
    };
    let d = s.definition.expect("validated definition");
    let start = tick_at(time(&c.position, clock), clock)?;
    Ok(Some(time(
        &Position::Ticks {
            ticks: Signed(start - d.window.start.at(d.ticks.0 as u64) as i64),
        },
        clock,
    )))
}
pub fn compatible(a: &AudioClip, b: &AudioClip) -> AppResult<bool> {
    let (a, b) = (get(a)?, get(b)?);
    match (a, b) {
        (None, None) => Ok(true),
        (Some(a), Some(b)) => Ok(a.enabled == b.enabled
            && a.source_bpm == b.source_bpm
            && match (a.definition, b.definition) {
                (Some(a), Some(b)) => {
                    a.source_start == b.source_start
                        && a.source_end == b.source_end
                        && a.ticks == b.ticks
                        && a.envelope == b.envelope
                }
                (None, None) => true,
                _ => false,
            }),
        _ => Ok(false),
    }
}
pub fn joined(
    first: &AudioClip,
    last: &AudioClip,
    out: &mut AudioClip,
    clock: &MusicalTime,
    rate: u32,
) -> AppResult<()> {
    let Some(mut s) = get(first)?.filter(|s| s.enabled) else {
        return Ok(());
    };
    let last = get(last)?
        .expect("compatible")
        .definition
        .expect("definition");
    let d = s.definition.as_mut().expect("definition");
    d.window.end = last.window.end;
    d.window.fade_out = last.window.fade_out;
    if !out.extensions.contains_key(ENVELOPE_WINDOW) {
        if let Some(e) = d.envelope.take() {
            d.window.fade_in = e.fade_in;
            d.window.fade_out = e.fade_out;
        }
    }
    put(out, s)?;
    materialize(out, clock, rate)
}
