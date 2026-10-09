//! Immutable, device-rate playback plan compiled off the callback. A worker sums
//! active clip voices into the existing bounded stereo ring; no project reaches RT.
use super::{
    reader::Cancel,
    resample::PlaybackReader,
    source::{AssetStorage, AudioAsset},
    streaming::FrameReader,
};
use crate::{
    error::AppResult,
    project::{
        schema::*,
        time::{time, Time},
    },
};
use std::{
    collections::{BTreeSet, HashMap},
    sync::Arc,
    time::Instant,
};
pub const MAX_ACTIVE_VOICES: usize = 128;
#[derive(Clone)]
pub struct Voice {
    pub track: usize,
    pub track_gains: [f64; 2],
    pub asset: Arc<AudioAsset>,
    pub clip: AudioClip,
    pub envelope: AudioClip,
    pub start: usize,
    pub end: usize,
    pub anchor: i128,
    pub position: Time,
}
#[derive(Clone)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub voices: Vec<usize>,
}
pub struct PlaybackPlan {
    pub audio_latency: std::sync::atomic::AtomicUsize,
    pub effect_meters: Vec<Option<Arc<super::effect_runtime::Meters>>>,
    pub midi: super::midi::MidiPlan,
    pub rate: u32,
    pub frames: usize,
    pub voices: Vec<Voice>,
    pub spans: Vec<Span>,
    pub build_ms: f64,
    pub document: Arc<Project>,
    pub assets: HashMap<String, Arc<AudioAsset>>,
}
impl PlaybackPlan {
    pub fn compile(
        document: Arc<Project>,
        assets: HashMap<String, Arc<AudioAsset>>,
        rate: u32,
    ) -> AppResult<Arc<Self>> {
        let began = Instant::now();
        document.validate()?;
        if !(8000..=384000).contains(&rate) {
            return Err(invalid("출력 sample rate 범위"));
        }
        let mut voices: Vec<Voice> = vec![];
        let midi = super::midi::MidiPlan::compile(&document, rate)?;
        let mut frames = midi.end;
        let mut track_ends = vec![0usize; document.tracks.len()];
        if let Some(c) = super::cycle::CycleFrames::compile(&document, rate)? {
            frames = frames.max(c.end);
        }
        let any_solo = document.tracks.iter().any(|t| t.mix.solo);
        if !document.extensions.is_empty()
            || document
                .tracks
                .iter()
                .any(|t| t.extensions.keys().any(|k| k != crate::plugins::INSTRUMENT))
        {
            return Err(invalid("아직 지원하지 않는 프로젝트 확장 데이터입니다."));
        }
        for (track_index, track, c) in document.tracks.iter().enumerate().flat_map(|(i, t)| {
            t.clips
                .iter()
                .filter_map(Clip::as_audio)
                .map(move |c| (i, t, c))
        }) {
            let track_gains = if document
                .automation
                .iter()
                .any(|a| a.track_id == track.track_id && a.read && !a.lanes.is_empty())
            {
                if track.mix.mute || (any_solo && !track.mix.solo) {
                    [0.; 2]
                } else {
                    [1.; 2]
                }
            } else {
                track.mix.gains(any_solo)
            };
            if c.extensions.keys().any(|k| {
                k != ENVELOPE_WINDOW
                    && k != crate::project::tempo_sync::KEY
                    && k != crate::project::glue::KEY
                    && k != crate::project::stretch::KEY
                    && k != crate::project::pitch::KEY
            }) {
                return Err(invalid("아직 지원하지 않는 Clip 확장 데이터입니다."));
            }
            let mut envelope = c.clone();
            if let Some(w) = c.extensions.get(ENVELOPE_WINDOW) {
                let w: EnvelopeWindow = serde_json::from_value(w.clone()).map_err(invalid)?;
                envelope.source_start = w.source_start;
                envelope.source_end = w.source_end;
                envelope.fade_in = w.fade_in;
                envelope.fade_out = w.fade_out;
            }
            let meta = document
                .assets
                .iter()
                .find(|a| a.asset_id == c.asset_id)
                .expect("validated asset");
            let position = time(&c.position, &document.musical_time);
            if position.n < 0 {
                return Err(invalid("음수 Clip 위치는 지원하지 않습니다."));
            }
            let start = usize::try_from(position.ceil_frame(rate))
                .map_err(|_| invalid("timeline 위치 범위"))?;
            let end = usize::try_from(
                crate::project::tempo_sync::end(
                    c,
                    &document.musical_time,
                    meta.metadata.sample_rate,
                )?
                .ceil_frame(rate),
            )
            .map_err(|_| invalid("timeline 길이 범위"))?;
            frames = frames.max(end);
            track_ends[track_index] = track_ends[track_index].max(end);
            if c.mute || track_gains == [0.0; 2] {
                continue;
            }
            let Some(asset) = assets.get(&c.asset_id) else {
                continue;
            }; // Missing contribution is silence.
            let asset = if let Some(r) = crate::project::stretch::recipe(c)? {
                super::stretch::prepare_pitched(
                    asset.clone(),
                    &r,
                    crate::project::pitch::get(c)?.total(),
                    &meta.fingerprint.sampled_sha256,
                )?
            } else {
                asset.clone()
            };
            if c.source_end.0 > asset.info.frames as u64 {
                return Err(invalid("Clip 범위가 재연결한 원본 길이를 초과합니다."));
            }
            let anchor = crate::project::tempo_sync::anchor(c, &document.musical_time)?
                .unwrap_or_else(|| {
                    position.minus(Time::frames(c.source_start.0, asset.info.sample_rate))
                })
                .ceil_frame(rate);
            voices.push(Voice {
                track: track_index,
                track_gains,
                asset: asset.clone(),
                clip: c.clone(),
                envelope,
                start,
                end,
                anchor,
                position,
            });
        }
        for (i, t) in document
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.kind == TrackKind::Audio)
        {
            if track_ends[i] > 0 && t.mix.gains(any_solo) != [0.; 2] {
                frames = frames.max(
                    track_ends[i]
                        + (super::effects::project_tail(&document, &t.track_id) * rate as f64)
                            .ceil() as usize,
                );
            }
        }
        let effect_meters = document
            .tracks
            .iter()
            .map(|t| {
                if t.kind == TrackKind::Audio && !t.inserts.is_empty() {
                    Some(super::effect_runtime::Meters::new(
                        &t.track_id,
                        &t.inserts,
                        true,
                    ))
                } else {
                    None
                }
            })
            .collect();
        voices.sort_by_key(|v| v.start);
        // Coalesce sample-contiguous neutral split segments before creating decoder voices.
        let mut merged: Vec<Voice> = vec![];
        for v in voices {
            if let Some(last) = merged.last_mut() {
                if last.end == v.start
                    && last.track == v.track
                    && last.clip.asset_id == v.clip.asset_id
                    && last.anchor == v.anchor
                    && last.clip.source_end == v.clip.source_start
                    && last.clip.gain == v.clip.gain
                    && last.clip.fade_in.source_frames.0 == 0
                    && last.clip.fade_out.source_frames.0 == 0
                    && v.clip.fade_in.source_frames.0 == 0
                    && v.clip.fade_out.source_frames.0 == 0
                    && last.clip.extensions.is_empty()
                    && v.clip.extensions.is_empty()
                {
                    last.end = v.end;
                    last.clip.source_end = v.clip.source_end;
                    last.envelope.source_end = v.clip.source_end;
                    continue;
                }
            }
            merged.push(v);
        }
        let mut events = vec![];
        for (i, v) in merged.iter().enumerate() {
            if v.start < v.end {
                events.push((v.start, true, i));
                events.push((v.end, false, i));
            }
        }
        events.sort_unstable();
        let mut active = BTreeSet::new();
        let mut spans = vec![];
        let mut previous = 0;
        for (at, add, index) in events {
            if at > previous {
                spans.push(Span {
                    start: previous,
                    end: at,
                    voices: active.iter().copied().collect(),
                });
                previous = at;
            }
            if add {
                active.insert(index);
            } else {
                active.remove(&index);
            }
            if active.len() > MAX_ACTIVE_VOICES {
                return Err(invalid("동시 재생 Clip 한도(128)를 초과했습니다."));
            }
        }
        if previous < frames {
            spans.push(Span {
                start: previous,
                end: frames,
                voices: vec![],
            });
        }
        Ok(Arc::new(Self {
            audio_latency: std::sync::atomic::AtomicUsize::new(0),
            effect_meters,
            midi,
            rate,
            frames,
            voices: merged,
            spans,
            build_ms: began.elapsed().as_secs_f64() * 1000.0,
            document,
            assets,
        }))
    }
    pub fn at_rate(&self, rate: u32) -> AppResult<Arc<Self>> {
        Self::compile(self.document.clone(), self.assets.clone(), rate)
    }
    pub fn resident_bytes(&self) -> usize {
        self.assets
            .values()
            .map(|a| match &a.storage {
                AssetStorage::Memory(d) => d.samples.len() * 4,
                AssetStorage::File(_) | AssetStorage::Rendered(_) => 0,
            })
            .sum()
    }
    pub fn streaming(&self) -> bool {
        self.assets
            .values()
            .chain(self.voices.iter().map(|v| &v.asset))
            .any(|a| matches!(a.storage, AssetStorage::File(_) | AssetStorage::Rendered(_)))
    }
}
pub fn envelope(clip: &AudioClip, source_position: f64) -> f64 {
    fn ramp(x: f64, curve: &FadeCurve) -> f64 {
        let x = x.clamp(0.0, 1.0);
        match curve {
            FadeCurve::Linear => x,
            FadeCurve::Cosine => 0.5 - 0.5 * (std::f64::consts::PI * x).cos(),
        }
    }
    let local = source_position - clip.source_start.0 as f64;
    let length = (clip.source_end.0 - clip.source_start.0) as f64;
    let mut value = clip.gain;
    if clip.fade_in.source_frames.0 > 0 {
        value *= ramp(
            local / (clip.fade_in.source_frames.0.saturating_sub(1).max(1)) as f64,
            &clip.fade_in.curve,
        );
    }
    if clip.fade_out.source_frames.0 > 0 {
        value *= ramp(
            (length - 1.0 - local)
                / (clip.fade_out.source_frames.0.saturating_sub(1).max(1)) as f64,
            &clip.fade_out.curve,
        );
    }
    value
}
struct Active {
    index: usize,
    reader: PlaybackReader,
    next: usize,
}
struct TrackEffect {
    automation: Option<super::automation::Automation>,
    track: usize,
    chain: super::effects::Chain,
    meter: Option<Arc<super::effect_runtime::Meters>>,
}
pub struct TimelineReader {
    input_position: usize,
    output_position: usize,
    latency: usize,
    latencies: Vec<usize>,
    delays: Vec<super::pdc::Line>,
    warm: usize,
    cycle: Option<super::cycle::CycleFrames>,
    effects: Vec<TrackEffect>,
    effect_map: Vec<Option<usize>>,
    buses: Vec<[f64; 2]>,
    pub plan: Arc<PlaybackPlan>,
    position: usize,
    span: usize,
    active: Vec<Active>,
    cancel: Cancel,
    track_filter: Option<usize>,
}
impl TimelineReader {
    pub fn new(plan: Arc<PlaybackPlan>) -> Self {
        let effects: Vec<_> = plan
            .document
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, t)| t.kind == TrackKind::Audio)
            .map(|(track, t)| {
                let automated = plan
                    .document
                    .automation
                    .iter()
                    .any(|a| a.track_id == t.track_id && a.read && !a.lanes.is_empty());
                TrackEffect {
                    automation: automated.then(|| {
                        super::automation::Automation::compile(
                            &plan.document,
                            &t.track_id,
                            plan.rate,
                        )
                    }),
                    track,
                    chain: super::effects::Chain::from_project(
                        &plan.document,
                        &t.track_id,
                        plan.rate,
                    ),
                    meter: plan.effect_meters[track].clone(),
                }
            })
            .collect();
        let mut effect_map = vec![None; plan.document.tracks.len()];
        for (index, e) in effects.iter().enumerate() {
            effect_map[e.track] = Some(index);
        }
        let buses = vec![[0.; 2]; effects.len()];
        let latencies: Vec<_> = effects.iter().map(|e| e.chain.latency()).collect();
        let latency = latencies.iter().copied().max().unwrap_or(0);
        let safe_capacity = if latency
            .saturating_add(1)
            .saturating_mul(effects.len())
            .saturating_mul(16)
            <= 256 * 1024 * 1024
        {
            latency
        } else {
            0
        };
        let delays = (0..effects.len())
            .map(|_| super::pdc::Line::new(safe_capacity))
            .collect();
        plan.audio_latency
            .store(latency, std::sync::atomic::Ordering::Release);
        Self {
            input_position: 0,
            output_position: 0,
            latency,
            latencies,
            delays,
            warm: latency,
            cycle: None,
            effects,
            effect_map,
            buses,
            plan,
            position: 0,
            span: usize::MAX,
            active: Vec::with_capacity(MAX_ACTIVE_VOICES),
            cancel: Arc::new(|| false),
            track_filter: None,
        }
    }
    /// Analysis-only reader, same immutable post-fader plan and SRC arithmetic.
    /// Its cursors and I/O belong to the Spectrum worker, never to playback.
    pub fn for_track(plan: Arc<PlaybackPlan>, track: usize) -> Self {
        let _scope = crate::plugins::OfflineScope::new();
        let mut reader = Self::new(plan);
        reader.track_filter = Some(track);
        for e in &mut reader.effects {
            e.meter = None;
        }
        reader
    }
    fn update(&mut self) -> AppResult<()> {
        let span = self.plan.spans.partition_point(|s| s.end <= self.position);
        if self.span == span {
            return Ok(());
        }
        self.span = span;
        let Some(s) = self.plan.spans.get(span) else {
            self.active.clear();
            return Ok(());
        };
        self.active.retain(|a| s.voices.contains(&a.index));
        for &index in &s.voices {
            if self
                .track_filter
                .is_some_and(|t| self.plan.voices[index].track != t)
            {
                continue;
            }
            if self.active.iter().any(|a| a.index == index) {
                continue;
            }
            let v = &self.plan.voices[index];
            let target = (self.position as i128 - v.anchor).max(0) as usize;
            let mut reader = PlaybackReader::new(v.asset.clone(), self.plan.rate)?;
            reader.seek(target, self.cancel.clone())?;
            self.active.push(Active {
                index,
                reader,
                next: target,
            });
        }
        Ok(())
    }
}
impl FrameReader for TimelineReader {
    fn lookahead(&self) -> usize {
        if self
            .plan
            .document
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Audio)
            .flat_map(|t| &t.inserts)
            .any(|e| {
                matches!(
                    e.processor,
                    crate::project::effects::Processor::External { .. }
                )
            })
        {
            2048
        } else {
            super::streaming::CAPACITY
        }
    }
    fn set_cycle(&mut self, cycle: Option<super::cycle::CycleFrames>) {
        self.cycle = cycle;
    }
    fn wrap(&mut self, _target: usize, _cancel: Cancel) -> AppResult<()> {
        Ok(())
    }
    fn seek(&mut self, target: usize, cancel: Cancel) -> AppResult<()> {
        self.cancel = cancel;
        self.output_position = target;
        self.restart_pdc(target)
    }
    fn read_frame(&mut self) -> AppResult<[f32; 2]> {
        for _ in 0..16 {
            if self
                .latency
                .saturating_add(1)
                .saturating_mul(self.effects.len())
                .saturating_mul(16)
                > 256 * 1024 * 1024
            {
                return Err(invalid("PDC buffer exceeds 256 MiB safety budget"));
            }
            for e in &self.effects {
                e.chain.settle_latency();
            }
            for e in &mut self.effects {
                if !e.chain.pdc_ready() {
                    e.chain.pdc_supply().service();
                    if !e.chain.pdc_ready() {
                        return Err(invalid("PDC buffer capacity exceeded"));
                    }
                }
            }
            if self
                .effects
                .iter()
                .zip(&self.latencies)
                .any(|(e, l)| e.chain.latency() != *l)
            {
                for (e, l) in self.effects.iter().zip(&mut self.latencies) {
                    *l = e.chain.latency();
                }
                self.latency = self.latencies.iter().copied().max().unwrap_or(0);
                if self
                    .latency
                    .saturating_add(1)
                    .saturating_mul(self.effects.len())
                    .saturating_mul(16)
                    > 256 * 1024 * 1024
                {
                    return Err(invalid("PDC buffer exceeds 256 MiB safety budget"));
                }
                if self.track_filter.is_none() {
                    self.plan
                        .audio_latency
                        .store(self.latency, std::sync::atomic::Ordering::Release);
                }
                self.delays = (0..self.effects.len())
                    .map(|_| super::pdc::Line::new(self.latency))
                    .collect();
                self.restart_pdc(self.output_position)?;
            }
            while self.warm > 0 {
                if (self.cancel)() {
                    return Err(crate::error::AppError::new(
                        "cancelled",
                        "PDC preparation cancelled",
                    ));
                }
                self.read_raw()?;
                self.warm -= 1;
                if self.effects.iter().any(|e| e.chain.latency_pending()) {
                    break;
                }
            }
            if self.effects.iter().any(|e| e.chain.latency_pending()) {
                self.restart_pdc(self.output_position)?;
                continue;
            }
            let out = self.read_raw()?;
            if self
                .effects
                .iter()
                .zip(&self.latencies)
                .any(|(e, l)| e.chain.latency_pending() || e.chain.latency() != *l)
            {
                self.restart_pdc(self.output_position)?;
                continue;
            }
            self.output_position += 1;
            return Ok(out);
        }
        Err(invalid("Plugin latency does not settle"))
    }
}
impl TimelineReader {
    pub fn latency(&self) -> usize {
        self.latency
    }
    fn restart_pdc(&mut self, target: usize) -> AppResult<()> {
        self.input_position = target;
        self.position = super::pdc::position(target, 0, self.cycle);
        self.warm = self.latency;
        self.span = usize::MAX;
        self.active.clear();
        for e in &mut self.effects {
            e.chain.reset();
        }
        for l in &mut self.delays {
            l.reset();
        }
        self.update()
    }
    fn read_raw(&mut self) -> AppResult<[f32; 2]> {
        self.position = super::pdc::position(self.input_position, 0, self.cycle);
        self.update()?;
        let mut sum = [0.0f64; 2];
        self.buses.fill([0.; 2]);
        for a in &mut self.active {
            let v = &self.plan.voices[a.index];
            let target = (self.position as i128 - v.anchor).max(0) as usize;
            if a.next != target {
                a.reader.seek(target, self.cancel.clone())?;
            }
            let sample = a.reader.read_frame()?;
            a.next = target + 1;
            let source_position =
                target as f64 * v.asset.info.sample_rate as f64 / self.plan.rate as f64;
            let gain = envelope(&v.envelope, source_position);
            for ch in 0..2 {
                let value = sample[ch] as f64 * gain * v.track_gains[ch];
                if let Some(slot) = self.effect_map[v.track] {
                    self.buses[slot][ch] += value;
                } else {
                    sum[ch] += value;
                }
            }
        }
        for (index, e) in self.effects.iter_mut().enumerate() {
            if self.track_filter.is_some_and(|t| t != e.track) {
                continue;
            }
            let mut input = self.buses[index];
            e.chain.set_time(0, self.cycle);
            if let Some(a) = &mut e.automation {
                let values = a.apply(Some(self.input_position), &mut e.chain);
                let gains = values.gains(false);
                for ch in 0..2 {
                    input[ch] *= gains[ch];
                }
            }
            let signature = &self.plan.document.musical_time.time_signatures[0];
            e.chain.clock(
                self.input_position,
                signature.numerator as i32,
                signature.denominator as i32,
                true,
            );
            let processed = self.delays[index]
                .sample(e.chain.process(input), self.latency - self.latencies[index]);
            for ch in 0..2 {
                sum[ch] += processed[ch];
            }
            if self.position.is_multiple_of(256) {
                if let Some(m) = &e.meter {
                    m.publish(&e.chain.reduction, Some(self.position));
                }
            }
        }
        self.input_position += 1;
        self.position = super::pdc::position(self.input_position, 0, self.cycle);
        Ok(sum.map(|v| v.clamp(-(f32::MAX as f64), f32::MAX as f64) as f32))
    }
}
