use serde::Serialize;
use std::fmt;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppError {
    pub code: &'static str,
    pub message: String,
    pub detail: Option<String>,
}

impl AppError {
    pub fn stream_read(detail: impl fmt::Display) -> Self {
        Self::new(
            "stream_read",
            "오디오를 계속 읽을 수 없습니다. 원본 파일과 디스크 연결을 확인한 뒤 다시 열어 주세요.",
        )
        .detail(detail)
    }

    pub fn stream_worker() -> Self {
        Self::new(
            "stream_worker",
            "오디오 준비 작업이 중단되었습니다. 파일을 다시 열어 주세요.",
        )
    }

    pub fn waveform_read(detail: impl fmt::Display) -> Self {
        Self::new(
            "waveform_read",
            "파형 분석을 완료하지 못했습니다. 원본 파일과 디스크 연결을 확인해 주세요.",
        )
        .detail(detail)
    }

    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            detail: None,
        }
    }

    pub fn detail(mut self, detail: impl fmt::Display) -> Self {
        self.detail = Some(detail.to_string());
        self
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)?;
        if let Some(detail) = &self.detail {
            write!(f, " ({detail})")?;
        }
        Ok(())
    }
}

impl std::error::Error for AppError {}
