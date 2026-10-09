//! MiniDAW Basic Synth: bounded additive oscillator + smooth amplitude envelope.
//! All tables/storage are built before the device starts. No allocation, locks,
//! I/O, transcendental functions or project access in event()/sample().
use super::midi::{Control, MidiControlEvent, MidiEvent, MidiPlan, MidiSink, MAX_VOICES};
use crate::project::schema::Instrument;

const TRACKS: usize = 4096;
const TABLE: usize = 8192;
const PARTIALS: [f64; 3] = [1.0, 0.25, 0.125];
const LEVEL: f64 = 0.16 / 1.375;
pub const ATTACK_SECONDS: f64 = 0.005;
pub const RELEASE_SECONDS: f64 = 0.040;

#[derive(Clone, Copy)]
struct Channel {
    epoch: u64,
    pedal: bool,
    bend: usize,
    volume: f64,
    expression: f64,
    pan: [f64; 2],
}
impl Default for Channel {
    fn default() -> Self {
        Self {
            epoch: 0,
            pedal: false,
            bend: 8192,
            volume: 1.0,
            expression: 1.0,
            pan: [1.0; 2],
        }
    }
}
#[derive(Clone, Copy, Default)]
struct Voice {
    id: u32,
    key: usize,
    pitch: u8,
    down: bool,
    releasing: bool,
    velocity: f64,
    phase: f64,
    step: f64,
    weights: [f64; 3],
    age: u64,
    progress: f64,
    release_level: f64,
    envelope: f64,
    last: [f64; 2],
    steal_anchor: [f64; 2],
    steal_remaining: f64,
}

