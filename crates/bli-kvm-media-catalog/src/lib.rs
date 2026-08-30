use std::{
    collections::BTreeSet,
    fs,
    os::unix::fs::{FileTypeExt, PermissionsExt},
    path::{Path, PathBuf},
};

use bli_kvm_core::VirtualMediaControlMode;
use serde::Deserialize;
use thiserror::Error;

const CATALOG_SCHEMA_VERSION: u16 = 1;
const MAX_CATALOG_BYTES: u64 = 256 * 1024;
const MAX_CATALOG_ENTRIES: usize = 1024;
const MAX_IMAGE_NAME_BYTES: usize = 128;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MediaCatalogError {
    #[error("media catalog path is invalid")]
    InvalidPath,
    #[error("media catalog directory is unsafe")]
    UnsafeDirectory,
    #[error("media catalog file is unsafe")]
    UnsafeFile,
    #[error("media catalog is too large")]
    TooLarge,
    #[error("media catalog JSON is invalid")]
    InvalidJson,
    #[error("media catalog entry is invalid")]
    InvalidEntry,
    #[error("media image ID was not found")]
    NotFound,
    #[error("media selection is not allowed by the catalog")]
    SelectionNotAllowed,
    #[error("media catalog I/O failed")]
    Io,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaSelection {
    image_id: u64,
    image_name: String,
    mode: VirtualMediaControlMode,
    writable: bool,
}

impl MediaSelection {
    pub const fn image_id(&self) -> u64 {
        self.image_id
    }

    pub fn image_name(&self) -> &str {
        &self.image_name
    }

    pub const fn mode(&self) -> VirtualMediaControlMode {
        self.mode
    }

    pub const fn writable(&self) -> bool {
        self.writable
    }
}

#[derive(Debug)]
pub struct MediaCatalog {
    entries: Vec<CatalogEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CatalogDocument {
    schema_version: u16,
    entries: Vec<CatalogEntry>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CatalogEntry {
    image_id: u64,
    image_name: String,
    size: u64,
    sha256: String,
    cdrom_allowed: bool,
    removable_allowed: bool,
    writable_allowed: bool,
}

impl MediaCatalog {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, MediaCatalogError> {
        let path = path.into();
        if !path.is_absolute() || path.file_name().is_none() {
            return Err(MediaCatalogError::InvalidPath);
        }
        validate_directory(path.parent().ok_or(MediaCatalogError::InvalidPath)?)?;
        validate_file(&path)?;
        let metadata = fs::symlink_metadata(&path).map_err(|_| MediaCatalogError::Io)?;
        if metadata.len() > MAX_CATALOG_BYTES {
            return Err(MediaCatalogError::TooLarge);
        }
        let bytes = fs::read(&path).map_err(|_| MediaCatalogError::Io)?;
        if bytes.len() as u64 > MAX_CATALOG_BYTES {
            return Err(MediaCatalogError::TooLarge);
        }
        let document: CatalogDocument =
            serde_json::from_slice(&bytes).map_err(|_| MediaCatalogError::InvalidJson)?;
        validate_document(&document)?;
        Ok(Self {
            entries: document.entries,
        })
    }

    pub fn resolve(
        &self,
        image_id: u64,
        mode: VirtualMediaControlMode,
        writable: bool,
    ) -> Result<MediaSelection, MediaCatalogError> {
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.image_id == image_id)
            .ok_or(MediaCatalogError::NotFound)?;
        let mode_allowed = match mode {
            VirtualMediaControlMode::CdRom => entry.cdrom_allowed,
            VirtualMediaControlMode::RemovableDisk => entry.removable_allowed,
        };
        if !mode_allowed
            || (writable && !entry.writable_allowed)
            || (mode == VirtualMediaControlMode::CdRom && writable)
        {
            return Err(MediaCatalogError::SelectionNotAllowed);
        }
        Ok(MediaSelection {
            image_id,
            image_name: entry.image_name.clone(),
            mode,
            writable,
        })
    }
}

fn validate_document(document: &CatalogDocument) -> Result<(), MediaCatalogError> {
    if document.schema_version != CATALOG_SCHEMA_VERSION
        || document.entries.len() > MAX_CATALOG_ENTRIES
    {
        return Err(MediaCatalogError::InvalidJson);
    }
    let mut ids = BTreeSet::new();
    let mut names = BTreeSet::new();
    for entry in &document.entries {
        if entry.image_id == 0
            || entry.size == 0
            || !valid_image_name(&entry.image_name)
            || !valid_sha256(&entry.sha256)
            || (!entry.cdrom_allowed && !entry.removable_allowed)
            || !ids.insert(entry.image_id)
            || !names.insert(entry.image_name.as_str())
        {
            return Err(MediaCatalogError::InvalidEntry);
        }
    }
    Ok(())
}

fn valid_image_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    (1..=MAX_IMAGE_NAME_BYTES).contains(&bytes.len())
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(*byte, b'.' | b'_' | b'-'))
        && !value.contains("..")
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && !value.bytes().all(|byte| byte == b'0')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_directory(path: &Path) -> Result<(), MediaCatalogError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| MediaCatalogError::Io)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.file_type().is_socket()
        || metadata.permissions().mode() & 0o022 != 0
    {
        return Err(MediaCatalogError::UnsafeDirectory);
    }
    Ok(())
}

fn validate_file(path: &Path) -> Result<(), MediaCatalogError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| MediaCatalogError::Io)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o022 != 0
    {
        return Err(MediaCatalogError::UnsafeFile);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    fn write_catalog(path: &Path, entries: &str) {
        fs::write(
            path,
            format!(r#"{{"schemaVersion":1,"entries":[{entries}]}}"#),
        )
        .unwrap();
    }

    fn valid_entry(id: u64, name: &str) -> String {
        format!(
            r#"{{"imageId":{id},"imageName":"{name}","size":4096,"sha256":"1111111111111111111111111111111111111111111111111111111111111111","cdromAllowed":true,"removableAllowed":true,"writableAllowed":false}}"#
        )
    }

    #[test]
    fn resolves_only_catalog_allowed_opaque_ids() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("catalog.json");
        write_catalog(&path, &valid_entry(7, "rescue.iso"));
        let catalog = MediaCatalog::open(&path).unwrap();
        let selection = catalog
            .resolve(7, VirtualMediaControlMode::CdRom, false)
            .unwrap();
        assert_eq!(selection.image_id(), 7);
        assert_eq!(selection.image_name(), "rescue.iso");
        assert_eq!(
            catalog.resolve(8, VirtualMediaControlMode::CdRom, false),
            Err(MediaCatalogError::NotFound)
        );
        assert_eq!(
            catalog.resolve(7, VirtualMediaControlMode::RemovableDisk, true),
            Err(MediaCatalogError::SelectionNotAllowed)
        );
    }

    #[test]
    fn rejects_paths_duplicates_and_unsafe_files() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("catalog.json");
        write_catalog(&path, &valid_entry(7, "../escape.iso"));
        assert!(matches!(
            MediaCatalog::open(&path),
            Err(MediaCatalogError::InvalidEntry)
        ));
        write_catalog(
            &path,
            &format!(
                "{},{}",
                valid_entry(7, "one.iso"),
                valid_entry(7, "two.iso")
            ),
        );
        assert!(matches!(
            MediaCatalog::open(&path),
            Err(MediaCatalogError::InvalidEntry)
        ));
        let dangling = directory.path().join("dangling.json");
        symlink(directory.path().join("missing"), &dangling).unwrap();
        assert!(matches!(
            MediaCatalog::open(dangling),
            Err(MediaCatalogError::UnsafeFile)
        ));
    }
}
