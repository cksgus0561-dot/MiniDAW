use serde::Serialize;
use std::sync::atomic::{fence, AtomicU64, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PlayState {
    Stopped,
    Playing,
    Paused,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Transport {
    pub state: PlayState,
    pub frame: f64,
    pub clip_id: u64,
    pub applied_command: u64,
    pub cycle_pass: u64,
}

impl Default for Transport {
    fn default() -> Self {
        Self {
            state: PlayState::Stopped,
            frame: 0.0,
            clip_id: 0,
            applied_command: 0,
            cycle_pass: 0,
        }
    }
}

#[derive(Default)]
pub struct TransportCell {
    sequence: AtomicU64,
    state: AtomicU64,
    frame: AtomicU64,
    clip_id: AtomicU64,
    applied_command: AtomicU64,
    cycle_pass: AtomicU64,
}

impl TransportCell {
    // Exactly one writer: audio callback. No lock or retry loop on the writer.
    pub fn publish(&self, value: Transport) {
        self.sequence.fetch_add(1, Ordering::AcqRel);
        self.state.store(value.state as u64, Ordering::Relaxed);
        self.frame.store(value.frame.to_bits(), Ordering::Relaxed);
        self.cycle_pass.store(value.cycle_pass, Ordering::Relaxed);
        self.clip_id.store(value.clip_id, Ordering::Relaxed);
        self.applied_command
            .store(value.applied_command, Ordering::Relaxed);
        self.sequence.fetch_add(1, Ordering::Release);
    }

    // Non-real-time readers may retry; never hold up the callback.
    pub fn read(&self) -> Transport {
        loop {
            let before = self.sequence.load(Ordering::Acquire);
            if before & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }
            let value = Transport {
                state: match self.state.load(Ordering::Relaxed) {
                    1 => PlayState::Playing,
                    2 => PlayState::Paused,
                    _ => PlayState::Stopped,
                },
                frame: f64::from_bits(self.frame.load(Ordering::Relaxed)),
                clip_id: self.clip_id.load(Ordering::Relaxed),
                applied_command: self.applied_command.load(Ordering::Relaxed),
                cycle_pass: self.cycle_pass.load(Ordering::Relaxed),
            };
            fence(Ordering::Acquire);
            if before == self.sequence.load(Ordering::Relaxed) {
                return value;
            }
        }
    }
}
