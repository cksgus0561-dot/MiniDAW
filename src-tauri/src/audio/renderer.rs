//! The only audio data callback work: bounded commands, sample reads, conversion,
//! and atomic publication. Audio owners are retained by the non-RT controller.
use super::declick::{Declick, RAMP_MS};
use super::metrics::{AudioMetrics, CallbackTotals};
use super::{
    source::AudioSource,
    transport::{PlayState, Transport, TransportCell},
};
use rtrb::Consumer;
use std::sync::atomic::Ordering::Relaxed;
use std::{sync::Arc, time::Instant};

// The observer is diagnostic only. Production and tests use the same Synth and
// sample-clock delivery; no MIDI queue or UI timer sits between event and audio.
struct SynthSink<'a, S> {
    synth: &'a mut super::synth::BasicSynth,
    observer: &'a mut S,
}
impl<S: super::midi::MidiSink> super::midi::MidiSink for SynthSink<'_, S> {
    fn event(&mut self, e: super::midi::MidiEvent) {
        self.synth.event(e);
        self.observer.event(e);
    }
    fn control(&mut self, e: super::midi::MidiControlEvent) {
        self.synth.control(e);
        self.observer.control(e);
    }
}

pub const COMMAND_CAPACITY: usize = 32;

pub enum Action {
    Unload,
    Load {
        clip_id: u64,
        audio: Arc<AudioSource>,
    },
    AutomationReplace {
        clip_id: u64,
        audio: Arc<AudioSource>,
    },
    Replace {
        clip_id: u64,
        audio: Arc<AudioSource>,
    },
    Play,
    Pause,
    Toggle,
    Stop,
    Seek(f64),
    Declick(bool),
    MidiInput(Arc<super::midi_input::LiveMidi>),
    ComputerMidi(Arc<super::midi_input::LiveMidi>),
}

pub struct Command {
    pub id: u64,
    pub issued: Instant,
    pub action: Action,
}

pub struct Renderer {
    pdc_delay: usize,
    pdc_origin: usize,
    pdc_elapsed: usize,
    pdc_eof: bool,
    pdc_waiting: bool,
    commands: Consumer<Command>,
    pub transport: Transport,
    audio: Option<Arc<AudioSource>>,
    output_rate: f64,
    output_channels: usize,
    shared: Arc<TransportCell>,
    metrics: Arc<AudioMetrics>,
    totals: CallbackTotals,
    master: super::master::MasterGain,
    effects: super::effect_runtime::Port,
    pending_play: Option<Instant>,
    pending_seek: Option<Instant>,
    output_frame: usize,
    declick: Declick,
    pending_resume: bool,
    previous_callback: Option<Instant>,
    spectrum: Option<super::spectrum::SpectrumTap>,
    midi: super::midi::MidiScheduler,
    midi_input: Option<Arc<super::midi_input::LiveMidi>>,
    live: super::midi_input::LiveReader,
    computer_input: Option<Arc<super::midi_input::LiveMidi>>,
    computer_live: super::midi_input::LiveReader,
    synth: Option<Box<super::synth::BasicSynth>>,
}

impl Renderer {
    pub fn new(
        commands: Consumer<Command>,
        shared: Arc<TransportCell>,
        metrics: Arc<AudioMetrics>,
        rate: u32,
        channels: usize,
    ) -> Self {
        let effects = metrics.effects.attach(rate);
        Self {
            pdc_delay: 0,
            pdc_origin: 0,
            pdc_elapsed: 0,
            pdc_eof: false,
            pdc_waiting: false,
            effects,
            commands,
            transport: Transport::default(),
            audio: None,
            output_rate: rate as f64,
            output_channels: channels,
            shared,
            metrics,
            totals: CallbackTotals::default(),
            master: super::master::MasterGain::new(rate),
            pending_play: None,
            pending_seek: None,
            output_frame: 0,
            declick: Declick::new(rate, RAMP_MS),
            pending_resume: false,
            previous_callback: None,
            spectrum: None,
            midi: super::midi::MidiScheduler::default(),
            midi_input: None,
            live: super::midi_input::LiveReader::default(),
            computer_input: None,
            computer_live: super::midi_input::LiveReader::computer(),
            synth: Some(Box::new(super::synth::BasicSynth::new(rate))),
        }
    }

