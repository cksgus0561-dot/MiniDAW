//! Real output-only device validation. No recording or driver configuration changes.
use minidaw_lib::{
    audio::renderer::Action,
    preferences::{AudioPreferences, DriverType},
    state::AudioService,
};
use serde_json::json;
use std::{path::Path, thread, time::Duration};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    let backend = if args.get(1).is_some_and(|s| s == "asio") {
        DriverType::Asio
    } else {
        DriverType::Wasapi
    };
    let service = AudioService::new(None)?;
    let devices = service.devices(backend)?;
    eprintln!("Devices: {devices:?}");
    let preferences = AudioPreferences {
        driver_type: backend,
        asio_driver: if backend == DriverType::Asio {
            devices.first().cloned()
        } else {
            None
        },
        transport_declick: args.get(3).is_none_or(|s| s != "off"),
        ..Default::default()
    };
    service.apply_settings(preferences)?;
    let mut records = Vec::new();
    for file in [
        "tests/fixtures/stereo-44100.wav",
        "tests/fixtures/stereo-44100.mp3",
        "tests/fixtures/stereo-44100.flac",
        "tests/fixtures/모노-48000.WAV",
    ] {
        service.load(Path::new(file))?;
        let mut commands = Vec::new();
        for action in [
            Action::Play,
            Action::Pause,
            Action::Play,
            Action::Seek(0.137),
            Action::Stop,
        ] {
            let id = service.command(action)?;
            service.wait_for(id, Duration::from_secs(5))?;
            thread::sleep(Duration::from_millis(80));
            commands.push(service.snapshot()?);
        }
        records.push(json!({"file":file,"commands":commands}));
    }
    let result =
        json!({"devices":devices,"backend":backend,"runs":records,"final":service.snapshot()?});
    if let Some(path) = args.get(2) {
        std::fs::write(path, serde_json::to_vec_pretty(&result)?)?;
    }
    println!("{}", serde_json::to_string(&service.snapshot()?)?);
    Ok(())
}
