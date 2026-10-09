//! Bounded output comparison through the actual prepared playback rings.
use minidaw_lib::audio::{
    resample::PlaybackReader,
    source::{AudioAsset, AudioSource},
};
use serde_json::json;
use std::{
    fs,
    path::Path,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};
#[derive(Default)]
struct Stats {
    count: u64,
    mismatch: u64,
    max: f64,
    sum: f64,
    first: Option<usize>,
}
impl Stats {
    fn add(&mut self, a: [f32; 2], b: [f32; 2], i: usize) {
        for ch in 0..2 {
            assert!(a[ch].is_finite());
            self.count += 1;
            if a[ch].to_bits() != b[ch].to_bits() {
                self.mismatch += 1;
                if self.first.is_none() {
                    self.first = Some(i * 2 + ch);
                }
            }
            let e = (a[ch] as f64 - b[ch] as f64).abs();
            self.max = self.max.max(e);
            self.sum += e * e;
        }
    }
    fn json(&self) -> serde_json::Value {
        json!({"samples":self.count,"mismatch":self.mismatch,"maxAbs":self.max,"rms":(self.sum/self.count.max(1) as f64).sqrt(),"firstMismatch":self.first})
    }
}
fn frame(src: &AudioSource, i: usize) -> [f32; 2] {
    let start = Instant::now();
    loop {
        if let Some(p) = src.pair(i, i) {
            return p[0];
        }
        assert!(
            !src.failed(),
            "{}",
            serde_json::to_string(&src.snapshot()).unwrap()
        );
        assert!(start.elapsed() < Duration::from_secs(15));
        thread::sleep(Duration::from_micros(100));
    }
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let dest = &args[1];
    let mut reports = Vec::new();
    for path in &args[2..] {
        let memory = AudioAsset::open_with_budget(Path::new(path), usize::MAX).unwrap();
        let file = AudioAsset::open_with_budget(Path::new(path), 0).unwrap();
        let rate = if memory.info.sample_rate == 44100 {
            48000
        } else {
            44100
        };
        let mem = AudioSource::for_output(memory.clone(), rate).unwrap();
        let disk = AudioSource::for_output(file, rate).unwrap();
        let total = mem.playback_frames;
        let mut reference = PlaybackReader::new(memory, rate).unwrap();
        reference.seek(0, Arc::new(|| false)).unwrap();
        let positions = vec![
            0,
            1,
            13,
            total / 2,
            total * 9 / 10,
            total / 10,
            total / 4,
            total * 3 / 4,
            total.saturating_sub(4096),
            total - 1,
        ];
        let mut windows: Vec<Vec<[f32; 2]>> = positions.iter().map(|_| Vec::new()).collect();
        let (mut ms, mut ds) = (Stats::default(), Stats::default());
        let started = Instant::now();
        for i in 0..total {
            let r = reference.read_frame().unwrap();
            ms.add(frame(&mem, i), r, i);
            ds.add(frame(&disk, i), r, i);
            for (j, &p) in positions.iter().enumerate() {
                if i >= p && i < p + 4096 {
                    windows[j].push(r);
                }
            }
            if i % 512 == 511 {
                mem.consumed(i + 1);
                disk.consumed(i + 1);
            }
        }
        let mut seeks = Vec::new();
        for (mode, src) in [("Memory", &mem), ("Streaming", &disk)] {
            for (&pos, window) in positions.iter().zip(&windows) {
                let t = Instant::now();
                src.seek(pos);
                let mut stat = Stats::default();
                for (offset, &expected) in window.iter().enumerate() {
                    let i = pos + offset;
                    stat.add(frame(src, i), expected, i);
                    if offset % 512 == 511 {
                        src.consumed(i + 1);
                    }
                }
                assert_eq!(stat.mismatch, 0);
                seeks.push(json!({"mode":mode,"outputFrame":pos,"ms":t.elapsed().as_secs_f64()*1000.,"stats":stat.json()}));
            }
            for i in 0..1000 {
                src.seek((i * 7919) % total);
            }
            src.seek(positions[3]);
            let mut stat = Stats::default();
            for (offset, &expected) in windows[3].iter().enumerate() {
                let i = positions[3] + offset;
                stat.add(frame(src, i), expected, i);
                if offset % 512 == 511 {
                    src.consumed(i + 1);
                }
            }
            assert_eq!(stat.mismatch, 0);
            seeks.push(json!({"mode":mode,"rapidRequests":1000,"stats":stat.json()}));
        }
        reports.push(json!({"source":path,"inputRate":mem.asset.info.sample_rate,"outputRate":rate,"sourceFrames":mem.asset.info.frames,"outputFrames":total,"memory":ms.json(),"streaming":ds.json(),"seeks":seeks,"memorySnapshot":mem.snapshot(),"streamSnapshot":disk.snapshot(),"wallSeconds":started.elapsed().as_secs_f64()}));
        assert_eq!(ms.mismatch, 0);
        assert_eq!(ds.mismatch, 0);
        println!("{path}: {total} output frames exact; reset/seeks exact");
    }
    fs::write(dest, serde_json::to_vec_pretty(&reports).unwrap()).unwrap();
}
