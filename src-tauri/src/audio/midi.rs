//! Control-thread compilation; callback-side sample-clock scheduling. No UI clock,
//! allocation, locks, I/O, tick conversion or project traversal in the scheduler.
use crate::{
    error::AppResult,
    project::{schema::*, time::time},
};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_VOICES: usize = 256;
#[derive(Clone, Copy, Debug)]
pub struct Tone {
    pub start: usize,
    pub end: usize,
    pub track: u16,
    pub pitch: u8,
    pub velocity: u8,
    pub release_velocity: u8,
    pub channel: u8,
    held_end: usize,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Control {
    Cc(u8, u8),
    PitchBend(i16),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiControlEvent {
    pub frame: usize,
    pub offset: usize,
    pub track: u16,
    pub channel: u8,
    pub control: Control,
}
struct ControlLane {
    events: Vec<MidiControlEvent>,
}
#[derive(Clone, Copy)]
struct Boundary {
    frame: usize,
    on: bool,
    voice: u32,
}
struct Chase {
    frame: usize,
    voices: Vec<u32>,
}
pub struct MidiPlan {
    pub tones: Vec<Tone>,
    pub end: usize,
    // Stable document IDs remain on the control side for future Instrument routing.
    pub track_ids: Vec<String>,
    pub note_ids: Vec<String>,
    pub track_keys: Vec<[u8; 16]>,
    pub instruments: Vec<Instrument>,
    pub track_gains: Vec<[f64; 2]>,
    pub synth_settings: Vec<crate::project::automation::SynthSettings>,
    events: Vec<Boundary>,
    chase: Vec<Chase>,
    controls: Vec<MidiControlEvent>,
    lanes: Vec<ControlLane>,
    channels: Vec<(u16, u8)>,
}
impl MidiPlan {
    pub fn compile(p: &Project, rate: u32) -> AppResult<Self> {
        p.validate()?;
        if rate == 0 {
            return Err(invalid("MIDI output sample rate"));
        }
        let frame = |tick| -> AppResult<usize> {
            usize::try_from(
                time(
                    &Position::Ticks {
                        ticks: Signed(tick),
                    },
                    &p.musical_time,
                )
                .ceil_frame(rate),
            )
            .map_err(|_| invalid("MIDI sample position"))
        };
        let mut plan = Self {
            tones: vec![],
            end: 0,
            track_ids: vec![],
            note_ids: vec![],
            track_keys: vec![],
            instruments: vec![],
            track_gains: vec![],
            synth_settings: vec![],
            events: vec![],
            chase: vec![],
            controls: vec![],
            lanes: vec![],
            channels: vec![],
        };
        let any_solo = p.tracks.iter().any(|t| t.mix.solo);
        for track in p.tracks.iter().filter(|t| t.kind == TrackKind::Midi) {
            let index = plan.track_ids.len() as u16;
            plan.track_ids.push(track.track_id.clone());
            plan.instruments.push(track.instrument);
            plan.track_gains.push(track.mix.gains(any_solo));
            plan.synth_settings.push(track.synth.clone());
            plan.track_keys.push(
                *uuid::Uuid::parse_str(&track.track_id)
                    .map_err(invalid)?
                    .as_bytes(),
            );
            for c in track.clips.iter().filter_map(Clip::midi) {
                plan.end = plan.end.max(frame(c.start_tick.0 + c.length_tick.0)?);
                for n in &c.notes {
                    let Some((from, to)) = c.note_window(n) else {
                        continue;
                    };
                    let start = frame(from)?;
                    // Sub-sample notes retain exact ticks in the document. At the output
                    // boundary they occupy at least one sample, never a reversed On/Off.
                    let end = frame(to)?.max(start + 1);
                    plan.end = plan.end.max(end);
                    let voice = plan.tones.len() as u32;
                    plan.tones.push(Tone {
                        start,
                        end,
                        track: index,
                        pitch: n.pitch,
                        velocity: n.velocity,
                        release_velocity: n.release_velocity,
                        channel: n.channel,
                        held_end: end,
                    });
                    plan.note_ids.push(n.note_id.clone());
                    plan.events.extend([
                        Boundary {
                            frame: start,
                            on: true,
                            voice,
                        },
                        Boundary {
                            frame: end,
                            on: false,
                            voice,
                        },
                    ]);
                }
                for e in &c.controls {
                    let Some(at) = c.control_position(e) else {
                        continue;
                    };
                    plan.controls.push(MidiControlEvent {
                        frame: frame(at)?,
                        offset: 0,
                        track: index,
                        channel: e.channel,
                        control: match e.data {
                            MidiControlData::Cc { controller, value } => {
                                Control::Cc(controller, value)
                            }
                            MidiControlData::PitchBend { value } => Control::PitchBend(value),
                        },
                    });
                }
            }
        }
        plan.controls.sort_by_key(|e| e.frame);
        let mut lanes = BTreeMap::<(u16, u8, u8), Vec<MidiControlEvent>>::new();
        let mut channels = BTreeSet::new();
        for e in &plan.controls {
            lanes
                .entry((
                    e.track,
                    e.channel,
                    match e.control {
                        Control::Cc(c, _) => c,
                        Control::PitchBend(_) => 128,
                    },
                ))
                .or_default()
                .push(*e);
            channels.insert((e.track, e.channel));
        }
        if lanes.len() > 1024 || channels.len() > MAX_VOICES {
            return Err(invalid("MIDI controller lane 한도 초과"));
        }
        if plan
            .controls
            .chunk_by(|a, b| a.frame == b.frame)
            .any(|g| g.len() > 512)
        {
            return Err(invalid("MIDI sample당 controller 한도 초과"));
        }
        // Hold-aware chase is compiled off the callback. Normal playback still sends
        // the original Note Off; the instrument applies CC64, as MIDI specifies.
        let releases: BTreeMap<_, Vec<_>> = lanes
            .iter()
            .filter(|((_, _, kind), _)| *kind == 64)
            .map(|(&(track, channel, _), events)| {
                (
                    (track, channel),
                    events
                        .iter()
                        .filter(|e| matches!(e.control,Control::Cc(64,v) if v<64))
                        .map(|e| e.frame)
                        .collect(),
                )
            })
            .collect();
        for tone in &mut plan.tones {
            if let Some(pedal) = lanes.get(&(tone.track, tone.channel, 64)) {
                let at = pedal.partition_point(|e| e.frame <= tone.end);
                if at > 0 && matches!(pedal[at-1].control, Control::Cc(64,v) if v >= 64) {
                    let off = &releases[&(tone.track, tone.channel)];
                    tone.held_end = off
                        .get(off.partition_point(|f| *f <= tone.end))
                        .copied()
                        .unwrap_or(plan.end);
                }
            }
        }
        plan.lanes = lanes
            .into_values()
            .map(|events| ControlLane { events })
            .collect();
        plan.channels = channels.into_iter().collect();
        // Note Off before Note On at the same sample, including repeated pitches.
        plan.events
            .sort_unstable_by_key(|e| (e.frame, e.on, e.voice));
        if plan
            .events
            .chunk_by(|a, b| a.frame == b.frame)
            .any(|g| g.len() > MAX_VOICES * 2)
        {
            return Err(invalid("MIDI sample당 event 한도 초과"));
        }
        let mut chase_events = plan.events.clone();
        for e in &mut chase_events {
            if !e.on {
                e.frame = plan.tones[e.voice as usize].held_end;
            }
        }
        chase_events.sort_unstable_by_key(|e| (e.frame, e.on, e.voice));
        let mut active = BTreeSet::new();
        let mut i = 0;
        let mut references = 0;
        while i < chase_events.len() {
            let at = chase_events[i].frame;
            let first = i;
            while i < chase_events.len() && chase_events[i].frame == at {
                let e = chase_events[i];
                if e.on {
                    active.insert(e.voice);
                } else {
                    active.remove(&e.voice);
                }
                if active.len() > MAX_VOICES {
                    return Err(invalid("MIDI 동시 Note 한도(256) 초과"));
                }
                i += 1;
            }
            if i - first > MAX_VOICES * 2 {
                return Err(invalid("MIDI sample당 event 한도 초과"));
            }
            references += active.len();
            if references > 2_000_000 {
                return Err(invalid("MIDI scheduling plan 한도 초과"));
            }
            plan.chase.push(Chase {
                frame: at,
                voices: active.iter().copied().collect(),
            });
        }
        Ok(plan)
    }
    pub fn is_empty(&self) -> bool {
        self.tones.is_empty() && self.controls.is_empty()
    }
}

/// Delivered synchronously on the render thread. Future instruments consume
/// this before rendering the given sample. A sink must itself be real-time safe.
/// offset == buffer length is the trailing boundary (e.g. EOF Note Off).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiEvent {
    pub frame: usize,
    pub offset: usize,
    pub voice: u32,
    pub track: u16,
    pub pitch: u8,
    pub on: bool,
    pub velocity: u8,
    pub channel: u8,
}
pub trait MidiSink {
    fn event(&mut self, event: MidiEvent);
    fn control(&mut self, _event: MidiControlEvent) {}
}
pub struct NoInstrument;
impl MidiSink for NoInstrument {
    fn event(&mut self, _: MidiEvent) {}
}

pub struct MidiScheduler {
    active: [Option<(u32, Tone)>; MAX_VOICES],
    count: usize,
    cursor: usize,
    next: Option<usize>,
    control_cursor: usize,
    channels: [Option<(u16, u8)>; MAX_VOICES],
}
impl Default for MidiScheduler {
    fn default() -> Self {
        Self {
            active: [None; MAX_VOICES],
            count: 0,
            cursor: 0,
            next: None,
            control_cursor: 0,
            channels: [None; MAX_VOICES],
        }
    }
}
impl MidiScheduler {
    fn emit(
        voice: u32,
        tone: Tone,
        on: bool,
        frame: usize,
        offset: usize,
        sink: &mut impl MidiSink,
    ) {
        sink.event(MidiEvent {
            frame,
            offset,
            voice,
            track: tone.track,
            pitch: tone.pitch,
            on,
            velocity: if on {
                tone.velocity
            } else {
                tone.release_velocity
            },
            channel: tone.channel,
        });
    }
    pub fn reset(&mut self, frame: usize, offset: usize, sink: &mut impl MidiSink) {
        for i in 0..self.count {
            let (voice, tone) = self.active[i].take().unwrap();
            Self::emit(voice, tone, false, frame, offset, sink);
        }
        self.count = 0;
        for channel in &mut self.channels {
            if let Some((track, channel)) = channel.take() {
                for control in [
                    Control::Cc(64, 0),
                    Control::Cc(121, 0),
                    Control::PitchBend(0),
                ] {
                    sink.control(MidiControlEvent {
                        frame,
                        offset,
                        track,
                        channel,
                        control,
                    });
                }
            }
        }
        self.next = None;
    }
    fn on(
        &mut self,
        plan: &MidiPlan,
        voice: u32,
        frame: usize,
        offset: usize,
        sink: &mut impl MidiSink,
    ) {
        let tone = plan.tones[voice as usize];
        debug_assert!(self.count < MAX_VOICES);
        self.active[self.count] = Some((voice, tone));
        self.count += 1;
        Self::emit(voice, tone, true, frame, offset, sink);
    }
    pub fn sample(
        &mut self,
        plan: &MidiPlan,
        frame: usize,
        offset: usize,
        sink: &mut impl MidiSink,
    ) {
        if self.next != Some(frame) {
            self.reset(frame, offset, sink);
            self.cursor = plan.events.partition_point(|e| e.frame < frame);
            self.control_cursor = plan.controls.partition_point(|e| e.frame < frame);
            for (i, &channel) in plan.channels.iter().enumerate() {
                self.channels[i] = Some(channel);
            }
            for lane in &plan.lanes {
                let i = lane.events.partition_point(|e| e.frame < frame);
                if i > 0 {
                    sink.control(MidiControlEvent {
                        frame,
                        offset,
                        ..lane.events[i - 1]
                    });
                }
            }
            // Indexed chase on seek/resume/cycle wrap. No full note scan in RT.
            let i = plan.chase.partition_point(|s| s.frame < frame);
            if i > 0 {
                for &voice in &plan.chase[i - 1].voices {
                    let tone = plan.tones[voice as usize];
                    if tone.held_end > frame {
                        if tone.end > frame {
                            self.on(plan, voice, frame, offset, sink);
                        } else {
                            Self::emit(voice, tone, true, frame, offset, sink);
                            Self::emit(voice, tone, false, frame, offset, sink);
                        }
                    }
                }
            }
        }
        // Controllers precede Note On at a shared sample (pedal/bend effective
        // immediately). Original Note Offs retain their exact sample boundary.
        while let Some(e) = plan
            .controls
            .get(self.control_cursor)
            .filter(|e| e.frame == frame)
        {
            sink.control(MidiControlEvent { offset, ..*e });
            self.control_cursor += 1;
        }
        while let Some(e) = plan.events.get(self.cursor).filter(|e| e.frame == frame) {
            if e.on {
                self.on(plan, e.voice, frame, offset, sink);
            } else if let Some(i) = self.active[..self.count]
                .iter()
                .position(|v| v.is_some_and(|(voice, _)| voice == e.voice))
            {
                Self::emit(
                    e.voice,
                    plan.tones[e.voice as usize],
                    false,
                    frame,
                    offset,
                    sink,
                );
                self.count -= 1;
                self.active[i] = self.active[self.count].take();
            }
            self.cursor += 1;
        }
        self.next = Some(frame + 1);
    }
}
