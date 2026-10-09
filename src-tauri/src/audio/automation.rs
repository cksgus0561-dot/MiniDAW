//! Curves are compiled off RT into device sample positions. Interpolation uses
//! the actual transport frame (also after seek/cycle), never a wall/UI clock.
use crate::project::{
    automation::{self, Key, Shape},
    schema::*,
    time::time,
};
pub struct Curve {
    points: Vec<(usize, f64, Shape)>,
    cursor: usize,
    pub key: Key,
    pub slot: Option<usize>,
    base: f64,
    discrete: bool,
}
impl Curve {
    pub fn value(&mut self, frame: Option<usize>) -> f64 {
        let Some(f) = frame else { return self.base };
        if self.points.is_empty() {
            return self.base;
        }
        if self.cursor >= self.points.len()
            || self.points[self.cursor].0 > f
            || self.points.get(self.cursor + 1).is_some_and(|p| p.0 <= f)
        {
            self.cursor = self.points.partition_point(|p| p.0 <= f).saturating_sub(1);
        }
        let a = self.points[self.cursor];
        let Some(b) = self.points.get(self.cursor + 1) else {
            return a.1;
        };
        if f <= a.0 || self.discrete || a.2 == Shape::Step {
            a.1
        } else {
            a.1 + (b.1 - a.1) * (f - a.0) as f64 / (b.0 - a.0) as f64
        }
    }
}
pub struct Automation {
    pub curves: Vec<Curve>,
    base: Values,
}
#[derive(Clone, Copy)]
pub struct Values {
    pub volume: f64,
    pub pan: f64,
    pub level: f64,
    pub attack: f64,
    pub release: f64,
    pub audible: bool,
}
impl Values {
    pub fn gains(&self, master: bool) -> [f64; 2] {
        let g = if !self.audible || (master && self.volume <= -96.) {
            0.
        } else {
            10f64.powf(self.volume / 20.)
        };
        [g * (1. - self.pan).min(1.), g * (1. + self.pan).min(1.)]
    }
}
impl Automation {
    pub fn compile(p: &Project, id: &str, rate: u32) -> Self {
        let t = p.tracks.iter().find(|t| t.track_id == id);
        let effects = t.map_or(&p.master.inserts, |t| &t.inserts);
        let base = Values {
            volume: t.map_or(p.master.volume_db, |t| t.mix.volume_db),
            pan: t.map_or(0., |t| t.mix.pan),
            level: t.map_or(0., |t| t.synth.level_db),
            attack: t.map_or(5., |t| t.synth.attack_ms),
            release: t.map_or(40., |t| t.synth.release_ms),
            audible: t.is_none_or(|t| {
                !t.mix.mute && (!p.tracks.iter().any(|t| t.mix.solo) || t.mix.solo)
            }),
        };
        let curves = p
            .automation
            .iter()
            .find(|c| c.track_id == id && c.read)
            .into_iter()
            .flat_map(|c| &c.lanes)
            .map(|l| {
                let s =
                    automation::spec(p, id, &l.parameter).expect("validated automation reference");
                let mut points: Vec<(usize, f64, Shape)> = Vec::with_capacity(l.points.len());
                for q in &l.points {
                    let frame = time(&Position::Ticks { ticks: q.tick }, &p.musical_time)
                        .ceil_frame(rate)
                        .max(0) as usize;
                    if points.last().is_some_and(|p| p.0 == frame) {
                        points.pop();
                    }
                    points.push((frame, q.value, q.shape));
                }
                Curve {
                    points,
                    cursor: 0,
                    key: s.key,
                    slot: l
                        .parameter
                        .effect_id
                        .as_ref()
                        .and_then(|id| effects.iter().position(|e| e.effect_id == *id)),
                    base: s.base,
                    discrete: s.discrete,
                }
            })
            .collect();
        Self { curves, base }
    }
    pub fn apply(&mut self, frame: Option<usize>, chain: &mut super::effects::Chain) -> Values {
        let mut v = self.base;
        for c in &mut self.curves {
            let value = c.value(frame.map(|f| chain.position(f, c.slot)));
            if let Some(slot) = c.slot {
                chain.parameter(slot, c.key, value)
            } else {
                match c.key {
                    Key::Volume => v.volume = value,
                    Key::Pan => v.pan = value,
                    Key::SynthLevel => v.level = value,
                    Key::SynthAttack => v.attack = value,
                    Key::SynthRelease => v.release = value,
                    _ => {}
                }
            }
        }
        v
    }
    pub fn volume_active(&self) -> bool {
        self.curves
            .iter()
            .any(|c| c.key == Key::Volume && !c.points.is_empty())
    }
}
