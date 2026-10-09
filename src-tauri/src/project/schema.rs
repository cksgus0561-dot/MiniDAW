//! Persisted musical document only. No engine, device, cache or UI preference types.
use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

pub const VERSION: u32 = 1;
pub const MAX_BYTES: u64 = 16 * 1024 * 1024;
// JSON numbers remain exact in JavaScript. Allow tick-time + source-sample
// rational denominators without rounding again after each Trim/Split.
pub const MAX_TIME_DENOMINATOR: u64 = 9_007_199_254_740_991;
pub type Extensions = BTreeMap<String, Value>;
pub fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}
pub fn invalid(detail: impl std::fmt::Display) -> AppError {
    AppError::new("project_invalid", "프로젝트 데이터가 올바르지 않습니다.").detail(detail)
}

// Decimal strings keep all 64 bits across JSON/JavaScript, including future edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Frames(pub u64);
impl TryFrom<String> for Frames {
    type Error = &'static str;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        if s.is_empty() || s.len() > 20 || !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err("invalid unsigned decimal integer");
        }
        s.parse().map(Self).map_err(|_| "frame integer overflow")
    }
}
impl From<Frames> for String {
    fn from(n: Frames) -> Self {
        n.0.to_string()
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Signed(pub i64);
impl TryFrom<String> for Signed {
    type Error = &'static str;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        let digits = s.strip_prefix('-').unwrap_or(&s);
        if digits.is_empty() || s.len() > 20 || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err("invalid signed decimal integer");
        }
        s.parse().map(Self).map_err(|_| "time integer overflow")
    }
}
impl From<Signed> for String {
    fn from(n: Signed) -> Self {
        n.0.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "unit", rename_all = "camelCase", deny_unknown_fields)]
