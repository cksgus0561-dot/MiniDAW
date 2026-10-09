//! Windows plugin lifecycle, discovery and immutable project settings.
//! Native lifecycle/UI calls run on the bridge's message thread, never the callback.
use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use std::{
    cell::Cell,
    collections::HashMap,
    ffi::{c_char, c_void, CStr, CString},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};

pub const INSTRUMENT: &str = "minidaw.plugin.v1";
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Descriptor {
    pub path: String,
    pub format: String,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub vendor: String,
    pub instrument: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Parameter {
    pub id: u32,
    pub name: String,
    pub min: f64,
    pub max: f64,
    pub value: f64,
    pub stepped: bool,
    #[serde(default)]
    pub step: f64,
    pub readonly: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Selection {
    pub descriptor: Descriptor,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub parameters: Vec<Parameter>,
    #[serde(default)]
    pub latency: u32,
    #[serde(default)]
    pub tail: u32,
    #[serde(default = "default_rate")]
    pub sample_rate: u32,
}
fn default_rate() -> u32 {
    48000
}
impl Selection {
    pub fn validate(&self) -> AppResult<()> {
        let d = &self.descriptor;
        if !["vst3", "clap"].contains(&d.format.as_str())
            || d.id.is_empty()
            || d.id.len() > 1024
            || d.name.len() > 1024
            || !Path::new(&d.path).is_absolute()
            || self.state.len() > 64 * 1024 * 1024
            || !self.state.len().is_multiple_of(2)
            || !self.state.bytes().all(|c| c.is_ascii_hexdigit())
            || !(8000..=384000).contains(&self.sample_rate)
            || self.parameters.len() > 16384
            || self.parameters.iter().any(|p| {
                !p.step.is_finite()
                    || p.step < 0.
                    || !p.min.is_finite()
                    || !p.max.is_finite()
                    || p.min > p.max
                    || !p.value.is_finite()
                    || p.value < p.min
                    || p.value > p.max
            })
        {
            return Err(error("Plugin 설정 형식/범위 오류"));
        }
        crate::project::paths::validate_path(&d.path)?;
        Ok(())
    }
    pub fn tail_seconds(&self, _rate: u32) -> f64 {
        (self.tail as f64 / self.sample_rate as f64).min(30.)
    }
}
pub fn instrument(t: &crate::project::schema::Track) -> Option<Selection> {
    t.extensions
        .get(INSTRUMENT)
        .and_then(|v| serde_json::from_value(v.clone()).ok())
}
fn error(s: impl std::fmt::Display) -> AppError {
    AppError::new("plugin", s.to_string())
}
#[cfg(windows)]
unsafe extern "C" {
    fn md_plugin_string_free(p: *mut c_char);
    fn md_plugin_scan(path: *const c_char, format: *const c_char) -> *mut c_char;
    fn md_plugin_create(
        path: *const c_char,
        format: *const c_char,
        id: *const c_char,
        rate: f64,
        state: *const c_char,
        name: *const c_char,
        instrument: bool,
        offline: bool,
        error: *mut *mut c_char,
    ) -> *mut c_void;
    fn md_plugin_destroy(p: *mut c_void);
    fn md_plugin_dirty(p: *mut c_void) -> bool;
    fn md_plugin_latency(p: *mut c_void) -> u32;
    fn md_plugin_prime(p: *mut c_void);
    fn md_plugin_latency_pending(p: *mut c_void) -> bool;
    fn md_plugin_poll_latency(p: *mut c_void);
    fn md_plugin_fault(p: *mut c_void) -> u32;
    fn md_plugin_status(p: *mut c_void) -> *mut c_char;
    fn md_plugin_info(p: *mut c_void) -> *mut c_char;
    fn md_plugin_state(p: *mut c_void) -> *mut c_char;
    fn md_plugin_editor(p: *mut c_void, show: bool, owner: usize) -> *mut c_char;
    fn md_plugin_process(
        p: *mut c_void,
        pair: *mut f32,
        frame: i64,
        bpm: f64,
        num: i32,
        den: i32,
        playing: bool,
    ) -> bool;
    fn md_plugin_midi(p: *mut c_void, s: u8, a: u8, b: u8, voice: u32);
    fn md_plugin_initial_parameter(p: *mut c_void, id: u32, v: f64);
    fn md_plugin_parameter(p: *mut c_void, id: u32, v: f64);
    fn md_plugin_reset(p: *mut c_void);
}
fn c(s: &str) -> AppResult<CString> {
    CString::new(s).map_err(error)
}
unsafe fn text(p: *mut c_char) -> AppResult<serde_json::Value> {
    if p.is_null() {
        return Err(error("Plugin host 응답 없음"));
    }
    let s = CStr::from_ptr(p).to_string_lossy().into_owned();
    md_plugin_string_free(p);
    let value: serde_json::Value = serde_json::from_str(&s).map_err(error)?;
    if let Some(e) = value.get("error").and_then(|v| v.as_str()) {
        Err(error(e))
    } else {
        Ok(value)
    }
}
pub fn scan_one(path: &Path, format: &str) -> AppResult<Vec<Descriptor>> {
    let path = crate::project::paths::display(path);
    let cp = c(&path)?;
    let cf = c(format)?;
    let value = unsafe { text(md_plugin_scan(cp.as_ptr(), cf.as_ptr()))? };
    value
        .as_array()
        .ok_or_else(|| error("Plugin scan 형식"))?
        .iter()
        .map(|v| {
            let mut v = v.clone();
            v["path"] = path.clone().into();
            v["format"] = format.into();
            serde_json::from_value(v).map_err(error)
        })
        .collect()
}
struct Native {
    ptr: *mut c_void,
    selection: Selection,
    tail: u32,
}
// Processing has exactly one mutable Instance owner. Main-thread operations are
// marshalled by C++ and use only SDK-designated main-thread APIs. Drop is off RT.
unsafe impl Send for Native {}
unsafe impl Sync for Native {}
impl Drop for Native {
    fn drop(&mut self) {
        unsafe { md_plugin_destroy(self.ptr) }
    }
}
type Registry = HashMap<String, Vec<Weak<Native>>>;
fn registry() -> &'static Mutex<Registry> {
    static R: OnceLock<Mutex<Registry>> = OnceLock::new();
    R.get_or_init(Default::default)
}
thread_local! {static OFFLINE:Cell<bool>=const{Cell::new(false)};static FAILED:Cell<bool>=const{Cell::new(false)};static FAILED_KIND:Cell<u32>=const{Cell::new(0)};static LOAD_ERROR:std::cell::RefCell<Option<String>>=const{std::cell::RefCell::new(None)};}
pub struct OfflineScope(bool);
impl OfflineScope {
    pub fn new() -> Self {
        FAILED.set(false);
        FAILED_KIND.set(0);
        LOAD_ERROR.with(|e| *e.borrow_mut() = None);
        Self(OFFLINE.replace(true))
    }
}
impl Default for OfflineScope {
    fn default() -> Self {
        Self::new()
    }
}
impl Drop for OfflineScope {
    fn drop(&mut self) {
        OFFLINE.set(self.0);
    }
}
pub fn check_offline() -> AppResult<()> {
    if FAILED.get() {
        Err(error(
            LOAD_ERROR.with(|e| e.borrow().clone()).unwrap_or_else(|| {
                if FAILED_KIND.get() == 2 {
                    "Plugin I/O 또는 Parameter 구조가 변경됐습니다. 프로젝트를 다시 열어 주세요."
                        .into()
                } else {
                    "Offline Plugin DSP 처리 실패. Plugin 상태를 확인해 주세요.".into()
                }
            }),
        ))
    } else {
        Ok(())
    }
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatus {
    pub instance_id: String,
    pub name: String,
    pub error: Option<String>,
    pub info: serde_json::Value,
}
fn failures() -> &'static Mutex<HashMap<String, String>> {
    static R: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    R.get_or_init(Default::default)
}
pub struct Instance {
    native: Option<Arc<Native>>,
    pub failure: Option<String>,
    pub tail: usize,
    frame: usize,
    bpm: f64,
    num: i32,
    den: i32,
    playing: bool,
}
impl Instance {
    pub fn new(selection: &Selection, key: &str, rate: u32, bpm: f64) -> Self {
        let mut out = Self {
            native: None,
            failure: None,
            tail: 0,
            frame: 0,
            bpm,
            num: 4,
            den: 4,
            playing: true,
        };
        match Self::load(selection, rate) {
            Ok(n) => {
                out.tail = (n.tail as usize).min(rate as usize * 30);
                if !OFFLINE.get() {
                    let mut r = registry().lock().unwrap_or_else(|e| e.into_inner());
                    let list = r.entry(key.into()).or_default();
                    list.retain(|w| w.strong_count() > 0);
                    list.push(Arc::downgrade(&n));
                    failures().lock().unwrap().remove(key);
                }
                out.native = Some(n);
            }
            Err(e) => {
                out.failure = Some(e.message.clone());
                if OFFLINE.get() {
                    FAILED.set(true);
                    LOAD_ERROR.with(|x| *x.borrow_mut() = Some(e.message));
                } else {
                    failures().lock().unwrap().insert(key.into(), e.message);
                }
            }
        }
        out
    }
    fn load(s: &Selection, rate: u32) -> AppResult<Arc<Native>> {
        s.validate()?;
        let d = &s.descriptor;
        let path = c(&d.path)?;
        let format = c(&d.format)?;
        let id = c(&d.id)?;
        let state = c(&s.state)?;
        let name = c(&d.name)?;
        let mut err = std::ptr::null_mut();
        let ptr = unsafe {
            md_plugin_create(
                path.as_ptr(),
                format.as_ptr(),
                id.as_ptr(),
                rate as f64,
                state.as_ptr(),
                name.as_ptr(),
                d.instrument,
                OFFLINE.get(),
                &mut err,
            )
        };
        if ptr.is_null() {
            let message = if err.is_null() {
                "Plugin 생성 실패".into()
            } else {
                unsafe {
                    let message = CStr::from_ptr(err).to_string_lossy().into_owned();
                    md_plugin_string_free(err);
                    message
                }
            };
            return Err(error(format!("{}: {}", d.name, message)));
        }
        for p in &s.parameters {
            if !p.readonly {
                unsafe { md_plugin_initial_parameter(ptr, p.id, p.value) }
            }
        }
        unsafe { md_plugin_prime(ptr) };
        let info = unsafe { text(md_plugin_status(ptr))? };
        Ok(Arc::new(Native {
            ptr,
            selection: s.clone(),
            tail: info["tail"].as_u64().unwrap_or(0) as u32,
        }))
    }
    pub fn latency_pending(&self) -> bool {
        self.native
            .as_ref()
            .is_some_and(|n| unsafe { md_plugin_latency_pending(n.ptr) })
    }
    pub fn settle_latency(&self) {
        if let Some(n) = &self.native {
            unsafe { md_plugin_poll_latency(n.ptr) }
        }
    }
    pub fn latency(&self) -> usize {
        self.native
            .as_ref()
            .map_or(0, |n| unsafe { md_plugin_latency(n.ptr) as usize })
    }
    pub fn clock(&mut self, frame: usize, bpm: f64, num: i32, den: i32, playing: bool) {
        self.frame = frame;
        self.bpm = bpm;
        self.num = num;
        self.den = den;
        self.playing = playing;
    }
    pub fn process(&mut self, pair: [f64; 2]) -> [f64; 2] {
        let Some(n) = &self.native else { return pair };
        let mut pair = pair.map(|v| v as f32);
        if !unsafe {
            md_plugin_process(
                n.ptr,
                pair.as_mut_ptr(),
                self.frame as i64,
                self.bpm,
                self.num,
                self.den,
                self.playing,
            )
        } && OFFLINE.get()
        {
            FAILED.set(true);
            FAILED_KIND.set(unsafe { md_plugin_fault(n.ptr) });
        }
        self.frame = self.frame.saturating_add(1);
        pair.map(f64::from)
    }
    pub fn midi(&mut self, status: u8, a: u8, b: u8, voice: u32) {
        if let Some(n) = &self.native {
            unsafe { md_plugin_midi(n.ptr, status, a, b, voice) }
        }
    }
    pub fn parameter(&mut self, id: u32, value: f64) {
        if let Some(n) = &self.native {
            unsafe { md_plugin_parameter(n.ptr, id, value) }
        }
    }
    pub fn reset(&mut self) {
        if let Some(n) = &self.native {
            unsafe { md_plugin_reset(n.ptr) }
        }
    }
}
fn live(key: &str) -> AppResult<Arc<Native>> {
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(key)
        .and_then(|v| v.iter().rev().find_map(Weak::upgrade))
        .ok_or_else(|| error("Plugin 인스턴스를 사용할 수 없습니다."))
}
pub fn prepare(d: Descriptor, rate: u32) -> AppResult<Selection> {
    let mut s = Selection {
        descriptor: d,
        state: String::new(),
        parameters: vec![],
        latency: 0,
        tail: 0,
        sample_rate: rate,
    };
    let n = Instance::load(&s, rate)?;
    let info = unsafe { text(md_plugin_info(n.ptr))? };
    s.parameters = serde_json::from_value(info["parameters"].clone()).map_err(error)?;
    s.latency = info["latency"].as_u64().unwrap_or(0) as u32;
    s.tail = info["tail"].as_u64().unwrap_or(0) as u32;
    s.state = unsafe { text(md_plugin_state(n.ptr))? }["state"]
        .as_str()
        .unwrap_or_default()
        .into();
    s.validate()?;
    Ok(s)
}
pub fn editor(key: &str, show: bool, owner: usize) -> AppResult<()> {
    let n = live(key)?;
    unsafe {
        text(md_plugin_editor(n.ptr, show, owner))?;
    }
    Ok(())
}
pub fn status(keys: &[String]) -> Vec<RuntimeStatus> {
    keys.iter()
        .map(|key| match live(key) {
            Ok(n) => RuntimeStatus {
                instance_id: key.clone(),
                name: n.selection.descriptor.name.clone(),
                error: None,
                info: unsafe { text(md_plugin_status(n.ptr)) }.unwrap_or_default(),
            },
            Err(_) => RuntimeStatus {
                instance_id: key.clone(),
                name: String::new(),
                error: failures().lock().unwrap().get(key).cloned(),
                info: serde_json::Value::Null,
            },
        })
        .collect()
}
pub fn capture(key: &str, s: &mut Selection, force: bool) -> AppResult<bool> {
    let Ok(n) = live(key) else { return Ok(false) };
    if n.selection.descriptor != s.descriptor {
        return Ok(false);
    }
    let edited = unsafe { md_plugin_dirty(n.ptr) };
    if !edited && !force {
        return Ok(false);
    }
    let info = unsafe { text(md_plugin_info(n.ptr))? };
    let state = unsafe { text(md_plugin_state(n.ptr))? }["state"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let mut params: Vec<Parameter> =
        serde_json::from_value(info["parameters"].clone()).map_err(error)?;
    if !edited {
        for p in &mut params {
            if let Some(base) = s.parameters.iter().find(|q| q.id == p.id) {
                p.value = base.value;
            }
        }
    }
    let latency = info["latency"].as_u64().unwrap_or(s.latency as u64) as u32;
    let tail = info["tail"].as_u64().unwrap_or(s.tail as u64) as u32;
    let changed =
        s.state != state || s.parameters != params || s.latency != latency || s.tail != tail;
    s.latency = latency;
    s.tail = tail;
    s.state = state;
    s.parameters = params;
    Ok(changed)
}
#[derive(Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub paths: Vec<String>,
    pub plugins: Vec<Descriptor>,
    pub errors: Vec<String>,
}
pub struct Service {
    path: PathBuf,
    catalog: Mutex<Catalog>,
    pub scanning: std::sync::atomic::AtomicBool,
    progress: Mutex<String>,
}
impl Service {
    pub fn new(path: PathBuf) -> Self {
        let catalog = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            path,
            catalog: Mutex::new(catalog),
            scanning: Default::default(),
            progress: Mutex::new(String::new()),
        }
    }
    pub fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({"catalog":*self.catalog.lock().unwrap(),"scanning":self.scanning.load(std::sync::atomic::Ordering::Acquire),"progress":*self.progress.lock().unwrap()})
    }
    pub fn scan(&self, paths: Vec<String>) -> AppResult<Catalog> {
        use std::sync::atomic::Ordering::*;
        if self.scanning.swap(true, AcqRel) {
            return Err(error("Plugin Scan이 진행 중입니다."));
        }
        struct Running<'a>(&'a std::sync::atomic::AtomicBool);
        impl Drop for Running<'_> {
            fn drop(&mut self) {
                self.0.store(false, Release);
            }
        }
        let _running = Running(&self.scanning);
        let mut catalog = Catalog {
            paths: paths.clone(),
            ..Default::default()
        };
        let mut files = vec![];
        for path in standard_paths()
            .into_iter()
            .chain(paths.iter().map(PathBuf::from))
        {
            if !path.is_absolute() {
                return Err(error("Plugin 검색 경로는 절대 경로여야 합니다."));
            }
            candidates(&path, &mut files, 0);
        }
        files.sort();
        files.dedup();
        for (i, path) in files.iter().enumerate() {
            *self.progress.lock().unwrap() =
                format!("{}/{} · {}", i + 1, files.len(), path.display());
            let format = path.extension().unwrap().to_string_lossy().to_lowercase();
            let result = (|| -> AppResult<Vec<Descriptor>> {
                let output = std::env::temp_dir()
                    .join(format!("minidaw-plugin-scan-{}.json", uuid::Uuid::new_v4()));
                struct Temp(PathBuf);
                impl Drop for Temp {
                    fn drop(&mut self) {
                        let _ = std::fs::remove_file(&self.0);
                    }
                }
                let _file = Temp(output.clone());
                let mut cmd = std::process::Command::new(std::env::current_exe().map_err(error)?);
                cmd.arg("--plugin-scan")
                    .arg(path)
                    .arg(&format)
                    .arg(&output)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null());
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    cmd.creation_flags(0x08000000);
                }
                let mut child = cmd.spawn().map_err(error)?;
                let began = std::time::Instant::now();
                loop {
                    if let Some(status) = child.try_wait().map_err(error)? {
                        if !status.success() {
                            return Err(error(format!("Scan process failed ({status})")));
                        }
                        break;
                    }
                    if began.elapsed().as_secs() >= 20 {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(error("Scan timeout (20 s)"));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(30));
                }
                let data: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&output).map_err(error)?)
                        .map_err(error)?;
                if let Some(e) = data.get("error") {
                    return Err(error(e));
                }
                serde_json::from_value(data["plugins"].clone()).map_err(error)
            })();
            match result {
                Ok(p) => catalog.plugins.extend(p),
                Err(e) => catalog
                    .errors
                    .push(format!("{}: {}", path.display(), e.message)),
            }
        }
        catalog
            .plugins
            .sort_by(|a, b| a.name.cmp(&b.name).then(a.format.cmp(&b.format)));
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(error)?;
        }
        std::fs::write(
            &self.path,
            serde_json::to_vec_pretty(&catalog).map_err(error)?,
        )
        .map_err(error)?;
        *self.catalog.lock().unwrap() = catalog.clone();
        *self.progress.lock().unwrap() = String::new();
        Ok(catalog)
    }
}
pub fn standard_paths() -> Vec<PathBuf> {
    let mut out = vec![];
    for base in ["COMMONPROGRAMFILES", "LOCALAPPDATA"] {
        if let Some(root) = std::env::var_os(base) {
            let root = PathBuf::from(root);
            for format in ["VST3", "CLAP"] {
                out.push(if base == "LOCALAPPDATA" {
                    root.join("Programs/Common").join(format)
                } else {
                    root.join(format)
                });
            }
        }
    }
    if let Some(paths) = std::env::var_os("CLAP_PATH") {
        out.extend(std::env::split_paths(&paths));
    }
    out
}
pub fn candidates(root: &Path, out: &mut Vec<PathBuf>, depth: u8) {
    if depth > 12 || out.len() >= 4096 {
        return;
    }
    let ext = root
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_lowercase();
    if (ext == "clap" || ext == "vst3") && root.is_file() {
        out.push(root.to_owned());
        return;
    }
    if ext == "vst3" && root.is_dir() {
        let bin = root.join("Contents/x86_64-win");
        if let Ok(read) = std::fs::read_dir(bin) {
            out.extend(
                read.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|e| e == "vst3")),
            );
        }
        return;
    }
    if let Ok(read) = std::fs::read_dir(root) {
        for e in read.flatten() {
            if e.file_type().is_ok_and(|t| !t.is_symlink()) {
                candidates(&e.path(), out, depth + 1);
            }
        }
    }
}
pub fn apply(
    p: &crate::project::schema::Project,
    r: &crate::project::edit::EditRequest,
) -> AppResult<crate::project::schema::Project> {
    use crate::project::schema::*;
    let mut next = p.clone();
    match r.command.as_str() {
        "plugin.instrument" => {
            let plugin = r
                .plugin
                .as_ref()
                .ok_or_else(|| error("Instrument 선택 없음"))?;
            plugin.validate()?;
            if !plugin.descriptor.instrument {
                return Err(error("가상악기 플러그인을 선택하세요."));
            }
            let track = next
                .tracks
                .iter_mut()
                .find(|t| {
                    r.track_ids.len() == 1
                        && t.track_id == r.track_ids[0]
                        && t.kind == TrackKind::Midi
                })
                .ok_or_else(|| error("MIDI Track 선택"))?;
            track.instrument = Instrument::External;
            track.extensions.insert(
                INSTRUMENT.into(),
                serde_json::to_value(plugin).map_err(error)?,
            );
        }
        "plugin.parameter" => {
            let track = next
                .tracks
                .iter_mut()
                .find(|t| {
                    r.track_ids.len() == 1
                        && t.track_id == r.track_ids[0]
                        && t.instrument == Instrument::External
                })
                .ok_or_else(|| error("External Instrument Track"))?;
            let mut plugin = instrument(track).ok_or_else(|| error("Instrument 설정 없음"))?;
            let id = r
                .parameter
                .as_ref()
                .and_then(|p| p.name.strip_prefix("plugin."))
                .and_then(|s| s.parse::<u32>().ok())
                .ok_or_else(|| error("Parameter ID"))?;
            let value = r.value.ok_or_else(|| error("Parameter 값"))?;
            let param = plugin
                .parameters
                .iter_mut()
                .find(|p| p.id == id && !p.readonly)
                .ok_or_else(|| error("Parameter 없음"))?;
            param.value = value;
            plugin.validate()?;
            track.extensions.insert(
                INSTRUMENT.into(),
                serde_json::to_value(plugin).map_err(error)?,
            );
        }
        "plugin.capture" => {
            capture_project(&mut next)?;
        }
        _ => return Err(error("Plugin command")),
    }
    crate::project::automation::prune(&mut next);
    next.validate()?;
    Ok(next)
}
pub fn capture_project(p: &mut crate::project::schema::Project) -> AppResult<bool> {
    capture_document(p, false)
}
pub fn capture_snapshot(p: &mut crate::project::schema::Project) -> AppResult<bool> {
    capture_document(p, true)
}
fn capture_document(p: &mut crate::project::schema::Project, force: bool) -> AppResult<bool> {
    use crate::project::{effects::Processor, schema::Instrument};
    let mut changed = false;
    for t in &mut p.tracks {
        if t.instrument == Instrument::External {
            if let Some(mut s) = instrument(t) {
                if capture(&t.track_id, &mut s, force)? {
                    t.extensions
                        .insert(INSTRUMENT.into(), serde_json::to_value(s).map_err(error)?);
                    changed = true;
                }
            }
        }
    }
    for e in p
        .tracks
        .iter_mut()
        .flat_map(|t| &mut t.inserts)
        .chain(p.master.inserts.iter_mut())
    {
        if let Processor::External { plugin } = &mut e.processor {
            changed |= capture(&e.effect_id, plugin, force)?;
        }
    }
    Ok(changed)
}
pub fn project_keys(p: &crate::project::schema::Project) -> Vec<String> {
    use crate::project::{effects::Processor, schema::Instrument};
    p.tracks
        .iter()
        .filter(|t| t.instrument == Instrument::External)
        .map(|t| t.track_id.clone())
        .chain(
            p.tracks
                .iter()
                .flat_map(|t| &t.inserts)
                .chain(p.master.inserts.iter())
                .filter(|e| matches!(e.processor, Processor::External { .. }))
                .map(|e| e.effect_id.clone()),
        )
        .collect()
}

pub fn dirty(p: &crate::project::schema::Project) -> bool {
    project_keys(p)
        .iter()
        .any(|id| live(id).is_ok_and(|n| unsafe { md_plugin_dirty(n.ptr) }))
}
