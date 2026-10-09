#[tauri::command]
pub fn app_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[tauri::command]
pub async fn spectrum_configure(
    settings: crate::audio::spectrum::Settings,
    sources: Option<Vec<String>>,
    projects: State<'_, Arc<ProjectService>>,
    service: State<'_, Arc<AudioService>>,
) -> AppResult<()> {
    let service = service.inner().clone();
    let projects = projects.inner().clone();
    worker(move || {
        crate::project::spectrum::configure_sources(
            &service,
            &projects,
            settings,
            sources.unwrap_or_default(),
        )
    })
    .await
}
#[tauri::command]
pub async fn spectrum_snapshot(
    after: u64,
    service: State<'_, Arc<AudioService>>,
) -> AppResult<crate::audio::spectrum::SpectrumSnapshot> {
    service.spectrum.snapshot(after)
}
#[tauri::command]
pub fn spectrum_cancel(service: State<'_, Arc<AudioService>>) {
    service.spectrum.cancel_selection();
}
#[tauri::command]
pub async fn spectrum_analyze(
    request: crate::project::spectrum::Selection,
    service: State<'_, Arc<AudioService>>,
    projects: State<'_, Arc<ProjectService>>,
) -> AppResult<crate::audio::spectrum::SpectrumFrame> {
    let service = service.inner().clone();
    let projects = projects.inner().clone();
    let generation = service
        .spectrum
        .selection_generation
        .load(std::sync::atomic::Ordering::Acquire);
    worker(move || crate::project::spectrum::analyze(&service, &projects, request, generation))
        .await
}

async fn worker<T: Send + 'static>(
    work: impl FnOnce() -> AppResult<T> + Send + 'static,
) -> AppResult<T> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| AppError::new("worker", "오디오 작업을 완료하지 못했습니다.").detail(e))?
}

