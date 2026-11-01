//! SFZ instruments: parsing and sample decoding on control threads, then an
//! immutable `Bank` shared by Arc. The cache keeps one owner of every Bank and
//! releases it only when it is the last one, so the audio callback (which only
//! clones/drops Arcs held by a plan) never frees sample memory.
use crate::error::{AppError, AppResult};
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

const MAX_DEPTH: usize = 16;
const MAX_REGIONS: usize = 100_000;
// Decoded PCM budget. 16-bit sources stay 16-bit in memory.
const MAX_BYTES: usize = 8 << 30;

pub enum Data {
    I16(Box<[i16]>),
    F32(Box<[f32]>),
}
pub struct Sample {
    pub rate: f64,
    pub channels: usize,
    pub frames: usize,
    data: Data,
}
impl Sample {
    #[inline]
    fn at(&self, i: usize) -> f32 {
        match &self.data {
            Data::I16(d) => f32::from(d[i]) / 32768.0,
            Data::F32(d) => d[i],
        }
    }
    /// Stereo frame; silence outside the sample. Mono feeds both channels.
    #[inline]
    pub fn frame(&self, frame: usize) -> [f32; 2] {
        if frame >= self.frames {
            return [0.0; 2];
        }
        let i = frame * self.channels;
        if self.channels == 1 {
            let v = self.at(i);
            [v, v]
        } else {
            [self.at(i), self.at(i + 1)]
        }
    }
    fn bytes(&self) -> usize {
        match &self.data {
            Data::I16(d) => d.len() * 2,
            Data::F32(d) => d.len() * 4,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    Attack,
    Release,
    First,
    Legato,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopMode {
    NoLoop,
    OneShot,
    Continuous,
    Sustain,
}

#[derive(Debug, Clone)]
pub struct Region {
    pub sample: usize,
    /// Negative bounds disable note triggering (CC-triggered regions).
    pub lokey: i16,
    pub hikey: i16,
    pub lovel: u8,
    pub hivel: u8,
    pub keycenter: f64,
    /// Cents per key.
    pub keytrack: f64,
    /// tune + transpose, cents.
    pub tune: f64,
    pub gain: f64,
    pub pan: [f64; 2],
    pub veltrack: f64,
    pub trigger: Trigger,
    /// dB per second the note was held (release triggers).
    pub rt_decay: f64,
    pub lorand: f64,
    pub hirand: f64,
    pub seq_length: u32,
    pub seq_position: u32,
    pub group: i64,
    pub off_by: Option<i64>,
    pub off_fast: bool,
    pub on_cc: Option<(u8, u8, u8)>,
    pub cc: Vec<(u8, u8, u8)>,
    pub sw_last: Option<u8>,
    pub offset: usize,
    pub end: usize,
    pub loop_mode: LoopMode,
    pub loop_start: usize,
    pub loop_end: usize,
    pub delay: f64,
    pub attack: f64,
    pub hold: f64,
    pub decay: f64,
    pub sustain: f64,
    pub release: f64,
    pub bend_up: f64,
    pub bend_down: f64,
}
impl Region {
    /// Note-triggered; CC-triggered regions (on_ccN) never respond to keys.
    pub fn plays_key(&self, key: u8) -> bool {
        self.on_cc.is_none()
            && self.lokey >= 0
            && self.hikey >= 0
            && (self.lokey..=self.hikey).contains(&i16::from(key))
    }
}

pub struct Bank {
    pub path: PathBuf,
    pub name: String,
    pub regions: Vec<Region>,
    pub samples: Vec<Sample>,
    /// Keyswitch range; keys inside it select `sw_last` regions.
    pub sw: Option<(u8, u8)>,
    pub sw_default: Option<u8>,
    pub cc_init: Vec<(u8, u8)>,
    pub unsupported: Vec<(String, usize)>,
    pub missing: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub path: String,
    pub name: String,
    pub loaded: bool,
    pub regions: usize,
    pub samples: usize,
    pub bytes: usize,
    pub unsupported: Vec<(String, usize)>,
    pub missing: Vec<String>,
    pub error: Option<AppError>,
}

fn error(message: impl Into<String>) -> AppError {
    AppError::new("sfz", message)
}

/// Middle C = c4 = 60, as in the SFZ format.
fn note(v: &str) -> Option<i16> {
    if let Ok(n) = v.parse::<i16>() {
        return Some(n);
    }
    let v = v.to_ascii_lowercase();
    let mut c = v.chars();
    let base = match c.next()? {
        'c' => 0,
        'd' => 2,
        'e' => 4,
        'f' => 5,
        'g' => 7,
        'a' => 9,
        'b' => 11,
        _ => return None,
    };
    let rest: String = c.collect();
    let (shift, octave) = if let Some(r) = rest.strip_prefix('#') {
        (1, r)
    } else if let Some(r) = rest.strip_prefix('♯') {
        (1, r)
    } else if let Some(r) = rest.strip_prefix('b') {
        (-1, r)
    } else if let Some(r) = rest.strip_prefix('♭') {
        (-1, r)
    } else {
        (0, rest.as_str())
    };
    let octave: i16 = octave.parse().ok()?;
    Some((octave + 1) * 12 + base + shift)
}

/// Comments removed, `#define` substituted and `#include` expanded.
fn preprocess(
    path: &Path,
    root: &Path,
    depth: usize,
    defines: &mut Vec<(String, String)>,
    out: &mut String,
) -> AppResult<()> {
    if depth > MAX_DEPTH {
        return Err(error("SFZ #include가 너무 깊습니다."));
    }
    let bytes = std::fs::read(path)
        .map_err(|e| AppError::new("file", "SFZ 파일을 읽을 수 없습니다.").detail(format!("{}: {e}", path.display())))?;
    let text = String::from_utf8_lossy(&bytes);
    let mut clean = String::with_capacity(text.len());
    let mut rest = text.as_ref();
    while !rest.is_empty() {
        let line = rest.find("//");
        let block = rest.find("/*");
        match (line, block) {
            (Some(l), b) if b.is_none_or(|b| l < b) => {
                clean.push_str(&rest[..l]);
                rest = rest[l..].find('\n').map_or("", |n| &rest[l + n..]);
            }
            (_, Some(b)) => {
                clean.push_str(&rest[..b]);
                rest = rest[b + 2..].find("*/").map_or("", |e| &rest[b + 2 + e + 2..]);
            }
            _ => {
                clean.push_str(rest);
                rest = "";
            }
        }
    }
    for line in clean.lines() {
        let trimmed = line.trim();
        if let Some(d) = trimmed.strip_prefix("#define") {
            let mut parts = d.trim().splitn(2, char::is_whitespace);
            if let (Some(name), Some(value)) = (parts.next(), parts.next()) {
                defines.retain(|(n, _)| n != name);
                defines.push((name.to_owned(), value.trim().to_owned()));
                // Longest names first so $A never replaces part of $AB.
                defines.sort_by_key(|d| std::cmp::Reverse(d.0.len()));
            }
            continue;
        }
        let mut line = line.to_owned();
        for (name, value) in defines.iter() {
            if line.contains(name.as_str()) {
                line = line.replace(name.as_str(), value);
            }
        }
        if let Some(inc) = line.trim().strip_prefix("#include") {
            let file = inc.trim().trim_matches('"').replace('\\', "/");
            preprocess(&root.join(file), root, depth + 1, defines, out)?;
            continue;
        }
        out.push_str(&line);
        out.push('\n');
    }
    Ok(())
}

type Opcodes = BTreeMap<String, String>;
/// Headers and opcodes in file order. Values may contain spaces (sample paths):
/// a value runs until the next `name=` word or header.
fn tokens(text: &str) -> Vec<(String, Opcodes)> {
    let spaced = text.replace('<', " <").replace('>', "> ");
    let mut out: Vec<(String, Opcodes)> = vec![("control".into(), Opcodes::new())];
    let mut last: Option<String> = None;
    for word in spaced.split_whitespace() {
        if let Some(h) = word.strip_prefix('<').and_then(|w| w.strip_suffix('>')) {
            out.push((h.to_ascii_lowercase(), Opcodes::new()));
            last = None;
            continue;
        }
        let opcode = word
            .split_once('=')
            .filter(|(k, _)| !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
        let map = &mut out.last_mut().unwrap().1;
        if let Some((k, v)) = opcode {
            let k = k.to_ascii_lowercase();
            map.insert(k.clone(), v.to_owned());
            last = Some(k);
        } else if let Some(k) = &last {
            let v = map.get_mut(k).unwrap();
            v.push(' ');
            v.push_str(word);
        }
    }
    out
}

struct Ctx {
    default_path: String,
    note_offset: i16,
    unsupported: BTreeMap<String, usize>,
}
fn cc_index(key: &str, prefix: &str) -> Option<u8> {
    key.strip_prefix(prefix)?.parse::<u8>().ok().filter(|n| *n < 128)
}
fn number(v: &str) -> Option<f64> {
    v.trim().parse::<f64>().ok().filter(|n| n.is_finite())
}

const IGNORED: &[&str] = &[
    // Metadata / host hints that do not affect sound.
    "sw_label", "hint_ram_based", "hint_stealing", "note_polyphony", "polyphony", "image",
    "global_label", "master_label", "group_label", "region_label", "key_ranges",
];

struct Pending {
    sample: String,
    region: Region,
}

fn region(map: &Opcodes, ctx: &mut Ctx) -> Option<Pending> {
    let mut r = Region {
        sample: 0,
        lokey: 0,
        hikey: 127,
        lovel: 1,
        hivel: 127,
        keycenter: 60.0,
        keytrack: 100.0,
        tune: 0.0,
        gain: 1.0,
        pan: [1.0; 2],
        veltrack: 1.0,
        trigger: Trigger::Attack,
        rt_decay: 0.0,
        lorand: 0.0,
        hirand: 1.0,
        seq_length: 1,
        seq_position: 1,
        group: 0,
        off_by: None,
        off_fast: true,
        on_cc: None,
        cc: vec![],
        sw_last: None,
        offset: 0,
        end: usize::MAX,
        loop_mode: LoopMode::NoLoop,
        loop_start: 0,
        loop_end: usize::MAX,
        delay: 0.0,
        attack: 0.0,
        hold: 0.0,
        decay: 0.0,
        sustain: 1.0,
        release: 0.0,
        bend_up: 200.0,
        bend_down: -200.0,
    };
    let key = |v: &str, ctx: &Ctx| note(v).map(|n| if n < 0 { n } else { n + ctx.note_offset });
    let (mut volume, mut amplitude, mut pan, mut transpose) = (0.0, 100.0, 0.0, 0.0);
    let (mut on_lo, mut on_hi) = (BTreeMap::<u8, u8>::new(), BTreeMap::<u8, u8>::new());
    let (mut lo, mut hi) = (BTreeMap::<u8, u8>::new(), BTreeMap::<u8, u8>::new());
    let mut sample = None;
    let mut keyswitch = false;
    for (k, v) in map {
        let n = number(v);
        let known = match k.as_str() {
            "sample" => {
                sample = Some(v.trim().to_owned());
                true
            }
            "lokey" => key(v, ctx).map(|x| r.lokey = x).is_some(),
            "hikey" => key(v, ctx).map(|x| r.hikey = x).is_some(),
            "key" => key(v, ctx)
                .map(|x| {
                    r.lokey = x;
                    r.hikey = x;
                    r.keycenter = f64::from(x);
                })
                .is_some(),
            "pitch_keycenter" => key(v, ctx).map(|x| r.keycenter = f64::from(x)).is_some(),
            "lovel" => n.map(|x| r.lovel = x.clamp(0.0, 127.0) as u8).is_some(),
            "hivel" => n.map(|x| r.hivel = x.clamp(0.0, 127.0) as u8).is_some(),
            "lorand" => n.map(|x| r.lorand = x).is_some(),
            "hirand" => n.map(|x| r.hirand = x).is_some(),
            "seq_length" => n.map(|x| r.seq_length = x.max(1.0) as u32).is_some(),
            "seq_position" => n.map(|x| r.seq_position = x.max(1.0) as u32).is_some(),
            "trigger" => {
                r.trigger = match v.trim() {
                    "release" => Trigger::Release,
                    "first" => Trigger::First,
                    "legato" => Trigger::Legato,
                    _ => Trigger::Attack,
                };
                matches!(v.trim(), "attack" | "release" | "first" | "legato")
            }
            "rt_decay" => n.map(|x| r.rt_decay = x.max(0.0)).is_some(),
            "volume" => n.map(|x| volume = x).is_some(),
            "amplitude" => n.map(|x| amplitude = x).is_some(),
            "pan" => n.map(|x| pan = x.clamp(-100.0, 100.0)).is_some(),
            "amp_veltrack" => n.map(|x| r.veltrack = x.clamp(-100.0, 100.0) / 100.0).is_some(),
            "tune" | "pitch" => n.map(|x| r.tune = x).is_some(),
            "transpose" => n.map(|x| transpose = x).is_some(),
            "pitch_keytrack" => n.map(|x| r.keytrack = x).is_some(),
            "ampeg_delay" => n.map(|x| r.delay = x.max(0.0)).is_some(),
            "ampeg_attack" => n.map(|x| r.attack = x.max(0.0)).is_some(),
            "ampeg_hold" => n.map(|x| r.hold = x.max(0.0)).is_some(),
            "ampeg_decay" => n.map(|x| r.decay = x.max(0.0)).is_some(),
            "ampeg_sustain" => n.map(|x| r.sustain = x.clamp(0.0, 100.0) / 100.0).is_some(),
            "ampeg_release" => n.map(|x| r.release = x.max(0.0)).is_some(),
            "offset" => n.map(|x| r.offset = x.max(0.0) as usize).is_some(),
            "end" => n.map(|x| r.end = if x < 0.0 { 0 } else { x as usize }).is_some(),
            "loop_mode" | "loopmode" => {
                r.loop_mode = match v.trim() {
                    "one_shot" => LoopMode::OneShot,
                    "loop_continuous" => LoopMode::Continuous,
                    "loop_sustain" => LoopMode::Sustain,
                    _ => LoopMode::NoLoop,
                };
                true
            }
            "loop_start" | "loopstart" => n.map(|x| r.loop_start = x.max(0.0) as usize).is_some(),
            "loop_end" | "loopend" => n.map(|x| r.loop_end = x.max(0.0) as usize).is_some(),
            "group" => n.map(|x| r.group = x as i64).is_some(),
            "off_by" => n.map(|x| r.off_by = Some(x as i64)).is_some(),
            "off_mode" => {
                r.off_fast = v.trim() != "normal";
                true
            }
            "sw_last" => key(v, ctx).map(|x| r.sw_last = Some(x.clamp(0, 127) as u8)).is_some(),
            "sw_lokey" | "sw_hikey" | "sw_default" => {
                keyswitch = true;
                true
            }
            "bend_up" => n.map(|x| r.bend_up = x).is_some(),
            "bend_down" => n.map(|x| r.bend_down = x).is_some(),
            _ => {
                if let Some(c) = cc_index(k, "on_locc") {
                    n.map(|x| on_lo.insert(c, x.clamp(0.0, 127.0) as u8)).is_some()
                } else if let Some(c) = cc_index(k, "on_hicc") {
                    n.map(|x| on_hi.insert(c, x.clamp(0.0, 127.0) as u8)).is_some()
                } else if let Some(c) = cc_index(k, "locc") {
                    n.map(|x| lo.insert(c, x.clamp(0.0, 127.0) as u8)).is_some()
                } else if let Some(c) = cc_index(k, "hicc") {
                    n.map(|x| hi.insert(c, x.clamp(0.0, 127.0) as u8)).is_some()
                } else {
                    IGNORED.contains(&k.as_str()) || cc_index(k, "set_cc").is_some() || k == "default_path"
                        || k == "note_offset" || k == "octave_offset"
                }
            }
        };
        if !known {
            *ctx.unsupported.entry(k.clone()).or_default() += 1;
        }
    }
    let _ = keyswitch;
    r.tune += transpose * 100.0;
    r.gain = 10f64.powf(volume / 20.0) * (amplitude / 100.0).max(0.0);
    r.pan = [(1.0 - pan / 100.0).min(1.0), (1.0 + pan / 100.0).min(1.0)];
    if let Some((&c, _)) = on_lo.iter().next().or(on_hi.iter().next()) {
        r.on_cc = Some((c, on_lo.get(&c).copied().unwrap_or(0), on_hi.get(&c).copied().unwrap_or(127)));
    }
    for c in lo.keys().chain(hi.keys()).copied().collect::<std::collections::BTreeSet<_>>() {
        r.cc.push((c, lo.get(&c).copied().unwrap_or(0), hi.get(&c).copied().unwrap_or(127)));
    }
    let sample = sample?;
    if sample.starts_with('*') {
        *ctx.unsupported.entry(format!("sample={sample}")).or_default() += 1;
        return None;
    }
    Some(Pending {
        sample: format!("{}{}", ctx.default_path, sample).replace('\\', "/"),
        region: r,
    })
}

/// Later levels override earlier ones. `key=` is shorthand for lokey/hikey/
/// pitch_keycenter at its own level, so an inherited lokey never overrides a
/// region's own key (and vice versa), independent of opcode order.
fn merge(into: &mut Opcodes, level: &Opcodes) {
    if let Some(k) = level.get("key") {
        for name in ["lokey", "hikey", "pitch_keycenter"] {
            into.insert(name.into(), level.get(name).unwrap_or(k).clone());
        }
    }
    for (k, v) in level {
        if k != "key" {
            into.insert(k.clone(), v.clone());
        }
    }
}

fn decode(path: &Path) -> AppResult<Sample> {
    let audio = super::decoder::decode(path)?;
    let channels = audio.info.channels;
    // 16-bit sources are exact multiples of 1/32768 after decoding: keep them
    // 16-bit in memory (half the RAM), losslessly. Others stay 32-bit float.
    let exact = audio
        .samples
        .iter()
        .all(|s| (s * 32768.0).fract() == 0.0 && (-32768.0..32768.0).contains(&(s * 32768.0)));
    let data = if exact {
        Data::I16(audio.samples.iter().map(|s| (s * 32768.0) as i16).collect())
    } else {
        Data::F32(audio.samples.into_boxed_slice())
    };
    Ok(Sample {
        rate: f64::from(audio.info.sample_rate),
        channels,
        frames: audio.info.frames,
        data,
    })
}

pub fn load(path: &Path) -> AppResult<Bank> {
    if !path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("sfz"))
    {
        return Err(error("SFZ 파일(.sfz)을 선택해 주세요."));
    }
    let root = path.parent().unwrap_or(Path::new("."));
    let mut text = String::new();
    preprocess(path, root, 0, &mut vec![], &mut text)?;
    let mut ctx = Ctx {
        default_path: String::new(),
        note_offset: 0,
        unsupported: BTreeMap::new(),
    };
    let (mut global, mut master, mut group) = (Opcodes::new(), Opcodes::new(), Opcodes::new());
    let mut pending = vec![];
    let mut cc_init = vec![];
    let (mut sw_lo, mut sw_hi, mut sw_default) = (None::<i16>, None::<i16>, None::<i16>);
    for (header, map) in tokens(&text) {
        // Keyswitch bounds may live on any level; the widest declared range wins.
        for (k, v) in &map {
            let n = note(v);
            match k.as_str() {
                "sw_lokey" => sw_lo = n.map(|n| sw_lo.map_or(n, |o| o.min(n))),
                "sw_hikey" => sw_hi = n.map(|n| sw_hi.map_or(n, |o| o.max(n))),
                "sw_default" => sw_default = n,
                _ => {}
            }
            if let (Some(c), Some(x)) = (cc_index(k, "set_cc"), number(v)) {
                cc_init.push((c, x.clamp(0.0, 127.0) as u8));
            }
        }
        match header.as_str() {
            "control" => {
                if let Some(p) = map.get("default_path") {
                    ctx.default_path = p.trim().replace('\\', "/");
                }
                let notes = map.get("note_offset").and_then(|v| number(v)).unwrap_or(0.0);
                let octaves = map.get("octave_offset").and_then(|v| number(v)).unwrap_or(0.0);
                ctx.note_offset = (notes + octaves * 12.0) as i16;
                for k in map.keys() {
                    if !matches!(k.as_str(), "default_path" | "note_offset" | "octave_offset")
                        && cc_index(k, "set_cc").is_none()
                        && !k.starts_with("label_cc")
                    {
                        *ctx.unsupported.entry(k.clone()).or_default() += 1;
                    }
                }
            }
            "global" => {
                global = map;
                master.clear();
                group.clear();
            }
            "master" => {
                master = map;
                group.clear();
            }
            "group" => group = map,
            "region" => {
                let mut merged = Opcodes::new();
                for level in [&global, &master, &group, &map] {
                    merge(&mut merged, level);
                }
                if let Some(p) = region(&merged, &mut ctx) {
                    pending.push(p);
                }
                if pending.len() > MAX_REGIONS {
                    return Err(error("SFZ Region이 너무 많습니다."));
                }
            }
            other => *ctx.unsupported.entry(format!("<{other}>")).or_default() += 1,
        }
    }
    // Unique sample files, decoded in parallel.
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut files: Vec<PathBuf> = vec![];
    for p in &pending {
        if let std::collections::hash_map::Entry::Vacant(e) = index.entry(p.sample.to_ascii_lowercase()) {
            e.insert(files.len());
            files.push(root.join(&p.sample));
        }
    }
    let total: u64 = files
        .iter()
        .filter_map(|f| std::fs::metadata(f).ok())
        .map(|m| m.len())
        .sum();
    if total as usize > MAX_BYTES {
        return Err(error("SFZ 샘플이 너무 큽니다 (8 GB 초과)."));
    }
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(1, 8);
    let mut decoded: Vec<Option<Result<Sample, String>>> = (0..files.len()).map(|_| None).collect();
    std::thread::scope(|s| {
        let chunk = files.len().div_ceil(workers).max(1);
        let handles: Vec<_> = files
            .chunks(chunk)
            .enumerate()
            .map(|(n, part)| (n * chunk, s.spawn(move || part.iter().map(|f| decode(f).map_err(|e| e.to_string())).collect::<Vec<_>>())))
            .collect();
        for (start, h) in handles {
            // A failed worker leaves its files as missing, never shifts others.
            for (i, r) in h.join().unwrap_or_default().into_iter().enumerate() {
                decoded[start + i] = Some(r);
            }
        }
    });
    let mut samples = vec![];
    let mut slot = vec![None; files.len()];
    let mut missing = vec![];
    for (i, d) in decoded.into_iter().enumerate() {
        match d {
            Some(Ok(s)) => {
                slot[i] = Some(samples.len());
                samples.push(s);
            }
            _ => missing.push(
                files[i]
                    .strip_prefix(root)
                    .unwrap_or(&files[i])
                    .display()
                    .to_string(),
            ),
        }
    }
    let mut regions = vec![];
    for p in pending {
        if let Some(s) = slot[index[&p.sample.to_ascii_lowercase()]] {
            let mut r = p.region;
            r.sample = s;
            let frames = samples[s].frames;
            r.end = r.end.min(frames.saturating_sub(1));
            r.loop_end = r.loop_end.min(r.end);
            if r.loop_start >= r.loop_end {
                r.loop_mode = match r.loop_mode {
                    LoopMode::Continuous | LoopMode::Sustain => LoopMode::NoLoop,
                    m => m,
                };
            }
            regions.push(r);
        }
    }
    if regions.is_empty() {
        return Err(error("재생할 수 있는 SFZ Region이 없습니다.")
            .detail(missing.first().cloned().unwrap_or_default()));
    }
    let sw = match (sw_lo, sw_hi) {
        (Some(a), Some(b)) if a >= 0 && b >= a => Some((a.min(127) as u8, b.min(127) as u8)),
        _ => None,
    };
    Ok(Bank {
        name: path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        path: path.to_path_buf(),
        regions,
        samples,
        sw,
        sw_default: sw_default.filter(|n| (0..128).contains(n)).map(|n| n as u8),
        cc_init,
        unsupported: ctx.unsupported.into_iter().collect(),
        missing,
    })
}

impl Bank {
    pub fn bytes(&self) -> usize {
        self.samples.iter().map(Sample::bytes).sum()
    }
}

struct Cache {
    banks: HashMap<String, Result<Arc<Bank>, AppError>>,
}
fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(Cache { banks: HashMap::new() }))
}
fn loading() -> &'static Mutex<()> {
    static LOADING: OnceLock<Mutex<()>> = OnceLock::new();
    LOADING.get_or_init(|| Mutex::new(()))
}
fn key(path: &str) -> String {
    path.replace('/', "\\").to_lowercase()
}

