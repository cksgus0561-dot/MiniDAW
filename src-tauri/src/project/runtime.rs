//! Transaction boundary for preparing and installing immutable arrangements.
//! Tests can exercise storage/commit failures without an output device.
use crate::{
    audio::source::AudioAsset,
    error::AppResult,
    state::{AudioService, LoadedAudio},
};
use std::sync::Arc;
#[derive(Clone, Copy)]
pub struct AutomationClock {
    pub frame: u64,
    pub rate: u32,
    pub playing: bool,
    pub cycle_pass: u64,
}
pub trait ProjectAudio {
    fn automation_clock(&self) -> Option<AutomationClock> {
        None
    }
    fn replace_automation(&self, p: Option<Self::Prepared>) -> AppResult<()> {
        self.replace(p)
    }
    type Prepared;
    fn prepare_asset(&self, asset: Arc<AudioAsset>) -> AppResult<Self::Prepared>;
    fn install(&self, prepared: Option<Self::Prepared>) -> AppResult<()>;
    fn effects(&self, _p: &super::schema::Project) {}
    fn master_volume(&self, _db: f64) {}
    fn arrangement(&self) -> bool {
        false
    }
    fn prepare_project(
        &self,
        _document: &super::schema::Project,
        _states: &[super::session::AssetState],
    ) -> AppResult<Option<Self::Prepared>> {
        Ok(None)
    }
    fn replace(&self, prepared: Option<Self::Prepared>) -> AppResult<()> {
        self.install(prepared)
    }
}
impl ProjectAudio for AudioService {
    fn automation_clock(&self) -> Option<AutomationClock> {
        let s = self.snapshot().ok()?;
        let rate = s.output.as_ref()?.sample_rate;
        Some(AutomationClock {
            frame: (s.position * rate as f64).round() as u64,
            cycle_pass: s.transport.cycle_pass,
            rate,
            playing: s.transport.state == crate::audio::transport::PlayState::Playing,
        })
    }
    fn replace_automation(&self, p: Option<Self::Prepared>) -> AppResult<()> {
        self.install_mode_automation(p).map(|_| ())
    }
    type Prepared = LoadedAudio;
    fn effects(&self, p: &super::schema::Project) {
        self.metrics.effects.configure(p);
    }
    fn master_volume(&self, db: f64) {
        self.metrics.master.set_db(db);
    }
    fn prepare_asset(&self, asset: Arc<AudioAsset>) -> AppResult<Self::Prepared> {
        AudioService::prepare_asset(self, asset)
    }
    fn install(&self, prepared: Option<Self::Prepared>) -> AppResult<()> {
        AudioService::install(self, prepared).map(|_| ())
    }
    fn arrangement(&self) -> bool {
        true
    }
    fn prepare_project(
        &self,
        document: &super::schema::Project,
        states: &[super::session::AssetState],
    ) -> AppResult<Option<Self::Prepared>> {
        self.prepare_project(document, states)
    }
    fn replace(&self, prepared: Option<Self::Prepared>) -> AppResult<()> {
        self.install_mode(prepared, true).map(|_| ())
    }
}
impl<T: ProjectAudio> ProjectAudio for Arc<T> {
    fn automation_clock(&self) -> Option<AutomationClock> {
        (**self).automation_clock()
    }
    fn replace_automation(&self, p: Option<Self::Prepared>) -> AppResult<()> {
        (**self).replace_automation(p)
    }
    type Prepared = T::Prepared;
    fn effects(&self, p: &super::schema::Project) {
        (**self).effects(p);
    }
    fn master_volume(&self, db: f64) {
        (**self).master_volume(db);
    }
    fn prepare_asset(&self, asset: Arc<AudioAsset>) -> AppResult<Self::Prepared> {
        (**self).prepare_asset(asset)
    }
    fn install(&self, prepared: Option<Self::Prepared>) -> AppResult<()> {
        (**self).install(prepared)
    }
    fn arrangement(&self) -> bool {
        (**self).arrangement()
    }
    fn prepare_project(
        &self,
        p: &super::schema::Project,
        s: &[super::session::AssetState],
    ) -> AppResult<Option<Self::Prepared>> {
        (**self).prepare_project(p, s)
    }
    fn replace(&self, p: Option<Self::Prepared>) -> AppResult<()> {
        (**self).replace(p)
    }
}
