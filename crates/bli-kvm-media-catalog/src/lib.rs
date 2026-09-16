use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    os::unix::fs::{FileTypeExt, PermissionsExt},
    path::{Path, PathBuf},
};

use bli_kvm_core::VirtualMediaControlMode;
use serde::{Deserialize, Serialize};
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

/// Sanitized read-only view of one catalog entry for the operator UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaSummary {
    pub image_id: u64,
    pub image_name: String,
    pub size: u64,
    pub cdrom_allowed: bool,
    pub removable_allowed: bool,
    pub writable_allowed: bool,
}

#[derive(Debug)]
pub struct MediaCatalog {
    entries: Vec<CatalogEntry>,
}

/// One image to insert or replace in the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogImage {
    pub image_name: String,
    pub size: u64,
    pub sha256: String,
    pub cdrom_allowed: bool,
    pub removable_allowed: bool,
    pub writable_allowed: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CatalogDocument {
    schema_version: u16,
    entries: Vec<CatalogEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

    /// Returns the sanitized catalog entries in a stable order.
    pub fn summaries(&self) -> Vec<MediaSummary> {
        self.entries
            .iter()
            .map(|entry| MediaSummary {
                image_id: entry.image_id,
                image_name: entry.image_name.clone(),
                size: entry.size,
                cdrom_allowed: entry.cdrom_allowed,
                removable_allowed: entry.removable_allowed,
                writable_allowed: entry.writable_allowed,
            })
            .collect()
    }

    /// Find one sanitized entry by its exact file name.
    pub fn find_by_name(&self, image_name: &str) -> Option<MediaSummary> {
        self.summaries()
            .into_iter()
            .find(|entry| entry.image_name == image_name)
    }

    /// Remove one entry by image name and persist the catalog atomically.
    pub fn remove_image(path: &Path, image_name: &str) -> Result<MediaSummary, MediaCatalogError> {
        if !valid_image_name(image_name) {
            return Err(MediaCatalogError::InvalidEntry);
        }
        let catalog = Self::open(path)?;
        let mut entries = catalog.entries;
        let Some(index) = entries
            .iter()
            .position(|entry| entry.image_name == image_name)
        else {
            return Err(MediaCatalogError::NotFound);
        };
        let removed = entries.remove(index);
        let document = CatalogDocument {
            schema_version: CATALOG_SCHEMA_VERSION,
            entries,
        };
        validate_document(&document)?;
        write_document(path, &document)?;
        Ok(MediaSummary {
            image_id: removed.image_id,
            image_name: removed.image_name,
            size: removed.size,
            cdrom_allowed: removed.cdrom_allowed,
            removable_allowed: removed.removable_allowed,
            writable_allowed: removed.writable_allowed,
        })
    }

    /// Insert or replace one entry by image name and persist the catalog
    /// atomically. An existing entry keeps its image ID; a new one gets the
    /// next free ID. The catalog directory and file must already be safe.
    pub fn upsert_image(
        path: &Path,
        image: &CatalogImage,
    ) -> Result<MediaSummary, MediaCatalogError> {
        if image.size == 0
            || !valid_image_name(&image.image_name)
            || !valid_sha256(&image.sha256)
            || (!image.cdrom_allowed && !image.removable_allowed)
            || (image.cdrom_allowed && image.writable_allowed)
        {
            return Err(MediaCatalogError::InvalidEntry);
        }
        let catalog = Self::open(path)?;
        let mut entries = catalog.entries;
        let summary = if let Some(existing) = entries
            .iter_mut()
            .find(|entry| entry.image_name == image.image_name)
        {
            existing.size = image.size;
            existing.sha256.clone_from(&image.sha256);
            existing.cdrom_allowed = image.cdrom_allowed;
            existing.removable_allowed = image.removable_allowed;
            existing.writable_allowed = image.writable_allowed;
            MediaSummary {
                image_id: existing.image_id,
                image_name: existing.image_name.clone(),
                size: existing.size,
                cdrom_allowed: existing.cdrom_allowed,
                removable_allowed: existing.removable_allowed,
                writable_allowed: existing.writable_allowed,
            }
        } else {
            let image_id = entries
                .iter()
                .map(|entry| entry.image_id)
                .max()
                .unwrap_or(0)
                + 1;
            entries.push(CatalogEntry {
                image_id,
                image_name: image.image_name.clone(),
                size: image.size,
                sha256: image.sha256.clone(),
                cdrom_allowed: image.cdrom_allowed,
                removable_allowed: image.removable_allowed,
                writable_allowed: image.writable_allowed,
            });
            MediaSummary {
                image_id,
                image_name: image.image_name.clone(),
                size: image.size,
                cdrom_allowed: image.cdrom_allowed,
                removable_allowed: image.removable_allowed,
                writable_allowed: image.writable_allowed,
            }
        };
        let document = CatalogDocument {
            schema_version: CATALOG_SCHEMA_VERSION,
            entries,
        };
        validate_document(&document)?;
        write_document(path, &document)?;
        Ok(summary)
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

fn write_document(path: &Path, document: &CatalogDocument) -> Result<(), MediaCatalogError> {
    let directory = path.parent().ok_or(MediaCatalogError::InvalidPath)?;
    let file_name = path
        .file_name()
        .ok_or(MediaCatalogError::InvalidPath)?
        .to_string_lossy()
        .into_owned();
    validate_directory(directory)?;
    let bytes = serde_json::to_vec_pretty(document).map_err(|_| MediaCatalogError::InvalidJson)?;
    if bytes.len() as u64 > MAX_CATALOG_BYTES {
        return Err(MediaCatalogError::TooLarge);
    }
    let temporary = directory.join(format!(".{file_name}.tmp"));
    let result = (|| -> Result<(), MediaCatalogError> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temporary)
            .map_err(|_| MediaCatalogError::Io)?;
        file.write_all(&bytes).map_err(|_| MediaCatalogError::Io)?;
        file.sync_all().map_err(|_| MediaCatalogError::Io)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
        return result;
    }
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o644))
        .map_err(|_| MediaCatalogError::Io)?;
    fs::rename(&temporary, path).map_err(|_| MediaCatalogError::Io)
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
    fn summaries_are_sanitized_and_ordered() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("catalog.json");
        write_catalog(
            &path,
            &format!(
                "{},{}",
                valid_entry(7, "rescue.iso"),
                valid_entry(8, "tools.img")
            ),
        );
        let catalog = MediaCatalog::open(&path).unwrap();
        let summaries = catalog.summaries();
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].image_id, 7);
        assert_eq!(summaries[0].image_name, "rescue.iso");
        assert!(summaries[0].cdrom_allowed);
        assert!(!summaries[0].writable_allowed);
        assert_eq!(summaries[1].image_id, 8);
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

    fn image(name: &str, size: u64, sha: char, cdrom: bool) -> CatalogImage {
        CatalogImage {
            image_name: name.to_string(),
            size,
            sha256: sha.to_string().repeat(64),
            cdrom_allowed: cdrom,
            removable_allowed: true,
            writable_allowed: false,
        }
    }

    #[test]
    fn upsert_appends_with_the_next_id_and_persists_atomically() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("catalog.json");
        write_catalog(
            &path,
            r#"{"imageId":4,"imageName":"old.img","size":10,"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","cdromAllowed":false,"removableAllowed":true,"writableAllowed":false}"#,
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        let summary =
            MediaCatalog::upsert_image(&path, &image("new.img", 2048, 'b', false)).unwrap();
        assert_eq!(summary.image_id, 5);
        assert_eq!(summary.image_name, "new.img");
        let reloaded = MediaCatalog::open(&path).unwrap().summaries();
        assert_eq!(reloaded.len(), 2);
        assert_eq!(reloaded[1].image_id, 5);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o644
        );
        assert!(!path.with_file_name(".catalog.json.tmp").exists());
    }

    #[test]
    fn upsert_replaces_by_name_and_keeps_the_id() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("catalog.json");
        write_catalog(
            &path,
            r#"{"imageId":7,"imageName":"same.iso","size":10,"sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","cdromAllowed":true,"removableAllowed":true,"writableAllowed":false}"#,
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        let summary = MediaCatalog::upsert_image(&path, &image("same.iso", 99, 'c', true)).unwrap();
        assert_eq!(summary.image_id, 7);
        assert_eq!(summary.size, 99);
        assert_eq!(MediaCatalog::open(&path).unwrap().summaries().len(), 1);
    }

    #[test]
    fn upsert_rejects_invalid_images_and_unsafe_targets() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("catalog.json");
        write_catalog(&path, "");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            MediaCatalog::upsert_image(&path, &image("bad..img", 1, 'd', false)),
            Err(MediaCatalogError::InvalidEntry)
        );
        assert_eq!(
            MediaCatalog::upsert_image(&path, &image("ok.img", 0, 'd', false)),
            Err(MediaCatalogError::InvalidEntry)
        );
        let mut writable_cdrom = image("ok.iso", 1, 'd', true);
        writable_cdrom.writable_allowed = true;
        assert_eq!(
            MediaCatalog::upsert_image(&path, &writable_cdrom),
            Err(MediaCatalogError::InvalidEntry)
        );
        let mut nothing_allowed = image("ok.img", 1, 'd', false);
        nothing_allowed.removable_allowed = false;
        assert_eq!(
            MediaCatalog::upsert_image(&path, &nothing_allowed),
            Err(MediaCatalogError::InvalidEntry)
        );
    }
}
