//! Sibling temporary + flush/sync + one rename. Never truncate/remove the target.
use super::{
    migrations,
    schema::{self, Project, MAX_BYTES},
};
use crate::error::{AppError, AppResult};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
fn io_error(e: impl std::fmt::Display) -> AppError {
    AppError::new(
        "project_io",
        "프로젝트 파일을 읽거나 저장할 수 없습니다. 파일 경로와 쓰기 권한을 확인해 주세요.",
    )
    .detail(e)
}
pub fn read(path: &Path) -> AppResult<Project> {
    let file = File::open(path).map_err(io_error)?;
    if !file.metadata().map_err(io_error)?.is_file() {
        return Err(io_error("일반 파일이 아닙니다."));
    }
    let mut bytes = vec![];
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    migrations::decode(&bytes)
}
pub fn save(path: &Path, project: &Project) -> AppResult<usize> {
    let bytes = migrations::encode(project)?;
    // Verify the exact bytes before committing the filesystem transaction.
    let decoded = migrations::decode(&bytes)?;
    if decoded != *project {
        return Err(schema::invalid("저장 round-trip 불일치"));
    }
    atomic_write(path, &bytes)?;
    Ok(bytes.len())
}
struct Temporary(PathBuf);
impl Drop for Temporary {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
pub fn atomic_write(path: &Path, bytes: &[u8]) -> AppResult<()> {
    atomic_write_with(path, bytes, |_| Ok(()))
}
// Injection is limited to unit tests; production always uses the no-op hook.
fn atomic_write_with(
    path: &Path,
    bytes: &[u8],
    before_rename: impl FnOnce(&Path) -> std::io::Result<()>,
) -> AppResult<()> {
    let parent = path.parent().ok_or_else(|| io_error("부모 폴더 없음"))?;
    let temp = Temporary(parent.join(format!(".minidaw-{}.tmp", schema::id())));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp.0)
        .map_err(io_error)?;
    file.write_all(bytes).map_err(io_error)?;
    file.flush().map_err(io_error)?;
    file.sync_all().map_err(io_error)?;
    drop(file);
    before_rename(&temp.0).map_err(io_error)?;
    // std Windows rename uses MoveFileExW / SetFileInformationByHandle; same volume.
    fs::rename(&temp.0, path).map_err(io_error)?;
    #[cfg(unix)]
    {
        if let Ok(dir) = File::open(parent) {
            let _ = dir.sync_all();
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn injected_write_commit_failure_preserves_old_target_and_cleans_temp() {
        let dir = std::env::temp_dir().join(schema::id());
        fs::create_dir(&dir).unwrap();
        let path = dir.join("song.minidaw");
        fs::write(&path, b"old valid state").unwrap();
        assert!(
            atomic_write_with(&path, b"new", |_| Err(std::io::Error::other(
                "simulated disk/commit failure"
            )))
            .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), b"old valid state");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_file(path).unwrap();
        fs::remove_dir(dir).unwrap();
    }
}
