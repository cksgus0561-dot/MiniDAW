//! Per-user audio choices. Never stored in a music project or read in the callback.
use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DriverType {
    #[default]
    Wasapi,
    Asio,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AudioPreferences {
    pub driver_type: DriverType,
    pub output_device: Option<String>,
    pub asio_driver: Option<String>,
    pub transport_declick: bool,
}
impl Default for AudioPreferences {
    fn default() -> Self {
        Self {
            driver_type: DriverType::Wasapi,
            output_device: None,
            asio_driver: None,
            transport_declick: true,
        }
    }
}
impl AudioPreferences {
    pub fn selected_device(&self) -> Option<&str> {
        match self.driver_type {
            DriverType::Wasapi => self.output_device.as_deref(),
            DriverType::Asio => self.asio_driver.as_deref(),
        }
    }
    pub fn same_output(&self, other: &Self) -> bool {
        self.driver_type == other.driver_type && self.selected_device() == other.selected_device()
    }
    pub fn read(path: &Path) -> AppResult<Self> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
                AppError::new(
                    "preferences",
                    "오디오 설정 파일을 읽을 수 없습니다. 설정에서 출력 장치를 다시 선택해 주세요.",
                )
                .detail(e)
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => {
                Err(AppError::new("preferences", "오디오 설정 파일을 읽을 수 없습니다.").detail(e))
            }
        }
    }
    pub fn save(&self, path: &Path) -> AppResult<()> {
        let fail = |e| AppError::new("preferences", "오디오 설정을 저장하지 못했습니다.").detail(e);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| fail(e.to_string()))?;
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| fail(e.to_string()))?;
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, bytes).map_err(|e| fail(e.to_string()))?;
        std::fs::rename(temporary, path).map_err(|e| fail(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_and_roundtrip() {
        assert!(AudioPreferences::default().transport_declick);
        let preference = AudioPreferences {
            driver_type: DriverType::Asio,
            asio_driver: Some("Actual installed name".into()),
            transport_declick: false,
            ..Default::default()
        };
        let restored: AudioPreferences =
            serde_json::from_slice(&serde_json::to_vec(&preference).unwrap()).unwrap();
        assert_eq!(preference, restored);
        assert!(serde_json::from_str::<AudioPreferences>("{\"driverType\":\"unknown\"}").is_err());
    }
}
