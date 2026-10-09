//! Application/control-thread ownership. Mutexes and blocking replies here never
//! run on the audio callback. A retained owner prevents PCM destruction there.
use crate::audio::metrics::{AudioMetrics, MetricsSnapshot};
use crate::preferences::{AudioPreferences, DriverType};
use crate::{
    audio::{
        analysis::{Analysis, WaveformSnapshot},
        decoder::FileInfo,
        output::{self, OutputInfo},
        renderer::{Action, Command},
        source::{AudioAsset, AudioSource},
        streaming::StreamSnapshot,
        transport::{Transport, TransportCell},
    },
    error::{AppError, AppResult},
};
use serde::Serialize;
use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, SyncSender},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

fn unavailable() -> AppError {
    AppError::new(
        "engine",
        "오디오 엔진이 응답하지 않습니다. 출력 장치를 다시 연결해 주세요.",
    )
}
fn busy() -> AppError {
    AppError::new(
        "busy",
        "오디오 명령이 밀려 있습니다. 잠시 후 다시 시도해 주세요.",
    )
}

enum Request {
    Source(u64, Option<LoadedAudio>, u8, SyncSender<AppResult<()>>),
    Command(Command, SyncSender<AppResult<u64>>),
    Reconnect(
        Option<u32>,
        AudioPreferences,
        SyncSender<AppResult<OutputInfo>>,
    ),
    Devices(DriverType, SyncSender<AppResult<Vec<String>>>),
    ControlPanel(SyncSender<AppResult<()>>),
}

#[derive(Clone)]
pub struct LoadedAudio {
    pub id: u64,
    pub audio: Arc<AudioSource>,
    pub waveform: Option<Arc<Analysis>>,
    pub waveform_asset: Option<Arc<AudioAsset>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSnapshot {
    pub master: crate::audio::master::MasterSnapshot,
    pub effects: Vec<crate::audio::effect_runtime::Reading>,
    pub pdc: crate::audio::effect_runtime::PdcSnapshot,
    pub source_pending: bool,
    pub analysis_builds: u64,
    pub plan_build_ms: Option<f64>,
    pub metrics: MetricsSnapshot,
    pub cache_build_ms: Option<f64>,
    pub source: Option<StreamSnapshot>,
    pub waveform: Option<WaveformSnapshot>,
    pub transport: Transport,
    pub file: Option<FileInfo>,
    pub position: f64,
    pub duration: f64,
    pub output: Option<OutputInfo>,
    pub output_error: Option<AppError>,
    pub stream_errors: u64,
    pub preferences: AudioPreferences,
    pub asio_available: bool,
}

pub struct AudioService {
    pub midi_input: Arc<crate::audio::midi_input::MidiInputService>,
    pub spectrum: Arc<crate::audio::spectrum::Spectrum>,
    pub media: Mutex<crate::project::media::MediaPool>,
    pub metrics: Arc<AudioMetrics>,
    pub transport: Arc<TransportCell>,
    pub stream_errors: Arc<AtomicU64>,
    sender: SyncSender<Request>,
    output: Arc<Mutex<AppResult<OutputInfo>>>,
    loaded: Arc<Mutex<Option<LoadedAudio>>>,
    loading: Mutex<()>,
    submission: Mutex<()>,
    command_id: AtomicU64,
    clip_id: AtomicU64,
    preferences: Mutex<AudioPreferences>,
    preference_path: Option<PathBuf>,
}

impl AudioService {
    pub fn new(buffer: Option<u32>) -> AppResult<Self> {
        Self::with_preferences(buffer, None)
    }

