//! Worker-only analysis strategy. No source rewrite and no full decoded residency.
use super::schema::*;
use crate::{
    audio::{resample::PlaybackReader, source::AudioAsset},
    error::AppResult,
};
use std::{path::Path, sync::Arc};
pub trait NormalizeStrategy {
    fn observe(&mut self, frame: [f32; 2]);
    fn gain(&self, target_db: f64) -> Option<f64>;
}
#[derive(Default)]
pub struct Peak {
    pub peak: f64,
}
impl NormalizeStrategy for Peak {
    fn observe(&mut self, frame: [f32; 2]) {
        for sample in frame {
            self.peak = self.peak.max((sample as f64).abs());
        }
    }
    fn gain(&self, target_db: f64) -> Option<f64> {
        (self.peak > 0.0).then(|| 10_f64.powf(target_db / 20.0) / self.peak)
    }
}
pub fn analyze(
    path: &Path,
    start: u64,
    end: u64,
    strategy: &mut impl NormalizeStrategy,
) -> AppResult<()> {
    let asset = AudioAsset::open_with_budget(path, 0)?;
    analyze_asset(asset, start, end, strategy)
}
pub fn analyze_asset(
    asset: Arc<AudioAsset>,
    start: u64,
    end: u64,
    strategy: &mut impl NormalizeStrategy,
) -> AppResult<()> {
    if start >= end || end > asset.info.frames as u64 {
        return Err(invalid("Normalize 원본 범위"));
    }
    let mut reader = PlaybackReader::new(asset.clone(), asset.info.sample_rate)?;
    reader.seek(start as usize, Arc::new(|| false))?;
    for _ in start..end {
        strategy.observe(reader.read_frame()?);
    }
    Ok(())
}