/// Cached Bank, loading it on first use (control/worker threads only). A load
/// failure is remembered and reported by `status`; the Track stays silent.
pub fn bank(path: &str) -> Option<Arc<Bank>> {
    let k = key(path);
    if let Some(r) = cache().lock().ok()?.banks.get(&k) {
        return r.as_ref().ok().cloned();
    }
    let _serial = loading().lock().ok()?;
    if let Some(r) = cache().lock().ok()?.banks.get(&k) {
        return r.as_ref().ok().cloned();
    }
    let result = load(Path::new(path)).map(Arc::new);
    let out = result.as_ref().ok().cloned();
    cache().lock().ok()?.banks.insert(k, result);
    out
}

/// Forget a failed load or an unused Bank so the next use reads the file again.
pub fn forget(path: &str) {
    if let Ok(mut c) = cache().lock() {
        let k = key(path);
        if c.banks.get(&k).is_some_and(|r| r.as_ref().map_or(true, |b| Arc::strong_count(b) == 1)) {
            c.banks.remove(&k);
        }
    }
}

/// Release Banks no document uses and nothing else holds (control thread).
pub fn release_unused(used: &[&str]) {
    let used: Vec<String> = used.iter().map(|p| key(p)).collect();
    if let Ok(mut c) = cache().lock() {
        c.banks.retain(|k, r| {
            used.contains(k) || r.as_ref().is_ok_and(|b| Arc::strong_count(b) > 1)
        });
    }
}

