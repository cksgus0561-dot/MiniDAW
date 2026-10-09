//! Independent offline Master instance. Same readers, MIDI scheduler, Synth and
//! Insert/Automation DSP as Renderer, with a monotonically increasing sample clock.
//! No device, callback, wall clock, live MIDI, Cycle or shared playback DSP state.
use super::{
    effect_runtime::{Exchange, Port},
    midi::MidiScheduler,
    reader::Cancel,
    streaming::FrameReader,
    synth::BasicSynth,
    timeline::{PlaybackPlan, TimelineReader},
};
use crate::error::AppResult;
use std::sync::Arc;

pub struct OfflineMaster {
    pub plan: Arc<PlaybackPlan>,
    audio: TimelineReader,
    midi: MidiScheduler,
    synth: BasicSynth,
    effects: Port,
    _exchange: Exchange,
    master_gain: f64,
    processing_position: usize,
    emitted: usize,
    warm: usize,
    cancel: Cancel,
}
impl OfflineMaster {
    pub fn new(plan: Arc<PlaybackPlan>, cancel: Cancel) -> AppResult<Self> {
        let _scope = crate::plugins::OfflineScope::new();
        let mut audio = TimelineReader::new(plan.clone());
        audio.seek(0, cancel.clone())?;
        let exchange = Exchange::default();
        exchange.configure(&plan.document);
        let mut effects = exchange.attach(plan.rate);
        let mut synth = BasicSynth::new(plan.rate);
        synth.configure(Some(&plan.midi));
        effects.route(1, Some(&plan.midi), &mut synth);
        effects.set_cycle(None);
        let warm = effects.latency();
        // Use exactly the persisted realtime fader conversion, already settled.
        let gain = super::master::Master::default();
        gain.set_db(plan.document.master.volume_db);
        crate::plugins::check_offline()?;
        Ok(Self {
            plan,
            audio,
            midi: MidiScheduler::default(),
            synth,
            effects,
            _exchange: exchange,
            master_gain: gain.gain(),
            processing_position: 0,
            emitted: 0,
            warm,
            cancel,
        })
    }
    pub fn position(&self) -> usize {
        self.emitted
    }
    pub fn finished(&self) -> bool {
        self.processing_position >= self.plan.frames.saturating_add(self.effects.latency())
            && !self.synth.active()
            && !self.effects.active()
    }
    pub fn next_frame(&mut self) -> AppResult<Option<[f32; 2]>> {
        loop {
            self.effects.settle_latency();
            let (ready, changed) = self.effects.sync_pdc();
            if !ready {
                super::pdc::service_all();
                if !self.effects.sync_pdc().0 {
                    return Err(crate::error::AppError::new(
                        "pdc",
                        "PDC buffer capacity exceeded",
                    ));
                }
            }
            if changed {
                self.processing_position = self.emitted;
                self.audio
                    .seek(self.processing_position, self.cancel.clone())?;
                self.effects.reset();
                self.midi
                    .reset(self.processing_position, 0, &mut self.synth);
                self.synth.reset();
                self.warm = self.effects.latency();
            }
            let output = self.render_frame()?;
            if self.effects.latency_dirty() {
                self.effects.settle_latency();
                self.processing_position = self.emitted;
                self.audio
                    .seek(self.processing_position, self.cancel.clone())?;
                self.effects.reset();
                self.midi
                    .reset(self.processing_position, 0, &mut self.synth);
                self.synth.reset();
                self.effects.sync_pdc();
                self.warm = self.effects.latency();
                continue;
            }
            if self.warm > 0 {
                self.warm -= 1;
                if output.is_some() {
                    continue;
                }
            }
            if output.is_some() {
                self.emitted += 1;
            }
            return Ok(output);
        }
    }
    fn render_frame(&mut self) -> AppResult<Option<[f32; 2]>> {
        if self.finished() {
            return Ok(None);
        }
        let playing = self.processing_position < self.plan.frames;
        // Mirror Renderer's EOF and Automation Read behavior, including tails.
        let gain = self
            .effects
            .automate(
                (self.processing_position < self.plan.frames + self.effects.latency())
                    .then_some(self.processing_position),
                &mut self.synth,
            )
            .unwrap_or(self.master_gain);
        let mut input = if playing {
            let audio = self.audio.read_frame()?.map(f64::from);
            if !self.plan.midi.is_empty() {
                self.midi.sample(
                    &self.plan.midi,
                    self.processing_position,
                    0,
                    &mut self.synth,
                );
            }
            Some(audio)
        } else {
            None
        };
        let delayed = self.effects.audio(input.unwrap_or([0.; 2]));
        if input.is_some()
            || self.effects.active()
            || self.processing_position < self.plan.frames + self.effects.latency()
        {
            input = Some(delayed);
        }
        let sampled = self.synth.active();
        let tails = self.effects.active();
        let dry = if sampled {
            self.synth.sample()
        } else {
            [0.; 2]
        };
        let wet = self.effects.tracks(dry, &mut self.synth, sampled);
        if sampled || tails {
            let mix = input.get_or_insert([0.; 2]);
            for ch in 0..2 {
                mix[ch] += wet[ch];
            }
        }
        let mut output = input.unwrap_or([0.; 2]).map(|x| x * gain);
        if input.is_some() && self.effects.has_master() {
            output = self.effects.master(output, gain == 0.);
        }
        // Only an explicitly configured final Limiter imposes a ceiling.
        if let Some(ceiling) = self.effects.ceiling() {
            output = output.map(|x| x.clamp(-ceiling, ceiling));
        }
        self.processing_position += 1;
        if self.processing_position == self.plan.frames && !self.plan.midi.is_empty() {
            self.midi.sample(
                &self.plan.midi,
                self.processing_position,
                0,
                &mut self.synth,
            );
            self.midi
                .reset(self.processing_position, 0, &mut self.synth);
        }
        // Device clipping and transport de-click are monitoring transitions, not
        // project DSP. Float files preserve headroom; PCM conversion saturates.
        Ok(Some(output.map(|x| x as f32)))
    }
}
