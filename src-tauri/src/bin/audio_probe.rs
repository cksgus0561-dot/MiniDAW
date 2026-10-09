//! Real output-device smoke test. No microphone or loopback capture is opened.
use minidaw_lib::{
    audio::{renderer::Action, transport::PlayState},
    state::AudioService,
};
use std::{path::Path, thread, time::Duration};

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let folder = args
        .first()
        .map(String::as_str)
        .unwrap_or("../tests/fixtures");
    let buffer = args.get(1).map(|s| s.parse::<u32>()).transpose()?;
    let service = AudioService::new(buffer)?;
    for extension in ["wav", "mp3", "flac"] {
        service.load(&Path::new(folder).join(format!("stereo-44100.{extension}")))?;
        let send = |action| -> Result<(), Box<dyn std::error::Error>> {
            let id = service.command(action)?;
            service.wait_for(id, Duration::from_secs(2))?;
            Ok(())
        };
        send(Action::Play)?;
        thread::sleep(Duration::from_millis(180));
        assert!(service.snapshot()?.position > 0.05);
        send(Action::Pause)?;
        let paused = service.snapshot()?.position;
        thread::sleep(Duration::from_millis(70));
        assert_eq!(paused, service.snapshot()?.position);
        send(Action::Play)?;
        thread::sleep(Duration::from_millis(70));
        assert!(service.snapshot()?.position > paused);
        send(Action::Seek(3.0))?;
        let sought = service.snapshot()?;
        assert!((3.0..3.2).contains(&sought.position));
        assert_eq!(sought.transport.state, PlayState::Playing);
        send(Action::Stop)?;
        assert_eq!(service.snapshot()?.position, 0.0);
        assert_eq!(service.snapshot()?.stream_errors, 0);
        let metrics = service.metrics.snapshot();
        assert!(metrics.callbacks > 0);
        assert!(metrics.rendered_frames > 0);
        assert!(metrics.non_silent_frames > 0);
        assert!(metrics.play_ms.is_some());
        assert!(metrics.seek_ms.is_some());
        // Invalid input must preserve the currently loaded file and engine.
        let current = service.snapshot()?.transport.clip_id;
        assert!(service
            .load(&Path::new(folder).join("corrupt.wav"))
            .is_err());
        assert_eq!(service.snapshot()?.transport.clip_id, current);
        println!("{}", serde_json::to_string(&service.snapshot()?)?);
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("실제 출력 검증 실패: {error}");
        std::process::exit(1);
    }
}