    pub fn with_preferences(
        buffer: Option<u32>,
        preference_path: Option<PathBuf>,
    ) -> AppResult<Self> {
        let saved = preference_path
            .as_ref()
            .map(|p| AudioPreferences::read(p))
            .unwrap_or_else(|| Ok(AudioPreferences::default()));
        let preferences = saved.as_ref().cloned().unwrap_or_default();
        let (sender, receiver) = mpsc::sync_channel::<Request>(32);
        let transport = Arc::new(TransportCell::default());
        let metrics = Arc::new(AudioMetrics::default());
        let stream_errors = Arc::new(AtomicU64::new(0));
        let output_info = Arc::new(Mutex::new(Err(unavailable())));
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let shared = transport.clone();
        let errors = stream_errors.clone();
        let info = output_info.clone();
        let shared_metrics = metrics.clone();
        let loaded: Arc<Mutex<Option<LoadedAudio>>> = Arc::new(Mutex::new(None));
        let shared_loaded = loaded.clone();
        let spectrum = Arc::new(crate::audio::spectrum::Spectrum::default());
        let shared_spectrum = spectrum.clone();
        let midi_input = Arc::new(crate::audio::midi_input::MidiInputService::default());
        let shared_midi = midi_input.clone();
        thread::Builder::new()
            .name("minidaw-output-control".into())
            .spawn(move || {
                let opened = saved.and_then(|prefs| {
                    output::open(
                        shared.clone(),
                        shared_metrics.clone(),
                        errors.clone(),
                        buffer,
                        &prefs,
                        &shared_spectrum,
                    )
                });
                if let Ok(mut info) = info.lock() {
                    *info = opened
                        .as_ref()
                        .map(|o| o.info.clone())
                        .map_err(Clone::clone);
                }
                let mut output = opened.ok();
                if let Some(o) = &mut output {
                    let _ = o.commands.push(Command {
                        id: 0,
                        issued: Instant::now(),
                        action: Action::MidiInput(shared_midi.hub.clone()),
                    });
                    let _ = o.commands.push(Command { id: 0, issued: Instant::now(), action: Action::ComputerMidi(shared_midi.computer.clone()) });
                }
                let mut retained: Vec<Arc<AudioSource>> = Vec::new();
                let mut current: Option<(u64, Arc<AudioSource>)> = None;
                let _ = ready_tx.send(());
                loop {
                    match receiver.recv_timeout(Duration::from_millis(20)) {
                        Ok(Request::Source(id, loaded, preserve, reply)) => {
                            let next = loaded.as_ref().map(|l| (l.id, l.audio.clone()));
                            if let Some((_, audio)) = &next {
                                retained.push(audio.clone());
                            }
                            let action =
                                next.as_ref().map_or(Action::Unload, |(clip_id, audio)| {
                                    if preserve == 2 {
                                        Action::AutomationReplace {
                                            clip_id: *clip_id,
                                            audio: audio.clone(),
                                        }
                                    } else if preserve == 1 {
                                        Action::Replace {
                                            clip_id: *clip_id,
                                            audio: audio.clone(),
                                        }
                                    } else {
                                        Action::Load {
                                            clip_id: *clip_id,
                                            audio: audio.clone(),
                                        }
                                    }
                                });
                            // A document can open even with an unavailable output device.
                            // Stop any failed stream before publishing without its writer.
                            if errors.load(Ordering::Relaxed) > 0 {
                                drop(output.take());
                            }
                            let result = if let Some(output) = &mut output {
                                output
                                    .commands
                                    .push(Command {
                                        id,
                                        issued: Instant::now(),
                                        action,
                                    })
                                    .map_err(|_| busy())
                            } else {
                                shared.publish(Transport {
                                    applied_command: id,
                                    clip_id: next.as_ref().map_or(0, |(id, _)| *id),
                                    ..Transport::default()
                                });
                                Ok(())
                            };
                            if result.is_ok() {
                                current = next;
                                if let Ok(mut state) = shared_loaded.lock() {
                                    *state = loaded;
                                }
                            }
                            let _ = reply.send(result);
                        }
                        Ok(Request::Command(command, reply)) => {
                            let id = command.id;
                            let next = if let Action::Load { clip_id, audio } = &command.action {
                                retained.push(audio.clone());
                                Some((*clip_id, audio.clone()))
                            } else {
                                None
                            };
                            let result = if errors.load(Ordering::Relaxed) > 0 {
                                Err(unavailable())
                            } else if let Some(output) = &mut output {
                                output
                                    .commands
                                    .push(command)
                                    .map(|_| id)
                                    .map_err(|_| busy())
                            } else {
                                Err(unavailable())
                            };
                            if result.is_ok() && next.is_some() {
                                current = next;
                            }
                            let _ = reply.send(result);
                        }
                        Ok(Request::Devices(driver_type, reply)) => {
                            let _ = reply.send(output::devices(driver_type));
                        }
                        Ok(Request::ControlPanel(reply)) => {
                            let _ = reply.send(output::control_panel(output.as_ref()));
                        }
                        Ok(Request::Reconnect(buffer, preferences, reply)) => {
                            let previous_id = shared.read().applied_command;
                            // Stop/join the old callback before resetting single-writer cells.
                            drop(output.take());
                            shared.publish(Transport {
                                applied_command: previous_id,
                                clip_id: current.as_ref().map_or(0, |(id, _)| *id),
                                ..Transport::default()
                            });
                            errors.store(0, Ordering::Relaxed);
                            shared_metrics.reset();
                            let opened = output::open(
                                shared.clone(),
                                shared_metrics.clone(),
                                errors.clone(),
                                buffer,
                                &preferences,
                                &shared_spectrum,
                            );
                            let mut result = opened
                                .as_ref()
                                .map(|o| o.info.clone())
                                .map_err(Clone::clone);
                            output = opened.ok();
                            if let Some(o) = &mut output {
                                let _ = o.commands.push(Command {
                                    id: previous_id,
                                    issued: Instant::now(),
                                    action: Action::MidiInput(shared_midi.hub.clone()),
                                });
                                let _ = o.commands.push(Command { id: previous_id, issued: Instant::now(), action: Action::ComputerMidi(shared_midi.computer.clone()) });
                            }
                            if let (Some(output), Some((clip_id, audio))) =
                                (&mut output, &mut current)
                            {
                                if audio.playback_rate != output.info.sample_rate {
                                    match audio.at_rate(output.info.sample_rate) {
                                        Ok(prepared) => {
                                            retained.push(prepared.clone());
                                            *audio = prepared;
                                            if let Ok(mut loaded) = shared_loaded.lock() {
                                                if let Some(loaded) = loaded.as_mut() {
                                                    loaded.audio = audio.clone();
                                                }
                                            }
                                        }
                                        Err(error) => {
                                            result = Err(error);
                                        }
                                    }
                                }
                                if result.is_ok() {
                                    audio.seek(0);
                                    let _ = output.commands.push(Command {
                                        id: previous_id,
                                        issued: Instant::now(),
                                        action: Action::Load {
                                            clip_id: *clip_id,
                                            audio: audio.clone(),
                                        },
                                    });
                                }
                            }
                            if let Ok(mut info) = info.lock() {
                                *info = result.clone();
                            }
                            if result.is_err() {
                                drop(output.take());
                            }
                            let _ = reply.send(result);
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                    }
                    // Only this non-RT thread can release the last PCM owner. Once
                    // strong_count == 1 there is no other owner that can clone it.
                    retained.retain(|audio| Arc::strong_count(audio) > 1);
                    shared_metrics.effects.collect();
                }
                drop(output); // callback ends while retained owners are still alive
                drop(current);
                drop(retained);
            })
            .map_err(|e| unavailable().detail(e))?;
        ready_rx
            .recv_timeout(Duration::from_secs(20))
            .map_err(|e| unavailable().detail(e))?;
        Ok(Self {
            midi_input,
            spectrum,
            media: Mutex::new(crate::project::media::MediaPool::default()),
            metrics,
            transport,
            stream_errors,
            sender,
            output: output_info,
            loaded,
            loading: Mutex::new(()),
            submission: Mutex::new(()),
            command_id: AtomicU64::new(0),
            clip_id: AtomicU64::new(0),
            preferences: Mutex::new(preferences),
            preference_path,
        })
    }

    fn send(&self, action: Action, issued: Instant) -> AppResult<u64> {
        let _submit = self.submission.lock().map_err(|_| unavailable())?;
        let id = self.command_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender
            .try_send(Request::Command(Command { id, issued, action }, tx))
            .map_err(|_| busy())?;
        rx.recv_timeout(Duration::from_secs(2))
            .map_err(|_| unavailable())?
    }

    pub fn command(&self, action: Action) -> AppResult<u64> {
        self.command_at(action, Instant::now())
    }

    pub fn command_at(&self, action: Action, issued: Instant) -> AppResult<u64> {
        if self.loaded.lock().map_err(|_| unavailable())?.is_none() {
            return Err(AppError::new(
                "no_file",
                "먼저 오디오 파일을 불러와 주세요.",
            ));
        }
        if let Action::Seek(seconds) = &action {
            if !seconds.is_finite() {
                return Err(AppError::new("seek", "유효한 재생 위치를 입력해 주세요."));
            }
        }
        self.send(action, issued)
    }

    pub fn load(&self, path: &Path) -> AppResult<LoadedAudio> {
        let prepared = self.prepare_asset(AudioAsset::open(path)?)?;
        self.install(Some(prepared))?.ok_or_else(unavailable)
    }

    /// Non-RT staging: prepare fully before replacing a currently playable source.
    pub fn prepare_asset(&self, asset: Arc<AudioAsset>) -> AppResult<LoadedAudio> {
        let rate = self
            .output
            .lock()
            .map_err(|_| unavailable())?
            .as_ref()
            .map_or(asset.info.sample_rate, |o| o.sample_rate);
        let audio = AudioSource::for_output(asset.clone(), rate)?;
        let waveform = Arc::new(Analysis::new(&asset)?);
        Ok(LoadedAudio {
            id: 0,
            audio,
            waveform: Some(waveform),
            waveform_asset: Some(asset),
        })
    }

    pub fn install(&self, mut prepared: Option<LoadedAudio>) -> AppResult<Option<LoadedAudio>> {
        self.install_mode(prepared.take(), false)
    }
    pub fn install_mode(
        &self,
        prepared: Option<LoadedAudio>,
        preserve: bool,
    ) -> AppResult<Option<LoadedAudio>> {
        self.install_kind(prepared, u8::from(preserve))
    }
    pub fn install_mode_automation(
        &self,
        prepared: Option<LoadedAudio>,
    ) -> AppResult<Option<LoadedAudio>> {
        self.install_kind(prepared, 2)
    }
    fn install_kind(
        &self,
        mut prepared: Option<LoadedAudio>,
        preserve: u8,
    ) -> AppResult<Option<LoadedAudio>> {
        let _guard = self
            .loading
            .try_lock()
            .map_err(|_| AppError::new("loading", "다른 파일을 불러오는 중입니다."))?;
        if let Some(loaded) = &mut prepared {
            let rate = self
                .output
                .lock()
                .map_err(|_| unavailable())?
                .as_ref()
                .map_or(loaded.audio.playback_rate, |o| o.sample_rate);
            if rate != loaded.audio.playback_rate {
                loaded.audio = loaded.audio.at_rate(rate)?;
            }
            loaded.id = self.clip_id.fetch_add(1, Ordering::Relaxed) + 1;
        }
        let _submit = self.submission.lock().map_err(|_| unavailable())?;
        let id = self.command_id.fetch_add(1, Ordering::Relaxed) + 1;
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender
            .try_send(Request::Source(id, prepared.clone(), preserve, tx))
            .map_err(|_| busy())?;
        rx.recv_timeout(Duration::from_secs(2))
            .map_err(|_| unavailable())??;
        self.wait_for(id, Duration::from_secs(2))?;
        Ok(prepared)
    }

    pub fn loaded(&self) -> AppResult<Option<LoadedAudio>> {
        Ok(self.loaded.lock().map_err(|_| unavailable())?.clone())
    }
    pub fn prepare_project(
        &self,
        document: &crate::project::schema::Project,
        states: &[crate::project::session::AssetState],
    ) -> AppResult<Option<LoadedAudio>> {
        use crate::{
            audio::timeline::PlaybackPlan,
            project::{media::Media, schema::invalid},
        };
        use std::collections::HashMap;
        let mut media: HashMap<String, Media> = HashMap::new();
        {
            let mut pool = self.media.lock().map_err(|_| unavailable())?;
            pool.entries
                .retain(|id, _| document.assets.iter().any(|a| a.asset_id == *id));
            for asset in &document.assets {
                if let Some(state) = states
                    .iter()
                    .find(|s| s.asset_id == asset.asset_id && s.status == "available")
                {
                    let path = state.resolved_path.as_deref().expect("available path");
                    let key = format!("{}:{}", path, asset.fingerprint.sampled_sha256);
                    let entry = pool.get(&asset.asset_id, Path::new(path), &key)?;
                    if entry.asset.info.sample_rate != asset.metadata.sample_rate
                        || entry.asset.info.channels != asset.metadata.channels as usize
                        || entry.asset.info.frames as u64 != asset.metadata.source_frames.0
                    {
                        return Err(invalid(
                            "원본 metadata가 변경되었습니다. 파일을 다시 연결해 주세요.",
                        ));
                    }
                    media.insert(asset.asset_id.clone(), entry);
                }
            }
        }
        let has_instrument = document.tracks.iter().any(|t| !t.instrument.is_none());
        if !has_instrument
            && !document
                .automation
                .iter()
                .any(|a| a.track_id != "master" && a.read && !a.lanes.is_empty())
            && document.midi_clips().next().is_none()
            && (document.clips().next().is_none() || media.is_empty())
        {
            return Ok(None);
        }
        let rate = self
            .output
            .lock()
            .map_err(|_| unavailable())?
            .as_ref()
            .map_or(48000, |o| o.sample_rate);
        // The unedited one-clip case retains the original zero-additional-work path.
        if !has_instrument
            && !document
                .automation
                .iter()
                .any(|a| a.track_id != "master" && a.read && !a.lanes.is_empty())
            && !document
                .tracks
                .iter()
                .any(|t| t.inserts.iter().any(|e| e.enabled))
            && document.midi_clips().next().is_none()
            && document.clips().count() == 1
            && !document.cycle.as_ref().is_some_and(|c| c.enabled)
        {
            if let Some(c) = document.primary().filter(|c| document.preview_supported(c)) {
                if let Some(m) = media.get(&c.asset_id) {
                    return Ok(Some(LoadedAudio {
                        id: 0,
                        audio: AudioSource::for_output(m.asset.clone(), rate)?,
                        waveform: Some(m.waveform.clone()),
                        waveform_asset: Some(m.asset.clone()),
                    }));
                }
            }
        }
        let plan = PlaybackPlan::compile(
            Arc::new(document.clone()),
            media
                .iter()
                .map(|(id, m)| (id.clone(), m.asset.clone()))
                .collect(),
            rate,
        )?;
        let start = (self.snapshot()?.position * rate as f64) as usize;
        let waveform = media.values().next().map(|m| m.waveform.clone());
        Ok(Some(LoadedAudio {
            id: 0,
            audio: AudioSource::timeline(plan, start)?,
            waveform,
            waveform_asset: media.values().next().map(|m| m.asset.clone()),
        }))
    }

    pub fn wait_for(&self, command: u64, timeout: Duration) -> AppResult<()> {
        let started = Instant::now();
        while self.transport.read().applied_command < command {
            if started.elapsed() > timeout || self.stream_errors.load(Ordering::Relaxed) > 0 {
                return Err(unavailable());
            }
            thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    }

    pub fn reconnect(&self, buffer: Option<u32>) -> AppResult<OutputInfo> {
        if buffer.is_some_and(|size| !matches!(size, 128 | 256 | 512 | 1024)) {
            return Err(AppError::new(
                "buffer",
                "지원하지 않는 요청 버퍼 크기입니다.",
            ));
        }
        let _guard = self.loading.try_lock().map_err(|_| busy())?;
        let preferences = self.preferences.lock().map_err(|_| unavailable())?.clone();
        self.reconnect_locked(buffer, preferences)
    }

    fn reconnect_locked(
        &self,
        buffer: Option<u32>,
        preferences: AudioPreferences,
    ) -> AppResult<OutputInfo> {
        let _submit = self.submission.lock().map_err(|_| unavailable())?;
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender
            .try_send(Request::Reconnect(buffer, preferences, tx))
            .map_err(|_| busy())?;
        rx.recv_timeout(Duration::from_secs(20))
            .map_err(|_| unavailable())?
    }

    pub fn apply_settings(&self, preferences: AudioPreferences) -> AppResult<()> {
        let _guard = self.loading.try_lock().map_err(|_| busy())?;
        for name in [&preferences.output_device, &preferences.asio_driver]
            .into_iter()
            .flatten()
        {
            if name.is_empty() || name.len() > 4096 || name.contains('\0') {
                return Err(AppError::new("device", "유효한 출력 장치를 선택해 주세요."));
            }
        }
        let previous = self.preferences.lock().map_err(|_| unavailable())?.clone();
        if previous.same_output(&preferences)
            && previous.transport_declick != preferences.transport_declick
            && self.output.lock().map_err(|_| unavailable())?.is_ok()
            && self.stream_errors.load(Ordering::Relaxed) == 0
        {
            let id = self.send(
                Action::Declick(preferences.transport_declick),
                Instant::now(),
            )?;
            self.wait_for(id, Duration::from_secs(2))?;
            if let Some(path) = &self.preference_path {
                if let Err(error) = preferences.save(path) {
                    let rollback =
                        self.send(Action::Declick(previous.transport_declick), Instant::now())?;
                    self.wait_for(rollback, Duration::from_secs(2))?;
                    return Err(error);
                }
            }
            *self.preferences.lock().map_err(|_| unavailable())? = preferences;
        } else {
            if let Some(path) = &self.preference_path {
                preferences.save(path)?;
            }
            *self.preferences.lock().map_err(|_| unavailable())? = preferences.clone();
            self.reconnect_locked(None, preferences)?;
        }
        Ok(())
    }

    pub fn devices(&self, driver_type: DriverType) -> AppResult<Vec<String>> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender
            .try_send(Request::Devices(driver_type, tx))
            .map_err(|_| busy())?;
        rx.recv_timeout(Duration::from_secs(10))
            .map_err(|_| unavailable())?
    }

    pub fn control_panel(&self) -> AppResult<()> {
        let _guard = self.loading.try_lock().map_err(|_| busy())?;
        let (tx, rx) = mpsc::sync_channel(1);
        self.sender
            .try_send(Request::ControlPanel(tx))
            .map_err(|_| busy())?;
        // A driver panel can be modal. This waits on a Tauri blocking worker,
        // never the WebView/event loop or audio callback.
        rx.recv().map_err(|_| unavailable())?
    }

    pub fn snapshot(&self) -> AppResult<AppSnapshot> {
        let transport = self.transport.read();
        let loaded = self.loaded()?;
        // Control-thread publication and callback acknowledgement are separate.
        // A mismatched pair is a pending swap, never an actual empty project.
        let source_pending = loaded
            .as_ref()
            .map_or(transport.clip_id != 0, |a| a.id != transport.clip_id);
        let loaded = loaded.filter(|a| a.id == transport.clip_id);
        let waveform = loaded.as_ref().and_then(|a| {
            a.waveform
                .as_ref()
                .zip(a.waveform_asset.as_ref())
                .map(|(w, asset)| w.snapshot(asset))
        });
        let plan_build_ms = loaded
            .as_ref()
            .and_then(|a| a.audio.timeline.as_ref().map(|p| p.build_ms));
        let cache_build_ms = waveform.as_ref().map(|w| w.build_ms);
        let source = loaded.as_ref().map(|a| a.audio.snapshot());
        let file = loaded.map(|a| a.audio.asset.info.clone());
        let position = file
            .as_ref()
            .map_or(0.0, |f| transport.frame / f.sample_rate as f64);
        let duration = file.as_ref().map_or(0.0, |f| f.duration);
        let output = self.output.lock().map_err(|_| unavailable())?.clone();
        let stream_errors = self.stream_errors.load(Ordering::Relaxed);
        let output_error = if stream_errors > 0 {
            Some(AppError::new("stream_error", "출력 오류 또는 드라이버 설정 변경이 감지되었습니다. 오디오 설정에서 출력을 다시 적용해 주세요."))
        } else {
            output.as_ref().err().cloned()
        };
        let mut effects = self.metrics.effects.snapshot();
        let mut pdc = self.metrics.effects.pdc_snapshot();
        let output_error =
            output_error.or_else(|| pdc.error.as_ref().map(|e| AppError::new("pdc", e)));
        if let Some(plan) = self.loaded()?.and_then(|l| l.audio.timeline.clone()) {
            pdc.audio_lookahead_samples = plan.audio_latency.load(Ordering::Acquire);
            let at = (position * plan.rate as f64) as usize;
            effects.extend(
                plan.effect_meters
                    .iter()
                    .flatten()
                    .flat_map(|m| m.read(Some(at))),
            );
        }
        Ok(AppSnapshot {
            effects,
            pdc,
            master: self.metrics.master.snapshot(),
            source_pending,
            analysis_builds: self
                .media
                .lock()
                .map_err(|_| unavailable())?
                .analysis_builds,
            plan_build_ms,
            metrics: self.metrics.snapshot(),
            cache_build_ms,
            source,
            waveform,
            transport,
            file,
            position,
            duration,
            output: output.ok(),
            output_error,
            stream_errors,
            preferences: self.preferences.lock().map_err(|_| unavailable())?.clone(),
            asio_available: cfg!(all(windows, feature = "asio")),
        })
    }
}
