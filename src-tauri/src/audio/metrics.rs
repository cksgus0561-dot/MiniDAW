//! The callback only stores integers. Formatting and division happen in snapshot().
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

#[derive(Default)]
pub struct AudioMetrics {
    pub automation_end_cycle: AtomicU64,
    pub automation_end_frame: AtomicU64,
    pub automation_end_rate: AtomicU64,
    pub master: super::master::Master,
    pub effects: super::effect_runtime::Exchange,
    pub callbacks: AtomicU64,
    pub total_ns: AtomicU64,
    pub max_ns: AtomicU64,
    pub buffer_frames: AtomicU64,
    pub min_buffer_frames: AtomicU64,
    pub max_buffer_frames: AtomicU64,
    pub overruns: AtomicU64,
    pub play_ns: AtomicU64,
    pub seek_ns: AtomicU64,
    pub pause_ns: AtomicU64,
    pub resume_ns: AtomicU64,
    pub stop_ns: AtomicU64,
    pub interval_count: AtomicU64,
    pub interval_total_ns: AtomicU64,
    pub interval_max_ns: AtomicU64,
    pub device_callback_count: AtomicU64,
    pub device_callback_total_ns: AtomicU64,
    pub device_callback_max_ns: AtomicU64,
    pub device_callback_overruns: AtomicU64,
    pub rendered_frames: AtomicU64,
    pub non_silent_frames: AtomicU64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetricsSnapshot {
    pub callbacks: u64,
    pub callback_avg_ms: Option<f64>,
    pub callback_max_ms: Option<f64>,
    pub buffer_frames: Option<u64>,
    pub min_buffer_frames: Option<u64>,
    pub max_buffer_frames: Option<u64>,
    pub overruns: u64,
    pub play_ms: Option<f64>,
    pub seek_ms: Option<f64>,
    pub pause_ms: Option<f64>,
    pub resume_ms: Option<f64>,
    pub stop_ms: Option<f64>,
    pub callback_interval_avg_ms: Option<f64>,
    pub callback_interval_max_ms: Option<f64>,
    pub device_callback_avg_ms: Option<f64>,
    pub device_callback_max_ms: Option<f64>,
    pub device_callback_overruns: u64,
    pub rendered_frames: u64,
    pub non_silent_frames: u64,
}

impl AudioMetrics {
    pub fn record_device_callback(&self, ns: u64, frames: usize, rate: u32) {
        self.device_callback_total_ns.fetch_add(ns, Relaxed);
        self.device_callback_max_ns.fetch_max(ns, Relaxed);
        self.device_callback_count.fetch_add(1, Relaxed);
        if ns > frames as u64 * 1_000_000_000 / rate as u64 {
            self.device_callback_overruns.fetch_add(1, Relaxed);
        }
    }
    pub fn record_interval(&self, ns: u64) {
        self.interval_total_ns.fetch_add(ns, Relaxed);
        self.interval_max_ns.fetch_max(ns, Relaxed);
        self.interval_count.fetch_add(1, Relaxed);
    }
    pub fn snapshot(&self) -> MetricsSnapshot {
        let callbacks = self.callbacks.load(Relaxed);
        let measured = callbacks > 0;
        let optional_ms = |ns: &AtomicU64| {
            let value = ns.load(Relaxed);
            (value > 0).then_some(value as f64 / 1_000_000.0)
        };
        MetricsSnapshot {
            callbacks,
            callback_avg_ms: measured
                .then(|| self.total_ns.load(Relaxed) as f64 / callbacks as f64 / 1_000_000.0),
            callback_max_ms: measured.then(|| self.max_ns.load(Relaxed) as f64 / 1_000_000.0),
            buffer_frames: measured.then(|| self.buffer_frames.load(Relaxed)),
            min_buffer_frames: measured.then(|| self.min_buffer_frames.load(Relaxed)),
            max_buffer_frames: measured.then(|| self.max_buffer_frames.load(Relaxed)),
            overruns: self.overruns.load(Relaxed),
            play_ms: optional_ms(&self.play_ns),
            seek_ms: optional_ms(&self.seek_ns),
            pause_ms: optional_ms(&self.pause_ns),
            resume_ms: optional_ms(&self.resume_ns),
            stop_ms: optional_ms(&self.stop_ns),
            callback_interval_avg_ms: (self.interval_count.load(Relaxed) > 0).then(|| {
                self.interval_total_ns.load(Relaxed) as f64
                    / self.interval_count.load(Relaxed).max(1) as f64
                    / 1_000_000.0
            }),
            callback_interval_max_ms: optional_ms(&self.interval_max_ns),
            device_callback_avg_ms: (self.device_callback_count.load(Relaxed) > 0).then(|| {
                self.device_callback_total_ns.load(Relaxed) as f64
                    / self.device_callback_count.load(Relaxed).max(1) as f64
                    / 1_000_000.0
            }),
            device_callback_max_ms: optional_ms(&self.device_callback_max_ns),
            device_callback_overruns: self.device_callback_overruns.load(Relaxed),
            rendered_frames: self.rendered_frames.load(Relaxed),
            non_silent_frames: self.non_silent_frames.load(Relaxed),
        }
    }

    // Only after the old stream has been dropped/joined, before opening a new one.
    pub fn reset(&self) {
        self.master.reset_meter();
        for counter in [
            &self.callbacks,
            &self.total_ns,
            &self.max_ns,
            &self.buffer_frames,
            &self.min_buffer_frames,
            &self.max_buffer_frames,
            &self.overruns,
            &self.play_ns,
            &self.seek_ns,
            &self.pause_ns,
            &self.resume_ns,
            &self.stop_ns,
            &self.interval_count,
            &self.interval_total_ns,
            &self.interval_max_ns,
            &self.device_callback_count,
            &self.device_callback_total_ns,
            &self.device_callback_max_ns,
            &self.device_callback_overruns,
            &self.rendered_frames,
            &self.non_silent_frames,
        ] {
            counter.store(0, Relaxed);
        }
    }
}

#[derive(Default)]
pub struct CallbackTotals {
    count: u64,
    total: u64,
    max: u64,
    min_frames: u64,
    max_frames: u64,
    overruns: u64,
    rendered: u64,
    non_silent: u64,
}

impl CallbackTotals {
    pub fn record(
        &mut self,
        shared: &AudioMetrics,
        elapsed_ns: u64,
        frames: u64,
        rate: u64,
        rendered: u64,
        non_silent: u64,
    ) {
        self.count += 1;
        self.total += elapsed_ns;
        self.max = self.max.max(elapsed_ns);
        if self.min_frames == 0 {
            self.min_frames = frames;
        } else {
            self.min_frames = self.min_frames.min(frames);
        }
        self.max_frames = self.max_frames.max(frames);
        if frames > 0 && elapsed_ns > frames * 1_000_000_000 / rate {
            self.overruns += 1;
        }
        self.rendered += rendered;
        self.non_silent += non_silent;
        shared.total_ns.store(self.total, Relaxed);
        shared.max_ns.store(self.max, Relaxed);
        shared.buffer_frames.store(frames, Relaxed);
        shared.min_buffer_frames.store(self.min_frames, Relaxed);
        shared.max_buffer_frames.store(self.max_frames, Relaxed);
        shared.overruns.store(self.overruns, Relaxed);
        shared.rendered_frames.store(self.rendered, Relaxed);
        shared.non_silent_frames.store(self.non_silent, Relaxed);
        shared.callbacks.store(self.count, Relaxed);
    }
}
