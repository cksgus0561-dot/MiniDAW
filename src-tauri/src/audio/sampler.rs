//! SFZ sample playback inside the instrument host (BasicSynth). All storage is
//! allocated in `new`; event()/render() only read immutable Banks, update fixed
//! arrays and clone/drop Arcs whose last owner is the control-side cache.
//! Envelope/pitch coefficients are computed per note event, not per sample.
use super::midi::{Control, MidiControlEvent, MidiEvent, MidiPlan, MAX_VOICES};
use super::sfz::{Bank, LoopMode, Region, Trigger};
use std::sync::Arc;

const TRACKS: usize = 4096;
const SLOTS: usize = 64;
const NONE: u16 = u16::MAX;
// Click-safe minimum release and the fast off_by / steal fade, seconds.
const MIN_RELEASE: f64 = 0.004;
const FAST: f64 = 0.006;
const STEAL: f64 = 0.002;
const SILENT: f64 = 1e-4;

#[derive(Clone, Copy)]
struct Chan {
    epoch: u64,
    pedal: bool,
    bend: i16,
    volume: f64,
    expression: f64,
    pan: [f64; 2],
    keys_down: u32,
    keyswitch: Option<u8>,
    cc: [u8; 128],
    held: [bool; 128],
    sustained: [bool; 128],
    velocity: [u8; 128],
    onset: [u64; 128],
    seq: [u32; 128],
}
impl Chan {
    fn fresh(epoch: u64, bank: Option<&Bank>) -> Self {
        let mut c = Self {
            epoch,
            pedal: false,
            bend: 0,
            volume: 1.0,
            expression: 1.0,
            pan: [1.0; 2],
            keys_down: 0,
            keyswitch: None,
            cc: [0; 128],
            held: [false; 128],
            sustained: [false; 128],
            velocity: [0; 128],
            onset: [0; 128],
            seq: [0; 128],
        };
        if let Some(b) = bank {
            for &(n, v) in &b.cc_init {
                c.cc[n as usize] = v;
                match n {
                    7 => c.volume = f64::from(v) / 127.0,
                    11 => c.expression = f64::from(v) / 127.0,
                    10 => c.pan = if v <= 64 { [1.0, f64::from(v) / 64.0] } else { [f64::from(127 - v) / 63.0, 1.0] },
                    _ => {}
                }
            }
            c.keyswitch = b.sw_default;
        }
        c
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Stage {
    #[default]
    Delay,
    Attack,
    Hold,
    Decay,
    Sustain,
    Release,
}
#[derive(Clone, Copy, Default)]
struct Voice {
    id: u32,
    track: u16,
    slot: u16,
    channel: u8,
    key: u8,
    region: u32,
    pos: f64,
    step: f64,
    bend: f64,
    gain: [f64; 2],
    stage: Stage,
    level: f64,
    left: f64,
    attack: f64,
    decay: f64,
    sustain: f64,
    release: f64,
    down: bool,
    // Release-trigger and one-shot voices ignore Note Off.
    free: bool,
    age: u64,
    steal: [f64; 2],
    steal_left: f64,
    last: [f64; 2],
}

pub struct Sampler {
    rate: f64,
    banks: Box<[Option<Arc<Bank>>]>,
    slot_of: Box<[u16]>,
    slot_track: Box<[u16]>,
    chans: Box<[Chan]>,
    voices: Box<[Voice]>,
    count: usize,
    bus: Box<[[f64; 2]]>,
    stamps: Box<[u64]>,
    touched: Box<[u16]>,
    touched_count: usize,
    sample_epoch: u64,
    epoch: u64,
    serial: u64,
    clock: u64,
    rng: u64,
}

fn pow2(cents: f64) -> f64 {
    (cents / 1200.0).exp2()
}
/// Multiplier reaching -60 dB after `seconds`.
fn decay_coef(seconds: f64, rate: f64) -> f64 {
    (0.001f64.ln() / (seconds.max(1e-6) * rate)).exp()
}

impl Sampler {
    pub fn new(rate: u32) -> Self {
        Self {
            rate: f64::from(rate.max(1)),
            banks: (0..TRACKS).map(|_| None).collect(),
            slot_of: vec![NONE; TRACKS].into_boxed_slice(),
            slot_track: vec![NONE; SLOTS].into_boxed_slice(),
            chans: vec![Chan::fresh(0, None); SLOTS * 16].into_boxed_slice(),
            voices: vec![Voice::default(); MAX_VOICES].into_boxed_slice(),
            count: 0,
            bus: vec![[0.0; 2]; TRACKS].into_boxed_slice(),
            stamps: vec![0; TRACKS].into_boxed_slice(),
            touched: vec![0; MAX_VOICES].into_boxed_slice(),
            touched_count: 0,
            sample_epoch: 0,
            epoch: 1,
            serial: 0,
            clock: 0,
            rng: 0x9e37_79b9_7f4a_7c15,
        }
    }
    /// Plan Banks are cloned into fixed slots: Arc refcount changes only; the
    /// control-side cache holds the last owner.
    pub fn configure(&mut self, plan: Option<&MidiPlan>) {
        self.reset();
        self.slot_of.fill(NONE);
        self.slot_track.fill(NONE);
        let mut next = 0;
        for (i, bank) in self.banks.iter_mut().enumerate() {
            let b = plan.and_then(|p| p.samplers.get(i)).and_then(Clone::clone);
            if b.is_some() && next < SLOTS {
                self.slot_of[i] = next as u16;
                self.slot_track[next] = i as u16;
                next += 1;
                *bank = b;
            } else if bank.is_some() {
                *bank = None;
            }
        }
    }
    pub fn reset(&mut self) {
        self.count = 0;
        self.epoch = self.epoch.wrapping_add(1);
    }
    pub fn active(&self) -> bool {
        self.count != 0
    }
    pub fn voice_count(&self) -> usize {
        self.count
    }
    pub fn owns(&self, track: u16) -> bool {
        self.slot_of.get(track as usize).is_some_and(|s| *s != NONE)
    }
    fn random(&mut self) -> f64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 11) as f64 / (1u64 << 53) as f64
    }
    fn chan(&mut self, slot: u16, channel: u8) -> usize {
        let k = slot as usize * 16 + (channel & 15) as usize;
        if self.chans[k].epoch != self.epoch {
            let track = self.slot_track[slot as usize] as usize;
            self.chans[k] = Chan::fresh(self.epoch, self.banks[track].as_deref());
        }
        k
    }
    fn matches(r: &Region, c: &Chan, bank: &Bank, vel: u8, rand: f64, seq: u32) -> bool {
        vel >= r.lovel
            && vel <= r.hivel
            && rand >= r.lorand
            && (rand < r.hirand || r.hirand >= 1.0)
            && (r.seq_length <= 1 || (seq.wrapping_sub(1) % r.seq_length) + 1 == r.seq_position)
            && r.cc.iter().all(|&(n, lo, hi)| (lo..=hi).contains(&c.cc[n as usize]))
            && r.sw_last.is_none_or(|sw| c.keyswitch.or(bank.sw_default) == Some(sw))
    }
    fn bend_factor(bend: i16, r: &Region) -> f64 {
        let cents = if bend >= 0 {
            f64::from(bend) / 8191.0 * r.bend_up
        } else {
            f64::from(-bend) / 8192.0 * r.bend_down
        };
        pow2(cents)
    }
    #[allow(clippy::too_many_arguments)]
    fn start(&mut self, track: u16, slot: u16, chan: usize, region: usize, key: u8, vel: u8, id: u32, free: bool, extra: f64) {
        let Some(bank) = self.banks[track as usize].clone() else { return };
        let r = &bank.regions[region];
        let sample = &bank.samples[r.sample];
        // off_by: this group's start chokes voices waiting for it.
        for v in &mut self.voices[..self.count] {
            if v.track == track && v.stage != Stage::Release && bank.regions[v.region as usize].off_by == Some(r.group) {
                let fast = bank.regions[v.region as usize].off_fast;
                let seconds = if fast { FAST } else { bank.regions[v.region as usize].release.max(MIN_RELEASE) };
                v.stage = Stage::Release;
                v.release = decay_coef(seconds, self.rate);
            }
        }
        let curve = (f64::from(vel) / 127.0).powi(2);
        let (track_amount, v) = if r.veltrack < 0.0 { (-r.veltrack, 1.0 - curve) } else { (r.veltrack, curve) };
        let velocity_gain = 1.0 - track_amount + track_amount * v;
        let g = r.gain * velocity_gain * extra;
        let cents = (f64::from(key) - r.keycenter) * r.keytrack + r.tune;
        let c = self.chans[chan];
        let stealing = self.count == self.voices.len();
        let index = if stealing {
            self.voices
                .iter()
                .enumerate()
                .min_by_key(|(_, v)| (v.stage != Stage::Release, v.age))
                .map_or(0, |(i, _)| i)
        } else {
            self.count += 1;
            self.count - 1
        };
        let anchor = if stealing { self.voices[index].last } else { [0.0; 2] };
        self.serial = self.serial.wrapping_add(1);
        let rate = self.rate;
        let mut voice = Voice {
            id,
            track,
            slot,
            channel: (chan % 16) as u8,
            key,
            region: region as u32,
            pos: r.offset.min(r.end) as f64,
            step: pow2(cents) * sample.rate / rate,
            bend: Self::bend_factor(c.bend, r),
            gain: [g * r.pan[0], g * r.pan[1]],
            stage: Stage::Delay,
            level: 0.0,
            left: r.delay * rate,
            attack: if r.attack > 0.0 { 1.0 / (r.attack * rate) } else { 1.0 },
            decay: decay_coef(r.decay, rate),
            sustain: r.sustain,
            release: decay_coef(r.release.max(MIN_RELEASE), rate),
            down: true,
            free: free || r.loop_mode == LoopMode::OneShot,
            age: self.serial,
            steal: anchor,
            steal_left: if anchor == [0.0; 2] { 0.0 } else { STEAL * rate },
            last: [0.0; 2],
        };
        if voice.left <= 0.0 {
            voice.stage = Stage::Attack;
        }
        self.voices[index] = voice;
    }
    fn release_voice(v: &mut Voice) {
        if v.stage != Stage::Release {
            v.stage = Stage::Release;
        }
    }
    fn trigger_release(&mut self, track: u16, slot: u16, chan: usize, key: u8) {
        let Some(bank) = self.banks[track as usize].clone() else { return };
        let c = self.chans[chan];
        let vel = c.velocity[key as usize];
        let held = (self.clock.saturating_sub(c.onset[key as usize])) as f64 / self.rate;
        let rand = self.random();
        for (i, r) in bank.regions.iter().enumerate() {
            if r.trigger == Trigger::Release
                && r.plays_key(key)
                && Self::matches(r, &c, &bank, vel, rand, c.seq[key as usize])
            {
                let extra = 10f64.powf(-r.rt_decay * held / 20.0);
                if extra > SILENT {
                    self.start(track, slot, chan, i, key, vel, 0, true, extra);
                }
            }
        }
    }
    fn note_off(&mut self, track: u16, slot: u16, chan: usize, key: u8, id: u32) {
        let k = key as usize;
        if !self.chans[chan].held[k] {
            return;
        }
        self.chans[chan].held[k] = false;
        self.chans[chan].keys_down = self.chans[chan].keys_down.saturating_sub(1);
        let channel = (chan % 16) as u8;
        for v in &mut self.voices[..self.count] {
            if v.track == track && v.channel == channel && v.down && (v.id == id || (id == 0 && v.key == key)) {
                v.down = false;
            }
        }
        if self.chans[chan].pedal {
            self.chans[chan].sustained[k] = true;
            return;
        }
        for v in &mut self.voices[..self.count] {
            if v.track == track && v.channel == channel && !v.down && !v.free && v.key == key {
                Self::release_voice(v);
            }
        }
        self.trigger_release(track, slot, chan, key);
    }
    /// True when the event belongs to an SFZ Track (handled here).
    pub fn event(&mut self, e: MidiEvent) -> bool {
        let Some(&slot) = self.slot_of.get(e.track as usize) else { return false };
        if slot == NONE {
            return false;
        }
        if e.channel >= 16 || e.pitch > 127 {
            return true;
        }
        let chan = self.chan(slot, e.channel);
        let Some(bank) = self.banks[e.track as usize].clone() else { return true };
        let key = e.pitch;
        if !e.on || e.velocity == 0 {
            self.note_off(e.track, slot, chan, key, e.voice);
            return true;
        }
        if bank.sw.is_some_and(|(lo, hi)| (lo..=hi).contains(&key)) {
            self.chans[chan].keyswitch = Some(key);
        }
        let k = key as usize;
        let first = self.chans[chan].keys_down == 0;
        {
            let c = &mut self.chans[chan];
            if !c.held[k] {
                c.keys_down += 1;
            }
            c.held[k] = true;
            c.sustained[k] = false;
            c.velocity[k] = e.velocity;
            c.onset[k] = self.clock;
            c.seq[k] = c.seq[k].wrapping_add(1);
        }
        let c = self.chans[chan];
        let rand = self.random();
        for (i, r) in bank.regions.iter().enumerate() {
            let ok = match r.trigger {
                Trigger::Release => false,
                Trigger::First => first,
                Trigger::Legato => !first,
                Trigger::Attack => true,
            };
            if ok && r.plays_key(key) && Self::matches(r, &c, &bank, e.velocity, rand, c.seq[k]) {
                self.start(e.track, slot, chan, i, key, e.velocity, e.voice, false, 1.0);
            }
        }
        true
    }
    pub fn control(&mut self, e: MidiControlEvent) -> bool {
        let Some(&slot) = self.slot_of.get(e.track as usize) else { return false };
        if slot == NONE {
            return false;
        }
        if e.channel >= 16 {
            return true;
        }
        let chan = self.chan(slot, e.channel);
        let channel = e.channel;
        match e.control {
            Control::PitchBend(value) => {
                self.chans[chan].bend = value.clamp(-8192, 8191);
                if let Some(bank) = self.banks[e.track as usize].clone() {
                    for v in &mut self.voices[..self.count] {
                        if v.track == e.track && v.channel == channel {
                            v.bend = Self::bend_factor(value, &bank.regions[v.region as usize]);
                        }
                    }
                }
            }
            Control::Cc(n, value) => {
                let previous = self.chans[chan].cc[n as usize];
                self.chans[chan].cc[n as usize] = value;
                match n {
                    7 => self.chans[chan].volume = f64::from(value) / 127.0,
                    11 => self.chans[chan].expression = f64::from(value) / 127.0,
                    10 => {
                        self.chans[chan].pan = if value <= 64 {
                            [1.0, f64::from(value) / 64.0]
                        } else {
                            [f64::from(127 - value) / 63.0, 1.0]
                        }
                    }
                    64 => {
                        let down = value >= 64;
                        let was = self.chans[chan].pedal;
                        self.chans[chan].pedal = down;
                        if was && !down {
                            for key in 0..128u8 {
                                if std::mem::take(&mut self.chans[chan].sustained[key as usize]) {
                                    for v in &mut self.voices[..self.count] {
                                        if v.track == e.track && v.channel == channel && !v.down && !v.free && v.key == key {
                                            Self::release_voice(v);
                                        }
                                    }
                                    self.trigger_release(e.track, slot, chan, key);
                                }
                            }
                        }
                    }
                    120 | 123 => {
                        for v in &mut self.voices[..self.count] {
                            if v.track == e.track && v.channel == channel {
                                v.down = false;
                                if n == 120 {
                                    v.stage = Stage::Release;
                                    v.release = decay_coef(FAST, self.rate);
                                } else if !v.free {
                                    Self::release_voice(v);
                                }
                            }
                        }
                        let c = &mut self.chans[chan];
                        c.held = [false; 128];
                        c.sustained = [false; 128];
                        c.keys_down = 0;
                    }
                    121 => {
                        let bank = self.banks[e.track as usize].clone();
                        let c = &mut self.chans[chan];
                        let keep = (c.held, c.sustained, c.velocity, c.onset, c.seq, c.keys_down);
                        *c = Chan::fresh(self.epoch, bank.as_deref());
                        (c.held, c.sustained, c.velocity, c.onset, c.seq, c.keys_down) = keep;
                    }
                    _ => {}
                }
                // CC-triggered regions (e.g. pedal noise) fire on entering their range.
                if let Some(bank) = self.banks[e.track as usize].clone() {
                    let c = self.chans[chan];
                    let rand = self.random();
                    for (i, r) in bank.regions.iter().enumerate() {
                        if let Some((cc, lo, hi)) = r.on_cc {
                            let range = lo..=hi;
                            if cc == n
                                && range.contains(&value)
                                && !range.contains(&previous)
                                && Self::matches(r, &c, &bank, 127, rand, 1)
                            {
                                let key = r.keycenter.clamp(0.0, 127.0) as u8;
                                self.start(e.track, slot, chan, i, key, 127, 0, true, 1.0);
                            }
                        }
                    }
                }
            }
        }
        true
    }
    /// Advance every voice one output sample into per-Track buses.
    pub fn render(&mut self) {
        self.sample_epoch = self.sample_epoch.wrapping_add(1);
        self.clock = self.clock.wrapping_add(1);
        self.touched_count = 0;
        let mut i = 0;
        while i < self.count {
            let v = &mut self.voices[i];
            let Some(bank) = self.banks[v.track as usize].as_deref() else {
                self.count -= 1;
                self.voices[i] = self.voices[self.count];
                continue;
            };
            let r = &bank.regions[v.region as usize];
            let s = &bank.samples[r.sample];
            match v.stage {
                Stage::Delay => {
                    v.left -= 1.0;
                    if v.left <= 0.0 {
                        v.stage = Stage::Attack;
                    }
                }
                Stage::Attack => {
                    v.level = (v.level + v.attack).min(1.0);
                    if v.level >= 1.0 {
                        v.stage = Stage::Hold;
                        v.left = r.hold * self.rate;
                    }
                }
                Stage::Hold => {
                    v.left -= 1.0;
                    if v.left <= 0.0 {
                        v.stage = Stage::Decay;
                    }
                }
                Stage::Decay => {
                    v.level = v.sustain + (v.level - v.sustain) * v.decay;
                    if (v.level - v.sustain).abs() < SILENT {
                        v.level = v.sustain;
                        v.stage = Stage::Sustain;
                    }
                }
                Stage::Sustain => {}
                Stage::Release => v.level *= v.release,
            }
            let mut out = [0.0; 2];
            if v.stage != Stage::Delay {
                let n = v.pos as usize;
                let f = v.pos - n as f64;
                let y = [s.frame(n.wrapping_sub(1)), s.frame(n), s.frame(n + 1), s.frame(n + 2)];
                let ch = self.chans[v.slot as usize * 16 + v.channel as usize];
                let amp = v.level * ch.volume * ch.expression;
                for (c, o) in out.iter_mut().enumerate() {
                    // 4-point Hermite interpolation.
                    let (y0, y1, y2, y3) = (f64::from(y[0][c]), f64::from(y[1][c]), f64::from(y[2][c]), f64::from(y[3][c]));
                    let c1 = 0.5 * (y2 - y0);
                    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
                    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
                    *o = (((c3 * f + c2) * f + c1) * f + y1) * amp * v.gain[c] * ch.pan[c];
                }
                v.pos += v.step * v.bend;
                let looping = match r.loop_mode {
                    LoopMode::Continuous => true,
                    LoopMode::Sustain => v.stage != Stage::Release,
                    _ => false,
                };
                if looping && v.pos > r.loop_end as f64 {
                    v.pos -= (r.loop_end - r.loop_start + 1) as f64;
                }
            }
            if v.steal_left > 0.0 {
                let t = v.steal_left / (STEAL * self.rate);
                for (c, o) in out.iter_mut().enumerate() {
                    *o += v.steal[c] * t;
                }
                v.steal_left -= 1.0;
            }
            v.last = out;
            let done = v.pos > r.end as f64 || (v.stage == Stage::Release && v.level < SILENT);
            let track = v.track as usize;
            if self.stamps[track] != self.sample_epoch {
                self.stamps[track] = self.sample_epoch;
                self.bus[track] = [0.0; 2];
                if self.touched_count < self.touched.len() {
                    self.touched[self.touched_count] = v.track;
                    self.touched_count += 1;
                }
            }
            self.bus[track][0] += out[0];
            self.bus[track][1] += out[1];
            if done && v.steal_left <= 0.0 {
                self.count -= 1;
                self.voices[i] = self.voices[self.count];
            } else {
                i += 1;
            }
        }
    }
    pub fn touched(&self) -> &[u16] {
        &self.touched[..self.touched_count]
    }
    /// This sample's bus, or silence when the Track rendered nothing.
    pub fn output(&self, track: usize) -> [f64; 2] {
        if self.stamps.get(track).copied() == Some(self.sample_epoch) {
            self.bus[track]
        } else {
            [0.0; 2]
        }
    }
}