    pub fn set_declick(&mut self, enabled: bool) {
        self.declick.set_enabled(enabled);
    }
    pub fn set_spectrum(&mut self, tap: super::spectrum::SpectrumTap) {
        self.spectrum = Some(tap);
    }
    pub fn processing_frame(&self) -> usize {
        self.output_frame
    }
    pub fn pdc_latency(&self) -> usize {
        self.pdc_delay
    }
    pub fn metrics(&self) -> Arc<AudioMetrics> {
        self.metrics.clone()
    }
    pub fn render_device<T: cpal::Sample + cpal::FromSample<f32>>(&mut self, output: &mut [T]) {
        let started = Instant::now();
        self.render(output);
        self.metrics.record_device_callback(
            started.elapsed().as_nanos() as u64,
            output.len() / self.output_channels,
            self.output_rate as u32,
        );
    }

    fn play(&mut self, issued: Instant) {
        if let Some(audio) = &self.audio {
            if self.transport.state != PlayState::Playing {
                self.pending_resume = self.transport.state == PlayState::Paused;
                self.declick.transition();
                self.pending_play = Some(issued);
            }
            if self.transport.frame >= audio.asset.info.frames as f64 {
                self.transport.frame = 0.0;
                self.output_frame = 0;
                self.pdc_origin = 0;
                self.pdc_elapsed = 0;
                self.pdc_eof = false;
                audio.seek(0);
            }
            self.transport.state = PlayState::Playing;
        }
    }

    fn pause(&mut self, issued: Instant) {
        self.pending_play = None;
        self.pending_seek = None;
        if self.transport.state == PlayState::Playing {
            self.declick.transition();
            self.transport.state = PlayState::Paused;
        }
        self.metrics
            .pause_ns
            .store((issued.elapsed().as_nanos() as u64).max(1), Relaxed);
    }

