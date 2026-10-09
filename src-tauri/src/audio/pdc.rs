//! Sample delays with control-side allocation and deferred reclamation.
//! A latency reconfiguration invalidates history; no stale samples cross a seek.
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::{
    atomic::{
        AtomicUsize,
        Ordering::{Acquire, Release},
    },
    Arc, Mutex, OnceLock, Weak,
};

pub fn position(frame: usize, delay: usize, cycle: Option<super::cycle::CycleFrames>) -> usize {
    let frame = frame.saturating_sub(delay);
    cycle.map_or(frame, |c| c.position(frame))
}

pub struct Line {
    data: Box<[[f64; 2]]>,
    at: usize,
    valid: usize,
    remaining: usize,
}
impl Line {
    pub fn new(capacity: usize) -> Self {
        Self {
            data: vec![[0.; 2]; capacity + 1].into_boxed_slice(),
            at: 0,
            valid: 0,
            remaining: 0,
        }
    }
    pub fn capacity(&self) -> usize {
        self.data.len() - 1
    }
    pub fn reset(&mut self) {
        self.at = 0;
        self.valid = 0;
        self.remaining = 0;
    }
    pub fn sample(&mut self, input: [f64; 2], delay: usize) -> [f64; 2] {
        self.remaining = if input[0] != 0. || input[1] != 0. {
            delay
        } else {
            self.remaining.saturating_sub(1)
        };
        debug_assert!(delay <= self.capacity());
        let output = if delay == 0 {
            input
        } else if self.valid >= delay {
            self.data[(self.at + self.data.len() - delay) % self.data.len()]
        } else {
            [0.; 2]
        };
        self.data[self.at] = input;
        self.at = (self.at + 1) % self.data.len();
        self.valid = (self.valid + 1).min(self.data.len());
        output
    }
}
pub struct Bank {
    pub lines: Vec<Line>,
    pub capacity: usize,
}
impl Bank {
    fn new(channels: usize, capacity: usize) -> Self {
        Self {
            lines: (0..channels).map(|_| Line::new(capacity)).collect(),
            capacity,
        }
    }
    fn reset(&mut self) {
        for l in &mut self.lines {
            l.reset();
        }
    }
}
struct Control {
    send: Producer<Box<Bank>>,
    retired: Consumer<Box<Bank>>,
    capacity: usize,
    channels: usize,
}
fn registry() -> &'static Mutex<Vec<Weak<Supply>>> {
    static R: OnceLock<Mutex<Vec<Weak<Supply>>>> = OnceLock::new();
    R.get_or_init(Default::default)
}
pub fn capacity_error() -> Option<String> {
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter_map(Weak::upgrade)
        .find(|s| s.exceeded.load(Acquire) > 0)
        .map(|_| {
            "PDC buffer exceeds the 256 MiB per-bank safety budget; playback is suspended.".into()
        })
}
pub fn service_all() {
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|w| {
            if let Some(s) = w.upgrade() {
                s.service();
                true
            } else {
                false
            }
        });
}
pub struct Supply {
    control: Mutex<Control>,
    requested: AtomicUsize,
    pub exceeded: AtomicUsize,
    pub track_latency: AtomicUsize,
    pub master_latency: AtomicUsize,
}
impl Supply {
    /// Device controller / offline worker only. A malformed plugin cannot force
    /// unbounded memory growth. Oversized graphs stay muted with a visible error.
    pub fn service(&self) {
        let mut c = self.control.lock().unwrap_or_else(|e| e.into_inner());
        while c.retired.pop().is_ok() {}
        let needed = self.requested.load(Acquire);
        if needed <= c.capacity || c.send.is_full() {
            return;
        }
        let capacity = needed.saturating_add(4096);
        if capacity
            .saturating_add(1)
            .saturating_mul(c.channels)
            .saturating_mul(16)
            > 256 * 1024 * 1024
        {
            self.exceeded.store(needed, Release);
            return;
        }
        let bank = Box::new(Bank::new(c.channels, capacity));
        if c.send.push(bank).is_ok() {
            c.capacity = capacity;
            self.exceeded.store(0, Release);
        }
    }
}
pub struct Port {
    bank: Box<Bank>,
    incoming: Consumer<Box<Bank>>,
    retired: Producer<Box<Bank>>,
    pub supply: Arc<Supply>,
}
impl Port {
    pub fn new(channels: usize) -> Self {
        let (send, incoming) = RingBuffer::new(2);
        let (retired, receive) = RingBuffer::new(4);
        let supply = Arc::new(Supply {
            control: Mutex::new(Control {
                send,
                retired: receive,
                capacity: 0,
                channels,
            }),
            requested: AtomicUsize::new(0),
            exceeded: AtomicUsize::new(0),
            track_latency: AtomicUsize::new(0),
            master_latency: AtomicUsize::new(0),
        });
        registry()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Arc::downgrade(&supply));
        Self {
            bank: Box::new(Bank::new(channels, 0)),
            incoming,
            retired,
            supply,
        }
    }
    pub fn prepare(&mut self, needed: usize) {
        self.supply.requested.store(needed, Release);
        self.supply.service();
        self.update();
    }
    pub fn update(&mut self) -> bool {
        if !self.retired.is_full() {
            if let Ok(next) = self.incoming.pop() {
                let old = std::mem::replace(&mut self.bank, next);
                assert!(self.retired.push(old).is_ok());
                return true;
            }
        }
        false
    }
    pub fn ready(&self, needed: usize) -> bool {
        if needed <= self.bank.capacity {
            true
        } else {
            self.supply.requested.fetch_max(needed, Release);
            false
        }
    }
    pub fn reset(&mut self) {
        self.bank.reset();
    }
    pub fn active(&self) -> bool {
        self.bank.lines.iter().any(|l| l.remaining > 0)
    }
    pub fn gate(&mut self, channel: usize, open: bool, delay: usize) -> f64 {
        let value = self.sample(channel, [if open { 1. } else { 0. }; 2], delay)[0];
        self.bank.lines[channel].remaining = 0;
        value
    }
    pub fn sample(&mut self, channel: usize, x: [f64; 2], delay: usize) -> [f64; 2] {
        if !self.ready(delay) {
            return [0.; 2];
        }
        self.bank.lines[channel].sample(x, delay)
    }
}