pub enum Position {
    Seconds { numerator: Signed, denominator: u64 },
    Ticks { ticks: Signed },
}
impl Position {
    pub fn zero() -> Self {
        Self::Seconds {
            numerator: Signed(0),
            denominator: 1,
        }
    }
    pub fn is_zero(&self) -> bool {
        match self {
            Self::Seconds { numerator, .. } => numerator.0 == 0,
            Self::Ticks { ticks } => ticks.0 == 0,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Tempo {
    pub tick: Signed,
    pub bpm: f64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TimeSignature {
    pub tick: Signed,
    pub numerator: u8,
    pub denominator: u8,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MusicalTime {
    pub ticks_per_quarter: u32,
    pub tempo_map: Vec<Tempo>,
    pub time_signatures: Vec<TimeSignature>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Cycle {
    pub enabled: bool,
    pub start_tick: Signed,
    pub end_tick: Signed,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PathReference {
    pub project_relative_path: Option<String>,
    pub original_absolute_path: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Fingerprint {
    pub file_bytes: Frames,
    // SHA256 of size + first/last 64KiB; bounded I/O, not a full-content hash.
    pub sampled_sha256: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AudioMetadata {
    pub sample_rate: u32,
    pub channels: u16,
    pub source_frames: Frames,
    pub container: String,
    pub codec: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Asset {
    pub asset_id: String,
    pub filename: String,
    pub path: PathReference,
    pub metadata: AudioMetadata,
    pub fingerprint: Fingerprint,
    #[serde(default)]
    pub extensions: Extensions,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FadeCurve {
    Linear,
    Cosine,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Fade {
    pub source_frames: Frames,
    pub curve: FadeCurve,
}
pub const ENVELOPE_WINDOW: &str = "minidaw.envelopeWindow.v1";
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnvelopeWindow {
    pub source_start: Frames,
    pub source_end: Frames,
    pub fade_in: Fade,
    pub fade_out: Fade,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AudioClip {
    pub clip_id: String,
    pub asset_id: String,
    pub name: String,
    pub position: Position,
    pub source_start: Frames,
    pub source_end: Frames,
    pub gain: f64,
    pub fade_in: Fade,
    pub fade_out: Fade,
    pub mute: bool,
    #[serde(default)]
    pub extensions: Extensions,
}
/// Exact musical ticks in the Part content coordinates (independent of its window).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MidiNote {
    pub note_id: String,
    pub start_tick: Signed,
    pub length_tick: Signed,
    pub pitch: u8,
    #[serde(default = "default_velocity")]
    pub velocity: u8,
    #[serde(default)]
    pub release_velocity: u8,
    #[serde(default)]
    pub channel: u8,
}
pub fn default_velocity() -> u8 {
    100
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum MidiControlData {
    Cc { controller: u8, value: u8 },
    PitchBend { value: i16 },
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MidiControl {
    pub event_id: String,
    pub tick: Signed,
    #[serde(default)]
    pub channel: u8,
    pub data: MidiControlData,
}
impl MidiControlData {
    pub fn valid(&self) -> bool {
        match self {
            Self::Cc { controller, value } => *controller < 128 && *value < 128,
            Self::PitchBend { value } => (-8192..=8191).contains(value),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MidiClip {
    pub clip_id: String,
    pub name: String,
    pub start_tick: Signed,
    pub length_tick: Signed,
    /// Part window in the unchanged content tick coordinates. Legacy Parts start at zero.
    #[serde(default = "zero_tick")]
    pub content_offset_tick: Signed,
    pub notes: Vec<MidiNote>,
    #[serde(default)]
    pub controls: Vec<MidiControl>,
}
fn zero_tick() -> Signed {
    Signed(0)
}
impl MidiClip {
    pub fn content_origin(&self) -> i64 {
        self.start_tick.0 - self.content_offset_tick.0
    }
    /// Resolve the non-destructive Part window before sample-clock scheduling/export.
    pub fn note_window(&self, n: &MidiNote) -> Option<(i64, i64)> {
        let start = (self.content_origin() + n.start_tick.0).max(self.start_tick.0);
        let end = (self.content_origin() + n.start_tick.0 + n.length_tick.0)
            .min(self.start_tick.0 + self.length_tick.0);
        (start < end).then_some((start, end))
    }
    pub fn control_position(&self, e: &MidiControl) -> Option<i64> {
        let at = self.content_origin() + e.tick.0;
        // End-point controls (e.g. pedal release) retain the existing inclusive boundary.
        (at >= self.start_tick.0 && at <= self.start_tick.0 + self.length_tick.0).then_some(at)
    }
    /// Existing note-edit commands use Part-relative coordinates. Rebase only on
    /// explicit content edits, never on a Part resize; all absolute events stay fixed.
    pub fn rebase_content(&mut self) {
        for n in &mut self.notes {
            n.start_tick.0 -= self.content_offset_tick.0;
        }
        for e in &mut self.controls {
            e.tick.0 -= self.content_offset_tick.0;
        }
        self.content_offset_tick = Signed(0);
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Clip {
    Audio(AudioClip),
    Midi(MidiClip),
}
impl Clip {
    pub fn as_audio(&self) -> Option<&AudioClip> {
        match self {
            Self::Audio(c) => Some(c),
            _ => None,
        }
    }
    pub fn as_audio_mut(&mut self) -> Option<&mut AudioClip> {
        match self {
            Self::Audio(c) => Some(c),
            _ => None,
        }
    }
    pub fn midi(&self) -> Option<&MidiClip> {
        match self {
            Self::Midi(c) => Some(c),
            _ => None,
        }
    }
    pub fn id(&self) -> &str {
        match self {
            Self::Audio(c) => &c.clip_id,
            Self::Midi(c) => &c.clip_id,
        }
    }

    pub fn audio(&self) -> &AudioClip {
        self.as_audio().expect("known Audio Clip")
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrackKind {
    Audio,
    Midi,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Instrument {
    #[default]
    None,
    BasicSynth,
    External,
}
impl Instrument {
    pub fn is_none(&self) -> bool {
        *self == Self::None
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct TrackMix {
    pub mute: bool,
    pub solo: bool,
    pub volume_db: f64,
    pub pan: f64,
}
impl TrackMix {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
    /// Stereo balance, unity at center. Global Solo spans Audio and MIDI alike.
    pub fn gains(&self, any_solo: bool) -> [f64; 2] {
        if self.mute || (any_solo && !self.solo) {
            return [0.0; 2];
        }
        let gain = 10f64.powf(self.volume_db / 20.0);
        [
            gain * (1.0 - self.pan).min(1.0),
            gain * (1.0 + self.pan).min(1.0),
        ]
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Track {
    #[serde(
        default,
        skip_serializing_if = "super::automation::SynthSettings::is_default"
    )]
    pub synth: super::automation::SynthSettings,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inserts: Vec<super::effects::Effect>,
    pub track_id: String,
    pub name: String,
    pub kind: TrackKind,
    #[serde(default, skip_serializing_if = "Instrument::is_none")]
    pub instrument: Instrument,
    #[serde(default, skip_serializing_if = "TrackMix::is_default")]
    pub mix: TrackMix,
    pub clips: Vec<Clip>,
    #[serde(default)]
    pub extensions: Extensions,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Project {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub automation: Vec<super::automation::Channel>,
    #[serde(default, skip_serializing_if = "MasterMix::is_default")]
    pub master: MasterMix,
    pub format: String,
    pub schema_version: u32,
    pub project_id: String,
    pub name: String,
    pub musical_time: MusicalTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cycle: Option<Cycle>,
    pub assets: Vec<Asset>,
    pub tracks: Vec<Track>,
    // Retained for old project/preview compatibility; arrangement plays all Tracks.
    pub primary_clip_id: Option<String>,
    #[serde(default)]
    pub extensions: Extensions,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct MasterMix {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inserts: Vec<super::effects::Effect>,
    pub volume_db: f64,
}
impl MasterMix {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}
impl Default for Project {
    fn default() -> Self {
        Self::new()
    }
}
impl Project {
    pub fn new() -> Self {
        Self {
            automation: vec![],
            master: MasterMix::default(),
            format: "MiniDAW".into(),
            schema_version: VERSION,
            project_id: id(),
            name: "Untitled".into(),
            musical_time: MusicalTime {
                ticks_per_quarter: 960_000,
                tempo_map: vec![Tempo {
                    tick: Signed(0),
                    bpm: 120.0,
                }],
                time_signatures: vec![TimeSignature {
                    tick: Signed(0),
                    numerator: 4,
                    denominator: 4,
                }],
            },
            assets: vec![],
            cycle: None,
            tracks: vec![],
            primary_clip_id: None,
            extensions: Extensions::new(),
        }
    }
    pub fn clips(&self) -> impl Iterator<Item = &AudioClip> {
        self.tracks
            .iter()
            .flat_map(|t| t.clips.iter().filter_map(Clip::as_audio))
    }
    pub fn midi_clips(&self) -> impl Iterator<Item = &MidiClip> {
        self.tracks
            .iter()
            .flat_map(|t| t.clips.iter().filter_map(Clip::midi))
    }
    pub fn primary(&self) -> Option<&AudioClip> {
        self.clips()
            .find(|c| Some(&c.clip_id) == self.primary_clip_id.as_ref())
    }
    pub fn import(&mut self, asset: Asset) {
        let clip = AudioClip {
            clip_id: id(),
            asset_id: asset.asset_id.clone(),
            name: asset.filename.clone(),
            position: Position::zero(),
            source_start: Frames(0),
            source_end: asset.metadata.source_frames,
            gain: 1.0,
            fade_in: Fade {
                source_frames: Frames(0),
                curve: FadeCurve::Cosine,
            },
            fade_out: Fade {
                source_frames: Frames(0),
                curve: FadeCurve::Cosine,
            },
            mute: false,
            extensions: Extensions::new(),
        };
        self.primary_clip_id = Some(clip.clip_id.clone());
        if !self.tracks.iter().any(|t| t.kind == TrackKind::Audio) {
            self.tracks.push(Track {
                synth: Default::default(),
                inserts: vec![],
                track_id: id(),
                name: "Audio 1".into(),
                kind: TrackKind::Audio,
                instrument: Instrument::None,
                mix: TrackMix::default(),
                clips: vec![],
                extensions: Extensions::new(),
            });
        }
        self.tracks
            .iter_mut()
            .find(|t| t.kind == TrackKind::Audio)
            .unwrap()
            .clips
            .push(Clip::Audio(clip));
        self.assets.push(asset);
    }
    pub fn validate(&self) -> AppResult<()> {
        super::effects::validate(self)?;
        super::automation::validate(self)?;
        if !self.master.volume_db.is_finite() || !(-96.0..=12.0).contains(&self.master.volume_db) {
            return Err(invalid("Master Volume 범위: -96~12 dB"));
        }
        if self.format != "MiniDAW" || self.schema_version != VERSION {
            return Err(invalid("format / schemaVersion"));
        }
        if self.assets.len() > 10_000
            || self.tracks.len() > 4096
            || self.tracks.iter().map(|t| t.clips.len()).sum::<usize>() > 100_000
            || self
                .midi_clips()
                .map(|c| c.notes.len() + c.controls.len())
                .sum::<usize>()
                > 100_000
        {
            return Err(invalid("객체 수 제한 초과"));
        }
        let mut ids = HashSet::new();
        let mut check_id = |s: &str| -> AppResult<()> {
            if uuid::Uuid::parse_str(s).is_err()
                || s.len() != 36
                || !ids.insert(s.to_ascii_lowercase())
            {
                return Err(invalid("중복되거나 잘못된 ID"));
            }
            Ok(())
        };
        check_id(&self.project_id)?;
        name(&self.name)?;
        let assets: HashSet<_> = self.assets.iter().map(|a| a.asset_id.as_str()).collect();
        for asset in &self.assets {
            check_id(&asset.asset_id)?;
            name(&asset.filename)?;
            super::paths::validate_reference(&asset.path)?;
            let m = &asset.metadata;
            if m.sample_rate == 0
                || m.sample_rate > 768_000
                || m.channels == 0
                || m.channels > 256
                || m.source_frames.0 == 0
            {
                return Err(invalid("오디오 metadata"));
            }
            name(&m.container)?;
            if let Some(codec) = &m.codec {
                name(codec)?;
            }
            if asset.fingerprint.sampled_sha256.len() != 64
                || !asset
                    .fingerprint
                    .sampled_sha256
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit())
            {
                return Err(invalid("fingerprint"));
            }
        }
        for track in &self.tracks {
            check_id(&track.track_id)?;
            name(&track.name)?;
            if !track.mix.volume_db.is_finite()
                || !(-96.0..=12.0).contains(&track.mix.volume_db)
                || !track.mix.pan.is_finite()
                || !(-1.0..=1.0).contains(&track.mix.pan)
            {
                return Err(invalid("Track Volume (-96~12 dB) / Pan (-1~1)"));
            }
            if track.kind != TrackKind::Midi && !track.instrument.is_none() {
                return Err(invalid("Instrument routing requires a MIDI Track"));
            }
            if track.instrument == Instrument::External {
                let plugin=crate::plugins::instrument(track).ok_or_else(||invalid("External Instrument 설정 없음"))?;plugin.validate()?;if !plugin.descriptor.instrument{return Err(invalid("External Instrument 종류 오류"));}
            }
            for clip in &track.clips {
                if let Clip::Midi(c) = clip {
                    if track.kind != TrackKind::Midi {
                        return Err(invalid("MIDI Clip / Track kind"));
                    }
                    check_id(&c.clip_id)?;
                    name(&c.name)?;
                    valid_tick_range(c.start_tick.0, c.length_tick.0)?;
                    let exact =
                        |v: i128| (-9_007_199_254_740_991..=9_007_199_254_740_991).contains(&v);
                    let offset = i128::from(c.content_offset_tick.0);
                    let origin = i128::from(c.start_tick.0) - offset;
                    if !exact(offset) || !exact(origin) {
                        return Err(invalid("MIDI content offset"));
                    }
                    for note in &c.notes {
                        check_id(&note.note_id)?;
                        let start = i128::from(note.start_tick.0);
                        let end = start + i128::from(note.length_tick.0);
                        if note.length_tick.0 <= 0
                            || !exact(start)
                            || !exact(end)
                            || !exact(start - offset)
                            || !exact(end - offset)
                            || !exact(origin + start)
                            || !exact(origin + end)
                        {
                            return Err(invalid("MIDI content note ticks"));
                        }
                        if note.pitch > 127
                            || note.velocity == 0
                            || note.velocity > 127
                            || note.release_velocity > 127
                            || note.channel > 15
                        {
                            return Err(invalid("MIDI note pitch / velocity / channel"));
                        }
                    }
                    for event in &c.controls {
                        check_id(&event.event_id)?;
                        if !exact(i128::from(event.tick.0))
                            || !exact(i128::from(event.tick.0) - offset)
                            || !exact(origin + i128::from(event.tick.0))
                            || event.channel > 15
                            || !event.data.valid()
                        {
                            return Err(invalid("MIDI controller range"));
                        }
                    }
                    continue;
                }
                if track.kind != TrackKind::Audio {
                    return Err(invalid("Audio Clip / Track kind"));
                }
                let c = clip.audio();
                check_id(&c.clip_id)?;
                name(&c.name)?;
                if let Some(value) = c.extensions.get(ENVELOPE_WINDOW) {
                    let w: EnvelopeWindow =
                        serde_json::from_value(value.clone()).map_err(invalid)?;
                    if w.source_start.0 >= w.source_end.0
                        || c.source_start.0 < w.source_start.0
                        || c.source_end.0 > w.source_end.0
                        || w.fade_in.source_frames.0 > w.source_end.0 - w.source_start.0
                        || w.fade_out.source_frames.0 > w.source_end.0 - w.source_start.0
                    {
                        return Err(invalid("split fade envelope window"));
                    }
                }
                if !assets.contains(c.asset_id.as_str()) {
                    return Err(invalid("clip이 존재하지 않는 assetId를 참조합니다."));
                }
                let meta=&self.assets.iter().find(|a|a.asset_id==c.asset_id).expect("asset").metadata;
                super::tempo_sync::validate(c,meta.source_frames.0,meta.sample_rate,&self.musical_time)?;
                super::glue::part(c)?;
                super::pitch::validate(c)?;
                super::stretch::validate(
                    c,
                    self.assets
                        .iter()
                        .find(|a| a.asset_id == c.asset_id)
                        .expect("checked asset")
                        .metadata
                        .source_frames
                        .0,
                )?;
                if c.source_start.0 >= c.source_end.0
                    || !c.gain.is_finite()
                    || c.gain < 0.0
                    || c.gain > 1.0e12
                    || c.fade_in.source_frames.0 > c.source_end.0 - c.source_start.0
                    || c.fade_out.source_frames.0 > c.source_end.0 - c.source_start.0
                {
                    return Err(invalid("clip source 범위 / gain / fade"));
                }
                if let Position::Seconds { denominator, .. } = c.position {
                    if denominator == 0 || denominator > MAX_TIME_DENOMINATOR {
                        return Err(invalid("timeline denominator"));
                    }
                }
            }
        }
        if self.primary_clip_id.is_some() && self.primary().is_none() {
            return Err(invalid("primaryClipId"));
        }
        let t = &self.musical_time;
        if t.ticks_per_quarter == 0
            || t.ticks_per_quarter > 1_000_000_000
            || t.tempo_map.is_empty()
            || t.tempo_map.len() > 4096
            || t.time_signatures.is_empty()
            || t.time_signatures.len() > 4096
        {
            return Err(invalid("musicalTime"));
        }
        if t.tempo_map[0].tick.0 != 0
            || t.time_signatures[0].tick.0 != 0
            || t.tempo_map
                .iter()
                .any(|t| !t.bpm.is_finite() || t.bpm <= 0.0 || t.bpm > 1000.0)
            || t.tempo_map.windows(2).any(|w| w[0].tick.0 >= w[1].tick.0)
            || t.time_signatures
                .windows(2)
                .any(|w| w[0].tick.0 >= w[1].tick.0)
            || t.time_signatures
                .iter()
                .any(|s| s.numerator == 0 || !s.denominator.is_power_of_two())
        {
            return Err(invalid("tempo / time signature map"));
        }
        for c in self.midi_clips() {
            let end = super::time::time(
                &Position::Ticks {
                    ticks: Signed(c.start_tick.0 + c.length_tick.0),
                },
                t,
            )
            .seconds();
            if !end.is_finite() || end > 7.0 * 86400.0 {
                return Err(invalid("MIDI Clip End must be within 7 days"));
            }
        }
        if let Some(c) = &self.cycle {
            if c.start_tick.0 < 0
                || c.end_tick.0 <= c.start_tick.0
                || c.end_tick.0 > 9_007_199_254_740_991
            {
                return Err(invalid("Cycle: 0 ≤ Start < End 범위가 필요합니다."));
            }
            let duration = super::time::time(&Position::Ticks { ticks: c.end_tick }, t).seconds();
            if !duration.is_finite() || duration > 7.0 * 86400.0 {
                return Err(invalid("Cycle End는 7일 이내여야 합니다."));
            }
        }
        Ok(())
    }
    pub fn preview_supported(&self, clip: &AudioClip) -> bool {
        let asset = self.assets.iter().find(|a| a.asset_id == clip.asset_id);
        asset.is_some_and(|a| {
            clip.source_start.0 == 0
                && clip.source_end == a.metadata.source_frames
                && a.extensions.is_empty()
        }) && clip.position.is_zero()
            && clip.gain == 1.0
            && !clip.mute
            && clip.fade_in.source_frames.0 == 0
            && clip.fade_out.source_frames.0 == 0
            && clip.extensions.is_empty()
            && self.extensions.is_empty()
            && self.tracks.iter().all(|t| t.extensions.is_empty())
            && self.tracks.iter().all(|t| t.mix.is_default())
    }
}
fn name(s: &str) -> AppResult<()> {
    if s.is_empty() || s.len() > 32768 || s.contains('\0') {
        Err(invalid("이름 길이 / NUL"))
    } else {
        Ok(())
    }
}

pub fn valid_tick_range(start: i64, length: i64) -> AppResult<()> {
    if start < 0
        || length <= 0
        || start
            .checked_add(length)
            .is_none_or(|end| end > 9_007_199_254_740_991)
    {
        return Err(invalid("MIDI tick range"));
    }
    Ok(())
}
