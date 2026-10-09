//! Shared stereo Insert DSP. Construction/coefficient calculation happens off RT.
//! process/reset do not allocate, lock, resize buffers or touch files.
use crate::project::effects::{Band, Effect, Processor};

fn amplitude(db: f64) -> f64 {
    10f64.powf(db / 20.)
}
fn assign(target: &mut f64, value: f64) -> bool {
    if *target == value {
        false
    } else {
        *target = value;
        true
    }
}
fn clean(v: f64) -> f64 {
    if v.is_finite() && v.abs() > 1e-20 {
        v.clamp(-1e12, 1e12)
    } else {
        0.
    }
}
pub fn delay_seconds(time_ms: f64, sync: Option<f64>, bpm: f64) -> f64 {
    sync.map_or(time_ms / 1000., |beats| beats * 60. / bpm)
}
pub fn tail_seconds(effects: &[Effect], bpm: f64) -> f64 {
    effects
        .iter()
        .filter(|e| e.enabled)
        .map(|e| match e.processor {
            Processor::External { ref plugin } => plugin.tail_seconds(48000),
            Processor::Reverb { decay, wet } if wet > 0. => decay * 1.5,
            Processor::Delay {
                time_ms,
                feedback,
                wet,
                sync_beats,
            } if wet > 0. => {
                delay_seconds(time_ms, sync_beats, bpm)
                    * if feedback > 0. {
                        (1e-5f64.ln() / feedback.ln()).ceil().max(1.)
                    } else {
                        1.
                    }
            }
            Processor::Eq { ref bands } => bands
                .iter()
                .filter(|b| b.gain_db != 0.)
                .map(|b| {
                    // Conservative -100 dB ring-out, including low-frequency/high-Q boosts.
                    2. * 1e5f64.ln() * b.q * 10f64.powf(b.gain_db.abs() / 40.)
                        / (std::f64::consts::PI * b.frequency)
                })
                .sum(),
            _ => 0.,
        })
        .sum()
}
/// Conservative tail from persisted curve extrema, evaluated only during preparation.
pub fn project_tail(p: &crate::project::schema::Project, id: &str) -> f64 {
    let effects = if id == "master" {
        &p.master.inserts
    } else {
        &p.tracks.iter().find(|t| t.track_id == id).unwrap().inserts
    };
    let mut upper = effects.clone();
    let channel = p.automation.iter().find(|c| c.track_id == id && c.read);
    for e in &mut upper {
        if let Some(c) = channel {
            for l in c
                .lanes
                .iter()
                .filter(|l| l.parameter.effect_id.as_ref() == Some(&e.effect_id))
            {
                let hi = |base: f64| l.points.iter().fold(base, |v, q| v.max(q.value));
                let lo = |base: f64| l.points.iter().fold(base, |v, q| v.min(q.value));
                if l.parameter.name == "bypass" && l.points.iter().any(|q| q.value == 0.) {
                    e.enabled = true;
                }
                match &mut e.processor {
                    Processor::Reverb { decay, wet } => match l.parameter.name.as_str() {
                        "decay" => *decay = hi(*decay),
                        "wet" => *wet = hi(*wet),
                        _ => {}
                    },
                    Processor::Delay {
                        time_ms,
                        feedback,
                        wet,
                        sync_beats,
                    } => match l.parameter.name.as_str() {
                        "timeMs" => *time_ms = hi(*time_ms),
                        "feedback" => *feedback = hi(*feedback),
                        "wet" => *wet = hi(*wet),
                        "syncBeats" => {
                            let value = hi(sync_beats.unwrap_or(0.));
                            *time_ms =
                                (*time_ms).max(value * 60000. / p.musical_time.tempo_map[0].bpm);
                            *sync_beats = None
                        }
                        _ => {}
                    },
                    Processor::Eq { bands } => {
                        for (i, b) in bands.iter_mut().enumerate() {
                            if l.parameter.name == format!("band{i}.frequency") {
                                b.frequency = lo(b.frequency)
                            } else if l.parameter.name == format!("band{i}.q") {
                                b.q = hi(b.q)
                            } else if l.parameter.name == format!("band{i}.gainDb") {
                                b.gain_db = l
                                    .points
                                    .iter()
                                    .fold(b.gain_db.abs(), |v, q| v.max(q.value.abs()))
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    tail_seconds(&upper, p.musical_time.tempo_map[0].bpm)
}
#[derive(Clone, Copy, Default)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [[f64; 2]; 2],
}
impl Biquad {
    fn new(b: &Band, rate: f64) -> Self {
        let a = 10f64.powf(b.gain_db / 40.);
        let w = std::f64::consts::TAU * b.frequency.min(rate * 0.45) / rate;
        let alpha = w.sin() / (2. * b.q);
        let denom = 1. + alpha / a;
        Self {
            b: [
                (1. + alpha * a) / denom,
                -2. * w.cos() / denom,
                (1. - alpha * a) / denom,
            ],
            a: [-2. * w.cos() / denom, (1. - alpha / a) / denom],
            z: [[0.; 2]; 2],
        }
    }
    fn process(&mut self, x: [f64; 2]) -> [f64; 2] {
        std::array::from_fn(|ch| {
            let y = self.b[0] * x[ch] + self.z[ch][0];
            self.z[ch][0] = clean(self.b[1] * x[ch] - self.a[0] * y + self.z[ch][1]);
            self.z[ch][1] = clean(self.b[2] * x[ch] - self.a[1] * y);
            clean(y)
        })
    }
}
struct Line {
    data: Box<[f32]>,
    at: usize,
    valid: usize,
}
impl Line {
    fn new(n: usize) -> Self {
        Self {
            data: vec![0.; n.max(1)].into_boxed_slice(),
            at: 0,
            valid: 0,
        }
    }
    fn read(&self) -> f64 {
        if self.valid == self.data.len() {
            self.data[self.at] as f64
        } else {
            0.
        }
    }
    fn read_at(&self, delay: usize) -> f64 {
        if self.valid < delay {
            0.
        } else {
            self.data[(self.at + self.data.len() - delay) % self.data.len()] as f64
        }
    }
    fn write(&mut self, v: f64) {
        self.data[self.at] = clean(v) as f32;
        self.at += 1;
        if self.at == self.data.len() {
            self.at = 0;
        }
        self.valid = (self.valid + 1).min(self.data.len());
    }
    fn reset(&mut self) {
        self.at = 0;
        self.valid = 0;
    } // Old samples cannot leak after seek/reset.
}
struct Comb {
    line: Line,
    feedback: f64,
    low: f64,
}
struct Room {
    combs: [Vec<Comb>; 2],
    diffusers: [Vec<Line>; 2],
    wet: f64,
}
impl Room {
    fn new(rate: f64, decay: f64, wet: f64) -> Self {
        Self {
            combs: std::array::from_fn(|ch| {
                [0.0297, 0.0371, 0.0411, 0.0437]
                    .iter()
                    .map(|&s| {
                        let n = (s * rate + ch as f64 * 23. * rate / 44100.).round() as usize;
                        Comb {
                            line: Line::new(n),
                            feedback: 10f64.powf(-3. * n as f64 / rate / decay),
                            low: 0.,
                        }
                    })
                    .collect()
            }),
            diffusers: std::array::from_fn(|ch| {
                [0.005, 0.0017]
                    .iter()
                    .map(|&s| {
                        Line::new((s * rate + ch as f64 * 11. * rate / 44100.).round() as usize)
                    })
                    .collect()
            }),
            wet,
        }
    }
    fn process(&mut self, x: [f64; 2]) -> [f64; 2] {
        std::array::from_fn(|ch| {
            let input = x[ch] * 0.8 + x[1 - ch] * 0.2;
            let mut sum = 0.;
            for c in &mut self.combs[ch] {
                let y = c.line.read();
                c.low = clean(y * 0.65 + c.low * 0.35);
                c.line.write(input + c.low * c.feedback);
                sum += y * 0.25;
            }
            for line in &mut self.diffusers[ch] {
                let delayed = line.read();
                let y = delayed - sum * 0.5;
                line.write(sum + y * 0.5);
                sum = y;
            }
            clean(x[ch] * (1. - self.wet) + sum * self.wet)
        })
    }
    fn reset(&mut self) {
        for channel in &mut self.combs {
            for c in channel {
                c.line.reset();
                c.low = 0.;
            }
        }
        for channel in &mut self.diffusers {
            for l in channel {
                l.reset();
            }
        }
    }
}
enum Dsp {
    External(crate::plugins::Instance),
    Eq([Biquad; 3]),
    Compressor {
        threshold: f64,
        slope: f64,
        attack: f64,
        release: f64,
        makeup: f64,
        envelope: f64,
    },
    Limiter {
        ceiling: f64,
        input: f64,
        release: f64,
        gain: f64,
    },
    Reverb(Room),
    Delay {
        lines: [Line; 2],
        delay: usize,
        feedback: f64,
        wet: f64,
    },
}
struct Slot {
    enabled: bool,
    settings: Processor,
    index: usize,
    dsp: Dsp,
}
pub struct Chain {
    pdc: super::pdc::Port,
    cycle: Option<super::cycle::CycleFrames>,
    input_latency: usize,
    rate: f64,
    bpm: f64,
    slots: Vec<Slot>,
    tail: usize,
    remaining: usize,
    pub reduction: [f32; 8],
}
impl Chain {
    /// Only automation channels reserve parameter-change capacity. Static chains
    /// retain their original buffers and arithmetic. This constructor is off RT.
    pub fn automated(effects: &[Effect], rate: u32, bpm: f64) -> Self {
        let mut all = effects.to_vec();
        for e in &mut all {
            e.enabled = true;
        }
        let mut c = Self::new(&all, rate, bpm);
        for s in &mut c.slots {
            s.enabled = effects[s.index].enabled;
            if let Dsp::Delay { lines, .. } = &mut s.dsp {
                *lines = std::array::from_fn(|_| {
                    Line::new((rate as f64 * 2f64.max(240. / bpm)).ceil() as usize)
                });
            }
        }
        // Covers maximum legal Decay/Feedback/tempo-synced delay without a resize.
        c.tail = (tail_seconds(&all, bpm) * rate as f64).ceil() as usize;
        c
    }
    pub fn from_project(p: &crate::project::schema::Project, id: &str, rate: u32) -> Self {
        let effects = if id == "master" {
            &p.master.inserts
        } else {
            &p.tracks.iter().find(|t| t.track_id == id).unwrap().inserts
        };
        let on = p
            .automation
            .iter()
            .any(|c| c.track_id == id && c.read && !c.lanes.is_empty());
        let mut c = if on {
            Self::automated(effects, rate, p.musical_time.tempo_map[0].bpm)
        } else {
            Self::new(effects, rate, p.musical_time.tempo_map[0].bpm)
        };
        c.tail = (project_tail(p, id) * rate as f64).ceil() as usize;
        c
    }
    pub fn latency_pending(&self) -> bool {
        self.slots
            .iter()
            .any(|s| matches!(&s.dsp,Dsp::External(p) if p.latency_pending()))
    }
    pub fn settle_latency(&self) {
        for s in &self.slots {
            if let Dsp::External(p) = &s.dsp {
                if p.latency_pending() {
                    p.settle_latency();
                }
            }
        }
    }
    pub fn latency(&self) -> usize {
        self.slots
            .iter()
            .map(|s| match &s.dsp {
                Dsp::External(p) => p.latency(),
                _ => 0,
            })
            .sum()
    }
    pub fn pdc_supply(&self) -> std::sync::Arc<super::pdc::Supply> {
        self.pdc.supply.clone()
    }
    pub fn pdc_ready(&mut self) -> bool {
        self.pdc.update();
        self.pdc.ready(
            self.slots
                .iter()
                .map(|s| match &s.dsp {
                    Dsp::External(p) => p.latency(),
                    _ => 0,
                })
                .max()
                .unwrap_or(0),
        )
    }
    pub fn set_time(&mut self, input_latency: usize, cycle: Option<super::cycle::CycleFrames>) {
        self.input_latency = input_latency;
        self.cycle = cycle;
    }
    pub fn position(&self, frame: usize, slot: Option<usize>) -> usize {
        let preceding = slot.map_or(0, |index| {
            self.slots
                .iter()
                .take_while(|s| s.index != index)
                .map(|s| match &s.dsp {
                    Dsp::External(p) => p.latency(),
                    _ => 0,
                })
                .sum()
        });
        super::pdc::position(frame, self.input_latency + preceding, self.cycle)
    }
    pub fn clock(&mut self, frame: usize, num: i32, den: i32, playing: bool) {
        let mut delay = self.input_latency;
        for slot in &mut self.slots {
            if let Dsp::External(plugin) = &mut slot.dsp {
                plugin.clock(
                    super::pdc::position(frame, delay, self.cycle),
                    self.bpm,
                    num,
                    den,
                    playing,
                );
                delay += plugin.latency();
            }
        }
    }
    pub fn inherit(&mut self, old: &mut Self) {
        let tail = self.tail;
        std::mem::swap(self, old);
        self.tail = tail;
    }
    pub fn parameter(&mut self, index: usize, key: crate::project::automation::Key, value: f64) {
        use crate::project::automation::Key;
        let Some(s) = self.slots.iter_mut().find(|s| s.index == index) else {
            return;
        };
        if let (Key::Plugin(id), Dsp::External(plugin)) = (key, &mut s.dsp) {
            plugin.parameter(id, value);
            return;
        }
        if key == Key::Bypass {
            s.enabled = value < 0.5;
            return;
        }
        let changed = match (&mut s.settings, key) {
            (Processor::Eq { bands }, Key::EqFrequency(i)) => {
                assign(&mut bands[i].frequency, value)
            }
            (Processor::Eq { bands }, Key::EqGain(i)) => assign(&mut bands[i].gain_db, value),
            (Processor::Eq { bands }, Key::EqQ(i)) => assign(&mut bands[i].q, value),
            (Processor::Compressor { threshold_db, .. }, Key::Threshold) => {
                assign(threshold_db, value)
            }
            (Processor::Compressor { ratio, .. }, Key::Ratio) => assign(ratio, value),
            (Processor::Compressor { attack_ms, .. }, Key::Attack) => assign(attack_ms, value),
            (Processor::Compressor { release_ms, .. }, Key::Release) => assign(release_ms, value),
            (Processor::Compressor { makeup_db, .. }, Key::Makeup) => assign(makeup_db, value),
            (Processor::Limiter { ceiling_db, .. }, Key::Ceiling) => assign(ceiling_db, value),
            (Processor::Limiter { input_db, .. }, Key::Input) => assign(input_db, value),
            (Processor::Reverb { decay, .. }, Key::Decay) => assign(decay, value),
            (Processor::Reverb { wet, .. } | Processor::Delay { wet, .. }, Key::Wet) => {
                assign(wet, value)
            }
            (Processor::Delay { time_ms, .. }, Key::Time) => assign(time_ms, value),
            (Processor::Delay { feedback, .. }, Key::Feedback) => assign(feedback, value),
            (Processor::Delay { sync_beats, .. }, Key::Sync) => {
                let next = (value > 0.).then_some(value);
                let changed = *sync_beats != next;
                *sync_beats = next;
                changed
            }
            _ => false,
        };
        if !changed {
            return;
        }
        match (&s.settings, &mut s.dsp) {
            (Processor::Eq { bands }, Dsp::Eq(filters)) => {
                let i = match key {
                    Key::EqFrequency(i) | Key::EqGain(i) | Key::EqQ(i) => i,
                    _ => return,
                };
                let z = filters[i].z;
                filters[i] = Biquad::new(&bands[i], self.rate);
                filters[i].z = z;
            }
            (
                Processor::Compressor {
                    threshold_db,
                    ratio,
                    attack_ms,
                    release_ms,
                    makeup_db,
                },
                Dsp::Compressor {
                    threshold,
                    slope,
                    attack,
                    release,
                    makeup,
                    ..
                },
            ) => match key {
                Key::Threshold => *threshold = amplitude(*threshold_db),
                Key::Ratio => *slope = 1. / ratio - 1.,
                Key::Attack => *attack = (-1. / (attack_ms * self.rate * 0.001)).exp(),
                Key::Release => *release = (-1. / (release_ms * self.rate * 0.001)).exp(),
                Key::Makeup => *makeup = amplitude(*makeup_db),
                _ => {}
            },
            (
                Processor::Limiter {
                    ceiling_db,
                    input_db,
                },
                Dsp::Limiter { ceiling, input, .. },
            ) => {
                *ceiling = amplitude(*ceiling_db);
                *input = amplitude(*input_db);
            }
            (Processor::Reverb { decay, wet }, Dsp::Reverb(room)) => {
                room.wet = *wet;
                if key == Key::Decay {
                    for comb in room.combs.iter_mut().flatten() {
                        comb.feedback =
                            10f64.powf(-3. * comb.line.data.len() as f64 / self.rate / decay);
                    }
                }
            }
            (
                Processor::Delay {
                    time_ms,
                    feedback,
                    wet,
                    sync_beats,
                },
                Dsp::Delay {
                    lines,
                    delay,
                    feedback: f,
                    wet: w,
                },
            ) => {
                *f = *feedback;
                *w = *wet;
                *delay = ((delay_seconds(*time_ms, *sync_beats, self.bpm) * self.rate).round()
                    as usize)
                    .clamp(1, lines[0].data.len());
            }
            _ => {}
        }
    }
    pub fn new(effects: &[Effect], rate: u32, bpm: f64) -> Self {
        let r = rate as f64;
        let mut slots = Vec::new();
        for (index, e) in effects
            .iter()
            .enumerate()
            .filter(|(_, e)| e.enabled || matches!(e.processor, Processor::External { .. }))
        {
            let dsp = match &e.processor {
                Processor::External { plugin } => Dsp::External(crate::plugins::Instance::new(
                    plugin,
                    &e.effect_id,
                    rate,
                    bpm,
                )),
                Processor::Eq { bands } => {
                    Dsp::Eq(std::array::from_fn(|i| Biquad::new(&bands[i], r)))
                }
                Processor::Compressor {
                    threshold_db,
                    ratio,
                    attack_ms,
                    release_ms,
                    makeup_db,
                } => Dsp::Compressor {
                    threshold: amplitude(*threshold_db),
                    slope: 1. / ratio - 1.,
                    attack: (-1. / (attack_ms * r * 0.001)).exp(),
                    release: (-1. / (release_ms * r * 0.001)).exp(),
                    makeup: amplitude(*makeup_db),
                    envelope: 0.,
                },
                Processor::Limiter {
                    ceiling_db,
                    input_db,
                } => Dsp::Limiter {
                    ceiling: amplitude(*ceiling_db),
                    input: amplitude(*input_db),
                    release: (-1. / (r * 0.05)).exp(),
                    gain: 1.,
                },
                Processor::Reverb { decay, wet } => Dsp::Reverb(Room::new(r, *decay, *wet)),
                Processor::Delay {
                    time_ms,
                    feedback,
                    wet,
                    sync_beats,
                } => Dsp::Delay {
                    delay: (delay_seconds(*time_ms, *sync_beats, bpm) * r)
                        .round()
                        .max(1.) as usize,
                    lines: std::array::from_fn(|_| {
                        Line::new((delay_seconds(*time_ms, *sync_beats, bpm) * r).round() as usize)
                    }),
                    feedback: *feedback,
                    wet: *wet,
                },
            };
            slots.push(Slot {
                index,
                dsp,
                enabled: e.enabled,
                settings: e.processor.clone(),
            });
        }
        let mut pdc = super::pdc::Port::new(slots.len());
        pdc.prepare(
            slots
                .iter()
                .map(|s| match &s.dsp {
                    Dsp::External(p) => p.latency(),
                    _ => 0,
                })
                .max()
                .unwrap_or(0),
        );
        Self {
            pdc,
            cycle: None,
            input_latency: 0,
            rate: r,
            bpm,
            slots,
            tail: (tail_seconds(effects, bpm) * r).ceil() as usize,
            remaining: 0,
            reduction: [0.; 8],
        }
    }
    pub fn ceiling(&self) -> Option<f64> {
        self.slots.iter().rev().find(|s| s.enabled).and_then(|s| {
            if let Dsp::Limiter { ceiling, .. } = s.dsp {
                Some(ceiling)
            } else {
                None
            }
        })
    }
    pub fn enabled(&self) -> bool {
        self.slots.iter().any(|s| s.enabled) || self.latency() > 0
    }
    pub fn active(&self) -> bool {
        self.remaining > 0
    }
    pub fn reset(&mut self) {
        self.pdc.reset();
        self.remaining = 0;
        self.reduction = [0.; 8];
        for s in &mut self.slots {
            match &mut s.dsp {
                Dsp::External(plugin) => plugin.reset(),
                Dsp::Eq(b) => {
                    for b in b {
                        b.z = [[0.; 2]; 2];
                    }
                }
                Dsp::Compressor { envelope, .. } => *envelope = 0.,
                Dsp::Limiter { gain, .. } => *gain = 1.,
                Dsp::Reverb(r) => r.reset(),
                Dsp::Delay { lines, .. } => {
                    for l in lines {
                        l.reset();
                    }
                }
            }
        }
    }
    pub fn process(&mut self, mut x: [f64; 2]) -> [f64; 2] {
        if x[0].abs().max(x[1].abs()) > 1e-12 {
            self.remaining = self.tail.saturating_add(self.latency());
        } else {
            self.remaining = self.remaining.saturating_sub(1);
        }
        for (i, slot) in self.slots.iter_mut().enumerate() {
            let latency = match &slot.dsp {
                Dsp::External(p) => p.latency(),
                _ => 0,
            };
            let bypass = self.pdc.sample(i, x, latency);
            if !slot.enabled {
                x = bypass;
                self.reduction[slot.index] = 0.;
                continue;
            }
            x = match &mut slot.dsp {
                Dsp::External(plugin) => plugin.process(x),
                Dsp::Eq(bands) => {
                    for b in bands {
                        x = b.process(x);
                    }
                    x
                }
                Dsp::Compressor {
                    threshold,
                    slope,
                    attack,
                    release,
                    makeup,
                    envelope,
                } => {
                    let peak = x[0].abs().max(x[1].abs());
                    let c = if peak > *envelope { *attack } else { *release };
                    *envelope = clean(c * *envelope + (1. - c) * peak);
                    let gain = if *envelope > *threshold {
                        (*envelope / *threshold).powf(*slope)
                    } else {
                        1.
                    };
                    self.reduction[slot.index] = (-20. * gain.max(1e-12).log10()) as f32;
                    x.map(|v| clean(v * gain * *makeup))
                }
                Dsp::Limiter {
                    ceiling,
                    input,
                    release,
                    gain,
                } => {
                    let input_pair = x.map(|v| v * *input);
                    let peak = input_pair[0].abs().max(input_pair[1].abs());
                    let target = if peak > *ceiling { *ceiling / peak } else { 1. };
                    *gain = if target < *gain {
                        target
                    } else {
                        *release * *gain + (1. - *release) * target
                    };
                    self.reduction[slot.index] = (-20. * gain.max(1e-12).log10()) as f32;
                    // A sample-peak brickwall, zero lookahead/latency. Later inserts
                    // can raise the level: place this slot last for an output ceiling.
                    input_pair.map(|v| (v * *gain).clamp(-*ceiling, *ceiling))
                }
                Dsp::Reverb(r) => r.process(x),
                Dsp::Delay {
                    lines,
                    delay,
                    feedback,
                    wet,
                } => std::array::from_fn(|ch| {
                    let delayed = lines[ch].read_at(*delay);
                    lines[ch].write(x[ch] + delayed * *feedback);
                    clean(x[ch] * (1. - *wet) + delayed * *wet)
                }),
            };
        }
        x
    }
}
