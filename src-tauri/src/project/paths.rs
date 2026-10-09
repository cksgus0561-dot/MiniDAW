use super::schema::{invalid, Fingerprint, Frames, PathReference};
use crate::error::{AppError, AppResult};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
};

pub fn validate_reference(r: &PathReference) -> AppResult<()> {
    if r.project_relative_path.is_none() && r.original_absolute_path.is_none() {
        return Err(invalid("asset 경로 누락"));
    }
    if let Some(p) = &r.project_relative_path {
        validate_path(p)?;
        if p.contains(':')
            || p.starts_with(['/', '\\'])
            || p.split(['/', '\\'])
                .any(|c| c.is_empty() || c == ".." || c == ".")
        {
            return Err(invalid("안전하지 않은 상대 경로"));
        }
    }
    if let Some(p) = &r.original_absolute_path {
        validate_path(p)?;
        if !Path::new(p).is_absolute() {
            return Err(invalid("절대 경로가 아닙니다."));
        }
    }
    Ok(())
}
pub fn validate_path(p: &str) -> AppResult<()> {
    if p.is_empty()
        || p.len() > 32768
        || p.chars().any(|c| c == '\0' || c < ' ')
        || p.starts_with("\\\\.\\")
        || p.starts_with("\\\\?\\")
    {
        return Err(invalid("잘못된 파일 경로"));
    }
    #[cfg(windows)]
    if p.chars()
        .enumerate()
        .any(|(i, c)| matches!(c, '<' | '>' | '"' | '|' | '?' | '*') || (c == ':' && i != 1))
    {
        return Err(invalid("잘못된 Windows 파일 경로"));
    }
    Ok(())
}
pub fn display(path: &Path) -> String {
    let s = path.to_string_lossy();
    if let Some(s) = s.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{s}")
    } else {
        s.strip_prefix("\\\\?\\").unwrap_or(&s).to_owned()
    }
}
pub fn absolute(path: &Path) -> AppResult<PathBuf> {
    let text = display(path);
    validate_path(&text)?;
    let p = std::path::absolute(Path::new(&text)).map_err(invalid)?;
    // Lexically normalize without requiring the media to exist (missing relinks).
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            _ => out.push(c),
        }
    }
    Ok(out)
}
pub fn project_destination(path: &Path) -> AppResult<PathBuf> {
    let mut p = absolute(path)?;
    if p.extension().is_none() {
        p.set_extension("minidaw");
    }
    if !p
        .extension()
        .is_some_and(|s| s.eq_ignore_ascii_case("minidaw"))
    {
        return Err(AppError::new(
            "project_extension",
            "프로젝트는 .minidaw 확장자로 저장해 주세요.",
        ));
    }
    let parent = p.parent().ok_or_else(|| invalid("프로젝트 폴더"))?;
    let parent = std::fs::canonicalize(parent).map_err(|e| {
        AppError::new("project_destination", "저장할 폴더를 사용할 수 없습니다.").detail(e)
    })?;
    Ok(Path::new(&display(&parent))
        .join(p.file_name().ok_or_else(|| invalid("프로젝트 파일 이름"))?))
}
pub fn candidates(reference: &PathReference, project: Option<&Path>) -> Vec<PathBuf> {
    let mut out = vec![];
    if let (Some(relative), Some(parent)) = (
        &reference.project_relative_path,
        project.and_then(Path::parent),
    ) {
        out.push(parent.join(relative));
    }
    if let Some(path) = &reference.original_absolute_path {
        let p = PathBuf::from(path);
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}
pub fn reference(path: &Path, project: Option<&Path>) -> PathReference {
    let relative = project
        .and_then(Path::parent)
        .and_then(|p| path.strip_prefix(p).ok())
        .filter(|p| !p.as_os_str().is_empty())
        .map(|p| display(p).replace('\\', "/"));
    PathReference {
        project_relative_path: relative,
        original_absolute_path: Some(display(path)),
    }
}
pub fn fingerprint(path: &Path) -> AppResult<Fingerprint> {
    let get = || -> std::io::Result<Fingerprint> {
        let mut file = File::open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(std::io::Error::other("not a regular file"));
        }
        let size = metadata.len();
        let mut hash = Sha256::new();
        hash.update(size.to_le_bytes());
        let mut buffer = vec![0; 64 * 1024];
        let n = (size.min(buffer.len() as u64)) as usize;
        file.read_exact(&mut buffer[..n])?;
        hash.update(&buffer[..n]);
        if size > n as u64 {
            let tail = (size - n as u64).min(buffer.len() as u64) as usize;
            file.seek(SeekFrom::End(-(tail as i64)))?;
            file.read_exact(&mut buffer[..tail])?;
            hash.update(&buffer[..tail]);
        }
        Ok(Fingerprint {
            file_bytes: Frames(size),
            sampled_sha256: format!("{:x}", hash.finalize()),
        })
    };
    get().map_err(|e| AppError::new("asset_read", "오디오 파일을 확인할 수 없습니다.").detail(e))
}
