//! Native MIDI callback -> fixed nonblocking queue -> render-buffer boundary.
//! OS timestamps are diagnostic only; the Rust output sample clock stamps delivery.
use super::midi::{Control, MidiControlEvent, MidiEvent, MidiPlan, MidiSink, MAX_VOICES};
use crate::{
    error::{AppError, AppResult},
    project::schema::invalid,
};
use crossbeam_queue::ArrayQueue;
use midir::{MidiInput, MidiInputConnection};
use serde::Serialize;
use std::sync::{
    atomic::{
        AtomicU64,
        Ordering::{Acquire, Relaxed, Release, SeqCst},
    },
    Arc, Mutex,
};
const CAPACITY: usize = 1024;
const PER_BUFFER: usize = 128;
#[derive(Clone, Copy)]
struct Packet {
    route: [u8; 16],
    generation: u64,
    bytes: [u8; 3],
}
pub struct LiveMidi {
    queue: ArrayQueue<Packet>,
    generation: AtomicU64,
    lo: AtomicU64,
    hi: AtomicU64,
    panic: AtomicU64,
    received: AtomicU64,
    dropped: AtomicU64,
    delivered: AtomicU64,
    last: AtomicU64,
    frame: AtomicU64,
    clock: AtomicU64,
}
impl Default for LiveMidi {
    fn default() -> Self {
        Self {
            queue: ArrayQueue::new(CAPACITY),
            generation: AtomicU64::new(0),
            lo: AtomicU64::new(0),
            hi: AtomicU64::new(0),
            panic: AtomicU64::new(0),
            received: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            delivered: AtomicU64::new(0),
            last: AtomicU64::new(0),
            frame: AtomicU64::new(0),
            clock: AtomicU64::new(0),
        }
    }
}
impl LiveMidi {
    /// Single control writer. Odd generation prevents torn route reads; no spin.
    pub fn route(&self, route: Option<[u8; 16]>) {
        self.generation.fetch_add(1, SeqCst);
        let b = route.unwrap_or([0; 16]);
        self.lo
            .store(u64::from_le_bytes(b[..8].try_into().unwrap()), SeqCst);
        self.hi
            .store(u64::from_le_bytes(b[8..].try_into().unwrap()), SeqCst);
        self.generation.fetch_add(1, SeqCst);
        self.panic.fetch_add(1, Release);
    }
    /// Same ingress for native hardware and deterministic tests. No allocation,
    /// locks, logging, IPC, project traversal, or transport commands here.
    pub fn receive(&self, bytes: &[u8]) {
        if bytes.len() != 3
            || !matches!(bytes[0] & 0xf0, 0x80 | 0x90 | 0xb0 | 0xe0)
            || bytes[1] > 127
            || bytes[2] > 127
        {
            return;
        }
        self.received.fetch_add(1, Relaxed);
        let generation = self.generation.load(SeqCst);
        if generation & 1 != 0 {
            return;
        }
        let mut route = [0; 16];
        route[..8].copy_from_slice(&self.lo.load(SeqCst).to_le_bytes());
        route[8..].copy_from_slice(&self.hi.load(SeqCst).to_le_bytes());
        if self.generation.load(SeqCst) != generation || route == [0; 16] {
            return;
        }
        if self
            .queue
            .push(Packet {
                route,
                generation,
                bytes: [bytes[0], bytes[1], bytes[2]],
            })
            .is_err()
        {
            self.dropped.fetch_add(1, Relaxed);
            self.panic.fetch_add(1, Release);
        }
    }
}
#[derive(Default)]
pub struct LiveReader {
    epoch: u64,
    generation: u64,
    sequence: u32,
    computer: bool,
    active: VecFreeVoices,
    route: Option<u16>,
}
struct VecFreeVoices([Option<MidiEvent>; MAX_VOICES]);
impl Default for VecFreeVoices {
    fn default() -> Self {
        Self([None; MAX_VOICES])
    }
}
impl LiveReader {
    pub fn computer() -> Self {
        Self { computer: true, ..Self::default() }
    }
    pub fn reset(&mut self, frame: usize, sink: &mut impl MidiSink) {
        for slot in &mut self.active.0 {
            if let Some(e) = slot.take() {
                sink.event(MidiEvent {
                    frame,
                    offset: 0,
                    on: false,
                    velocity: 0,
                    ..e
                });
            }
        }
        if let Some(track) = self.route.take() {
            // Computer keys send only their own Note Offs. They must not reset
            // USB MIDI controllers/voices sharing the same instrument channel.
            if self.computer { return; }
            for channel in 0..16 {
                for control in [
                    Control::Cc(64, 0),
                    Control::Cc(123, 0),
                    Control::PitchBend(0),
                ] {
                    sink.control(MidiControlEvent {
                        frame,
                        offset: 0,
                        track,
                        channel,
                        control,
                    });
                }
            }
        }
    }
    pub fn process(
        &mut self,
        hub: &LiveMidi,
        plan: Option<&MidiPlan>,
        frame: usize,
        frames: usize,
        sink: &mut impl MidiSink,
    ) {
        let epoch = hub.panic.load(Acquire);
        let generation = hub.generation.load(Acquire);
        let overflow = epoch != self.epoch && generation == self.generation;
        if epoch != self.epoch || generation != self.generation {
            self.reset(frame, sink);
            self.epoch = epoch;
            self.generation = generation;
        }
        // On overflow discard bounded queued input and send Offs/reset. Losing an
        // Off can never leave a held note/pedal active in a future instrument.
        if overflow {
            for _ in 0..CAPACITY {
                if hub.queue.pop().is_none() {
                    break;
                }
            }
        }
        let mut cached_track: Option<Option<u16>> = None;
        for _ in 0..PER_BUFFER {
            let Some(packet) = hub.queue.pop() else {
                break;
            };
            if packet.generation != generation || generation & 1 != 0 {
                continue;
            }
            let track = *cached_track.get_or_insert_with(|| {
                plan.and_then(|p| p.track_keys.iter().position(|k| k == &packet.route))
                    .map(|n| n as u16)
            });
            let Some(track) = track else {
                continue;
            };
            let [status, a, b] = packet.bytes;
            let channel = status & 15;
            self.route = Some(track);
            match status & 0xf0 {
                0x80 | 0x90 => {
                    let on = status & 0xf0 == 0x90 && b > 0;
                    let slot = self.active.0.iter().position(|e| {
                        e.is_some_and(|e| e.channel == channel && e.pitch == a && e.track == track)
                    });
                    if let Some(i) = slot {
                        let old = self.active.0[i].take().unwrap();
                        sink.event(MidiEvent {
                            frame,
                            offset: 0,
                            on: false,
                            velocity: if on { 0 } else { b },
                            ..old
                        });
                    }
                    if on {
                        if let Some(i) = self.active.0.iter().position(Option::is_none) {
                            self.sequence = self.sequence.wrapping_add(1);
                            let e = MidiEvent {
                                frame,
                                offset: 0,
                                // Separate from sequenced notes and each other;
                                // CLAP/VST3 host note IDs must remain positive i32.
                                voice: (self.sequence & 0x1fffffff) | if self.computer { 0x40000000 } else { 0x20000000 },
                                track,
                                channel,
                                pitch: a,
                                on,
                                velocity: b,
                            };
                            self.active.0[i] = Some(e);
                            sink.event(e);
                        } else {
                            self.reset(frame, sink);
                            hub.dropped.fetch_add(1, Relaxed);
                        }
                    }
                }
                0xb0 => sink.control(MidiControlEvent {
                    frame,
                    offset: 0,
                    track,
                    channel,
                    control: Control::Cc(a, b),
                }),
                0xe0 => sink.control(MidiControlEvent {
                    frame,
                    offset: 0,
                    track,
                    channel,
                    control: Control::PitchBend((i16::from(b) * 128 + i16::from(a)) - 8192),
                }),
                _ => {}
            }
            hub.delivered.fetch_add(1, Relaxed);
            hub.last.store(
                u64::from(status) << 16 | u64::from(a) << 8 | u64::from(b),
                Relaxed,
            );
            hub.frame.store(frame as u64, Relaxed);
        }
        hub.clock.fetch_add(frames as u64, Relaxed);
    }
}
#[derive(Serialize)]
pub struct Port {
    pub id: usize,
    pub name: String,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub device: Option<String>,
    pub received: u64,
    pub delivered: u64,
    pub dropped: u64,
    pub last: u64,
    pub frame: u64,
    pub sample_clock: u64,
}
struct Connection {
    input: Option<MidiInputConnection<()>>,
    device: Option<String>,
    route: Option<String>,
}
pub struct MidiInputService {
    pub hub: Arc<LiveMidi>,
    pub computer: Arc<LiveMidi>,
    typing: Mutex<TypingState>,
    connection: Mutex<Connection>,
}
struct TypingState {
    route: Option<[u8;16]>,
    notes: [bool;128],
}
impl Default for MidiInputService {
    fn default() -> Self {
        Self {
            hub: Arc::new(LiveMidi::default()),
            computer: Arc::new(LiveMidi::default()),
            typing: Mutex::new(TypingState { route: None, notes: [false;128] }),
            connection: Mutex::new(Connection {
                input: None,
                device: None,
                route: None,
            }),
        }
    }
}
impl MidiInputService {
    /// Control-thread ingress. A bounded held-key snapshot makes repeat On and
    /// release idempotent. USB input keeps its own route, queue and voice IDs.
    pub fn computer_notes(&self, track: Option<&str>, notes: &[u8]) -> AppResult<()> {
        if notes.len() > 32 || notes.iter().any(|n| *n > 127) {
            return Err(invalid("잘못된 컴퓨터 MIDI 노트입니다."));
        }
        let route = track.map(uuid::Uuid::parse_str).transpose().map_err(invalid)?.map(|u| *u.as_bytes());
        let mut state = self.typing.lock().map_err(invalid)?;
        if route != state.route {
            self.computer.route(route);
            state.route = route;
            state.notes.fill(false);
        }
        let mut next = [false;128];
        if route.is_some() { for n in notes { next[*n as usize] = true; } }
        for (pitch, &on) in next.iter().enumerate() {
            if state.notes[pitch] != on {
                self.computer.receive(&[if on { 0x90 } else { 0x80 }, pitch as u8, if on { 100 } else { 0 }]);
            }
        }
        state.notes = next;
        Ok(())
    }
    pub fn computer_status(&self) -> Status {
        self.snapshot(&self.computer, None)
    }
    pub fn ports() -> AppResult<Vec<Port>> {
        let input = MidiInput::new("MiniDAW MIDI").map_err(invalid)?;
        input
            .ports()
            .iter()
            .enumerate()
            .map(|(id, p)| {
                Ok(Port {
                    id,
                    name: input.port_name(p).map_err(invalid)?,
                })
            })
            .collect()
    }
    pub fn connect(&self, port: Option<usize>, name: Option<&str>) -> AppResult<()> {
        let mut c = self.connection.lock().map_err(invalid)?;
        c.input.take();
        c.device = None;
        self.hub.route(None);
        c.route = None;
        if let Some(index) = port {
            let mut input = MidiInput::new("MiniDAW MIDI").map_err(invalid)?;
            input.ignore(midir::Ignore::Time | midir::Ignore::Sysex);
            let ports = input.ports();
            let p = ports
                .get(index)
                .ok_or_else(|| invalid("MIDI 장치 목록이 변경되었습니다. 새로고침해 주세요."))?;
            let device = input.port_name(p).map_err(invalid)?;
            if name != Some(device.as_str()) {
                return Err(invalid("MIDI 장치가 변경되었습니다. 다시 선택해 주세요."));
            }
            let hub = self.hub.clone();
            c.input = Some(
                input
                    .connect(
                        p,
                        "MiniDAW keyboard",
                        move |_, bytes, _| hub.receive(bytes),
                        (),
                    )
                    .map_err(|e| {
                        AppError::new("midi_input", "MIDI 입력을 열 수 없습니다.").detail(e)
                    })?,
            );
            c.device = Some(device);
        }
        Ok(())
    }
    pub fn route(&self, track: Option<&str>) -> AppResult<()> {
        let mut c = self.connection.lock().map_err(invalid)?;
        if c.route.as_deref() != track {
            let key = track
                .map(uuid::Uuid::parse_str)
                .transpose()
                .map_err(invalid)?
                .map(|u| *u.as_bytes());
            self.hub.route(key);
            c.route = track.map(str::to_owned);
        }
        Ok(())
    }
    pub fn status(&self) -> AppResult<Status> {
        let c = self.connection.lock().map_err(invalid)?;
        Ok(self.snapshot(&self.hub, c.device.clone()))
    }
    fn snapshot(&self, h: &LiveMidi, device: Option<String>) -> Status {
        Status {
            device,
            received: h.received.load(Relaxed),
            delivered: h.delivered.load(Relaxed),
            dropped: h.dropped.load(Relaxed),
            last: h.last.load(Relaxed),
            frame: h.frame.load(Relaxed),
            sample_clock: h.clock.load(Relaxed),
        }
    }
}
