//! Control -> callback ownership transfer and deferred reclamation. No shared
//! mutable DSP, unsafe pointers, callback locks or last-owner drops.
use super::effects::Chain;
use crate::project::{
    effects::{Effect, Processor},
    schema::{Project, TrackKind},
};
use rtrb::{Consumer, Producer, RingBuffer};
use serde::Serialize;
use std::sync::{
    atomic::{AtomicU32, AtomicU64, Ordering::Relaxed},
    Arc, Mutex,
};

const HISTORY: usize = 2048; // > existing 262144-frame audio read-ahead at 256-frame buckets
struct Meter {
    id: String,
    index: usize,
    live: AtomicU32,
    history: Box<[AtomicU64]>,
}
pub struct Meters {
    channel: String,
    meters: Vec<Meter>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reading {
    pub channel_id: String,
    pub effect_id: String,
    pub reduction_db: f32,
}
impl Meters {
    pub fn new(channel: &str, effects: &[Effect], history: bool) -> Arc<Self> {
        Arc::new(Self {
            channel: channel.into(),
            meters: effects
                .iter()
                .enumerate()
                .filter(|(_, e)| {
                    matches!(
                        e.processor,
                        Processor::Compressor { .. } | Processor::Limiter { .. }
                    )
                })
                .map(|(index, e)| Meter {
                    id: e.effect_id.clone(),
                    index,
                    live: AtomicU32::new(0),
                    history: if history {
                        (0..HISTORY).map(|_| AtomicU64::new(0)).collect()
                    } else {
                        Box::default()
                    },
                })
                .collect(),
        })
    }
    pub fn publish(&self, values: &[f32; 8], frame: Option<usize>) {
        for m in &self.meters {
            let bits = values[m.index].to_bits();
            if let Some(at) = frame.filter(|_| !m.history.is_empty()) {
                let key = at / 256;
                m.history[key % HISTORY].store(((key as u64 + 1) << 32) | bits as u64, Relaxed);
            } else {
                m.live.store(bits, Relaxed);
            }
        }
    }
    pub fn read(&self, frame: Option<usize>) -> Vec<Reading> {
        self.meters
            .iter()
            .map(|m| {
                let bits = if let Some(at) = frame.filter(|_| !m.history.is_empty()) {
                    let key = at / 256;
                    let packed = m.history[key % HISTORY].load(Relaxed);
                    if packed >> 32 == key as u64 + 1 {
                        packed as u32
                    } else {
                        0
                    }
                } else {
                    m.live.load(Relaxed)
                };
                Reading {
                    channel_id: self.channel.clone(),
                    effect_id: m.id.clone(),
                    reduction_db: f32::from_bits(bits),
                }
            })
            .collect()
    }
}
#[derive(Clone, PartialEq)]
struct MidiConfig {
    id: String,
    index: usize,
    audible: bool,
    effects: Vec<Effect>,
}
#[derive(Clone, PartialEq)]
struct Config {
    solo: bool,
    document: Option<Project>,
    bpm: f64,
    midi: Vec<MidiConfig>,
    master: Vec<Effect>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            solo: false,
            document: None,
            bpm: 120.,
            midi: vec![],
            master: vec![],
        }
    }
}
impl Config {
    fn from_project(p: &Project) -> Self {
        let solo = p.tracks.iter().any(|t| t.mix.solo);
        let mut document = p.clone();
        document.assets.clear();
        document.tracks.retain(|t| t.kind == TrackKind::Midi);
        for t in &mut document.tracks {
            t.clips.clear();
        }
        document.primary_clip_id = None;
        document.master.volume_db = 0.;
        document.automation.retain(|c| {
            c.track_id == "master" || document.tracks.iter().any(|t| t.track_id == c.track_id)
        });
        Self {
            solo,
            document: Some(document),
            bpm: p.musical_time.tempo_map[0].bpm,
            master: p.master.inserts.clone(),
            midi: p
                .tracks
                .iter()
                .filter(|t| t.kind == TrackKind::Midi)
                .enumerate()
                .map(|(index, t)| MidiConfig {
                    id: t.track_id.clone(),
                    index,
                    audible: !t.instrument.is_none() && t.mix.gains(solo) != [0.; 2],
                    effects: t.inserts.clone(),
                })
                .collect(),
        }
    }
}
// Fingerprints and parameter restoration tables are prepared off the callback.
fn fingerprint(p: Option<&Project>, id: &str, rate: u32) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    rate.hash(&mut h);
    if let Some(p) = p {
        p.musical_time.tempo_map[0].bpm.to_bits().hash(&mut h);
        let effects = if id == "master" {
            &p.master.inserts
        } else {
            &p.tracks.iter().find(|t| t.track_id == id).unwrap().inserts
        };
        serde_json::to_vec(effects).unwrap().hash(&mut h);
        if let Some(t) = p.tracks.iter().find(|t| t.track_id == id) {
            serde_json::to_vec(&(t.instrument, t.extensions.get(crate::plugins::INSTRUMENT)))
                .unwrap()
                .hash(&mut h);
        }
        p.automation
            .iter()
            .any(|c| c.track_id == id && c.read && !c.lanes.is_empty())
            .hash(&mut h);
    }
    h.finish()
}
fn bases(p: Option<&Project>, id: &str) -> Vec<(usize, crate::project::automation::Key, f64)> {
    let Some(p) = p else { return vec![] };
    let effects = if id == "master" {
        &p.master.inserts
    } else {
        &p.tracks.iter().find(|t| t.track_id == id).unwrap().inserts
    };
    let mut out = vec![];
    for (i, e) in effects.iter().enumerate() {
        for name in crate::project::automation::parameter_names(e) {
            let param = crate::project::automation::Parameter {
                effect_id: Some(e.effect_id.clone()),
                name,
            };
            let s = crate::project::automation::spec(p, id, &param).unwrap();
            out.push((i, s.key, s.base));
        }
    }
    out
}
struct MidiChain {
    latency: usize,
    fingerprint: u64,
    bases: Vec<(usize, crate::project::automation::Key, f64)>,
    automation: super::automation::Automation,
    key: [u8; 16],
    index: usize,
    audible: bool,
    chain: Chain,
    last: [f64; 2],
    external: Option<crate::plugins::Instance>,
    external_remaining: usize,
    external_epoch: u64,
    gains: [f64; 2],
    meters: Arc<Meters>,
}
struct Rack {
    pdc: super::pdc::Port,
    midi_latency: usize,
    master_latency: usize,
    cycle: Option<super::cycle::CycleFrames>,
    fingerprint: u64,
    bases: Vec<(usize, crate::project::automation::Key, f64)>,
    defaults: Vec<(
        [u8; 16],
        [f64; 2],
        crate::project::automation::SynthSettings,
    )>,
    master_automation: Option<super::automation::Automation>,
    midi: Vec<MidiChain>,
    master: Chain,
    master_meters: Arc<Meters>,
    master_muted: bool,
    frame: Option<usize>,
    bpm: f64,
    num: i32,
    den: i32,
}
impl Rack {
    fn new(config: &Config, rate: u32) -> Box<Self> {
        let mut rack = Box::new(Self {
            pdc: super::pdc::Port::new(config.midi.len() + 2),
            midi_latency: 0,
            master_latency: 0,
            cycle: config
                .document
                .as_ref()
                .and_then(|p| super::cycle::CycleFrames::compile(p, rate).ok().flatten()),
            fingerprint: fingerprint(config.document.as_ref(), "master", rate),
            bases: bases(config.document.as_ref(), "master"),
            defaults: config
                .document
                .as_ref()
                .map(|p| {
                    p.tracks
                        .iter()
                        .map(|t| {
                            (
                                *uuid::Uuid::parse_str(&t.track_id).unwrap().as_bytes(),
                                t.mix.gains(config.solo),
                                t.synth.clone(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
            master_automation: config
                .document
                .as_ref()
                .map(|p| super::automation::Automation::compile(p, "master", rate)),
            midi: config
                .midi
                .iter()
                .map(|c| MidiChain {
                    latency: 0,
                    fingerprint: fingerprint(config.document.as_ref(), &c.id, rate),
                    bases: bases(config.document.as_ref(), &c.id),
                    automation: super::automation::Automation::compile(
                        config.document.as_ref().unwrap(),
                        &c.id,
                        rate,
                    ),
                    key: *uuid::Uuid::parse_str(&c.id)
                        .expect("validated Track UUID")
                        .as_bytes(),
                    index: c.index,
                    audible: c.audible,
                    chain: Chain::from_project(config.document.as_ref().unwrap(), &c.id, rate),
                    external: config
                        .document
                        .as_ref()
                        .and_then(|p| p.tracks.iter().find(|t| t.track_id == c.id))
                        .filter(|t| t.instrument == crate::project::schema::Instrument::External)
                        .and_then(crate::plugins::instrument)
                        .map(|s| crate::plugins::Instance::new(&s, &c.id, rate, config.bpm)),
                    external_remaining: 0,
                    external_epoch: 0,
                    gains: [1.; 2],
                    last: [0.; 2],
                    meters: Meters::new(&c.id, &c.effects, false),
                })
                .collect(),
            master: config.document.as_ref().map_or_else(
                || Chain::new(&config.master, rate, config.bpm),
                |p| Chain::from_project(p, "master", rate),
            ),
            master_meters: Meters::new("master", &config.master, false),
            master_muted: false,
            frame: None,
            bpm: config.bpm,
            num: config
                .document
                .as_ref()
                .map_or(4, |p| p.musical_time.time_signatures[0].numerator as i32),
            den: config
                .document
                .as_ref()
                .map_or(4, |p| p.musical_time.time_signatures[0].denominator as i32),
        });
        rack.refresh_pdc();
        rack.pdc.prepare(rack.midi_latency.max(rack.master_latency));
        rack
    }
    fn latency_pending(&self) -> bool {
        self.master.latency_pending()
            || self.midi.iter().any(|c| {
                c.chain.latency_pending()
                    || c.external.as_ref().is_some_and(|p| p.latency_pending())
            })
    }
    fn refresh_pdc(&mut self) -> (bool, bool) {
        let mut changed = self.pdc.update();
        let mut ready = self.master.pdc_ready();
        let mut maximum = 0;
        for c in &mut self.midi {
            ready &= c.chain.pdc_ready();
            let l = c.chain.latency() + c.external.as_ref().map_or(0, |p| p.latency());
            changed |= c.latency != l;
            c.latency = l;
            maximum = maximum.max(l);
        }
        let master = self.master.latency();
        changed |= self.midi_latency != maximum || self.master_latency != master;
        self.midi_latency = maximum;
        self.master_latency = master;
        self.pdc
            .supply
            .track_latency
            .store(maximum, std::sync::atomic::Ordering::Release);
        self.pdc
            .supply
            .master_latency
            .store(master, std::sync::atomic::Ordering::Release);
        (
            ready && self.pdc.ready(maximum.max(master)) && !self.latency_pending(),
            changed,
        )
    }
    fn inherit(&mut self, old: &mut Self) {
        if self.fingerprint == old.fingerprint {
            self.master.inherit(&mut old.master);
            for &(slot, key, value) in &self.bases {
                self.master.parameter(slot, key, value);
            }
        }
        for c in &mut self.midi {
            if let Some(previous) = old
                .midi
                .iter_mut()
                .find(|o| o.key == c.key && o.fingerprint == c.fingerprint)
            {
                c.chain.inherit(&mut previous.chain);
                for &(slot, key, value) in &c.bases {
                    c.chain.parameter(slot, key, value);
                }
                std::mem::swap(&mut c.external, &mut previous.external);
                c.external_remaining = previous.external_remaining;
                c.external_epoch = previous.external_epoch;
                c.last = previous.last;
            }
        }
    }
    fn meters(&self) -> Vec<Arc<Meters>> {
        self.midi
            .iter()
            .map(|c| c.meters.clone())
            .chain(std::iter::once(self.master_meters.clone()))
            .collect()
    }
}
struct Lane {
    incoming: Producer<Box<Rack>>,
    retired: Consumer<Box<Rack>>,
    pending: Option<Box<Rack>>,
}
struct Control {
    pdc: Option<Arc<super::pdc::Supply>>,
    config: Config,
    rate: u32,
    lane: Option<Lane>,
    meters: Vec<Arc<Meters>>,
}
#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PdcSnapshot {
    pub audio_lookahead_samples: usize,
    pub track_delay_samples: usize,
    pub master_latency_samples: usize,
    pub output_latency_samples: usize,
    pub error: Option<String>,
}
pub struct Exchange {
    control: Mutex<Control>,
}
impl Default for Exchange {
    fn default() -> Self {
        Self {
            control: Mutex::new(Control {
                pdc: None,
                config: Config::default(),
                rate: 48000,
                lane: None,
                meters: vec![],
            }),
        }
    }
}
fn pump(lane: &mut Lane) {
    while lane.retired.pop().is_ok() {} // Drop all retired buffers here, never in render().
    if !lane.incoming.is_full() {
        if let Some(next) = lane.pending.take() {
            assert!(lane.incoming.push(next).is_ok(), "single producer capacity");
        }
    }
}
impl Exchange {
    /// Called only before a new device callback starts. A reconnect joins the old one first.
    pub fn attach(&self, rate: u32) -> Port {
        let mut c = self.control.lock().unwrap_or_else(|e| e.into_inner());
        c.rate = rate;
        let rack = Rack::new(&c.config, rate);
        c.pdc = Some(rack.pdc.supply.clone());
        c.meters = rack.meters();
        let (tx, rx) = RingBuffer::new(2);
        let (retire_tx, retire_rx) = RingBuffer::new(4);
        c.lane = Some(Lane {
            incoming: tx,
            retired: retire_rx,
            pending: None,
        });
        Port {
            rack,
            incoming: rx,
            retired: retire_tx,
            pdc_changed: false,
            generation: u64::MAX,
            dirty: true,
        }
    }
    pub fn configure(&self, p: &Project) {
        let config = Config::from_project(p);
        let mut c = self.control.lock().unwrap_or_else(|e| e.into_inner());
        if c.config == config {
            return;
        }
        if c.lane.is_none() {
            c.config = config;
            return;
        }
        let rack = Rack::new(&config, c.rate);
        c.config = config;
        c.pdc = Some(rack.pdc.supply.clone());
        c.meters = rack.meters();
        if let Some(lane) = &mut c.lane {
            lane.pending = Some(rack);
            pump(lane);
        }
    }
    pub fn collect(&self) {
        super::pdc::service_all();
        let mut c = self.control.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(l) = &mut c.lane {
            pump(l);
        }
    }
    pub fn pdc_snapshot(&self) -> PdcSnapshot {
        use std::sync::atomic::Ordering::Acquire;
        let c = self.control.lock().unwrap_or_else(|e| e.into_inner());
        let track = c.pdc.as_ref().map_or(0, |p| p.track_latency.load(Acquire));
        let master = c.pdc.as_ref().map_or(0, |p| p.master_latency.load(Acquire));
        PdcSnapshot {
            audio_lookahead_samples: 0,
            track_delay_samples: track,
            master_latency_samples: master,
            output_latency_samples: track + master,
            error: super::pdc::capacity_error(),
        }
    }
    pub fn snapshot(&self) -> Vec<Reading> {
        let c = self.control.lock().unwrap_or_else(|e| e.into_inner());
        c.meters.iter().flat_map(|m| m.read(None)).collect()
    }
}
pub struct Port {
    pdc_changed: bool,
    generation: u64,
    dirty: bool,
    rack: Box<Rack>,
    incoming: Consumer<Box<Rack>>,
    retired: Producer<Box<Rack>>,
}
impl Port {
    pub fn update(&mut self) {
        // With one consumer, checking retirement capacity before popping guarantees
        // that an old DSP owner is never dropped on this thread under queue pressure.
        if !self.retired.is_full() {
            if let Ok(mut next) = self.incoming.pop() {
                self.pdc_changed |=
                    self.latency() > 0 || next.midi_latency + next.master_latency > 0;
                next.inherit(&mut self.rack);
                self.dirty = true;
                let old = std::mem::replace(&mut self.rack, next);
                assert!(
                    self.retired.push(old).is_ok(),
                    "single producer retirement capacity"
                );
            }
        }
    }
    pub fn settle_latency(&self) {
        self.rack.master.settle_latency();
        for c in &self.rack.midi {
            c.chain.settle_latency();
            if let Some(p) = &c.external {
                if p.latency_pending() {
                    p.settle_latency();
                }
            }
        }
    }
    pub fn latency_dirty(&self) -> bool {
        self.rack.latency_pending()
            || self.rack.master.latency() != self.rack.master_latency
            || self.rack.midi.iter().any(|c| {
                c.chain.latency() + c.external.as_ref().map_or(0, |p| p.latency()) != c.latency
            })
    }
    pub fn latency(&self) -> usize {
        self.rack.midi_latency + self.rack.master_latency
    }
    pub fn track_latency(&self) -> usize {
        self.rack.midi_latency
    }
    pub fn set_cycle(&mut self, c: Option<super::cycle::CycleFrames>) {
        self.rack.cycle = c;
    }
    pub fn sync_pdc(&mut self) -> (bool, bool) {
        let (ready, changed) = self.rack.refresh_pdc();
        let changed = changed || std::mem::take(&mut self.pdc_changed);
        (ready, changed)
    }
    pub fn audio(&mut self, input: [f64; 2]) -> [f64; 2] {
        self.rack.pdc.sample(0, input, self.rack.midi_latency)
    }
    pub fn route(
        &mut self,
        generation: u64,
        plan: Option<&super::midi::MidiPlan>,
        synth: &mut super::synth::BasicSynth,
    ) {
        if !self.dirty && self.generation == generation {
            return;
        }
        self.dirty = false;
        self.generation = generation;
        if let Some(p) = plan {
            for (i, (key, g, s)) in self.rack.defaults.iter().enumerate() {
                let index = if p.track_keys.get(i) == Some(key) {
                    Some(i)
                } else {
                    p.track_keys.iter().position(|k| k == key)
                };
                if let Some(index) = index {
                    synth.automate(index, *g, s.level_db, s.attack_ms, s.release_ms);
                }
            }
        }
        for c in &mut self.rack.midi {
            c.index = plan
                .and_then(|p| p.track_keys.iter().position(|k| k == &c.key))
                .unwrap_or(usize::MAX);
        }
    }
    pub fn reset(&mut self) {
        self.rack.pdc.reset();
        self.rack.master.reset();
        for c in &mut self.rack.midi {
            c.chain.reset();
            c.last = [0.; 2];
        }
    }
    pub fn active(&self) -> bool {
        self.rack.pdc.active()
            || self.rack.master.active()
            || self.rack.midi.iter().any(|c| {
                c.audible
                    && (c.chain.active()
                        || (c.external.is_some()
                            && (self.rack.frame.is_some() || c.external_remaining > 0)))
            })
    }
    pub fn ceiling(&self) -> Option<f64> {
        self.rack.master.ceiling()
    }
    pub fn has_master(&self) -> bool {
        self.rack.master.enabled()
    }
    pub fn automate(
        &mut self,
        frame: Option<usize>,
        synth: &mut super::synth::BasicSynth,
    ) -> Option<f64> {
        self.rack.frame = frame;
        self.rack
            .master
            .set_time(self.rack.midi_latency, self.rack.cycle);
        self.rack.master.clock(
            frame.unwrap_or(0),
            self.rack.num,
            self.rack.den,
            frame.is_some(),
        );
        for c in &mut self.rack.midi {
            if c.index != usize::MAX {
                c.chain.set_time(
                    c.external.as_ref().map_or(0, |p| p.latency()),
                    self.rack.cycle,
                );
                let mut v = c.automation.apply(frame, &mut c.chain);
                v.audible = c.audible;
                c.gains = v.gains(false);
                if let Some(plugin) = &mut c.external {
                    for curve in &mut c.automation.curves {
                        if curve.slot.is_none() {
                            if let crate::project::automation::Key::Plugin(id) = curve.key {
                                plugin.parameter(
                                    id,
                                    curve.value(
                                        frame.map(|f| super::pdc::position(f, 0, self.rack.cycle)),
                                    ),
                                );
                            }
                        }
                    }
                }
                c.chain.clock(
                    frame.unwrap_or(0),
                    self.rack.num,
                    self.rack.den,
                    frame.is_some(),
                );
                synth.automate(c.index, v.gains(false), v.level, v.attack, v.release);
            }
        }
        self.rack.master_automation.as_mut().and_then(|a| {
            let v = a.apply(frame, &mut self.rack.master);
            (frame.is_some() && a.volume_active()).then(|| v.gains(true)[0])
        })
    }
    pub fn tracks(
        &mut self,
        mut sum: [f64; 2],
        synth: &mut super::synth::BasicSynth,
        sampled: bool,
    ) -> [f64; 2] {
        for (i, c) in self.rack.midi.iter_mut().enumerate() {
            if c.index == usize::MAX {
                c.last = [0.; 2];
                continue;
            }
            if let Some(plugin) = &mut c.external {
                if c.external_epoch != synth.reset_epoch() {
                    plugin.reset();
                    c.external_epoch = synth.reset_epoch();
                    c.external_remaining = 0;
                }
                for e in synth
                    .external_events()
                    .iter()
                    .filter(|e| e.track == c.index)
                {
                    plugin.midi(e.status, e.a, e.b, e.voice);
                    c.external_remaining = plugin.tail.max(96000);
                }
                if self.rack.frame.is_some() {
                    c.external_remaining = plugin.tail.max(96000);
                } else {
                    c.external_remaining = c.external_remaining.saturating_sub(1);
                }
                plugin.clock(
                    super::pdc::position(self.rack.frame.unwrap_or(0), 0, self.rack.cycle),
                    self.rack.bpm,
                    self.rack.num,
                    self.rack.den,
                    self.rack.frame.is_some(),
                );
                let dry = plugin.process([0.; 2]);
                c.last = if c.audible {
                    c.chain.process([dry[0] * c.gains[0], dry[1] * c.gains[1]])
                } else {
                    [0.; 2]
                };
                c.last = self
                    .rack
                    .pdc
                    .sample(i + 1, c.last, self.rack.midi_latency - c.latency);
                for (v, last) in sum.iter_mut().zip(c.last) {
                    *v += last;
                }
                continue;
            }
            let dry = if sampled {
                synth.track_output(c.index).map(f64::from)
            } else {
                [0.; 2]
            };
            c.last = if c.audible {
                c.chain.process(dry)
            } else {
                [0.; 2]
            };
            c.last = self
                .rack
                .pdc
                .sample(i + 1, c.last, self.rack.midi_latency - c.latency);
            for ch in 0..2 {
                sum[ch] += c.last[ch] - dry[ch];
            }
        }
        synth.clear_external();
        sum
    }
    pub fn track_output(&self, index: usize, fallback: [f32; 2]) -> [f32; 2] {
        self.rack
            .midi
            .iter()
            .find(|c| c.index == index)
            .map_or(fallback, |c| c.last.map(|v| v as f32))
    }
    pub fn master(&mut self, input: [f64; 2], muted: bool) -> [f64; 2] {
        if self.rack.master_latency > 0 {
            // Mute reaches the output at the same presentation time as the audio.
            let gate =
                self.rack
                    .pdc
                    .gate(self.rack.midi.len() + 1, !muted, self.rack.master_latency);
            return self.rack.master.process(input).map(|x| x * gate);
        }
        if muted {
            if !self.rack.master_muted {
                self.rack.master.reset();
            }
            self.rack.master_muted = true;
            return [0.; 2];
        }
        self.rack.master_muted = false;
        self.rack.master.process(input)
    }
    pub fn publish(&self) {
        self.rack
            .master_meters
            .publish(&self.rack.master.reduction, None);
        for c in &self.rack.midi {
            c.meters.publish(&c.chain.reduction, None);
        }
    }
}