    fn audible_index(&self) -> usize {
        self.pdc_origin + self.pdc_elapsed.saturating_sub(self.pdc_delay)
    }
    fn pdc_rewind(&mut self, sink: &mut SynthSink<'_, impl super::midi::MidiSink>) {
        let target = self.audible_index();
        self.output_frame = target;
        self.pdc_origin = target;
        self.pdc_elapsed = 0;
        self.pdc_eof = false;
        self.pdc_delay = self.effects.latency();
        self.effects.reset();
        if let Some(a) = &self.audio {
            a.seek(target);
            self.midi.reset(a.timeline_frame(target), 0, sink);
        }
        sink.synth.reset();
        self.declick.transition();
    }
    fn apply(&mut self, command: Command, sink: &mut SynthSink<'_, impl super::midi::MidiSink>) {
        let reset_pdc = (self.pdc_delay > 0
            && matches!(&command.action, Action::AutomationReplace { .. }))
            || matches!(
                &command.action,
                Action::Load { .. }
                    | Action::Replace { .. }
                    | Action::Unload
                    | Action::Stop
                    | Action::Seek(_)
                    | Action::Pause
            )
            || matches!(&command.action,Action::Toggle if self.transport.state==PlayState::Playing);
        if self.pdc_delay > 0
            && (matches!(&command.action, Action::Pause)
                || matches!(&command.action,Action::Toggle if self.transport.state==PlayState::Playing))
        {
            self.output_frame = self.audible_index();
            if let Some(a) = &self.audio {
                a.seek(self.output_frame);
            }
        }
        if matches!(
            &command.action,
            Action::Pause | Action::Stop | Action::Seek(_) | Action::Toggle
        ) {
            self.metrics
                .automation_end_cycle
                .store(self.transport.cycle_pass, Relaxed);
            self.metrics.automation_end_frame.store(
                self.audio
                    .as_ref()
                    .map_or(0, |a| a.timeline_frame(self.output_frame)) as u64,
                Relaxed,
            );
            self.metrics
                .automation_end_rate
                .store(self.output_rate as u64, Relaxed);
        }
        if matches!(
            &command.action,
            Action::Load { .. }
                | Action::Replace { .. }
                | Action::Unload
                | Action::Pause
                | Action::Stop
        ) || matches!(&command.action, Action::Seek(s) if s.is_finite())
            || matches!(&command.action, Action::Toggle if self.transport.state == PlayState::Playing)
        {
            self.effects.reset();
            self.live.reset(
                self.audio
                    .as_ref()
                    .map_or(0, |a| a.timeline_frame(self.output_frame)),
                sink,
            );
            self.computer_live.reset(self.audio.as_ref().map_or(0, |a| a.timeline_frame(self.output_frame)), sink);
            self.midi.reset(
                self.audio
                    .as_ref()
                    .map_or(0, |a| a.timeline_frame(self.output_frame)),
                0,
                sink,
            );
            sink.synth.reset();
        }
        match command.action {
            Action::AutomationReplace { clip_id, audio } => {
                let seconds = self.audio.as_ref().map_or(0., |a| {
                    self.transport.frame / a.asset.info.sample_rate as f64
                });
                self.output_frame = (seconds * self.output_rate)
                    .round()
                    .min(audio.playback_frames as f64) as usize;
                self.transport.frame = self.output_frame as f64
                    * audio.asset.info.sample_rate as f64
                    / self.output_rate;
                audio.seek_if_needed(self.output_frame);
                self.audio = Some(audio);
                self.transport.clip_id = clip_id;
                self.declick.transition();
            }
            Action::Replace { clip_id, audio } => {
                sink.synth
                    .configure(audio.timeline.as_ref().map(|p| &p.midi));
                let seconds = self.audio.as_ref().map_or(0.0, |a| {
                    self.transport.frame / a.asset.info.sample_rate as f64
                });
                self.output_frame = (seconds * self.output_rate)
                    .round()
                    .min(audio.playback_frames as f64) as usize;
                self.transport.frame = self.output_frame as f64
                    * audio.asset.info.sample_rate as f64
                    / self.output_rate;
                audio.seek_if_needed(self.output_frame);
                self.audio = Some(audio);
                self.transport.clip_id = clip_id;
                self.declick.transition();
                self.pending_play = None;
                self.pending_seek = None;
            }
            Action::Unload => {
                sink.synth.configure(None);
                self.declick.transition();
                // The control thread retains the previous owner until this Arc is gone.
                self.audio = None;
                self.pending_play = None;
                self.pending_seek = None;
                self.transport.clip_id = 0;
                self.transport.frame = 0.0;
                self.transport.state = PlayState::Stopped;
                self.output_frame = 0;
            }
            Action::Load { clip_id, audio } => {
                sink.synth
                    .configure(audio.timeline.as_ref().map(|p| &p.midi));
                self.declick.transition();
                audio.reset();
                self.pending_play = None;
                self.pending_seek = None;
                self.metrics.play_ns.store(0, Relaxed);
                self.metrics.seek_ns.store(0, Relaxed);
                // A retained Arc on the controller guarantees this decrement
                // never frees a PCM allocation in the callback.
                self.audio = Some(audio);
                self.transport.clip_id = clip_id;
                self.transport.frame = 0.0;
                self.output_frame = 0;
                self.transport.state = PlayState::Stopped;
            }
            Action::Play => self.play(command.issued),
            Action::Pause => self.pause(command.issued),
            // Resolve at application time, in the existing bounded command queue.
            // Rapid clicks never depend on a delayed frontend snapshot.
            Action::Toggle => {
                if self.transport.state == PlayState::Playing {
                    self.pause(command.issued);
                } else {
                    self.play(command.issued);
                }
            }
            Action::Stop => {
                self.declick.transition();
                self.metrics
                    .stop_ns
                    .store((command.issued.elapsed().as_nanos() as u64).max(1), Relaxed);
                self.pending_play = None;
                self.pending_seek = None;
                self.transport.state = PlayState::Stopped;
                self.transport.frame = 0.0;
                self.output_frame = 0;
                if let Some(audio) = &self.audio {
                    audio.seek(0);
                }
            }
            Action::Seek(seconds) => {
                if let Some(audio) = &self.audio {
                    if seconds.is_finite() {
                        self.declick.transition();
                        self.pending_seek =
                            (self.transport.state == PlayState::Playing).then_some(command.issued);
                        self.metrics.seek_ns.store(0, Relaxed);
                        self.transport.frame = (seconds * audio.asset.info.sample_rate as f64)
                            .clamp(0.0, audio.asset.info.frames as f64);
                        self.output_frame = (seconds.max(0.0) * self.output_rate)
                            .ceil()
                            .min(audio.playback_frames as f64)
                            as usize;
                        audio.seek(self.output_frame);
                        if self.transport.frame >= audio.asset.info.frames as f64 {
                            self.transport.state = PlayState::Stopped;
                        }
                    }
                }
            }
            Action::MidiInput(hub) => {
                self.live.reset(0, sink);
                self.midi_input = Some(hub);
            }
            Action::ComputerMidi(hub) => {
                self.computer_live.reset(0, sink);
                self.computer_input = Some(hub);
            }
            Action::Declick(enabled) => self.set_declick(enabled),
        }
        if reset_pdc {
            self.pdc_origin = self.output_frame;
            self.pdc_elapsed = 0;
            self.pdc_eof = false;
            if self.pdc_delay > 0 {
                self.effects.reset();
                sink.synth.reset();
                if let Some(a) = &self.audio {
                    a.seek(self.output_frame);
                    self.midi
                        .reset(a.timeline_frame(self.output_frame), 0, sink);
                }
            }
        }
        self.transport.applied_command = command.id;
    }