pub fn status(path: &str) -> Status {
    let entry = cache().lock().ok().and_then(|c| c.banks.get(&key(path)).cloned());
    let name = Path::new(path)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    match entry {
        Some(Ok(b)) => Status {
            path: path.into(),
            name,
            loaded: true,
            regions: b.regions.len(),
            samples: b.samples.len(),
            bytes: b.bytes(),
            unsupported: b.unsupported.clone(),
            missing: b.missing.clone(),
            error: None,
        },
        other => Status {
            path: path.into(),
            name,
            loaded: false,
            regions: 0,
            samples: 0,
            bytes: 0,
            unsupported: vec![],
            missing: vec![],
            error: other.and_then(Result::err),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn note_names_use_sfz_middle_c() {
        assert_eq!(note("c4"), Some(60));
        assert_eq!(note("C#4"), Some(61));
        assert_eq!(note("db4"), Some(61));
        assert_eq!(note("a0"), Some(21));
        assert_eq!(note("c-1"), Some(0));
        assert_eq!(note("-1"), Some(-1));
        assert_eq!(note("x"), None);
    }
    #[test]
    fn values_keep_spaces_and_headers_need_no_spaces() {
        let t = tokens("<group>volume=-3 <region>sample=My Piano\\C4 v1.wav lokey=c4\n<region> key=61 sample=a.wav");
        assert_eq!(t[1].0, "group");
        assert_eq!(t[2].1["sample"], "My Piano\\C4 v1.wav");
        assert_eq!(t[2].1["lokey"], "c4");
        assert_eq!(t[3].1["key"], "61");
    }
}
