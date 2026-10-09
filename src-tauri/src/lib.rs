pub mod audio;
pub mod plugins;
pub mod error;
mod ipc;
pub mod preferences;
pub mod project;
pub mod state;
use std::sync::Arc;
use tauri::Manager;

pub fn run() -> tauri::Result<()> {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            app.manage(Arc::new(plugins::Service::new(app.path().app_config_dir()?.join("plugin-catalog.json"))));
            app.manage(Arc::new(project::export::ExportService::default()));
            app.manage(Arc::new(project::ProjectService::new(Some(
                app.path().app_config_dir()?.join("recent-projects.json"),
            ))));
            app.manage(Arc::new(state::AudioService::with_preferences(
                None,
                Some(app.path().app_config_dir()?.join("audio-preferences.json")),
            )?));
            Ok(())
        })
        .on_window_event(|window, event| {
            // Panel windows are ordinary, unowned taskbar windows. Their lifetime
            // still belongs to the one main workspace and its existing quit prompt.
            if window.label() == "main" && matches!(event, tauri::WindowEvent::Destroyed) {
                for (label, panel) in window.app_handle().webview_windows() {
                    if label.starts_with("panel-") { let _ = panel.destroy(); }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            ipc::plugin_catalog,
            ipc::plugin_scan,
            ipc::plugin_prepare,
            ipc::plugin_editor,
            ipc::plugin_status,
            ipc::app_version,
            ipc::export_audio,
            ipc::export_status,
            ipc::export_cancel,
            ipc::export_midi,
            ipc::midi_input_ports,
            ipc::midi_input_connect,
            ipc::midi_input_route,
            ipc::midi_input_status,
            ipc::computer_midi_notes,
            ipc::computer_midi_status,
            ipc::spectrum_configure,
            ipc::spectrum_snapshot,
            ipc::spectrum_analyze,
            ipc::spectrum_cancel,
            ipc::load_audio,
            ipc::transport_command,
            ipc::engine_snapshot,
            ipc::waveform_view,
            ipc::reconnect_output,
            ipc::output_devices,
            ipc::apply_audio_settings,
            ipc::asio_control_panel,
            ipc::project_snapshot,
            ipc::edit_project,
            ipc::asset_waveform,
            ipc::new_project,
            ipc::open_project,
            ipc::save_project,
            ipc::relink_asset,
            ipc::select_project_clip,
            ipc::remove_recent_project
        ])
        .run(tauri::generate_context!())
}
