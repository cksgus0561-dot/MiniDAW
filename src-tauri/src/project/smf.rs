//! SMF I/O runs on the project/control worker, never on the render thread.
use super::{schema::*, storage};
use crate::error::AppResult;
use midly::{Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, VecDeque},
    io::Read,
    path::Path,
};

const PPQ: u16 = 30_000;
const MARKER: &[u8] = b"\x7dMiniDAW.ticks.v1\0"; // educational/non-commercial sequencer metadata
const LIMIT: u64 = 16 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
struct Exact {
    hash: String,
    musical_time: MusicalTime,
    tracks: Vec<Track>,
}
pub struct Imported {
    pub musical_time: MusicalTime,
    pub tracks: Vec<Track>,
    pub skipped: usize,
}
fn scale(tick: u64, from: u32, to: u32) -> AppResult<i64> {
    i64::try_from((u128::from(tick) * u128::from(to) + u128::from(from) / 2) / u128::from(from))
        .map_err(invalid)
}
fn canonical(smf: &Smf<'_>) -> AppResult<Vec<u8>> {
    let mut copy = smf.clone();
    for track in &mut copy.tracks {
        // Our metadata is written at tick zero; only accept that placement.
        track.retain(|e| !(e.delta.as_int() == 0 && matches!(e.kind,TrackEventKind::Meta(MetaMessage::SequencerSpecific(b)) if b.starts_with(MARKER))));
    }
    let mut out = vec![];
    copy.write_std(&mut out).map_err(invalid)?;
    Ok(out)
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn read(path: &Path) -> AppResult<Imported> {
    let file = std::fs::File::open(path).map_err(invalid)?;
    if !file.metadata().map_err(invalid)?.is_file() {
        return Err(invalid("MIDI 일반 파일이 아닙니다."));
    }
    let mut bytes = vec![];
    file.take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(invalid)?;
    decode(&bytes)
}
pub fn decode(bytes: &[u8]) -> AppResult<Imported> {
    if bytes.len() as u64 > LIMIT {
        return Err(invalid("MIDI 파일 한도 16 MiB 초과"));
    }
    let smf = Smf::parse(bytes).map_err(invalid)?;
    if smf.header.format == Format::Sequential {
        return Err(invalid(
            "독립 sequence를 가진 SMF Type 2는 지원하지 않습니다. Type 0 또는 1로 저장해 주세요.",
        ));
    }
    let from = match smf.header.timing {
        Timing::Metrical(q) if q.as_int() > 0 => u32::from(q.as_int()),
        _ => {
            return Err(invalid(
                "SMPTE time-code MIDI는 지원하지 않습니다. PPQ 기반 SMF로 저장해 주세요.",
            ))
        }
    };
    if smf.tracks.len() > 4096 || smf.tracks.iter().map(Vec::len).sum::<usize>() > 500_000 {
        return Err(invalid("MIDI event/track 한도 초과"));
    }
    let default = Project::new().musical_time;
    // SMF's 15-bit PPQ cannot encode every 960000-PPQ tick. An optional, bound
    // metadata record restores exact Part boundaries/ticks for our own roundtrip.
    // Changed standard events invalidate it; external edits are never overwritten.
    for e in smf.tracks.iter().flatten() {
        if let TrackEventKind::Meta(MetaMessage::SequencerSpecific(data)) = e.kind {
            if e.delta.as_int() == 0 && data.starts_with(MARKER) {
                if let Ok(exact) = serde_json::from_slice::<Exact>(&data[MARKER.len()..]) {
                    if exact.hash == digest(&canonical(&smf)?)
                        && exact.musical_time.ticks_per_quarter == default.ticks_per_quarter
                        && exact.tracks.iter().all(|t| {
                            t.kind == TrackKind::Midi && t.clips.iter().all(|c| c.midi().is_some())
                        })
                    {
                        let mut p = Project::new();
                        p.musical_time = exact.musical_time.clone();
                        p.tracks = exact.tracks.clone();
                        if p.validate().is_ok() {
                            return Ok(Imported {
                                musical_time: exact.musical_time,
                                tracks: exact.tracks,
                                skipped: 0,
                            });
                        }
                    }
                }
            }
        }
    }
    let to = default.ticks_per_quarter;
    let mut tempos = BTreeMap::from([(0, 120.0)]);
    let mut signatures = BTreeMap::from([(0, (4, 4))]);
    let mut tracks = vec![];
    let mut skipped = 0;
    for (index, track) in smf.tracks.iter().enumerate() {
        let mut at = 0_u64;
        let mut name = format!("MIDI {}", index + 1);
        let mut pending = BTreeMap::<(u8, u8), VecDeque<(i64, u8)>>::new();
        let mut notes = vec![];
        let mut controls = vec![];
        for e in track {
            at = at
                .checked_add(u64::from(e.delta.as_int()))
                .ok_or_else(|| invalid("MIDI delta overflow"))?;
            let tick = scale(at, from, to)?;
            match e.kind {
                TrackEventKind::Midi { channel, message } => {
                    let channel = channel.as_int();
                    match message {
                        MidiMessage::NoteOn { key, vel } if vel.as_int() > 0 => pending
                            .entry((channel, key.as_int()))
                            .or_default()
                            .push_back((tick, vel.as_int())),
                        MidiMessage::NoteOff { key, vel } | MidiMessage::NoteOn { key, vel } => {
                            if let Some((start, velocity)) = pending
                                .get_mut(&(channel, key.as_int()))
                                .and_then(VecDeque::pop_front)
                            {
                                notes.push(MidiNote {
                                    note_id: id(),
                                    start_tick: Signed(start),
                                    length_tick: Signed((tick - start).max(1)),
                                    pitch: key.as_int(),
                                    velocity,
                                    release_velocity: vel.as_int(),
                                    channel,
                                });
                            } else {
                                skipped += 1;
                            }
                        }
                        MidiMessage::Controller { controller, value } => {
                            controls.push(MidiControl {
                                event_id: id(),
                                tick: Signed(tick),
                                channel,
                                data: MidiControlData::Cc {
                                    controller: controller.as_int(),
                                    value: value.as_int(),
                                },
                            })
                        }
                        MidiMessage::PitchBend { bend } => controls.push(MidiControl {
                            event_id: id(),
                            tick: Signed(tick),
                            channel,
                            data: MidiControlData::PitchBend {
                                value: bend.as_int(),
                            },
                        }),
                        _ => skipped += 1,
                    }
                }
                TrackEventKind::Meta(MetaMessage::TrackName(n)) => {
                    name = String::from_utf8_lossy(n).chars().take(128).collect()
                }
                TrackEventKind::Meta(MetaMessage::Tempo(m)) if m.as_int() > 0 => {
                    tempos.insert(tick, 60_000_000.0 / f64::from(m.as_int()));
                }
                TrackEventKind::Meta(MetaMessage::TimeSignature(n, d, _, _)) if d <= 6 => {
                    signatures.insert(tick, (n, 1_u8 << d));
                }
                TrackEventKind::SysEx(_) | TrackEventKind::Escape(_) => skipped += 1,
                _ => {}
            }
        }
        let mut end = scale(at, from, to)?.max(1);
        // Malformed files with missing Off are bounded at EndOfTrack, not stuck.
        for ((channel, pitch), queue) in pending {
            for (start, velocity) in queue {
                skipped += 1;
                notes.push(MidiNote {
                    note_id: id(),
                    start_tick: Signed(start),
                    length_tick: Signed((end - start).max(1)),
                    pitch,
                    velocity,
                    release_velocity: 0,
                    channel,
                });
            }
        }
        if !notes.is_empty() || !controls.is_empty() {
            notes.sort_by_key(|n| (n.start_tick.0, n.channel, n.pitch));
            for n in &notes {
                end = end.max(n.start_tick.0 + n.length_tick.0);
            }
            tracks.push(Track {
                synth: Default::default(),
                inserts: vec![],
                track_id: id(),
                name: name.clone(),
                kind: TrackKind::Midi,
                instrument: Instrument::None,
                mix: TrackMix::default(),
                extensions: Extensions::new(),
                clips: vec![Clip::Midi(MidiClip {
                    content_offset_tick: Signed(0),
                    clip_id: id(),
                    name,
                    start_tick: Signed(0),
                    length_tick: Signed(end),
                    notes,
                    controls,
                })],
            });
        }
    }
    if tracks.is_empty() {
        return Err(invalid("가져올 Note 또는 CC/Pitch Bend가 없습니다."));
    }
    let musical_time = MusicalTime {
        ticks_per_quarter: to,
        tempo_map: tempos
            .into_iter()
            .map(|(t, bpm)| Tempo {
                tick: Signed(t),
                bpm,
            })
            .collect(),
        time_signatures: signatures
            .into_iter()
            .map(|(t, (numerator, denominator))| TimeSignature {
                tick: Signed(t),
                numerator,
                denominator,
            })
            .collect(),
    };
    let mut p = Project::new();
    p.musical_time = musical_time.clone();
    p.tracks = tracks.clone();
    p.validate()?;
    Ok(Imported {
        musical_time,
        tracks,
        skipped,
    })
}
pub fn merge(p: &Project, mut input: Imported, tempo: bool) -> AppResult<(Project, String)> {
    let mut next = p.clone();
    // Legacy projects may use another PPQ. Keep the destination timebase intact.
    let from = input.musical_time.ticks_per_quarter;
    let to = p.musical_time.ticks_per_quarter;
    if from != to {
        let convert =
            |t: i64| scale(t.unsigned_abs(), from, to).map(|v| if t < 0 { -v } else { v });
        for t in &mut input.musical_time.tempo_map {
            t.tick = Signed(convert(t.tick.0)?);
        }
        for s in &mut input.musical_time.time_signatures {
            s.tick = Signed(convert(s.tick.0)?);
        }
        input.musical_time.ticks_per_quarter = to;
        for c in input.tracks.iter_mut().flat_map(|t| &mut t.clips) {
            if let Clip::Midi(c) = c {
                let old_start = c.start_tick.0;
                let old_origin = c.content_origin();
                let origin = convert(old_origin)?;
                let start = convert(old_start)?;
                c.length_tick = Signed((convert(old_start + c.length_tick.0)? - start).max(1));
                c.start_tick = Signed(start);
                c.content_offset_tick = Signed(start - origin);
                for n in &mut c.notes {
                    let at = convert(old_origin + n.start_tick.0)?;
                    let end = convert(old_origin + n.start_tick.0 + n.length_tick.0)?;
                    n.start_tick = Signed(at - origin);
                    n.length_tick = Signed((end - at).max(1));
                }
                for e in &mut c.controls {
                    e.tick = Signed(convert(old_origin + e.tick.0)? - origin);
                }
            }
        }
    }
    if tempo {
        next.musical_time = input.musical_time;
    }
    let count = input.tracks.len();
    for t in &mut input.tracks {
        t.track_id = id();
        for c in &mut t.clips {
            if let Clip::Midi(c) = c {
                c.clip_id = id();
                for n in &mut c.notes {
                    n.note_id = id();
                }
                for e in &mut c.controls {
                    e.event_id = id();
                }
            }
        }
    }
    next.tracks.extend(input.tracks);
    next.validate()?;
    Ok((
        next,
        format!(
            "MIDI {count} Track 가져옴 · Tempo/박자 {}{}",
            if tempo {
                "적용"
            } else {
                "프로젝트 유지"
            },
            if input.skipped > 0 {
                format!(
                    " · 지원하지 않거나 불완전한 이벤트 {}개 건너뜀/보정",
                    input.skipped
                )
            } else {
                String::new()
            }
        ),
    ))
}
fn sequence<'a>(mut events: Vec<(u64, u8, TrackEventKind<'a>)>) -> AppResult<Vec<TrackEvent<'a>>> {
    events.sort_by_key(|e| (e.0, e.1));
    let mut previous = 0;
    let mut track = vec![];
    for (at, _, kind) in events {
        let mut delta = at - previous;
        // VLQ deltas are 28-bit. Preserve long gaps with legal empty text events.
        while delta > 0x0fff_ffff {
            track.push(TrackEvent {
                delta: 0x0fff_ffff.into(),
                kind: TrackEventKind::Meta(MetaMessage::Text(b"")),
            });
            delta -= 0x0fff_ffff;
        }
        track.push(TrackEvent {
            delta: (delta as u32).into(),
            kind,
        });
        previous = at;
    }
    track.push(TrackEvent {
        delta: 0.into(),
        kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
    });
    Ok(track)
}
pub fn encode(p: &Project) -> AppResult<Vec<u8>> {
    p.validate()?;
    let tick = |t: i64| -> AppResult<u64> {
        Ok(scale(t as u64, p.musical_time.ticks_per_quarter, u32::from(PPQ))? as u64)
    };
    let mut meta = vec![];
    for t in &p.musical_time.tempo_map {
        let micros = (60_000_000.0 / t.bpm).round();
        if !(1.0..=16_777_215.0).contains(&micros) {
            return Err(invalid("Tempo가 SMF 24-bit µs/quarter 범위를 벗어납니다."));
        }
        meta.push((
            tick(t.tick.0)?,
            0,
            TrackEventKind::Meta(MetaMessage::Tempo((micros as u32).into())),
        ));
    }
    for s in &p.musical_time.time_signatures {
        meta.push((
            tick(s.tick.0)?,
            1,
            TrackEventKind::Meta(MetaMessage::TimeSignature(
                s.numerator,
                s.denominator.ilog2() as u8,
                24,
                8,
            )),
        ));
    }
    let mut smf = Smf {
        header: Header::new(Format::Parallel, Timing::Metrical(PPQ.into())),
        tracks: vec![sequence(meta)?],
    };
    let tracks: Vec<_> = p
        .tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Midi)
        .cloned()
        .collect();
    if tracks.is_empty() {
        return Err(invalid("내보낼 MIDI Track이 없습니다."));
    }
    for t in &tracks {
        let mut events = vec![(
            0,
            0,
            TrackEventKind::Meta(MetaMessage::TrackName(t.name.as_bytes())),
        )];
        let mut end = 0;
        for c in t.clips.iter().filter_map(Clip::midi) {
            end = end.max(tick(c.start_tick.0 + c.length_tick.0)?);
            for n in &c.notes {
                let Some((from, to)) = c.note_window(n) else {
                    continue;
                };
                let start = tick(from)?;
                let stop = tick(to)?.max(start + 1);
                events.push((
                    start,
                    3,
                    TrackEventKind::Midi {
                        channel: n.channel.into(),
                        message: MidiMessage::NoteOn {
                            key: n.pitch.into(),
                            vel: n.velocity.into(),
                        },
                    },
                ));
                events.push((
                    stop,
                    1,
                    TrackEventKind::Midi {
                        channel: n.channel.into(),
                        message: MidiMessage::NoteOff {
                            key: n.pitch.into(),
                            vel: n.release_velocity.into(),
                        },
                    },
                ));
                end = end.max(stop);
            }
            for e in &c.controls {
                let Some(at) = c.control_position(e) else {
                    continue;
                };
                events.push((
                    tick(at)?,
                    2,
                    TrackEventKind::Midi {
                        channel: e.channel.into(),
                        message: match e.data {
                            MidiControlData::Cc { controller, value } => MidiMessage::Controller {
                                controller: controller.into(),
                                value: value.into(),
                            },
                            MidiControlData::PitchBend { value } => MidiMessage::PitchBend {
                                bend: midly::PitchBend::from_int(value),
                            },
                        },
                    },
                ));
            }
        }
        events.push((end, 4, TrackEventKind::Meta(MetaMessage::Text(b""))));
        smf.tracks.push(sequence(events)?);
    }
    let hash = digest(&canonical(&smf)?);
    let mut data = MARKER.to_vec();
    data.extend(
        serde_json::to_vec(&Exact {
            hash,
            musical_time: p.musical_time.clone(),
            tracks: tracks.clone(),
        })
        .map_err(invalid)?,
    );
    smf.tracks[0].insert(
        0,
        TrackEvent {
            delta: 0.into(),
            kind: TrackEventKind::Meta(MetaMessage::SequencerSpecific(&data)),
        },
    );
    let mut bytes = vec![];
    smf.write_std(&mut bytes).map_err(invalid)?;
    if bytes.len() as u64 > LIMIT {
        return Err(invalid("MIDI 내보내기 한도 16 MiB 초과"));
    }
    Ok(bytes)
}
pub fn write(path: &Path, p: &Project) -> AppResult<()> {
    storage::atomic_write(path, &encode(p)?)
}
