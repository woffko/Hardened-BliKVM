//! Read-only autostart compatibility surface.
//!
//! Ported from the Hardened NanoKVM `GetAutostartRsp` shape. The hardened
//! server keeps this read-only: it lists and reads bounded autostart scripts
//! but performs no upload or delete from the network process.

use std::path::{Path, PathBuf};

use axum::{Json, extract::Path as AxumPath};
use serde::Serialize;

use crate::error::{ApiResponse, AppError, AppResult};

pub const MAX_AUTOSTART_BYTES: u64 = 256 * 1024;
const AUTOSTART_DIRECTORY: &str = "/etc/kvm/autostart";

#[derive(Debug, Serialize)]
pub struct AutostartList {
    pub files: Vec<String>,
}

pub async fn list() -> AppResult<Json<ApiResponse<AutostartList>>> {
    Ok(Json(ApiResponse::ok(AutostartList {
        files: list_files(Path::new(AUTOSTART_DIRECTORY)),
    })))
}

pub async fn content(AxumPath(name): AxumPath<String>) -> AppResult<Json<ApiResponse<String>>> {
    Ok(Json(ApiResponse::ok(read_content(
        Path::new(AUTOSTART_DIRECTORY),
        &name,
    )?)))
}

fn list_files(dir: &Path) -> Vec<String> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return files;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() || file_type.is_symlink() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if is_autostart_name(&name) {
            files.push(name);
        }
    }
    files.sort();
    files
}

fn read_content(dir: &Path, name: &str) -> AppResult<String> {
    let path = checked_path(dir, name)?;
    let metadata = std::fs::metadata(&path)?;
    if metadata.len() > MAX_AUTOSTART_BYTES {
        return Err(AppError::BadRequest("autostart file is too large".into()));
    }
    let bytes = std::fs::read(&path)?;
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

fn is_autostart_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn validate_autostart_name(name: &str) -> AppResult<String> {
    let name = name.trim();
    if is_autostart_name(name) {
        Ok(name.to_string())
    } else {
        Err(AppError::BadRequest("invalid autostart filename".into()))
    }
}

fn checked_path(dir: &Path, name: &str) -> AppResult<PathBuf> {
    let path = dir.join(validate_autostart_name(name)?);
    let metadata = std::fs::symlink_metadata(&path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(AppError::BadRequest("invalid autostart file".into()));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autostart_names_are_conservative() {
        assert!(is_autostart_name("boot.sh"));
        assert!(is_autostart_name("10-rescue_script"));
        assert!(!is_autostart_name(""));
        assert!(!is_autostart_name("."));
        assert!(!is_autostart_name(".."));
        assert!(!is_autostart_name("nested/boot.sh"));
        assert!(!is_autostart_name("back\\slash"));
        assert!(!is_autostart_name("bad name"));
        assert!(!is_autostart_name(&"a".repeat(129)));
        assert!(validate_autostart_name(" boot.sh ").is_ok());
    }

    #[test]
    fn listing_skips_symlinks_directories_and_invalid_names() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("boot.sh"), b"echo hi\n").unwrap();
        std::fs::create_dir(directory.path().join("nested")).unwrap();
        std::fs::write(directory.path().join("bad name"), b"x").unwrap();
        std::os::unix::fs::symlink("boot.sh", directory.path().join("link.sh")).unwrap();

        assert_eq!(list_files(directory.path()), vec!["boot.sh".to_string()]);
        assert!(list_files(Path::new("/nonexistent/autostart")).is_empty());
    }

    #[test]
    fn reading_bounds_and_validates_the_script() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join("boot.sh"), b"echo hi\n").unwrap();
        assert_eq!(
            read_content(directory.path(), "boot.sh").unwrap(),
            "echo hi\n"
        );
        assert!(read_content(directory.path(), "missing.sh").is_err());
        assert!(read_content(directory.path(), "../escape").is_err());

        let big = directory.path().join("big.sh");
        std::fs::write(&big, vec![b'x'; (MAX_AUTOSTART_BYTES + 1) as usize]).unwrap();
        assert!(read_content(directory.path(), "big.sh").is_err());
    }
}