#[tauri::command]
pub async fn load_audio(
    path: String,
    track_id: Option<String>,
    service: State<'_, Arc<AudioService>>,
    projects: State<'_, Arc<crate::project::ProjectService>>,
) -> AppResult<AppSnapshot> {
    let service = service.inner().clone();
    let projects = projects.inner().clone();
    worker(move || {
        projects.import_to(&service, Path::new(&path), track_id.as_deref())?;
        service.snapshot()
    })
    .await
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineSnapshot {
    #[serde(flatten)]
    audio: AppSnapshot,
    project_revision: u64,
}
#[tauri::command]
pub async fn engine_snapshot(
    service: State<'_, Arc<AudioService>>,
    projects: State<'_, Arc<crate::project::ProjectService>>,
) -> AppResult<EngineSnapshot> {
    let service = service.inner().clone();
    let projects = projects.inner().clone();
    worker(move || {
        if service.transport.read().state != crate::audio::transport::PlayState::Playing
            && projects.has_write_pass()
        {
            let _ = projects.finish_write(
                &service,
                Some(crate::project::runtime::AutomationClock {
                    frame: service
                        .metrics
                        .automation_end_frame
                        .load(std::sync::atomic::Ordering::Relaxed),
                    rate: service
                        .metrics
                        .automation_end_rate
                        .load(std::sync::atomic::Ordering::Relaxed)
                        .max(1) as u32,
                    playing: false,
                    cycle_pass: service
                        .metrics
                        .automation_end_cycle
                        .load(std::sync::atomic::Ordering::Relaxed),
                }),
            );
        }
        Ok(EngineSnapshot {
            audio: service.snapshot()?,
            project_revision: projects.revision()?,
        })
    })
    .await
}

#[tauri::command]
pub async fn transport_command(
    action: String,
    seconds: Option<f64>,
    service: State<'_, Arc<AudioService>>,
    projects: State<'_, Arc<ProjectService>>,
) -> AppResult<u64> {
    let issued = std::time::Instant::now();
    let service = service.inner().clone();
    let action = match action.as_str() {
        "play" => Action::Play,
        "pause" => Action::Pause,
        "toggle" => Action::Toggle,
        "stop" => Action::Stop,
        "seek" => Action::Seek(
            seconds.ok_or_else(|| AppError::new("seek", "재생 위치를 입력해 주세요."))?,
        ),
        _ => return Err(AppError::new("command", "지원하지 않는 재생 명령입니다.")),
    };
    let projects = projects.inner().clone();
    worker(move || {
        let finish=matches!(&action,Action::Pause|Action::Stop|Action::Seek(_))||matches!(&action,Action::Toggle if service.transport.read().state==crate::audio::transport::PlayState::Playing);
        let id=service.command_at(action,issued)?;
        if finish&&projects.has_write_pass(){service.wait_for(id,std::time::Duration::from_secs(2))?;
            projects.finish_write(&service,Some(crate::project::runtime::AutomationClock{frame:service.metrics.automation_end_frame.load(std::sync::atomic::Ordering::Relaxed),rate:service.metrics.automation_end_rate.load(std::sync::atomic::Ordering::Relaxed).max(1)as u32,playing:false,cycle_pass:service.metrics.automation_end_cycle.load(std::sync::atomic::Ordering::Relaxed)}))?;}
        Ok(id)
    }).await
}

#[tauri::command]
pub async fn waveform_view(
    clip_id: u64,
    start: f64,
    end: f64,
    width: usize,
    service: State<'_, Arc<AudioService>>,
) -> AppResult<WaveformView> {
    let service = service.inner().clone();
    worker(move || {
        let loaded = service
            .loaded()?
            .filter(|a| a.id == clip_id)
            .ok_or_else(|| AppError::new("stale", "파형을 표시할 파일이 변경되었습니다."))?;
        let (waveform, asset) = loaded
            .waveform
            .as_ref()
            .zip(loaded.waveform_asset.as_ref())
            .ok_or_else(|| AppError::new("waveform", "MIDI에는 오디오 파형이 없습니다."))?;
        waveform.view(asset, clip_id, start, end, width)
    })
    .await
}

#[tauri::command]
pub async fn reconnect_output(
    buffer: Option<u32>,
    service: State<'_, Arc<AudioService>>,
) -> AppResult<OutputInfo> {
    let service = service.inner().clone();
    worker(move || service.reconnect(buffer)).await
}
use crate::{
    audio::{output::OutputInfo, renderer::Action, waveform::WaveformView},
    error::{AppError, AppResult},
    state::{AppSnapshot, AudioService},
};
use std::{path::Path, sync::Arc};
use tauri::State;

#[tauri::command]
pub async fn output_devices(
    driver_type: crate::preferences::DriverType,
    service: State<'_, Arc<AudioService>>,
) -> AppResult<Vec<String>> {
    let service = service.inner().clone();
    worker(move || service.devices(driver_type)).await
}

#[tauri::command]
pub async fn apply_audio_settings(
    settings: crate::preferences::AudioPreferences,
    service: State<'_, Arc<AudioService>>,
) -> AppResult<AppSnapshot> {
    let service = service.inner().clone();
    worker(move || {
        service.apply_settings(settings)?;
        service.snapshot()
    })
    .await
}

#[tauri::command]
pub async fn asio_control_panel(service: State<'_, Arc<AudioService>>) -> AppResult<()> {
    let service = service.inner().clone();
    worker(move || service.control_panel()).await
}

use crate::project::{
    session::{ProjectView, RelinkResult},
    ProjectService,
};
#[tauri::command]
pub async fn project_snapshot(projects: State<'_, Arc<ProjectService>>) -> AppResult<ProjectView> {
    let projects = projects.inner().clone();
    worker(move || projects.view()).await
}
#[tauri::command]
pub async fn edit_project(
    revision: u64,
    request: crate::project::edit::EditRequest,
    projects: State<'_, Arc<ProjectService>>,
    service: State<'_, Arc<AudioService>>,
) -> AppResult<ProjectView> {
    let projects = projects.inner().clone();
    let service = service.inner().clone();
    worker(move || projects.edit(&service, revision, request)).await
}
#[tauri::command]
pub async fn asset_waveform(
    asset_id: String,
    start: f64,
    end: f64,
    width: usize,
    service: State<'_, Arc<AudioService>>,
) -> AppResult<AssetWaveform> {
    let service = service.inner().clone();
    worker(move || {
        let media = service
            .media
            .lock()
            .map_err(|_| AppError::new("waveform", "파형 상태 오류"))?
            .entries
            .get(&asset_id)
            .cloned()
            .ok_or_else(|| AppError::new("asset_missing", "연결되지 않은 미디어입니다."))?;
        Ok(AssetWaveform {
            view: media.waveform.view(&media.asset, 0, start, end, width)?,
            complete: media.waveform.snapshot(&media.asset).complete,
        })
    })
    .await
}
#[derive(serde::Serialize)]
pub struct AssetWaveform {
    #[serde(flatten)]
    view: WaveformView,
    complete: bool,
}
#[tauri::command]
pub async fn new_project(
    revision: u64,
    discard: bool,
    projects: State<'_, Arc<ProjectService>>,
    service: State<'_, Arc<AudioService>>,
) -> AppResult<ProjectView> {
    let projects = projects.inner().clone();
    let service = service.inner().clone();
    worker(move || projects.new_project(&service, revision, discard)).await
}
#[tauri::command]
pub async fn open_project(
    path: String,
    revision: u64,
    discard: bool,
    projects: State<'_, Arc<ProjectService>>,
    service: State<'_, Arc<AudioService>>,
) -> AppResult<ProjectView> {
    let projects = projects.inner().clone();
    let service = service.inner().clone();
    worker(move || projects.open(&service, Path::new(&path), revision, discard)).await
}
#[tauri::command]
pub async fn save_project(
    path: Option<String>,
    revision: u64,
    projects: State<'_, Arc<ProjectService>>,
    service: State<'_, Arc<AudioService>>,
) -> AppResult<ProjectView> {
    use crate::project::runtime::ProjectAudio;
    let projects = projects.inner().clone();
    let service = service.inner().clone();
    worker(move || {
        projects.save_at(
            path.as_deref().map(Path::new),
            revision,
            service.automation_clock(),
        )
    })
    .await
}

#[tauri::command]
pub async fn relink_asset(
    asset_id: String,
    path: String,
    revision: u64,
    confirmation: Option<String>,
    projects: State<'_, Arc<ProjectService>>,
    service: State<'_, Arc<AudioService>>,
) -> AppResult<RelinkResult> {
    let projects = projects.inner().clone();
    let service = service.inner().clone();
    worker(move || {
        projects.relink(
            &service,
            &asset_id,
            Path::new(&path),
            revision,
            confirmation.as_deref(),
        )
    })
    .await
}
#[tauri::command]
pub async fn select_project_clip(
    clip_id: String,
    revision: u64,
    projects: State<'_, Arc<ProjectService>>,
    service: State<'_, Arc<AudioService>>,
) -> AppResult<ProjectView> {
    let projects = projects.inner().clone();
    let service = service.inner().clone();
    worker(move || projects.select_clip(&service, &clip_id, revision)).await
}
#[tauri::command]
pub async fn remove_recent_project(
    path: String,
    projects: State<'_, Arc<ProjectService>>,
) -> AppResult<ProjectView> {
    let projects = projects.inner().clone();
    worker(move || projects.remove_recent(&path)).await
}

#[tauri::command]
pub async fn export_midi(
    path: String,
    revision: u64,
    projects: State<'_, Arc<ProjectService>>,
) -> AppResult<()> {
    let projects = projects.inner().clone();
    worker(move || {
        let v = projects.view()?;
        if v.revision != revision {
            return Err(AppError::new(
                "stale",
                "프로젝트가 변경되었습니다. 다시 시도해 주세요.",
            ));
        }
        crate::project::smf::write(Path::new(&path), &v.document)
    })
    .await
}

#[tauri::command]
pub async fn midi_input_ports() -> AppResult<Vec<crate::audio::midi_input::Port>> {
    worker(crate::audio::midi_input::MidiInputService::ports).await
}
#[tauri::command]
pub async fn midi_input_connect(
    port: Option<usize>,
    name: Option<String>,
    service: State<'_, Arc<AudioService>>,
) -> AppResult<()> {
    let service = service.inner().clone();
    worker(move || service.midi_input.connect(port, name.as_deref())).await
}
#[tauri::command]
pub async fn midi_input_route(
    track_id: Option<String>,
    projects: State<'_, Arc<ProjectService>>,
    service: State<'_, Arc<AudioService>>,
) -> AppResult<()> {
    let service = service.inner().clone();
    let projects = projects.inner().clone();
    worker(move || {
        if let Some(id) = &track_id {
            if !projects
                .view()?
                .document
                .tracks
                .iter()
                .any(|t| t.kind == crate::project::schema::TrackKind::Midi && &t.track_id == id)
            {
                return Err(crate::project::schema::invalid("MIDI Track이 없습니다."));
            }
        }
        service.midi_input.route(track_id.as_deref())
    })
    .await
}
#[tauri::command]
pub async fn midi_input_status(
    service: State<'_, Arc<AudioService>>,
) -> AppResult<crate::audio::midi_input::Status> {
    let service = service.inner().clone();
    worker(move || service.midi_input.status()).await
}

#[tauri::command]
pub fn computer_midi_notes(track_id: Option<String>, notes: Vec<u8>, service: State<'_, Arc<AudioService>>) -> AppResult<()> {
    service.midi_input.computer_notes(track_id.as_deref(), &notes)
}
#[tauri::command]
pub fn computer_midi_status(service: State<'_, Arc<AudioService>>) -> crate::audio::midi_input::Status {
    service.midi_input.computer_status()
}

#[tauri::command]
pub async fn export_audio(
    request: crate::project::export::Request,
    projects: State<'_,Arc<ProjectService>>,
    service: State<'_,Arc<AudioService>>,
    exports: State<'_,Arc<crate::project::export::ExportService>>,
)->AppResult<crate::project::export::Report> {
    let projects=projects.inner().clone();let service=service.inner().clone();let exports=exports.inner().clone();
    worker(move||{
        use crate::project::runtime::ProjectAudio;
        let rate=service.snapshot()?.output.map_or(48000,|o|o.sample_rate);
        if request.sample_rate!=rate{return Err(AppError::new("export_rate","출력 sample rate가 변경됐습니다. Export 창을 다시 열어 주세요."));}
        let (p,assets,path)=projects.export_snapshot(request.revision,service.automation_clock())?;
        exports.run(request,p,assets,path)
    }).await
}
#[tauri::command]
pub fn export_status(exports:State<'_,Arc<crate::project::export::ExportService>>)->crate::project::export::Status {exports.status()}
#[tauri::command]
pub fn export_cancel(job_id:String,exports:State<'_,Arc<crate::project::export::ExportService>>) {exports.cancel(&job_id);}

#[tauri::command]
pub fn plugin_catalog(plugins:State<'_,Arc<crate::plugins::Service>>)->serde_json::Value {plugins.snapshot()}
#[tauri::command]
pub async fn plugin_scan(paths:Vec<String>,plugins:State<'_,Arc<crate::plugins::Service>>)->AppResult<crate::plugins::Catalog>{
    let plugins=plugins.inner().clone();worker(move||plugins.scan(paths)).await
}
#[tauri::command]
pub async fn plugin_prepare(descriptor:crate::plugins::Descriptor,service:State<'_,Arc<AudioService>>)->AppResult<crate::plugins::Selection>{
    let rate=service.snapshot()?.output.map_or(48000,|o|o.sample_rate);worker(move||crate::plugins::prepare(descriptor,rate)).await
}
#[tauri::command]
pub async fn plugin_editor(window:tauri::WebviewWindow,instance_id:String,show:bool)->AppResult<()> {let owner=window.hwnd().map_err(|e|AppError::new("plugin_editor",e.to_string()))?.0 as usize;worker(move||crate::plugins::editor(&instance_id,show,owner)).await}
#[tauri::command]
pub async fn plugin_status(projects:State<'_,Arc<ProjectService>>)->AppResult<Vec<crate::plugins::RuntimeStatus>>{
    let keys=crate::plugins::project_keys(&projects.view()?.document);worker(move||Ok(crate::plugins::status(&keys))).await
}
