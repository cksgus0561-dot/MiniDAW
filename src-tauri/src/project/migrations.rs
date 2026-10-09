//! Add one pure `Value -> Value` step per historical schema here. V1 is the first
//! public format; no fictitious v0 format is silently guessed or coerced.
use super::schema::{invalid, Project, MAX_BYTES, VERSION};
use crate::error::{AppError, AppResult};
use serde_json::Value;
pub fn decode(bytes: &[u8]) -> AppResult<Project> {
    if bytes.len() as u64 > MAX_BYTES {
        return Err(invalid("프로젝트 파일이 16 MiB 제한을 초과합니다."));
    }
    let value: Value = serde_json::from_slice(bytes).map_err(invalid)?;
    let version = value
        .get("schemaVersion")
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid("schemaVersion 누락 / 형식"))?;
    if version > u64::from(VERSION) {
        return Err(AppError::new(
            "project_version_newer",
            "이 프로젝트는 더 새로운 버전의 MiniDAW에서 만들어졌습니다.",
        ));
    }
    let value = upgrade(version as u32, value)?;
    let project: Project = serde_json::from_value(value).map_err(invalid)?;
    project.validate()?;
    Ok(project)
}
fn upgrade(version: u32, value: Value) -> AppResult<Value> {
    match version {
        VERSION => Ok(value),
        // Future: 1 => upgrade(2, v1_to_v2(value)?), ...
        _ => Err(AppError::new(
            "project_version_unsupported",
            "지원하지 않는 이전 프로젝트 형식입니다.",
        )
        .detail(version)),
    }
}
pub fn encode(project: &Project) -> AppResult<Vec<u8>> {
    project.validate()?;
    let bytes = serde_json::to_vec_pretty(project).map_err(invalid)?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(invalid("프로젝트 파일이 16 MiB 제한을 초과합니다."));
    }
    Ok(bytes)
}