    pub fn render<T: cpal::Sample + cpal::FromSample<f32>>(&mut self, output: &mut [T]) {
        self.render_with_midi(output, &mut super::midi::NoInstrument);
    }
    pub fn render_with_midi<T: cpal::Sample + cpal::FromSample<f32>>(
        &mut self,
        output: &mut [T],
        sink: &mut impl super::midi::MidiSink,
    ) {
        // Move only the retained Box, never allocate/free callback storage.
        let mut synth = self.synth.take().expect("non-reentrant renderer");
        self.render_inner(
            output,
            &mut SynthSink {
                synth: &mut synth,
                observer: sink,
            },
        );
        self.synth = Some(synth);
    }
    fn render_inner<T: cpal::Sample + cpal::FromSample<f32>>(
        &mut self,
        output: &mut [T],
        sink: &mut SynthSink<'_, impl super::midi::MidiSink>,
    ) {
        let started = Instant::now();
        if let Some(previous) = self.previous_callback.replace(started) {
            self.metrics
                .record_interval(started.duration_since(previous).as_nanos() as u64);
        }
        let mut rendered = 0;
        let mut non_silent = 0;
        // A producer flood can never make this an unbounded command loop.
        for _ in 0..COMMAND_CAPACITY {
            let Ok(command) = self.commands.pop() else {
                break;
            };
            self.apply(command, sink);
        }
        if let Some(hub) = &self.midi_input {
            self.live.process(
                hub,
                self.audio
                    .as_ref()
                    .and_then(|a| a.timeline.as_ref())
                    .map(|p| &p.midi),
                self.audio
                    .as_ref()
                    .map_or(0, |a| a.timeline_frame(self.output_frame)),
                output.len() / self.output_channels,
                sink,
            );
        }
        if let Some(hub) = &self.computer_input {
            self.computer_live.process(
                hub,
                self.audio.as_ref().and_then(|a| a.timeline.as_ref()).map(|p| &p.midi),
                self.audio.as_ref().map_or(0, |a| a.timeline_frame(self.output_frame)),
                output.len() / self.output_channels,
                sink,
            );
        }
        self.effects.update();
        self.effects.route(
            self.transport.clip_id,
            self.audio
                .as_ref()
                .and_then(|a| a.timeline.as_ref())
                .map(|p| &p.midi),
            sink.synth,
        );
        self.effects
            .set_cycle(self.audio.as_ref().and_then(|a| a.cycle));
        let (pdc_ready, pdc_changed) = self.effects.sync_pdc();
        if pdc_changed
            || self.pdc_delay != self.effects.latency()
            || (!pdc_ready && !self.pdc_waiting)
        {
            self.pdc_rewind(sink);
        }
        self.pdc_waiting = !pdc_ready;
        output.fill(T::EQUILIBRIUM);
        if !pdc_ready {
            self.shared.publish(self.transport);
            return;
        }

        let capture = self
            .spectrum
            .as_mut()
            .is_some_and(|tap| tap.begin_source(self.transport.clip_id));
        let master = self.spectrum.as_ref().is_none_or(|tap| tap.is_master());
        let tracks = if capture && !master {
            self.spectrum
                .as_ref()
                .unwrap()
                .tracks(self.transport.clip_id)
        } else {
            [None; 2]
        };
        self.master.target(self.metrics.master.gain());
        let mut peaks = [0.0f32; 2];
        let mut captured = 0;
        let mut unavailable = false;
        for (offset, frame) in output.chunks_exact_mut(self.output_channels).enumerate() {
            let automation_frame = if self.transport.state == PlayState::Playing {
                self.audio.as_ref().map(|_| {
                    if self.pdc_eof {
                        self.pdc_origin + self.pdc_elapsed
                    } else {
                        self.output_frame
                    }
                })
            } else {
                None
            };
            let automated_master_gain = self.effects.automate(automation_frame, sink.synth);
            let previous_audible = self.audible_index();
            let mut consumed = false;
            let mut input = None;
            let mut capture_position = None;
            let mut track_samples = [[0.0; 2]; 2];
            let mut eof = None;
            if self.transport.state == PlayState::Playing && !unavailable {
                if let Some(audio) = &self.audio {
                    if audio.playback_rate as f64 != self.output_rate {
                        self.transport.state = PlayState::Paused;
                    } else if self.output_frame < audio.playback_frames || audio.cycle.is_some() {
                        if let Some(pair) = audio.pair(self.output_frame, self.output_frame) {
                            if capture && !master {
                                capture_position =
                                    (self.pdc_elapsed >= self.effects.track_latency()).then(|| {
                                        audio.timeline_frame(
                                            self.output_frame
                                                .saturating_sub(self.effects.track_latency()),
                                        )
                                    });
                            }
                            if let Some(plan) =
                                audio.timeline.as_ref().filter(|p| !p.midi.is_empty())
                            {
                                self.midi.sample(
                                    &plan.midi,
                                    audio.timeline_frame(self.output_frame),
                                    offset,
                                    sink,
                                );
                            }
                            let [left, right] = pair[0].map(f64::from);
                            input = Some([left, right]);
                            consumed = true;
                            self.output_frame += 1;
                        } else {
                            if audio.failed() {
                                self.transport.state = PlayState::Paused;
                            } else {
                                audio.missing();
                            }
                            unavailable = true; // One starvation/priming count per callback.
                        }
                    }
                    if audio.cycle.is_none()
                        && self.output_frame >= audio.playback_frames
                        && !self.pdc_eof
                    {
                        self.transport.frame = audio.asset.info.frames as f64;
                        self.pdc_eof = true;
                        if self.pdc_delay == 0 {
                            self.transport.state = PlayState::Stopped;
                        }
                        eof = Some(audio.timeline_frame(self.output_frame));
                        self.metrics
                            .automation_end_cycle
                            .store(self.transport.cycle_pass, Relaxed);
                        self.metrics
                            .automation_end_frame
                            .store(audio.timeline_frame(self.output_frame) as u64, Relaxed);
                        self.metrics
                            .automation_end_rate
                            .store(self.output_rate as u64, Relaxed);
                    }
                    if self.transport.state == PlayState::Paused {
                        self.midi
                            .reset(audio.timeline_frame(self.output_frame), offset, sink);
                        sink.synth.reset();
                    }
                }
            }
            if unavailable && self.pdc_delay > 0 {
                break;
            }
            let audio_delayed = self.effects.audio(input.unwrap_or([0.; 2]));
            if input.is_some() || self.effects.active() {
                input = Some(audio_delayed);
            }
            let sampled = sink.synth.active();
            let tails = self.effects.active();
            let dry_synth = if sampled {
                sink.synth.sample()
            } else {
                [0.0; 2]
            };
            let synth = self.effects.tracks(dry_synth, sink.synth, sampled);
            if sampled || tails {
                if capture && !master {
                    for (slot, track) in tracks.iter().enumerate() {
                        if let Some(index) = track {
                            track_samples[slot] = self.effects.track_output(
                                *index,
                                if sampled {
                                    sink.synth.track_output(*index)
                                } else {
                                    [0.0; 2]
                                },
                            );
                        }
                    }
                }
                let mix = input.get_or_insert([0.0; 2]);
                for ch in 0..2 {
                    mix[ch] += synth[ch];
                }
            }
            let master_gain = automated_master_gain.unwrap_or_else(|| self.master.next_gain());
            if let Some(mix) = &mut input {
                for value in mix.iter_mut() {
                    *value *= master_gain;
                }
                if self.effects.has_master() {
                    *mix = self.effects.master(*mix, master_gain == 0.0);
                }
                non_silent += u64::from(mix[0].abs().max(mix[1].abs()) > 0.000001);
                *mix = if self.output_channels == 1 {
                    [((mix[0] + mix[1]) * 0.5).clamp(-1.0, 1.0); 2]
                } else {
                    mix.map(|v| v.clamp(-1.0, 1.0))
                };
            }
            if self.effects.latency_dirty() {
                self.pdc_rewind(sink);
                break;
            }
            if consumed || self.pdc_eof {
                self.pdc_elapsed += 1;
                if self.pdc_elapsed > self.pdc_delay {
                    rendered += 1;
                }
                if let Some(audio) = &self.audio {
                    let audible = self.audible_index();
                    if audio.cycle.is_some_and(|c| {
                        audible > previous_audible
                            && audible >= c.end
                            && c.position(audible) == c.start
                    }) {
                        self.transport.cycle_pass = self.transport.cycle_pass.wrapping_add(1);
                    }
                    self.transport.frame =
                        audio.timeline_frame(audible.min(if audio.cycle.is_some() {
                            usize::MAX
                        } else {
                            audio.playback_frames
                        })) as f64
                            * audio.asset.info.sample_rate as f64
                            / self.output_rate;
                    if self.pdc_eof && audible >= audio.playback_frames {
                        self.transport.state = PlayState::Stopped;
                        self.pdc_eof = false;
                    }
                }
            }
            let priming = consumed && self.pdc_elapsed <= self.pdc_delay;
            if self.pdc_delay > 0 && self.pdc_elapsed == self.pdc_delay + 1 {
                self.declick.transition();
            }
            let mut pair = self
                .declick
                .process(if priming { Some([0.; 2]) } else { input });
            if let Some(ceiling) = self.effects.ceiling() {
                pair = pair.map(|v| v.clamp(-ceiling, ceiling));
            }
            for ch in 0..2 {
                peaks[ch] = peaks[ch].max(pair[ch].abs() as f32);
            }
            // A trailing Note Off belongs AFTER the final rendered sample.
            if let Some(at) = eof {
                if let Some(plan) = self
                    .audio
                    .as_ref()
                    .and_then(|a| a.timeline.as_ref())
                    .filter(|p| !p.midi.is_empty())
                {
                    self.midi.sample(&plan.midi, at, offset + 1, sink);
                }
                self.midi.reset(at, offset + 1, sink);
            }
            if capture {
                if master {
                    self.spectrum.as_mut().unwrap().push(pair.map(|v| v as f32));
                } else {
                    self.spectrum
                        .as_mut()
                        .unwrap()
                        .push_tracks(track_samples, capture_position);
                }
                captured += 1;
            }
            frame[0] = T::from_sample(pair[0] as f32);
            if self.output_channels > 1 {
                frame[1] = T::from_sample(pair[1] as f32);
            }
            if input.is_none() && !self.declick.active() && !self.effects.active() {
                break;
            }
        }
        if capture {
            for _ in captured..output.len() / self.output_channels {
                self.spectrum.as_mut().unwrap().push([0.0; 2]);
            }
        }
        if let Some(audio) = &self.audio {
            audio.consumed(self.output_frame);
        }
        // The first buffer containing source frames has now been processed.
        // This is deliberately NOT a claim about when the DAC becomes audible.
        if rendered > 0 {
            if let Some(issued) = self.pending_play.take() {
                if self.pending_resume {
                    self.metrics
                        .resume_ns
                        .store((issued.elapsed().as_nanos() as u64).max(1), Relaxed);
                }
                self.metrics
                    .play_ns
                    .store((issued.elapsed().as_nanos() as u64).max(1), Relaxed);
            }
            if let Some(issued) = self.pending_seek.take() {
                self.metrics
                    .seek_ns
                    .store((issued.elapsed().as_nanos() as u64).max(1), Relaxed);
            }
        }
        if self.transport.state != PlayState::Playing {
            self.pending_play = None;
            self.pending_seek = None;
        }
        self.effects.publish();
        self.metrics
            .master
            .publish(peaks, output.len() / self.output_channels, self.output_rate);
        self.shared.publish(self.transport);
        self.totals.record(
            &self.metrics,
            started.elapsed().as_nanos() as u64,
            (output.len() / self.output_channels) as u64,
            self.output_rate as u64,
            rendered,
            non_silent,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::super::decoder::{AudioData, FileInfo};
    use super::*;
    use rtrb::RingBuffer;

    pub fn fixture() -> Arc<AudioSource> {
        AudioSource::memory(AudioData {
            info: FileInfo {
                name: "ramp".into(),
                sample_rate: 4,
                channels: 2,
                frames: 4,
                duration: 1.0,
                sanitized_samples: 0,
            },
            samples: vec![0.1, -0.1, 0.2, -0.2, 0.3, -0.3, 0.4, -0.4],
        })
    }

    #[test]
    fn playback_pause_resume_stop_seek_and_end() {
        let (mut tx, rx) = RingBuffer::new(COMMAND_CAPACITY);
        let shared = Arc::new(TransportCell::default());
        let mut renderer =
            Renderer::new(rx, shared.clone(), Arc::new(AudioMetrics::default()), 4, 2);
        let owner = fixture();
        let mut send = |id, action| {
            tx.push(Command {
                id,
                issued: Instant::now(),
                action,
            })
            .ok()
            .unwrap()
        };
        send(
            1,
            Action::Load {
                clip_id: 9,
                audio: owner.clone(),
            },
        );
        send(2, Action::Play);
        let mut out = [0.0_f32; 2];
        renderer.render(&mut out);
        assert_eq!(out, [0.1, -0.1]);
        send(3, Action::Pause);
        renderer.render(&mut out);
        assert_eq!(out, [0.0, 0.0]);
        assert_eq!(shared.read().frame, 1.0);
        send(4, Action::Play);
        renderer.render(&mut out);
        assert_eq!(out, [0.2, -0.2]);
        send(5, Action::Seek(0.75));
        renderer.render(&mut out);
        assert_eq!(out, [0.4, -0.4]);
        assert_eq!(shared.read().state, PlayState::Stopped);
        assert_eq!(shared.read().frame, 4.0);
        send(6, Action::Play);
        renderer.render(&mut out);
        assert_eq!(out, [0.1, -0.1]);
        send(7, Action::Stop);
        renderer.render(&mut out);
        assert_eq!(shared.read().frame, 0.0);
        assert_eq!(shared.read().applied_command, 7);
        assert_eq!(out, [0.0, 0.0]);
    }

    #[test]
    fn toggle_resolves_current_engine_state_in_queue_order() {
        let (mut tx, rx) = RingBuffer::new(COMMAND_CAPACITY);
        let shared = Arc::new(TransportCell::default());
        let mut renderer =
            Renderer::new(rx, shared.clone(), Arc::new(AudioMetrics::default()), 4, 2);
        let owner = fixture();
        let mut id = 0;
        let mut send = |action| {
            id += 1;
            tx.push(Command {
                id,
                issued: Instant::now(),
                action,
            })
            .ok()
            .unwrap();
        };
        send(Action::Load {
            clip_id: 1,
            audio: owner.clone(),
        });
        send(Action::Toggle);
        let mut out = [0.0_f32; 2];
        renderer.render(&mut out);
        assert_eq!(out, [0.1, -0.1]);
        // Two rapid clicks in one buffer must pause then resume, not Play twice.
        send(Action::Toggle);
        send(Action::Toggle);
        renderer.render(&mut out);
        assert_eq!(out, [0.2, -0.2]);
        send(Action::Toggle);
        renderer.render(&mut out);
        assert_eq!(shared.read().state, PlayState::Paused);
        assert_eq!(shared.read().frame, 2.0);
        send(Action::Toggle);
        renderer.render(&mut out);
        assert_eq!(out, [0.3, -0.3]);
        send(Action::Stop);
        send(Action::Toggle);
        renderer.render(&mut out);
        assert_eq!(out, [0.1, -0.1]);
        send(Action::Seek(1.0));
        send(Action::Toggle);
        renderer.render(&mut out);
        assert_eq!(out, [0.1, -0.1]); // EOF restarts from zero.
        send(Action::Toggle);
        send(Action::Stop);
        renderer.render(&mut out);
        assert_eq!(shared.read().state, PlayState::Stopped);
        assert_eq!(shared.read().frame, 0.0);
        assert_eq!(out, [0.0, 0.0]);
    }

    #[test]
    fn prepared_src_has_correct_speed_and_invalid_seek_is_ignored() {
        let (mut tx, rx) = RingBuffer::new(COMMAND_CAPACITY);
        let shared = Arc::new(TransportCell::default());
        let mut renderer =
            Renderer::new(rx, shared.clone(), Arc::new(AudioMetrics::default()), 8, 1);
        let owner = AudioSource::for_output(fixture().asset.clone(), 8).unwrap();
        for (id, action) in [
            Action::Load {
                clip_id: 1,
                audio: owner.clone(),
            },
            Action::Play,
            Action::Seek(f64::NAN),
        ]
        .into_iter()
        .enumerate()
        {
            tx.push(Command {
                id: id as u64,
                issued: Instant::now(),
                action,
            })
            .ok()
            .unwrap();
        }
        let mut out = [1.0_f32; 4];
        renderer.render(&mut out);
        assert_eq!(out, [0.0; 4]); // opposite stereo channels downmix to silence
        assert_eq!(shared.read().frame, 2.0); // correct speed at 2x output rate
    }
}
