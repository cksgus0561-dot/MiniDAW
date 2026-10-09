//! Final stereo fader and sample-peak meter. No routing, limiter or automation.
use serde::Serialize;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};

pub struct Master {
    gain: AtomicU32,
    peaks: AtomicU64,
}
impl Default for Master {
    fn default() -> Self {
        Self {
            gain: AtomicU32::new(1f32.to_bits()),
            peaks: AtomicU64::new(0),
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MasterSnapshot {
    pub peak_db: [f32; 2],
}
impl Master {
    // Control thread only, after document validation. -96 dB is the fader's -inf stop.
    pub fn set_db(&self, db: f64) {
        let gain = if db <= -96.0 {
            0.0
        } else {
            10f32.powf(db as f32 / 20.0)
        };
        self.gain.store(gain.to_bits(), Relaxed);
    }
    pub fn gain(&self) -> f64 {
        f32::from_bits(self.gain.load(Relaxed)) as f64
    }
    pub fn reset_meter(&self) {
        self.peaks.store(0, Relaxed);
    }
    // One writer, once per callback. Fast attack and 24 dB/second fall retain short
    // peaks across the existing UI polling interval. One packed atomic publishes L/R.
    pub fn publish(&self, peaks: [f32; 2], frames: usize, rate: f64) {
        let old = self.peaks.load(Relaxed);
        let release = 10f32.powf((-24.0 * frames as f64 / rate / 20.0) as f32);
        let left = peaks[0].max(f32::from_bits(old as u32) * release);
        let right = peaks[1].max(f32::from_bits((old >> 32) as u32) * release);
        self.peaks.store(
            left.to_bits() as u64 | ((right.to_bits() as u64) << 32),
            Relaxed,
        );
    }
    pub fn snapshot(&self) -> MasterSnapshot {
        let pair = self.peaks.load(Relaxed);
        MasterSnapshot {
            peak_db: [pair as u32, (pair >> 32) as u32]
                .map(|bits| (20.0 * f32::from_bits(bits).max(1e-6).log10()).max(-120.0)),
        }
    }
}

// A fixed 5 ms gain ramp avoids zipper discontinuities; independent of Transport
// De-click and of source/scheduler state. Unity is bit-identical to the old path.
pub struct MasterGain {
    current: f64,
    target: f64,
    step: f64,
    left: u32,
    ramp: u32,
}
impl MasterGain {
    pub fn new(rate: u32) -> Self {
        Self {
            current: 1.0,
            target: 1.0,
            step: 0.0,
            left: 0,
            ramp: (rate / 200).max(1),
        }
    }
    pub fn target(&mut self, gain: f64) {
        if gain == self.target {
            return;
        }
        self.target = gain;
        self.left = self.ramp;
        self.step = (gain - self.current) / self.ramp as f64;
    }
    pub fn next_gain(&mut self) -> f64 {
        if self.left > 0 {
            self.left -= 1;
            self.current = if self.left == 0 {
                self.target
            } else {
                self.current + self.step
            };
        }
        self.current
    }
}
