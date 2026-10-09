use minidaw_lib::{
    audio::{
        decoder::{AudioData, FileInfo},
        metrics::AudioMetrics,
        renderer::{Action, Command, Renderer, COMMAND_CAPACITY},
        source::{AudioAsset, AudioSource},
        transport::TransportCell,
    },
    error::AppResult,
    project::{runtime::ProjectAudio, schema::*, session::ProjectService},
};
use serde_json::json;
use std::{cell::Cell, sync::Arc, time::Instant};

#[derive(Default)]
struct Runtime {
    installs: Cell<usize>,
    db: Cell<f64>,
}
impl ProjectAudio for Runtime {
    type Prepared = Arc<AudioAsset>;
    fn prepare_asset(&self, a: Self::Prepared) -> AppResult<Self::Prepared> {
        Ok(a)
    }
    fn install(&self, _: Option<Self::Prepared>) -> AppResult<()> {
        self.installs.set(self.installs.get() + 1);
        Ok(())
    }
    fn master_volume(&self, db: f64) {
        self.db.set(db);
    }
}
#[test]
fn master_persistence_legacy_default_history_and_no_source_replacement() {
    let s = ProjectService::new(None);
    let runtime = Runtime::default();
    let mut legacy = serde_json::to_value(Project::new()).unwrap();
    legacy.as_object_mut().unwrap().remove("master");
    assert_eq!(
        serde_json::from_value::<Project>(legacy)
            .unwrap()
            .master
            .volume_db,
        0.0
    );
    let apply = |value| {
        s.edit(
            &runtime,
            s.revision().unwrap(),
            serde_json::from_value(value).unwrap(),
        )
        .unwrap()
    };
    for db in [-2., -6., -12.] {
        apply(json!({"command":"master.volume","volumeDb":db,"historyGroup":"gesture-a"}));
    }
    assert_eq!(runtime.installs.get(), 0);
    assert_eq!(s.view().unwrap().history.undo, 1);
    apply(json!({"command":"edit.undo"}));
    assert_eq!(runtime.db.get(), 0.);
    apply(json!({"command":"edit.redo"}));
    assert_eq!(runtime.db.get(), -12.);
    assert_eq!(runtime.installs.get(), 0);
    let revision = s.revision().unwrap();
    assert!(s
        .edit(
            &runtime,
            revision,
            serde_json::from_value(json!({"command":"master.volume","volumeDb":13})).unwrap()
        )
        .is_err());
    assert_eq!(s.revision().unwrap(), revision);
    let folder = std::env::temp_dir().join(format!("minidaw-master-{}", id()));
    std::fs::create_dir(&folder).unwrap();
    let file = folder.join("master.minidaw");
    s.save(Some(&file), revision).unwrap();
    s.new_project(&runtime, s.revision().unwrap(), true)
        .unwrap();
    assert_eq!(runtime.db.get(), 0.);
    s.open(&runtime, &file, s.revision().unwrap(), true)
        .unwrap();
    assert_eq!(runtime.db.get(), -12.);
    assert_eq!(s.view().unwrap().document.master.volume_db, -12.);
    // Separate gestures are separate undo steps, even at the same control.
    apply(json!({"command":"master.volume","volumeDb":-24,"historyGroup":"gesture-b"}));
    apply(json!({"command":"master.volume","volumeDb":-30,"historyGroup":"gesture-c"}));
    apply(json!({"command":"edit.undo"}));
    assert_eq!(runtime.db.get(), -24.);
    std::fs::remove_dir_all(folder).unwrap();
}

#[test]
fn final_stereo_gain_ramp_meter_unity_and_silence() {
    for rate in [44100, 48000, 96000] {
        let (mut tx, rx) = rtrb::RingBuffer::new(COMMAND_CAPACITY);
        let metrics = Arc::new(AudioMetrics::default());
        let state = Arc::new(TransportCell::default());
        let mut renderer = Renderer::new(rx, state.clone(), metrics.clone(), rate, 2);
        let owner = AudioSource::memory(AudioData {
            info: FileInfo {
                name: "level".into(),
                sample_rate: rate,
                channels: 2,
                frames: rate as usize * 2,
                duration: 2.,
                sanitized_samples: 0,
            },
            samples: (0..rate * 2).flat_map(|_| [0.25, -0.5]).collect(),
        });
        for action in [
            Action::Load {
                clip_id: 1,
                audio: owner.clone(),
            },
            Action::Play,
        ] {
            tx.push(Command {
                id: 1,
                issued: Instant::now(),
                action,
            })
            .ok()
            .unwrap();
        }
        let mut out = vec![0f32; rate as usize / 100 * 2];
        renderer.render(&mut out);
        assert!(out.as_chunks::<2>().0.iter().all(|p| *p == [0.25, -0.5]));
        let db = metrics.master.snapshot().peak_db;
        assert!((db[0] + 12.0412).abs() < 0.001);
        assert!((db[1] + 6.0206).abs() < 0.001);
        metrics.master.set_db(-6.);
        renderer.render(&mut out);
        let gain = 10f32.powf(-6. / 20.);
        assert!((out[out.len() - 2] - 0.25 * gain).abs() < 1e-7);
        assert!(out
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| p[0])
            .collect::<Vec<_>>()
            .windows(2)
            .all(|p| p[0] >= p[1]));
        assert!(out
            .as_chunks::<2>()
            .0
            .iter()
            .all(|p| (p[1] + 2. * p[0]).abs() < 1e-7));
        assert_eq!(state.read().applied_command, 1); // Fader is not a transport command.
        metrics.master.set_db(12.);
        renderer.render(&mut out);
        assert_eq!(out[out.len() - 1], -1.);
        assert!(out[out.len() - 2] > 0.99);
        metrics.master.set_db(-96.);
        renderer.render(&mut out);
        assert_eq!(&out[out.len() - 2..], &[0., 0.]);
        metrics.master.reset_meter();
        renderer.render(&mut out);
        assert_eq!(metrics.master.snapshot().peak_db, [-120.; 2]);
        metrics.reset();
        assert_eq!(metrics.master.gain(), 0.); // Device reconnect retains fader.
    }
}