#[derive(Clone, Copy, Default)]
pub struct ExternalMidi {
    pub track: usize,
    pub status: u8,
    pub a: u8,
    pub b: u8,
    pub voice: u32,
}
pub struct BasicSynth {
    rate: f64,
    table: Box<[f64]>,
    frequency: [f64; 128],
    bend: Box<[f64]>,
    channels: Box<[Channel]>,
    routes: Box<[bool]>,
    external: Box<[bool]>,
    external_events: Box<[ExternalMidi]>,
    external_count: usize,
    track_gains: Box<[[f64; 2]]>,
    parameters: Box<[[f64; 3]]>,
    buses: Box<[[f64; 2]]>,
    bounds: Box<[f64]>,
    stamps: Box<[u64]>,
    touched: Box<[usize]>,
    sample_epoch: u64,
    voices: Box<[Voice]>,
    count: usize,
    serial: u64,
    epoch: u64,
}
impl BasicSynth {
    pub fn new(rate: u32) -> Self {
        Self {
            rate: f64::from(rate.max(1)),
            table: (0..=TABLE)
                .map(|i| (i as f64 / TABLE as f64 * std::f64::consts::TAU).sin())
                .collect(),
            frequency: std::array::from_fn(|p| 440.0 * 2f64.powf((p as f64 - 69.0) / 12.0)),
            // Standard fixed +/-2 semitone range; both extremes reach exactly two semitones.
            bend: (0..16384)
                .map(|i| {
                    let v = i as f64 - 8192.0;
                    2f64.powf(v / if v < 0.0 { 8192.0 } else { 8191.0 } / 6.0)
                })
                .collect(),
            channels: vec![Channel::default(); TRACKS * 16].into_boxed_slice(),
            routes: vec![false; TRACKS].into_boxed_slice(),
            external: vec![false; TRACKS].into_boxed_slice(),
            external_events: vec![ExternalMidi::default(); 4096].into_boxed_slice(),
            external_count: 0,
            track_gains: vec![[1.0; 2]; TRACKS].into_boxed_slice(),
            parameters: vec![[1., ATTACK_SECONDS, RELEASE_SECONDS]; TRACKS].into_boxed_slice(),
            buses: vec![[0.0; 2]; TRACKS].into_boxed_slice(),
            bounds: vec![0.0; TRACKS].into_boxed_slice(),
            stamps: vec![0; TRACKS].into_boxed_slice(),
            touched: vec![0; MAX_VOICES].into_boxed_slice(),
            sample_epoch: 0,
            voices: vec![Voice::default(); MAX_VOICES].into_boxed_slice(),
            count: 0,
            serial: 0,
            epoch: 1,
        }
    }
    pub fn automate(
        &mut self,
        track: usize,
        gains: [f64; 2],
        level: f64,
        attack: f64,
        release: f64,
    ) {
        if track < TRACKS {
            self.track_gains[track] = gains;
            self.parameters[track] = [10f64.powf(level / 20.), attack / 1000., release / 1000.];
        }
    }
    pub fn configure(&mut self, plan: Option<&MidiPlan>) {
        self.reset();
        self.routes.fill(false);
        self.external.fill(false);
        if let Some(p) = plan {
            self.track_gains[..p.track_gains.len()].copy_from_slice(&p.track_gains);
            for (i, s) in p.synth_settings.iter().enumerate() {
                self.parameters[i] = [
                    10f64.powf(s.level_db / 20.),
                    s.attack_ms / 1000.,
                    s.release_ms / 1000.,
                ];
            }
            for (out, instrument) in self.external.iter_mut().zip(&p.instruments) {
                *out = *instrument == Instrument::External;
            }
            for (out, instrument) in self.routes.iter_mut().zip(&p.instruments) {
                *out = *instrument == Instrument::BasicSynth;
            }
        }
    }
    /// Transport discontinuities use the existing output de-click. Reset channel
    /// state lazily: no full 4096-track traversal on Pause/Stop/Seek.
    pub fn reset(&mut self) {
        self.count = 0;
        self.external_count = 0;
        self.epoch = self.epoch.wrapping_add(1);
    }
    pub fn external_events(&self) -> &[ExternalMidi] {
        &self.external_events[..self.external_count]
    }
    pub fn clear_external(&mut self) {
        self.external_count = 0;
    }
    pub fn reset_epoch(&self) -> u64 {
        self.epoch
    }
    fn external_event(&mut self, e: ExternalMidi) -> bool {
        if !self.external.get(e.track).copied().unwrap_or(false) {
            return false;
        }
        if self.external_count < self.external_events.len() {
            self.external_events[self.external_count] = e;
            self.external_count += 1;
        }
        true
    }
    pub fn active(&self) -> bool {
        self.count != 0 || self.external_count != 0
    }
    pub fn voice_count(&self) -> usize {
        self.count
    }
    /// Actual last rendered instrument bus, after Track gain/balance and before Master.
    /// Call only immediately after sample(); untouched Tracks are silent.
    pub fn track_output(&self, track: usize) -> [f32; 2] {
        if self.stamps.get(track).copied() != Some(self.sample_epoch) {
            return [0.0; 2];
        }
        std::array::from_fn(|ch| {
            (self.buses[track][ch] / self.bounds[track].max(1.0)
                * self.track_gains[track][ch]
                * self.parameters[track][0]) as f32
        })
    }
    fn channel(&mut self, track: u16, channel: u8) -> Option<usize> {
        if channel >= 16 || !self.routes.get(track as usize).copied().unwrap_or(false) {
            return None;
        }
        let key = track as usize * 16 + channel as usize;
        if self.channels[key].epoch != self.epoch {
            self.channels[key] = Channel {
                epoch: self.epoch,
                ..Channel::default()
            };
        }
        Some(key)
    }
    fn tune(voice: &mut Voice, frequency: f64, rate: f64) {
        voice.step = frequency / rate;
        for (i, weight) in voice.weights.iter_mut().enumerate() {
            // Fade partials out BEFORE Nyquist. Never fold an above-Nyquist harmonic.
            let t = ((0.48 - voice.step * (i + 1) as f64) / 0.08).clamp(0.0, 1.0);
            *weight = PARTIALS[i] * smooth(t);
        }
    }
    fn release(v: &mut Voice) {
        if !v.releasing {
            v.releasing = true;
            v.progress = 0.0;
            v.release_level = v.envelope;
        }
    }
    pub fn sample(&mut self) -> [f64; 2] {
        let mut output = [0.0; 2];
        self.sample_epoch = self.sample_epoch.wrapping_add(1);
        let mut touched = 0;
        let mut i = 0;
        while i < self.count {
            let v = &mut self.voices[i];
            let track = v.key / 16;
            if self.stamps[track] != self.sample_epoch {
                self.stamps[track] = self.sample_epoch;
                self.buses[track] = [0.0; 2];
                self.bounds[track] = 0.0;
                self.touched[touched] = track;
                touched += 1;
            }
            if v.releasing {
                v.progress = (v.progress + 1.0 / (self.parameters[track][2] * self.rate)).min(1.0);
                v.envelope = v.release_level * (1.0 - smooth(v.progress));
            } else {
                v.progress = (v.progress + 1.0 / (self.parameters[track][1] * self.rate)).min(1.0);
                v.envelope = smooth(v.progress);
            }
            let mut wave = 0.0;
            for (h, weight) in v.weights.iter().enumerate() {
                if *weight == 0.0 {
                    continue;
                }
                let phase = (v.phase * (h + 1) as f64).fract() * TABLE as f64;
                let n = phase as usize;
                wave += (self.table[n] + (self.table[n + 1] - self.table[n]) * (phase - n as f64))
                    * weight;
            }
            v.phase = (v.phase + v.step).fract();
            let c = self.channels[v.key];
            let amplitude = LEVEL * v.velocity * v.envelope * c.volume * c.expression;
            // Conservative, waveform-independent headroom. No clipping/waveshaper
            // distortion, including 256 phase-aligned voices at full velocity.
            self.bounds[track] += amplitude * v.weights.iter().sum::<f64>();
            v.steal_remaining = (v.steal_remaining - 1.0 / (0.002 * self.rate)).max(0.0);
            for (ch, out) in self.buses[track].iter_mut().enumerate() {
                v.last[ch] =
                    wave * amplitude * c.pan[ch] + v.steal_anchor[ch] * smooth(v.steal_remaining);
                *out += v.last[ch];
            }
            self.bounds[track] +=
                v.steal_anchor[0].abs().max(v.steal_anchor[1].abs()) * smooth(v.steal_remaining);
            if v.releasing && v.progress >= 1.0 && v.steal_remaining == 0.0 {
                self.count -= 1;
                self.voices[i] = self.voices[self.count];
            } else {
                i += 1;
            }
        }
        // Each Track owns its Synth headroom: another Track's notes/Volume never
        // attenuate this instrument. Only touched buses are visited on the callback.
        for &track in &self.touched[..touched] {
            for (ch, out) in output.iter_mut().enumerate() {
                *out += self.buses[track][ch] / self.bounds[track].max(1.0)
                    * self.track_gains[track][ch]
                    * self.parameters[track][0];
            }
        }
        output
    }
}
fn smooth(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}
impl MidiSink for BasicSynth {
    fn event(&mut self, e: MidiEvent) {
        if self.external_event(ExternalMidi {
            track: e.track as usize,
            status: (if e.on { 0x90 } else { 0x80 }) | e.channel,
            a: e.pitch,
            b: e.velocity,
            voice: e.voice,
        }) {
            return;
        }
        let Some(key) = self.channel(e.track, e.channel) else {
            return;
        };
        if !e.on || e.velocity == 0 {
            for v in &mut self.voices[..self.count] {
                if v.id == e.voice && v.key == key && v.down {
                    v.down = false;
                    if !self.channels[key].pedal {
                        Self::release(v);
                    }
                }
            }
            return;
        }
        if e.pitch > 127 {
            return;
        }
        let stealing = self.count == MAX_VOICES;
        let slot = if !stealing {
            let i = self.count;
            self.count += 1;
            i
        } else {
            self.voices
                .iter()
                .enumerate()
                .min_by_key(|(_, v)| (!v.releasing, v.age))
                .unwrap()
                .0
        };
        let anchor = if stealing {
            self.voices[slot].last
        } else {
            [0.0; 2]
        };
        self.serial = self.serial.wrapping_add(1);
        let mut voice = Voice {
            id: e.voice,
            key,
            pitch: e.pitch,
            down: true,
            velocity: (f64::from(e.velocity) / 127.0).powi(2),
            age: self.serial,
            steal_anchor: anchor,
            steal_remaining: if anchor == [0.0; 2] { 0.0 } else { 1.0 },
            ..Voice::default()
        };
        Self::tune(
            &mut voice,
            self.frequency[e.pitch as usize] * self.bend[self.channels[key].bend],
            self.rate,
        );
        self.voices[slot] = voice;
    }
    fn control(&mut self, e: MidiControlEvent) {
        let (status, a, b) = match e.control {
            Control::Cc(a, b) => (0xb0 | e.channel, a, b),
            Control::PitchBend(v) => {
                let v = (v as i32 + 8192) as u16;
                (0xe0 | e.channel, (v & 127) as u8, (v >> 7) as u8)
            }
        };
        if self.external_event(ExternalMidi {
            track: e.track as usize,
            status,
            a,
            b,
            voice: 0,
        }) {
            return;
        }
        let Some(key) = self.channel(e.track, e.channel) else {
            return;
        };
        match e.control {
            Control::PitchBend(value) => {
                self.channels[key].bend = (i32::from(value) + 8192).clamp(0, 16383) as usize
            }
            Control::Cc(64, value) => self.channels[key].pedal = value >= 64,
            Control::Cc(7, value) => self.channels[key].volume = f64::from(value) / 127.0,
            Control::Cc(11, value) => self.channels[key].expression = f64::from(value) / 127.0,
            Control::Cc(10, value) => {
                self.channels[key].pan = if value <= 64 {
                    [1.0, f64::from(value) / 64.0]
                } else {
                    [f64::from(127 - value) / 63.0, 1.0]
                }
            }
            Control::Cc(121, _) => {
                self.channels[key] = Channel {
                    epoch: self.epoch,
                    ..Channel::default()
                }
            }
            _ => {}
        }
        let c = self.channels[key];
        for v in self.voices[..self.count]
            .iter_mut()
            .filter(|v| v.key == key)
        {
            if matches!(e.control, Control::PitchBend(_) | Control::Cc(121, _)) {
                Self::tune(
                    v,
                    self.frequency[v.pitch as usize] * self.bend[c.bend],
                    self.rate,
                );
            }
            if matches!(e.control, Control::Cc(120 | 123, _)) {
                v.down = false;
            }
            if (!v.down && !c.pedal) || matches!(e.control, Control::Cc(120, _)) {
                Self::release(v);
            }
        }
    }
}
