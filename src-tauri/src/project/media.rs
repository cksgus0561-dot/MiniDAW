//! Shared asset/peak residency, independent of clips and undo history.
use crate::{
    audio::{
        analysis::Analysis,
        source::{AssetStorage, AudioAsset, MEMORY_PCM_BYTES},
    },
    error::AppResult,
};
use std::{collections::HashMap, path::Path, sync::Arc};
#[derive(Clone)]
pub struct Media {
    pub key: String,
    pub asset: Arc<AudioAsset>,
    pub waveform: Arc<Analysis>,
}
#[derive(Default)]
pub struct MediaPool {
    pub entries: HashMap<String, Media>,
    pub analysis_builds: u64,
}
impl MediaPool {
    pub fn get(&mut self, id: &str, path: &Path, key: &str) -> AppResult<Media> {
        if let Some(media) = self.entries.get(id).filter(|m| m.key == key) {
            return Ok(media.clone());
        }
        self.entries.remove(id);
        let resident: usize = self
            .entries
            .values()
            .map(|m| match &m.asset.storage {
                AssetStorage::Memory(d) => d.samples.len() * 4,
                _ => 0,
            })
            .sum();
        let asset = AudioAsset::open_with_budget(path, MEMORY_PCM_BYTES.saturating_sub(resident))?;
        let media = Media {
            key: key.into(),
            waveform: Arc::new(Analysis::new(&asset)?),
            asset,
        };
        self.analysis_builds += 1;
        self.entries.insert(id.into(), media.clone());
        Ok(media)
    }
}
