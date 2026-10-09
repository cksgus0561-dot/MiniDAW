//! Stable parameter references and integer-PPQ project data. No DSP/cache in files.
use super::{edit::EditRequest, effects::Processor, schema::*};
use crate::error::AppResult;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Parameter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect_id: Option<String>,
    pub name: String,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Shape {
    #[default]
    Linear,
    Step,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Point {
    pub point_id: String,
    pub tick: Signed,
    pub value: f64,
    #[serde(default)]
    pub shape: Shape,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Lane {
    pub lane_id: String,
    pub parameter: Parameter,
    pub points: Vec<Point>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Channel {
    pub track_id: String,
    pub read: bool,
    pub write: bool,
    pub lanes: Vec<Lane>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct SynthSettings {
    pub level_db: f64,
    pub attack_ms: f64,
    pub release_ms: f64,
}
impl Default for SynthSettings {
    fn default() -> Self {
        Self {
            level_db: 0.,
            attack_ms: 5.,
            release_ms: 40.,
        }
    }
}
impl SynthSettings {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Volume,
    Pan,
    SynthLevel,
    SynthAttack,
    SynthRelease,
    Bypass,
    EqFrequency(usize),
    EqGain(usize),
    EqQ(usize),
    Threshold,
    Ratio,
    Attack,
    Release,
    Makeup,
    Ceiling,
    Input,
    Decay,
    Wet,
    Time,
    Feedback,
    Sync,
    Plugin(u32),
}
#[derive(Clone, Copy)]
pub struct Spec {
    pub key: Key,
    pub min: f64,
    pub max: f64,
    pub base: f64,
    pub discrete: bool,
}
pub fn spec(p: &Project, channel: &str, param: &Parameter) -> AppResult<Spec> {
    let track = if channel == "master" {
        None
    } else {
        Some(
            p.tracks
                .iter()
                .find(|t| t.track_id == channel)
                .ok_or_else(|| invalid("Automation Track 없음"))?,
        )
    };
    let (key, min, max, base, discrete) = if let Some(id) = &param.effect_id {
        let e = track
            .map_or(&p.master.inserts, |t| &t.inserts)
            .iter()
            .find(|e| e.effect_id == *id)
            .ok_or_else(|| invalid("Automation Effect 없음"))?;
        if param.name == "bypass" {
            (Key::Bypass, 0., 1., f64::from(!e.enabled), true)
        } else {
            match (&e.processor, param.name.as_str()) {
                (Processor::External { plugin }, name) => {
                    let id=name.strip_prefix("plugin.").and_then(|s|s.parse::<u32>().ok()).ok_or_else(||invalid("Plugin parameter ID"))?;
                    let q=plugin.parameters.iter().find(|q|q.id==id&&!q.readonly).ok_or_else(||invalid("Plugin parameter 없음"))?;
                    (Key::Plugin(id),q.min,q.max,q.value,q.stepped)
                }
                (Processor::Eq { bands }, name) => {
                    let parts: Vec<_> = name.split('.').collect();
                    let i = parts
                        .first()
                        .and_then(|s| s.strip_prefix("band"))
                        .and_then(|s| s.parse::<usize>().ok())
                        .filter(|i| *i < 3)
                        .ok_or_else(|| invalid("EQ Automation band"))?;
                    match parts.get(1).copied() {
                        Some("frequency") => {
                            (Key::EqFrequency(i), 20., 20000., bands[i].frequency, false)
                        }
                        Some("gainDb") => (Key::EqGain(i), -24., 24., bands[i].gain_db, false),
                        Some("q") => (Key::EqQ(i), 0.1, 20., bands[i].q, false),
                        _ => return Err(invalid("EQ Automation parameter")),
                    }
                }
                (Processor::Compressor { threshold_db, .. }, "thresholdDb") => {
                    (Key::Threshold, -60., 0., *threshold_db, false)
                }
                (Processor::Compressor { ratio, .. }, "ratio") => {
                    (Key::Ratio, 1., 20., *ratio, false)
                }
                (Processor::Compressor { attack_ms, .. }, "attackMs") => {
                    (Key::Attack, 0.1, 200., *attack_ms, false)
                }
                (Processor::Compressor { release_ms, .. }, "releaseMs") => {
                    (Key::Release, 10., 2000., *release_ms, false)
                }
                (Processor::Compressor { makeup_db, .. }, "makeupDb") => {
                    (Key::Makeup, -12., 24., *makeup_db, false)
                }
                (Processor::Limiter { ceiling_db, .. }, "ceilingDb") => {
                    (Key::Ceiling, -24., 0., *ceiling_db, false)
                }
                (Processor::Limiter { input_db, .. }, "inputDb") => {
                    (Key::Input, -24., 24., *input_db, false)
                }
                (Processor::Reverb { decay, .. }, "decay") => (Key::Decay, 0.2, 8., *decay, false),
                (Processor::Reverb { wet, .. } | Processor::Delay { wet, .. }, "wet") => {
                    (Key::Wet, 0., 1., *wet, false)
                }
                (Processor::Delay { time_ms, .. }, "timeMs") => {
                    (Key::Time, 1., 2000., *time_ms, false)
                }
                (Processor::Delay { feedback, .. }, "feedback") => {
                    (Key::Feedback, 0., 0.85, *feedback, false)
                }
                (Processor::Delay { sync_beats, .. }, "syncBeats") => {
                    (Key::Sync, 0., 4., sync_beats.unwrap_or(0.), true)
                }
                _ => return Err(invalid("Automation parameter")),
            }
        }
    } else {
        match param.name.as_str() {
            name if name.starts_with("plugin.") && track.is_some_and(|t|t.instrument==Instrument::External)=>{
                let plugin=crate::plugins::instrument(track.unwrap()).ok_or_else(||invalid("Instrument 설정 없음"))?;
                let id=name[7..].parse::<u32>().map_err(invalid)?;
                let q=plugin.parameters.iter().find(|q|q.id==id&&!q.readonly).ok_or_else(||invalid("Plugin parameter 없음"))?;
                (Key::Plugin(id),q.min,q.max,q.value,q.stepped)
            }
            "volumeDb" => (
                Key::Volume,
                -96.,
                12.,
                track.map_or(p.master.volume_db, |t| t.mix.volume_db),
                false,
            ),
            "pan" if track.is_some() => (Key::Pan, -1., 1., track.unwrap().mix.pan, false),
            "synth.levelDb" if track.is_some_and(|t| t.kind == TrackKind::Midi) => (
                Key::SynthLevel,
                -60.,
                12.,
                track.unwrap().synth.level_db,
                false,
            ),
            "synth.attackMs" if track.is_some_and(|t| t.kind == TrackKind::Midi) => (
                Key::SynthAttack,
                0.1,
                2000.,
                track.unwrap().synth.attack_ms,
                false,
            ),
            "synth.releaseMs" if track.is_some_and(|t| t.kind == TrackKind::Midi) => (
                Key::SynthRelease,
                1.,
                5000.,
                track.unwrap().synth.release_ms,
                false,
            ),
            _ => return Err(invalid("Automation parameter")),
        }
    };
    Ok(Spec {
        key,
        min,
        max,
        base,
        discrete,
    })
}
pub fn channel_mut<'a>(p: &'a mut Project, id: &str) -> &'a mut Channel {
    if !p.automation.iter().any(|c| c.track_id == id) {
        p.automation.push(Channel {
            track_id: id.into(),
            read: true,
            write: false,
            lanes: vec![],
        });
    }
    p.automation.iter_mut().find(|c| c.track_id == id).unwrap()
}
pub fn value(points: &[Point], tick: i64, base: f64, discrete: bool) -> f64 {
    let i = points.partition_point(|p| p.tick.0 <= tick);
    if i == 0 {
        return points.first().map_or(base, |p| p.value);
    }
    let a = &points[i - 1];
    let Some(b) = points.get(i) else {
        return a.value;
    };
    if discrete || a.shape == Shape::Step {
        a.value
    } else {
        a.value + (b.value - a.value) * (tick - a.tick.0) as f64 / (b.tick.0 - a.tick.0) as f64
    }
}
pub fn prune(p: &mut Project) {
    let valid: Vec<_> = p
        .automation
        .iter()
        .map(|c| {
            c.lanes
                .iter()
                .map(|l| spec(p, &c.track_id, &l.parameter).is_ok())
                .collect::<Vec<_>>()
        })
        .collect();
    for (c, valid) in p.automation.iter_mut().zip(valid) {
        let mut n = 0;
        c.lanes.retain(|_| {
            let keep = valid[n];
            n += 1;
            keep
        });
    }
    p.automation
        .retain(|c| c.track_id == "master" || p.tracks.iter().any(|t| t.track_id == c.track_id));
}
pub fn validate(p: &Project) -> AppResult<()> {
    let mut ids = std::collections::HashSet::new();
    let mut channels = std::collections::HashSet::new();
    let mut count = 0;
    for t in &p.tracks {
        let s = &t.synth;
        if !s.level_db.is_finite()
            || !(-60.0..=12.).contains(&s.level_db)
            || !(0.1..=2000.).contains(&s.attack_ms)
            || !(1.0..=5000.).contains(&s.release_ms)
        {
            return Err(invalid("Synth parameter 범위"));
        }
    }
    if p.automation.len() > 4097 {
        return Err(invalid("Automation channel 한도"));
    }
    for c in &p.automation {
        if !channels.insert(&c.track_id)
            || c.lanes.len() > 128
            || !(c.track_id == "master" || p.tracks.iter().any(|t| t.track_id == c.track_id))
        {
            return Err(invalid("Automation channel"));
        }
        let mut params = std::collections::HashSet::new();
        for lane in &c.lanes {
            let s = spec(p, &c.track_id, &lane.parameter)?;
            if !ids.insert(&lane.lane_id)
                || lane.lane_id.is_empty()
                || lane.lane_id.len() > 128
                || !params.insert(&lane.parameter)
            {
                return Err(invalid("Automation Lane ID/parameter"));
            }
            let mut last = -1;
            for q in &lane.points {
                count += 1;
                if q.tick.0 <= last
                    || q.tick.0 > 9_007_199_254_740_991
                    || !q.value.is_finite()
                    || q.value < s.min
                    || q.value > s.max
                    || !ids.insert(&q.point_id)
                    || q.point_id.is_empty()
                    || q.point_id.len() > 128
                    || (s.key == Key::Bypass && q.value != 0. && q.value != 1.)
                    || (s.key == Key::Sync
                        && ![0., 0.125, 0.25, 0.5, 1., 2., 4.].contains(&q.value))
                {
                    return Err(invalid("Automation Point 범위/순서"));
                }
                last = q.tick.0;
            }
        }
    }
    if p.automation
        .iter()
        .filter(|c| c.read)
        .map(|c| c.lanes.len())
        .sum::<usize>()
        > 256
    {
        return Err(invalid("활성 Automation Lane 최대 256개"));
    }
    if count > 100_000 {
        return Err(invalid("Automation Point 최대 100000개"));
    }
    Ok(())
}
pub fn apply(p: &Project, r: &EditRequest) -> AppResult<Project> {
    let mut next = p.clone();
    let id = r.track_ids.first().map_or("master", String::as_str);
    if id != "master" && !p.tracks.iter().any(|t| t.track_id == id) {
        return Err(invalid("Automation Track"));
    }
    let s = r.parameter.as_ref().map(|a| spec(p, id, a)).transpose()?;
    let c = channel_mut(&mut next, id);
    match r.command.as_str() {
        "automation.channel" => {
            if let Some(v) = r.read {
                c.read = v;
                if !v {
                    c.write = false
                }
            }
            if let Some(v) = r.write {
                c.write = v;
                if v {
                    c.read = true
                }
            }
        }
        "automation.lane.add" => {
            let parameter = r
                .parameter
                .clone()
                .ok_or_else(|| invalid("Automation parameter"))?;
            if !c.lanes.iter().any(|l| l.parameter == parameter) {
                c.lanes.push(Lane {
                    lane_id: super::schema::id(),
                    parameter,
                    points: vec![],
                })
            }
        }
        "automation.lane.delete" => {
            c.lanes.retain(|l| Some(&l.lane_id) != r.lane_id.as_ref());
        }
        "automation.point.set" | "automation.point.delete" => {
            let l = c
                .lanes
                .iter_mut()
                .find(|l| Some(&l.lane_id) == r.lane_id.as_ref())
                .ok_or_else(|| invalid("Automation Lane"))?;
            if r.command == "automation.point.delete" {
                l.points
                    .retain(|p| Some(&p.point_id) != r.point_id.as_ref());
            } else {
                let tick = r.target_tick.ok_or_else(|| invalid("Automation tick"))?;
                let value = r.value.ok_or_else(|| invalid("Automation value"))?;
                let point_id = r.point_id.clone().unwrap_or_else(super::schema::id);
                l.points
                    .retain(|p| p.point_id != point_id && p.tick != tick);
                l.points.push(Point {
                    point_id,
                    tick,
                    value,
                    shape: if s.is_some_and(|s| s.discrete) {
                        Shape::Step
                    } else {
                        r.shape.unwrap_or_default()
                    },
                });
                l.points.sort_by_key(|p| p.tick.0);
            }
        }
        _ => return Err(invalid("Automation command")),
    }
    next.validate()?;
    Ok(next)
}
/// Rust transport frame -> PPQ tick. Floor avoids recording a tick in a future sample.
pub fn tick_at_frame(frame: u64, rate: u32, time: &MusicalTime) -> i64 {
    let seconds = frame as f64 / rate as f64;
    let mut elapsed = 0.;
    let mut tick = 0;
    let mut bpm = time.tempo_map[0].bpm;
    for p in time.tempo_map.iter().skip(1) {
        let span = (p.tick.0 - tick) as f64 * 60. / bpm / time.ticks_per_quarter as f64;
        if elapsed + span > seconds {
            break;
        }
        elapsed += span;
        tick = p.tick.0;
        bpm = p.bpm;
    }
    let mut result = tick
        + ((seconds - elapsed) * bpm * time.ticks_per_quarter as f64 / 60. + 1e-7).floor() as i64;
    // Match the existing nanosecond-rational tick -> sample mapping, including
    // ceil at 44.1/96 kHz boundaries. Never stamp the next output sample.
    while result > 0
        && super::time::time(
            &Position::Ticks {
                ticks: Signed(result),
            },
            time,
        )
        .ceil_frame(rate)
            > frame as i128
    {
        result -= 1;
    }
    result
}
#[derive(Clone)]
struct WrittenLane {
    channel: String,
    parameter: Parameter,
    original: Vec<Point>,
    base: f64,
    discrete: bool,
    last: i64,
    value: f64,
}
#[derive(Clone)]
pub struct WritePass {
    pub group: String,
    cycle_pass: u64,
    lanes: Vec<WrittenLane>,
}
pub fn parameter_names(e: &super::effects::Effect) -> Vec<String> {
    let mut out = vec!["bypass".into()];
    match &e.processor {
        Processor::External {plugin} => out.extend(plugin.parameters.iter().filter(|p|!p.readonly).map(|p|format!("plugin.{}",p.id))),
        Processor::Eq { .. } => {
            for i in 0..3 {
                for p in ["frequency", "gainDb", "q"] {
                    out.push(format!("band{i}.{p}"));
                }
            }
        }
        Processor::Compressor { .. } => out.extend(
            ["thresholdDb", "ratio", "attackMs", "releaseMs", "makeupDb"].map(String::from),
        ),
        Processor::Limiter { .. } => out.extend(["ceilingDb", "inputDb"].map(String::from)),
        Processor::Reverb { .. } => out.extend(["decay", "wet"].map(String::from)),
        Processor::Delay { .. } => {
            out.extend(["timeMs", "feedback", "wet", "syncBeats"].map(String::from))
        }
    }
    out
}
/// Auto-Latch Write. The control thread stamps the accepting Rust transport
/// sample; frontend timestamps are deliberately not part of the request schema.
pub fn capture(
    before: &Project,
    next: &mut Project,
    r: &EditRequest,
    clock: Option<(u64, u32, u64)>,
    pass: &mut Option<WritePass>,
) -> AppResult<bool> {
    let Some((frame, rate, cycle_pass)) = clock else {
        return Ok(false);
    };
    let channel = r.track_ids.first().map_or("master", String::as_str);
    if !before
        .automation
        .iter()
        .any(|c| c.track_id == channel && c.write)
    {
        return Ok(false);
    }
    let mut values = vec![];
    if matches!(r.command.as_str(), "track.mix" | "master.volume") {
        if let Some(v) = r.volume_db {
            values.push((
                Parameter {
                    effect_id: None,
                    name: "volumeDb".into(),
                },
                v,
            ));
        }
        if let Some(v) = r.pan {
            values.push((
                Parameter {
                    effect_id: None,
                    name: "pan".into(),
                },
                v,
            ));
        }
        if channel == "master" {
            next.master.volume_db = before.master.volume_db;
        } else {
            let old = before
                .tracks
                .iter()
                .find(|t| t.track_id == channel)
                .unwrap();
            let t = next
                .tracks
                .iter_mut()
                .find(|t| t.track_id == channel)
                .unwrap();
            t.mix.volume_db = old.mix.volume_db;
            t.mix.pan = old.mix.pan;
        }
    } else if r.command == "effect.set" {
        let e = r.effect.as_ref().ok_or_else(|| invalid("Effect"))?;
        for name in parameter_names(e) {
            let p = Parameter {
                effect_id: Some(e.effect_id.clone()),
                name,
            };
            let old = spec(before, channel, &p)?.base;
            let value = spec(next, channel, &p)?.base;
            if value != old || r.parameter.as_ref() == Some(&p) {
                let v = if r.parameter.as_ref() == Some(&p) {
                    r.value.unwrap_or(value)
                } else {
                    value
                };
                values.push((p, v));
            }
        }
        let old = if channel == "master" {
            &before.master.inserts
        } else {
            &before
                .tracks
                .iter()
                .find(|t| t.track_id == channel)
                .unwrap()
                .inserts
        };
        let target = if channel == "master" {
            &mut next.master.inserts
        } else {
            &mut next
                .tracks
                .iter_mut()
                .find(|t| t.track_id == channel)
                .unwrap()
                .inserts
        };
        *target = old.clone();
    } else if r.command == "synth.set" {
        for name in ["synth.levelDb", "synth.attackMs", "synth.releaseMs"] {
            let p = Parameter {
                effect_id: None,
                name: name.into(),
            };
            let old = spec(before, channel, &p)?.base;
            let v = spec(next, channel, &p)?.base;
            if old != v || r.parameter.as_ref() == Some(&p) {
                let value = if r.parameter.as_ref() == Some(&p) {
                    r.value.unwrap_or(v)
                } else {
                    v
                };
                values.push((p, value));
            }
        }
        next.tracks
            .iter_mut()
            .find(|t| t.track_id == channel)
            .unwrap()
            .synth = before
            .tracks
            .iter()
            .find(|t| t.track_id == channel)
            .unwrap()
            .synth
            .clone();
    }
    if r.command=="plugin.parameter" {
        let parameter=r.parameter.clone().ok_or_else(||invalid("Plugin Parameter"))?;
        values.push((parameter,r.value.ok_or_else(||invalid("Plugin Parameter value"))?));
        let original=before.tracks.iter().find(|t|t.track_id==channel).and_then(|t|t.extensions.get(crate::plugins::INSTRUMENT)).cloned().ok_or_else(||invalid("Instrument 설정 없음"))?;
        next.tracks.iter_mut().find(|t|t.track_id==channel).unwrap().extensions.insert(crate::plugins::INSTRUMENT.into(),original);
    }
    if values.is_empty() {
        return Ok(false);
    }
    let tick = tick_at_frame(frame, rate, &before.musical_time);
    advance_cycle(next, pass, cycle_pass);
    // A cycle/seek discontinuity starts a fresh segment; never fabricate UI time.
    if pass
        .as_ref()
        .is_some_and(|p| p.lanes.iter().any(|l| l.last > tick))
    {
        finish(next, pass, None);
    }
    let take = pass.get_or_insert_with(|| WritePass {
        group: id(),
        cycle_pass,
        lanes: vec![],
    });
    for (parameter, value) in values {
        let s = spec(before, channel, &parameter)?;
        if !value.is_finite() || value < s.min || value > s.max {
            return Err(invalid("Automation Write value"));
        }
        let c = channel_mut(next, channel);
        c.read = true;
        if !c.lanes.iter().any(|l| l.parameter == parameter) {
            c.lanes.push(Lane {
                lane_id: id(),
                parameter: parameter.clone(),
                points: vec![],
            });
        }
        let lane = c
            .lanes
            .iter_mut()
            .find(|l| l.parameter == parameter)
            .unwrap();
        let index = take
            .lanes
            .iter()
            .position(|l| l.channel == channel && l.parameter == parameter);
        let index = if let Some(i) = index {
            i
        } else {
            let original = lane.points.clone();
            let previous = self::value(&original, tick, s.base, s.discrete);
            lane.points.retain(|q| q.tick.0 < tick.saturating_sub(1));
            if tick > 0 {
                lane.points.push(Point {
                    point_id: id(),
                    tick: Signed(tick - 1),
                    value: previous,
                    shape: Shape::Step,
                });
            }
            take.lanes.push(WrittenLane {
                channel: channel.into(),
                parameter: parameter.clone(),
                original,
                base: s.base,
                discrete: s.discrete,
                last: tick,
                value,
            });
            take.lanes.len() - 1
        };
        lane.points.retain(|p| p.tick.0 != tick);
        lane.points.push(Point {
            point_id: id(),
            tick: Signed(tick),
            value,
            shape: if s.discrete {
                Shape::Step
            } else {
                Shape::Linear
            },
        });
        lane.points.sort_by_key(|p| p.tick.0);
        take.lanes[index].last = tick;
        take.lanes[index].value = value;
    }
    next.validate()?;
    Ok(true)
}
pub fn finish(
    p: &mut Project,
    pass: &mut Option<WritePass>,
    clock: Option<(u64, u32, u64)>,
) -> Option<String> {
    if let Some((_, _, cycle_pass)) = clock {
        advance_cycle(p, pass, cycle_pass);
    }
    let take = pass.take()?;
    let tick = clock.map(|(frame, rate, _)| tick_at_frame(frame, rate, &p.musical_time));
    for w in take.lanes {
        let Some(lane) = p
            .automation
            .iter_mut()
            .find(|c| c.track_id == w.channel)
            .and_then(|c| c.lanes.iter_mut().find(|l| l.parameter == w.parameter))
        else {
            continue;
        };
        let end = tick
            .unwrap_or(w.last)
            .max(w.last)
            .min(9_007_199_254_740_990);
        lane.points.retain(|p| p.tick.0 < end);
        lane.points.push(Point {
            point_id: id(),
            tick: Signed(end),
            value: w.value,
            shape: Shape::Step,
        });
        if !w.original.is_empty() {
            lane.points.push(Point {
                point_id: id(),
                tick: Signed(end + 1),
                value: value(&w.original, end + 1, w.base, w.discrete),
                shape: Shape::Linear,
            });
            lane.points
                .extend(w.original.into_iter().filter(|q| q.tick.0 > end + 1));
        }
    }
    Some(take.group)
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WritingValue {
    pub track_id: String,
    pub parameter: Parameter,
    pub value: f64,
}
pub fn writing(pass: Option<&WritePass>) -> Vec<WritingValue> {
    pass.map_or_else(Vec::new, |p| {
        p.lanes
            .iter()
            .map(|w| WritingValue {
                track_id: w.channel.clone(),
                parameter: w.parameter.clone(),
                value: w.value,
            })
            .collect()
    })
}
/// Live Auto-Latch holds the last physical control value even through a cycle.
/// This prepared playback projection is never saved as project data.
pub fn playback(p: &Project, pass: Option<&WritePass>) -> Project {
    let mut next = p.clone();
    if let Some(pass) = pass {
        for w in &pass.lanes {
            if let Some(l) = next
                .automation
                .iter_mut()
                .find(|c| c.track_id == w.channel)
                .and_then(|c| c.lanes.iter_mut().find(|l| l.parameter == w.parameter))
            {
                l.points = vec![Point {
                    point_id: l.points.last().map_or_else(id, |q| q.point_id.clone()),
                    tick: Signed(0),
                    value: w.value,
                    shape: Shape::Step,
                }];
            }
        }
    }
    next
}
fn advance_cycle(p: &mut Project, pass: &mut Option<WritePass>, cycle_pass: u64) {
    let Some(take) = pass.as_mut() else { return };
    let Some(cycle) = p
        .cycle
        .as_ref()
        .filter(|c| c.enabled && cycle_pass > take.cycle_pass)
    else {
        return;
    };
    let start = cycle.start_tick.0;
    let end = cycle.end_tick.0;
    let laps = cycle_pass - take.cycle_pass;
    for w in &mut take.lanes {
        let Some(lane) = p
            .automation
            .iter_mut()
            .find(|c| c.track_id == w.channel)
            .and_then(|c| c.lanes.iter_mut().find(|l| l.parameter == w.parameter))
        else {
            continue;
        };
        // Complete the last lap. Extra laps without control input contain one held value.
        if laps > 1 {
            lane.points.retain(|q| q.tick.0 < start);
            lane.points.push(Point {
                point_id: id(),
                tick: Signed(start),
                value: w.value,
                shape: Shape::Step,
            });
        }
        lane.points.retain(|q| q.tick.0 < end - 1);
        lane.points.push(Point {
            point_id: id(),
            tick: Signed(end - 1),
            value: w.value,
            shape: Shape::Step,
        });
        lane.points.push(Point {
            point_id: id(),
            tick: Signed(end),
            value: value(&w.original, end, w.base, w.discrete),
            shape: Shape::Linear,
        });
        lane.points
            .extend(w.original.iter().filter(|q| q.tick.0 > end).cloned());
        w.original = lane.points.clone();
        lane.points.retain(|q| q.tick.0 < start);
        lane.points.push(Point {
            point_id: id(),
            tick: Signed(start),
            value: w.value,
            shape: Shape::Step,
        });
        w.last = start;
    }
    take.cycle_pass = cycle_pass;
}
