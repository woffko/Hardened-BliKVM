use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::CString,
    fs::{self, File, OpenOptions, Permissions},
    io::{self, Read, Seek, SeekFrom, Write},
    os::{
        fd::AsRawFd,
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        },
    },
    path::{Path, PathBuf},
};

use bli_kvm_update_verifier::{
    APPLICATION_MANIFEST_PATH, APPLICATION_PAYLOAD_ROOT, APPLICATION_SIGNATURE_PATH,
    ApplicationFile, ApplicationManifest, ApplicationTrustRoot, MAX_ARCHIVE_BYTES,
    MAX_MANIFEST_BYTES, MAX_PAYLOAD_BYTES, MAX_PAYLOAD_FILES, UpdateVerifyError,
    VerifiedApplicationBundle, verify_application_manifest_bytes, verify_application_reader,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const SLOT_METADATA_DIRECTORY: &str = ".hardened-blikvm";
pub const SLOT_RECEIPT_PATH: &str = ".hardened-blikvm/receipt.json";
pub const SLOT_MANIFEST_PATH: &str = ".hardened-blikvm/manifest.json";
pub const SLOT_SIGNATURE_PATH: &str = ".hardened-blikvm/manifest.sig";

const SLOT_RECEIPT_SCHEMA_VERSION: u32 = 1;
const STAGING_MARKER_SCHEMA_VERSION: u32 = 1;
const STAGING_MARKER_PURPOSE: &str = "hardened-blikvm-application-staging";
const DELETION_MARKER_SCHEMA_VERSION: u32 = 1;
const DELETION_MARKER_PURPOSE: &str = "hardened-blikvm-slot-deletion";
const MAX_SLOT_RECEIPT_BYTES: u64 = 4 * 1024;
const MAX_TREE_ENTRIES: usize = MAX_PAYLOAD_FILES * 2 + 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageDisposition {
    Published,
    AlreadyPresent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedApplication {
    pub slot_id: String,
    pub slot_path: PathBuf,
    pub disposition: StageDisposition,
    pub manifest: ApplicationManifest,
    pub manifest_sha256: String,
    pub payload_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StagingCleanup {
    pub removed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotDeletion {
    pub slot_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeletionRecovery {
    pub completed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SlotReceipt {
    schema_version: u32,
    slot_id: String,
    manifest_sha256: String,
    payload_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StagingMarker {
    schema_version: u32,
    purpose: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DeletionMarker {
    schema_version: u32,
    purpose: String,
    slot_id: String,
    manifest_sha256: String,
}

#[derive(Debug, Error)]
pub enum ApplicationStageError {
    #[error("application slot root path is invalid")]
    InvalidRoot,
    #[error("application slot root is unsafe")]
    UnsafeRoot,
    #[error("another application stager owns the slot-root lock")]
    StagerBusy,
    #[error("application slot identifier is invalid")]
    InvalidSlotId,
    #[error("application source path is invalid")]
    InvalidSource,
    #[error("application source file is unsafe")]
    UnsafeSource,
    #[error("application source changed while it was copied")]
    SourceChanged,
    #[error("application archive exceeded its size limit")]
    ArchiveTooLarge,
    #[error("application archive verification failed")]
    Verify(#[from] UpdateVerifyError),
    #[error("application archive extraction failed")]
    InvalidArchive,
    #[error("application extraction does not match the verified archive")]
    ExtractionMismatch,
    #[error("an existing application slot is unsafe or corrupt")]
    UnsafeExistingSlot,
    #[error("an abandoned application staging tree is unsafe")]
    UnsafeAbandonedStaging,
    #[error("application slot deletion state is unsafe or inconsistent")]
    UnsafeDeletionState,
    #[error("application slot is protected by active update state")]
    ProtectedSlot,
    #[error("application slot does not exist")]
    SlotNotFound,
    #[error("application slot root contains an unexpected entry")]
    UnexpectedRootEntry,
    #[error("an application slot conflicts with the verified bundle")]
    SlotConflict,
    #[error("atomic no-replace slot publication is unavailable")]
    AtomicPublishUnavailable,
    #[error("application slot metadata serialization failed")]
    Serialization,
    #[error("application staging I/O failed")]
    Io(#[from] io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RootIdentity {
    device: u64,
    inode: u64,
    owner: u32,
}

#[derive(Debug)]
pub struct ApplicationSlotStager {
    root: PathBuf,
    root_directory: File,
    identity: RootIdentity,
}

impl ApplicationSlotStager {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, ApplicationStageError> {
        let root = root.into();
        let identity = validate_root_path(&root)?;
        let root_directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(&root)
            .map_err(|_| ApplicationStageError::UnsafeRoot)?;
        if identity_from_metadata(&root_directory.metadata()?) != identity {
            return Err(ApplicationStageError::UnsafeRoot);
        }
        // SAFETY: flock observes only the valid owned directory descriptor.
        let lock_result =
            unsafe { libc::flock(root_directory.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if lock_result != 0 {
            return Err(ApplicationStageError::StagerBusy);
        }
        Ok(Self {
            root,
            root_directory,
            identity,
        })
    }

    pub fn root_path(&self) -> &Path {
        &self.root
    }

    pub fn stage(
        &self,
        source: impl AsRef<Path>,
        trust_root: &ApplicationTrustRoot,
    ) -> Result<StagedApplication, ApplicationStageError> {
        self.validate_root()?;
        let (deletion_markers, deletion_tombstones) = self.collect_deletion_state()?;
        if !deletion_markers.is_empty() || !deletion_tombstones.is_empty() {
            return Err(ApplicationStageError::UnsafeDeletionState);
        }
        let (mut snapshot, archive_size) = self.snapshot_source(source.as_ref())?;
        let verified = verify_application_reader(&mut snapshot, archive_size, trust_root)?;
        let slot_id = slot_id(&verified);
        let slot_path = self.root.join(&slot_id);
        if let Some(existing) = self.validate_if_present(&slot_path, &verified, trust_root)? {
            return Ok(existing);
        }

        snapshot.seek(SeekFrom::Start(0))?;
        let staging = tempfile::Builder::new()
            .prefix(".staging-")
            .tempdir_in(&self.root)?;
        fs::set_permissions(staging.path(), Permissions::from_mode(0o700))?;
        self.validate_root()?;
        validate_staging_parent(staging.path(), self.identity)?;
        create_staging_marker(staging.path())?;

        create_payload_directories(staging.path(), &verified.manifest)?;
        let proof = extract_verified_archive(&mut snapshot, staging.path(), &verified)?;
        let proof_manifest = verify_application_manifest_bytes(
            &proof.manifest_bytes,
            &proof.signature_bytes,
            trust_root,
        )?;
        if proof_manifest != verified.manifest
            || sha256_hex(&proof.manifest_bytes) != verified.manifest_sha256
        {
            return Err(ApplicationStageError::ExtractionMismatch);
        }

        let receipt = SlotReceipt {
            schema_version: SLOT_RECEIPT_SCHEMA_VERSION,
            slot_id: slot_id.clone(),
            manifest_sha256: verified.manifest_sha256.clone(),
            payload_bytes: verified.payload_bytes,
        };
        write_slot_metadata(staging.path(), &receipt, &proof)?;
        if let Err(error) = harden_staged_tree(staging.path(), &verified.manifest) {
            let _ = make_tree_owner_writable(staging.path());
            return Err(error);
        }

        if let Err(error) = self.validate_root() {
            let _ = make_tree_owner_writable(staging.path());
            return Err(error);
        }
        match rename_child_no_replace(
            &self.root_directory,
            staging.path(),
            slot_path
                .file_name()
                .ok_or(ApplicationStageError::InvalidRoot)?,
        ) {
            Ok(()) => {
                self.root_directory.sync_all()?;
            }
            Err(error) if error.raw_os_error() == Some(libc::EEXIST) => {
                let _ = make_tree_owner_writable(staging.path());
                return self
                    .validate_if_present(&slot_path, &verified, trust_root)?
                    .ok_or(ApplicationStageError::SlotConflict);
            }
            Err(error) if matches!(error.raw_os_error(), Some(libc::ENOSYS | libc::EINVAL)) => {
                let _ = make_tree_owner_writable(staging.path());
                return Err(ApplicationStageError::AtomicPublishUnavailable);
            }
            Err(error) => {
                let _ = make_tree_owner_writable(staging.path());
                return Err(ApplicationStageError::Io(error));
            }
        }

        validate_published_slot(&slot_path, &verified, trust_root, self.identity)?;
        Ok(staged_result(
            slot_id,
            slot_path,
            StageDisposition::Published,
            verified,
        ))
    }

    pub fn cleanup_abandoned_staging(&self) -> Result<StagingCleanup, ApplicationStageError> {
        self.validate_root()?;
        let mut removed = 0_usize;
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let name = entry
                .file_name()
                .to_str()
                .ok_or(ApplicationStageError::UnsafeAbandonedStaging)?
                .to_string();
            if !name.starts_with(".staging-") {
                continue;
            }
            if !valid_staging_name(&name) {
                return Err(ApplicationStageError::UnsafeAbandonedStaging);
            }
            let path = entry.path();
            validate_abandoned_staging_tree(&path, self.identity)?;
            make_tree_owner_writable(&path)
                .map_err(|_| ApplicationStageError::UnsafeAbandonedStaging)?;
            fs::remove_dir_all(&path)?;
            self.root_directory.sync_all()?;
            removed = removed.saturating_add(1);
        }
        Ok(StagingCleanup { removed })
    }

    pub fn list_slots(
        &self,
        trust_root: &ApplicationTrustRoot,
    ) -> Result<Vec<StagedApplication>, ApplicationStageError> {
        self.validate_root()?;
        let mut slots = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let name = entry
                .file_name()
                .to_str()
                .ok_or(ApplicationStageError::UnexpectedRootEntry)?
                .to_string();
            if valid_slot_id(&name) {
                slots.push(self.inspect_slot(&name, trust_root)?);
            } else if matches!(name.as_str(), "current" | ".current-next") {
                continue;
            } else {
                return Err(ApplicationStageError::UnexpectedRootEntry);
            }
        }
        slots.sort_by(|left, right| {
            left.manifest
                .sequence
                .cmp(&right.manifest.sequence)
                .then_with(|| left.slot_id.cmp(&right.slot_id))
        });
        Ok(slots)
    }

    pub fn recover_slot_deletions(
        &self,
        protected_slot_ids: &BTreeSet<String>,
        trust_root: &ApplicationTrustRoot,
    ) -> Result<DeletionRecovery, ApplicationStageError> {
        self.validate_root()?;
        let protected = self.with_current_protected(protected_slot_ids)?;
        let (markers, tombstones) = self.collect_deletion_state()?;
        for tombstone in &tombstones {
            if !markers.contains_key(tombstone) {
                return Err(ApplicationStageError::UnsafeDeletionState);
            }
        }
        let mut completed = 0_usize;
        for (slot_id, marker) in markers {
            if protected.contains(&slot_id) {
                return Err(ApplicationStageError::ProtectedSlot);
            }
            self.recover_one_deletion(&marker, trust_root)?;
            completed = completed.saturating_add(1);
        }
        Ok(DeletionRecovery { completed })
    }

    pub fn delete_unreferenced_slot(
        &self,
        slot_id: &str,
        protected_slot_ids: &BTreeSet<String>,
        trust_root: &ApplicationTrustRoot,
    ) -> Result<SlotDeletion, ApplicationStageError> {
        if !valid_slot_id(slot_id) {
            return Err(ApplicationStageError::InvalidSlotId);
        }
        let protected = self.with_current_protected(protected_slot_ids)?;
        if protected.contains(slot_id) {
            return Err(ApplicationStageError::ProtectedSlot);
        }
        let _ = self.recover_slot_deletions(&protected, trust_root)?;
        let staged = match self.inspect_slot(slot_id, trust_root) {
            Ok(staged) => staged,
            Err(ApplicationStageError::UnsafeExistingSlot) if !self.root.join(slot_id).exists() => {
                return Err(ApplicationStageError::SlotNotFound);
            }
            Err(error) => return Err(error),
        };
        let marker = DeletionMarker {
            schema_version: DELETION_MARKER_SCHEMA_VERSION,
            purpose: DELETION_MARKER_PURPOSE.to_string(),
            slot_id: staged.slot_id.clone(),
            manifest_sha256: staged.manifest_sha256,
        };
        self.write_deletion_marker(&marker)?;
        self.recover_one_deletion(&marker, trust_root)?;
        Ok(SlotDeletion {
            slot_id: staged.slot_id,
        })
    }

    pub fn inspect_slot(
        &self,
        slot_id: &str,
        trust_root: &ApplicationTrustRoot,
    ) -> Result<StagedApplication, ApplicationStageError> {
        self.validate_root()?;
        if !valid_slot_id(slot_id) {
            return Err(ApplicationStageError::InvalidSlotId);
        }
        let slot_path = self.root.join(slot_id);
        let verified = load_published_slot(&slot_path, trust_root, self.identity)?;
        Ok(staged_result(
            slot_id.to_string(),
            slot_path,
            StageDisposition::AlreadyPresent,
            verified,
        ))
    }

    fn with_current_protected(
        &self,
        protected_slot_ids: &BTreeSet<String>,
    ) -> Result<BTreeSet<String>, ApplicationStageError> {
        let mut protected = protected_slot_ids.clone();
        let current = self.root.join("current");
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if !metadata.file_type().is_symlink()
                    || metadata.dev() != self.identity.device
                    || metadata.uid() != self.identity.owner
                    || metadata.nlink() != 1
                {
                    return Err(ApplicationStageError::UnsafeDeletionState);
                }
                let target = fs::read_link(current)?;
                let target = target
                    .to_str()
                    .ok_or(ApplicationStageError::UnsafeDeletionState)?;
                if target.contains('/') || !valid_slot_id(target) {
                    return Err(ApplicationStageError::UnsafeDeletionState);
                }
                protected.insert(target.to_string());
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(ApplicationStageError::Io(error)),
        }
        Ok(protected)
    }

    fn collect_deletion_state(
        &self,
    ) -> Result<(BTreeMap<String, DeletionMarker>, BTreeSet<String>), ApplicationStageError> {
        let mut markers = BTreeMap::new();
        let mut tombstones = BTreeSet::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let name = entry
                .file_name()
                .to_str()
                .ok_or(ApplicationStageError::UnsafeDeletionState)?
                .to_string();
            if name.starts_with(".delete-") {
                let slot_id = slot_id_from_deletion_marker_name(&name)?;
                let marker = read_deletion_marker(&entry.path(), self.identity)?;
                if marker.slot_id != slot_id || markers.insert(slot_id, marker).is_some() {
                    return Err(ApplicationStageError::UnsafeDeletionState);
                }
            } else if name.starts_with(".deleting-") {
                let slot_id = slot_id_from_tombstone_name(&name)?;
                if !tombstones.insert(slot_id) {
                    return Err(ApplicationStageError::UnsafeDeletionState);
                }
            }
        }
        Ok((markers, tombstones))
    }

    fn write_deletion_marker(&self, marker: &DeletionMarker) -> Result<(), ApplicationStageError> {
        validate_deletion_marker(marker)?;
        let destination = self.root.join(deletion_marker_name(&marker.slot_id));
        if fs::symlink_metadata(&destination).is_ok() {
            return Err(ApplicationStageError::UnsafeDeletionState);
        }
        let content =
            serde_json::to_vec(marker).map_err(|_| ApplicationStageError::Serialization)?;
        let mut temporary = tempfile::NamedTempFile::new_in(&self.root)?;
        fs::set_permissions(temporary.path(), Permissions::from_mode(0o600))?;
        temporary.write_all(&content)?;
        temporary.flush()?;
        temporary.as_file().sync_all()?;
        temporary
            .persist_noclobber(destination)
            .map_err(|error| ApplicationStageError::Io(error.error))?;
        self.root_directory.sync_all()?;
        Ok(())
    }

    fn recover_one_deletion(
        &self,
        marker: &DeletionMarker,
        trust_root: &ApplicationTrustRoot,
    ) -> Result<(), ApplicationStageError> {
        validate_deletion_marker(marker)?;
        let original = self.root.join(&marker.slot_id);
        let tombstone = self.root.join(tombstone_name(&marker.slot_id));
        let original_exists = path_exists(&original)?;
        let tombstone_exists = path_exists(&tombstone)?;
        if original_exists && tombstone_exists {
            return Err(ApplicationStageError::UnsafeDeletionState);
        }
        if original_exists {
            let verified = load_published_slot(&original, trust_root, self.identity)?;
            if verified.manifest_sha256 != marker.manifest_sha256 {
                return Err(ApplicationStageError::UnsafeDeletionState);
            }
            rename_child_no_replace(
                &self.root_directory,
                &original,
                tombstone
                    .file_name()
                    .ok_or(ApplicationStageError::UnsafeDeletionState)?,
            )?;
            self.root_directory.sync_all()?;
        }
        if path_exists(&tombstone)? {
            validate_deleting_tree(&tombstone, self.identity)?;
            make_tree_owner_writable(&tombstone)
                .map_err(|_| ApplicationStageError::UnsafeDeletionState)?;
            fs::remove_dir_all(&tombstone)?;
            self.root_directory.sync_all()?;
        }
        remove_deletion_marker(&self.root, marker, self.identity)?;
        self.root_directory.sync_all()?;
        Ok(())
    }

    fn validate_root(&self) -> Result<(), ApplicationStageError> {
        if validate_root_path(&self.root)? != self.identity
            || identity_from_metadata(&self.root_directory.metadata()?) != self.identity
        {
            return Err(ApplicationStageError::UnsafeRoot);
        }
        Ok(())
    }

    fn snapshot_source(&self, path: &Path) -> Result<(File, u64), ApplicationStageError> {
        if !path.is_absolute() || path.file_name().is_none() {
            return Err(ApplicationStageError::InvalidSource);
        }
        let source = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(path)
            .map_err(|_| ApplicationStageError::UnsafeSource)?;
        let source_metadata = source
            .metadata()
            .map_err(|_| ApplicationStageError::UnsafeSource)?;
        if !source_metadata.is_file() || source_metadata.permissions().mode() & 0o022 != 0 {
            return Err(ApplicationStageError::UnsafeSource);
        }
        let expected_size = source_metadata.len();
        if expected_size == 0 || expected_size > MAX_ARCHIVE_BYTES {
            return Err(ApplicationStageError::ArchiveTooLarge);
        }

        self.validate_root()?;
        let mut temporary = tempfile::NamedTempFile::new_in(&self.root)?;
        fs::set_permissions(temporary.path(), Permissions::from_mode(0o600))?;
        let temporary_metadata = temporary.as_file().metadata()?;
        if !temporary_metadata.is_file()
            || temporary_metadata.dev() != self.identity.device
            || temporary_metadata.uid() != self.identity.owner
            || temporary_metadata.nlink() != 1
        {
            return Err(ApplicationStageError::UnsafeRoot);
        }

        let mut limited = source.take(MAX_ARCHIVE_BYTES + 1);
        let copied = io::copy(&mut limited, temporary.as_file_mut())?;
        if copied > MAX_ARCHIVE_BYTES {
            return Err(ApplicationStageError::ArchiveTooLarge);
        }
        if copied != expected_size {
            return Err(ApplicationStageError::SourceChanged);
        }
        temporary.as_file_mut().flush()?;
        temporary.as_file().sync_all()?;
        fs::set_permissions(temporary.path(), Permissions::from_mode(0o400))?;
        temporary.as_file().sync_all()?;

        let copied_metadata = temporary.as_file().metadata()?;
        let mut snapshot = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(temporary.path())?;
        let snapshot_metadata = snapshot.metadata()?;
        if copied_metadata.dev() != snapshot_metadata.dev()
            || copied_metadata.ino() != snapshot_metadata.ino()
            || copied_metadata.len() != snapshot_metadata.len()
            || snapshot_metadata.len() != copied
            || snapshot_metadata.permissions().mode() & 0o7777 != 0o400
        {
            return Err(ApplicationStageError::UnsafeRoot);
        }
        temporary.close()?;
        if snapshot.metadata()?.nlink() != 0 {
            return Err(ApplicationStageError::UnsafeRoot);
        }
        snapshot.seek(SeekFrom::Start(0))?;
        self.validate_root()?;
        Ok((snapshot, copied))
    }

    fn validate_if_present(
        &self,
        slot_path: &Path,
        verified: &VerifiedApplicationBundle,
        trust_root: &ApplicationTrustRoot,
    ) -> Result<Option<StagedApplication>, ApplicationStageError> {
        match fs::symlink_metadata(slot_path) {
            Ok(_) => {
                validate_published_slot(slot_path, verified, trust_root, self.identity)?;
                Ok(Some(staged_result(
                    slot_id(verified),
                    slot_path.to_path_buf(),
                    StageDisposition::AlreadyPresent,
                    verified.clone(),
                )))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(ApplicationStageError::Io(error)),
        }
    }
}

#[derive(Debug)]
struct ArchiveProof {
    manifest_bytes: Vec<u8>,
    signature_bytes: [u8; 64],
}

fn staged_result(
    slot_id: String,
    slot_path: PathBuf,
    disposition: StageDisposition,
    verified: VerifiedApplicationBundle,
) -> StagedApplication {
    StagedApplication {
        slot_id,
        slot_path,
        disposition,
        manifest: verified.manifest,
        manifest_sha256: verified.manifest_sha256,
        payload_bytes: verified.payload_bytes,
    }
}

fn slot_id(verified: &VerifiedApplicationBundle) -> String {
    slot_id_from_manifest(&verified.manifest, &verified.manifest_sha256)
}

fn slot_id_from_manifest(manifest: &ApplicationManifest, manifest_sha256: &str) -> String {
    format!("app-{:020}-{}", manifest.sequence, manifest_sha256)
}

fn valid_slot_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 89 || !bytes.starts_with(b"app-") || bytes[24] != b'-' {
        return false;
    }
    let sequence = &bytes[4..24];
    let digest = &bytes[25..];
    if !sequence.iter().all(u8::is_ascii_digit)
        || !digest
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
        || digest.iter().all(|byte| *byte == b'0')
    {
        return false;
    }
    let sequence = std::str::from_utf8(sequence).expect("ASCII digits are valid UTF-8");
    let Ok(sequence_number) = sequence.parse::<u64>() else {
        return false;
    };
    sequence_number != 0 && format!("{sequence_number:020}") == sequence
}

fn validate_root_path(path: &Path) -> Result<RootIdentity, ApplicationStageError> {
    if !path.is_absolute() || path.file_name().is_none() {
        return Err(ApplicationStageError::InvalidRoot);
    }
    let canonical = fs::canonicalize(path).map_err(|_| ApplicationStageError::UnsafeRoot)?;
    if canonical != path {
        return Err(ApplicationStageError::UnsafeRoot);
    }
    let metadata = fs::symlink_metadata(path).map_err(|_| ApplicationStageError::UnsafeRoot)?;
    let identity = identity_from_metadata(&metadata);
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o022 != 0
        || identity.owner != effective_uid()
    {
        return Err(ApplicationStageError::UnsafeRoot);
    }
    Ok(identity)
}

fn identity_from_metadata(metadata: &fs::Metadata) -> RootIdentity {
    RootIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        owner: metadata.uid(),
    }
}

fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and does not dereference pointers.
    unsafe { libc::geteuid() }
}

fn validate_staging_parent(
    staging_path: &Path,
    root_identity: RootIdentity,
) -> Result<(), ApplicationStageError> {
    let parent = staging_path
        .parent()
        .ok_or(ApplicationStageError::UnsafeRoot)?;
    let parent_metadata = fs::symlink_metadata(parent)?;
    if identity_from_metadata(&parent_metadata) != root_identity {
        return Err(ApplicationStageError::UnsafeRoot);
    }
    let staging_metadata = fs::symlink_metadata(staging_path)?;
    if !staging_metadata.is_dir()
        || staging_metadata.uid() != root_identity.owner
        || staging_metadata.dev() != root_identity.device
        || staging_metadata.permissions().mode() & 0o077 != 0
    {
        return Err(ApplicationStageError::UnsafeRoot);
    }
    Ok(())
}

fn payload_directories(manifest: &ApplicationManifest) -> BTreeSet<String> {
    let mut directories = BTreeSet::new();
    for file in &manifest.files {
        let mut current = PathBuf::new();
        let components = Path::new(&file.path).components().collect::<Vec<_>>();
        for component in components.iter().take(components.len().saturating_sub(1)) {
            current.push(component.as_os_str());
            directories.insert(current.to_string_lossy().into_owned());
        }
    }
    directories
}

fn archive_directories(manifest: &ApplicationManifest) -> BTreeSet<String> {
    let mut directories = BTreeSet::from([APPLICATION_PAYLOAD_ROOT.to_string()]);
    directories.extend(
        payload_directories(manifest)
            .into_iter()
            .map(|path| format!("{APPLICATION_PAYLOAD_ROOT}/{path}")),
    );
    directories
}

fn create_payload_directories(
    root: &Path,
    manifest: &ApplicationManifest,
) -> Result<(), ApplicationStageError> {
    for relative in payload_directories(manifest) {
        let path = root.join(relative);
        fs::create_dir(&path)?;
        fs::set_permissions(path, Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn extract_verified_archive(
    snapshot: &mut File,
    root: &Path,
    verified: &VerifiedApplicationBundle,
) -> Result<ArchiveProof, ApplicationStageError> {
    let expected_files = verified
        .manifest
        .files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect::<BTreeMap<_, _>>();
    let expected_directories = archive_directories(&verified.manifest);
    let mut observed_files = BTreeSet::new();
    let mut observed_directories = BTreeSet::new();
    let mut previous_path: Option<String> = None;
    let mut manifest_bytes: Option<Vec<u8>> = None;
    let mut signature_bytes: Option<[u8; 64]> = None;
    let mut payload_bytes = 0_u64;
    let mut entries_seen = 0_usize;
    let mut archive = tar::Archive::new(snapshot);
    let entries = archive
        .entries()
        .map_err(|_| ApplicationStageError::InvalidArchive)?
        .raw(true);

    for entry in entries {
        let mut entry = entry.map_err(|_| ApplicationStageError::InvalidArchive)?;
        entries_seen = entries_seen.saturating_add(1);
        if entries_seen > MAX_TREE_ENTRIES {
            return Err(ApplicationStageError::InvalidArchive);
        }
        let entry_type = entry.header().entry_type();
        if !(entry_type.is_file() || entry_type.is_dir()) {
            return Err(ApplicationStageError::ExtractionMismatch);
        }
        let path = exact_entry_path(entry.path_bytes().as_ref(), entry_type.is_dir())?;
        if previous_path
            .as_ref()
            .is_some_and(|previous| path <= *previous)
        {
            return Err(ApplicationStageError::ExtractionMismatch);
        }
        previous_path = Some(path.clone());
        validate_entry_header(entry.header(), entry_type.is_dir())?;

        if entry_type.is_dir() {
            if !expected_directories.contains(&path) || !observed_directories.insert(path) {
                return Err(ApplicationStageError::ExtractionMismatch);
            }
            continue;
        }

        match path.as_str() {
            APPLICATION_MANIFEST_PATH => {
                if entry.header().mode().ok() != Some(0o644)
                    || entry.size() == 0
                    || entry.size() > MAX_MANIFEST_BYTES
                    || manifest_bytes.is_some()
                {
                    return Err(ApplicationStageError::ExtractionMismatch);
                }
                let size = entry.size();
                manifest_bytes = Some(read_entry(&mut entry, size)?);
            }
            APPLICATION_SIGNATURE_PATH => {
                if entry.header().mode().ok() != Some(0o644)
                    || entry.size() != 64
                    || signature_bytes.is_some()
                {
                    return Err(ApplicationStageError::ExtractionMismatch);
                }
                signature_bytes = Some(
                    read_entry(&mut entry, 64)?
                        .try_into()
                        .map_err(|_| ApplicationStageError::ExtractionMismatch)?,
                );
            }
            _ => {
                let relative = path
                    .strip_prefix("payload/")
                    .ok_or(ApplicationStageError::ExtractionMismatch)?;
                let expected = expected_files
                    .get(relative)
                    .ok_or(ApplicationStageError::ExtractionMismatch)?;
                if !observed_files.insert(relative.to_string())
                    || entry.header().mode().ok() != Some(expected.mode)
                    || entry.size() != expected.size
                {
                    return Err(ApplicationStageError::ExtractionMismatch);
                }
                payload_bytes = payload_bytes
                    .checked_add(entry.size())
                    .ok_or(ApplicationStageError::ExtractionMismatch)?;
                if payload_bytes > MAX_PAYLOAD_BYTES {
                    return Err(ApplicationStageError::ExtractionMismatch);
                }
                extract_payload_file(&mut entry, root, expected)?;
            }
        }
    }

    if observed_directories != expected_directories
        || observed_files.len() != expected_files.len()
        || payload_bytes != verified.payload_bytes
    {
        return Err(ApplicationStageError::ExtractionMismatch);
    }
    Ok(ArchiveProof {
        manifest_bytes: manifest_bytes.ok_or(ApplicationStageError::ExtractionMismatch)?,
        signature_bytes: signature_bytes.ok_or(ApplicationStageError::ExtractionMismatch)?,
    })
}

fn exact_entry_path(raw: &[u8], directory: bool) -> Result<String, ApplicationStageError> {
    let value = std::str::from_utf8(raw).map_err(|_| ApplicationStageError::ExtractionMismatch)?;
    let value = if directory {
        value.strip_suffix('/').unwrap_or(value)
    } else {
        value
    };
    if value.is_empty()
        || value.starts_with('/')
        || value.ends_with('/')
        || value.contains('\0')
        || value.contains('\\')
    {
        return Err(ApplicationStageError::ExtractionMismatch);
    }
    Ok(value.to_string())
}

fn validate_entry_header(
    header: &tar::Header,
    directory: bool,
) -> Result<(), ApplicationStageError> {
    let mode = header
        .mode()
        .map_err(|_| ApplicationStageError::ExtractionMismatch)?;
    let uid = header
        .uid()
        .map_err(|_| ApplicationStageError::ExtractionMismatch)?;
    let gid = header
        .gid()
        .map_err(|_| ApplicationStageError::ExtractionMismatch)?;
    let mtime = header
        .mtime()
        .map_err(|_| ApplicationStageError::ExtractionMismatch)?;
    if uid != 0 || gid != 0 || mtime != 0 || (directory && mode != 0o755) {
        return Err(ApplicationStageError::ExtractionMismatch);
    }
    Ok(())
}

fn read_entry(entry: &mut impl Read, expected: u64) -> Result<Vec<u8>, ApplicationStageError> {
    let mut bytes = Vec::with_capacity(expected as usize);
    entry.take(expected + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != expected {
        return Err(ApplicationStageError::ExtractionMismatch);
    }
    Ok(bytes)
}

fn extract_payload_file(
    entry: &mut impl Read,
    root: &Path,
    expected: &ApplicationFile,
) -> Result<(), ApplicationStageError> {
    let destination = root.join(&expected.path);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(destination)?;
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = entry.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or(ApplicationStageError::ExtractionMismatch)?;
        if total > expected.size {
            return Err(ApplicationStageError::ExtractionMismatch);
        }
        hasher.update(&buffer[..read]);
        file.write_all(&buffer[..read])?;
    }
    if total != expected.size || hex_lower(hasher.finalize().as_ref()) != expected.sha256 {
        return Err(ApplicationStageError::ExtractionMismatch);
    }
    file.flush()?;
    file.sync_all()?;
    Ok(())
}

fn write_slot_metadata(
    root: &Path,
    receipt: &SlotReceipt,
    proof: &ArchiveProof,
) -> Result<(), ApplicationStageError> {
    let metadata_directory = root.join(SLOT_METADATA_DIRECTORY);
    validate_staging_marker(root)?;
    let receipt_bytes =
        serde_json::to_vec(receipt).map_err(|_| ApplicationStageError::Serialization)?;
    if receipt_bytes.len() as u64 > MAX_SLOT_RECEIPT_BYTES {
        return Err(ApplicationStageError::Serialization);
    }
    write_new_file(&root.join(SLOT_MANIFEST_PATH), &proof.manifest_bytes)?;
    write_new_file(&root.join(SLOT_SIGNATURE_PATH), &proof.signature_bytes)?;
    replace_private_file(&root.join(SLOT_RECEIPT_PATH), &receipt_bytes)?;
    File::open(metadata_directory)?.sync_all()?;
    Ok(())
}

fn create_staging_marker(root: &Path) -> Result<(), ApplicationStageError> {
    let metadata_directory = root.join(SLOT_METADATA_DIRECTORY);
    fs::create_dir(&metadata_directory)?;
    fs::set_permissions(&metadata_directory, Permissions::from_mode(0o700))?;
    let marker = StagingMarker {
        schema_version: STAGING_MARKER_SCHEMA_VERSION,
        purpose: STAGING_MARKER_PURPOSE.to_string(),
    };
    let bytes = serde_json::to_vec(&marker).map_err(|_| ApplicationStageError::Serialization)?;
    write_new_file(&root.join(SLOT_RECEIPT_PATH), &bytes)?;
    File::open(&metadata_directory)?.sync_all()?;
    File::open(root)?.sync_all()?;
    Ok(())
}

fn validate_staging_marker(root: &Path) -> Result<(), ApplicationStageError> {
    let path = root.join(SLOT_RECEIPT_PATH);
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| ApplicationStageError::UnsafeAbandonedStaging)?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.permissions().mode() & 0o7777 != 0o600
        || metadata.len() == 0
        || metadata.len() > MAX_SLOT_RECEIPT_BYTES
    {
        return Err(ApplicationStageError::UnsafeAbandonedStaging);
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_SLOT_RECEIPT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    let marker: StagingMarker = serde_json::from_slice(&bytes)
        .map_err(|_| ApplicationStageError::UnsafeAbandonedStaging)?;
    if marker
        != (StagingMarker {
            schema_version: STAGING_MARKER_SCHEMA_VERSION,
            purpose: STAGING_MARKER_PURPOSE.to_string(),
        })
    {
        return Err(ApplicationStageError::UnsafeAbandonedStaging);
    }
    Ok(())
}

fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), ApplicationStageError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)?;
    file.write_all(bytes)?;
    file.flush()?;
    file.sync_all()?;
    Ok(())
}

fn replace_private_file(path: &Path, bytes: &[u8]) -> Result<(), ApplicationStageError> {
    let parent = path
        .parent()
        .ok_or(ApplicationStageError::UnsafeAbandonedStaging)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    fs::set_permissions(temporary.path(), Permissions::from_mode(0o600))?;
    temporary.write_all(bytes)?;
    temporary.flush()?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| ApplicationStageError::Io(error.error))?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn harden_staged_tree(
    root: &Path,
    manifest: &ApplicationManifest,
) -> Result<(), ApplicationStageError> {
    for file in &manifest.files {
        let path = root.join(&file.path);
        fs::set_permissions(
            &path,
            Permissions::from_mode(immutable_file_mode(file.mode)),
        )?;
        File::open(path)?.sync_all()?;
    }
    for path in [SLOT_RECEIPT_PATH, SLOT_MANIFEST_PATH, SLOT_SIGNATURE_PATH] {
        let path = root.join(path);
        fs::set_permissions(&path, Permissions::from_mode(0o400))?;
        File::open(path)?.sync_all()?;
    }
    let mut directories = payload_directories(manifest)
        .into_iter()
        .collect::<Vec<_>>();
    directories.sort_by_key(|path| std::cmp::Reverse(path.matches('/').count()));
    for relative in directories {
        let path = root.join(relative);
        fs::set_permissions(&path, Permissions::from_mode(0o555))?;
        File::open(path)?.sync_all()?;
    }
    let metadata_directory = root.join(SLOT_METADATA_DIRECTORY);
    fs::set_permissions(&metadata_directory, Permissions::from_mode(0o500))?;
    File::open(metadata_directory)?.sync_all()?;
    fs::set_permissions(root, Permissions::from_mode(0o555))?;
    File::open(root)?.sync_all()?;
    Ok(())
}

fn immutable_file_mode(manifest_mode: u32) -> u32 {
    manifest_mode & !0o222
}

fn rename_child_no_replace(
    root: &File,
    source: &Path,
    destination_name: &std::ffi::OsStr,
) -> io::Result<()> {
    let source_name = source
        .file_name()
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    let source_name = CString::new(source_name.as_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    let destination_name = CString::new(destination_name.as_bytes())
        .map_err(|_| io::Error::from(io::ErrorKind::InvalidInput))?;
    // SAFETY: SYS_renameat2 receives two valid directory descriptors, two
    // live NUL-terminated child names, and the documented no-replace flag.
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            root.as_raw_fd(),
            source_name.as_ptr(),
            root.as_raw_fd(),
            destination_name.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn make_tree_owner_writable(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        fs::set_permissions(path, Permissions::from_mode(0o700))?;
        for entry in fs::read_dir(path)? {
            make_tree_owner_writable(&entry?.path())?;
        }
    } else if metadata.is_file() {
        fs::set_permissions(path, Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn deletion_marker_name(slot_id: &str) -> String {
    format!(".delete-{slot_id}.json")
}

fn tombstone_name(slot_id: &str) -> String {
    format!(".deleting-{slot_id}")
}

fn slot_id_from_deletion_marker_name(value: &str) -> Result<String, ApplicationStageError> {
    let slot_id = value
        .strip_prefix(".delete-")
        .and_then(|value| value.strip_suffix(".json"))
        .ok_or(ApplicationStageError::UnsafeDeletionState)?;
    if !valid_slot_id(slot_id) {
        return Err(ApplicationStageError::UnsafeDeletionState);
    }
    Ok(slot_id.to_string())
}

fn slot_id_from_tombstone_name(value: &str) -> Result<String, ApplicationStageError> {
    let slot_id = value
        .strip_prefix(".deleting-")
        .ok_or(ApplicationStageError::UnsafeDeletionState)?;
    if !valid_slot_id(slot_id) {
        return Err(ApplicationStageError::UnsafeDeletionState);
    }
    Ok(slot_id.to_string())
}

fn validate_deletion_marker(marker: &DeletionMarker) -> Result<(), ApplicationStageError> {
    if marker.schema_version != DELETION_MARKER_SCHEMA_VERSION
        || marker.purpose != DELETION_MARKER_PURPOSE
        || !valid_slot_id(&marker.slot_id)
        || !valid_sha256_text(&marker.manifest_sha256)
        || !marker.slot_id.ends_with(&marker.manifest_sha256)
    {
        return Err(ApplicationStageError::UnsafeDeletionState);
    }
    Ok(())
}

fn read_deletion_marker(
    path: &Path,
    identity: RootIdentity,
) -> Result<DeletionMarker, ApplicationStageError> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| ApplicationStageError::UnsafeDeletionState)?;
    let metadata = file
        .metadata()
        .map_err(|_| ApplicationStageError::UnsafeDeletionState)?;
    if !metadata.is_file()
        || metadata.dev() != identity.device
        || metadata.uid() != identity.owner
        || metadata.nlink() != 1
        || metadata.permissions().mode() & 0o7777 != 0o600
        || metadata.len() == 0
        || metadata.len() > MAX_SLOT_RECEIPT_BYTES
    {
        return Err(ApplicationStageError::UnsafeDeletionState);
    }
    let mut content = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_SLOT_RECEIPT_BYTES + 1)
        .read_to_end(&mut content)?;
    let marker: DeletionMarker =
        serde_json::from_slice(&content).map_err(|_| ApplicationStageError::UnsafeDeletionState)?;
    validate_deletion_marker(&marker)?;
    Ok(marker)
}

fn remove_deletion_marker(
    root: &Path,
    expected: &DeletionMarker,
    identity: RootIdentity,
) -> Result<(), ApplicationStageError> {
    let path = root.join(deletion_marker_name(&expected.slot_id));
    if read_deletion_marker(&path, identity)? != *expected {
        return Err(ApplicationStageError::UnsafeDeletionState);
    }
    fs::remove_file(path)?;
    Ok(())
}

fn path_exists(path: &Path) -> Result<bool, ApplicationStageError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(ApplicationStageError::Io(error)),
    }
}

fn validate_deleting_tree(
    root: &Path,
    identity: RootIdentity,
) -> Result<(), ApplicationStageError> {
    let metadata =
        fs::symlink_metadata(root).map_err(|_| ApplicationStageError::UnsafeDeletionState)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.dev() != identity.device
        || metadata.uid() != identity.owner
        || metadata.permissions().mode() & 0o7000 != 0
    {
        return Err(ApplicationStageError::UnsafeDeletionState);
    }
    let mut entries = 0_usize;
    let mut bytes = 0_u64;
    inspect_abandoned_directory(root, root, identity, &mut entries, &mut bytes)
        .map_err(|_| ApplicationStageError::UnsafeDeletionState)
}

fn valid_staging_name(value: &str) -> bool {
    let Some(suffix) = value.strip_prefix(".staging-") else {
        return false;
    };
    (6..=32).contains(&suffix.len()) && suffix.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

fn validate_abandoned_staging_tree(
    root: &Path,
    identity: RootIdentity,
) -> Result<(), ApplicationStageError> {
    let metadata =
        fs::symlink_metadata(root).map_err(|_| ApplicationStageError::UnsafeAbandonedStaging)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.dev() != identity.device
        || metadata.uid() != identity.owner
        || metadata.permissions().mode() & 0o7000 != 0
    {
        return Err(ApplicationStageError::UnsafeAbandonedStaging);
    }
    validate_abandoned_receipt(root, identity)?;
    let mut entries = 0_usize;
    let mut bytes = 0_u64;
    inspect_abandoned_directory(root, root, identity, &mut entries, &mut bytes)?;
    Ok(())
}

fn validate_abandoned_receipt(
    root: &Path,
    identity: RootIdentity,
) -> Result<(), ApplicationStageError> {
    let path = root.join(SLOT_RECEIPT_PATH);
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| ApplicationStageError::UnsafeAbandonedStaging)?;
    let metadata = file
        .metadata()
        .map_err(|_| ApplicationStageError::UnsafeAbandonedStaging)?;
    if !metadata.is_file()
        || metadata.dev() != identity.device
        || metadata.uid() != identity.owner
        || metadata.nlink() != 1
        || !matches!(metadata.permissions().mode() & 0o7777, 0o400 | 0o600)
        || metadata.len() == 0
        || metadata.len() > MAX_SLOT_RECEIPT_BYTES
    {
        return Err(ApplicationStageError::UnsafeAbandonedStaging);
    }
    let mut content = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_SLOT_RECEIPT_BYTES + 1)
        .read_to_end(&mut content)?;
    if let Ok(marker) = serde_json::from_slice::<StagingMarker>(&content) {
        if marker
            == (StagingMarker {
                schema_version: STAGING_MARKER_SCHEMA_VERSION,
                purpose: STAGING_MARKER_PURPOSE.to_string(),
            })
        {
            return Ok(());
        }
    }
    let receipt: SlotReceipt = serde_json::from_slice(&content)
        .map_err(|_| ApplicationStageError::UnsafeAbandonedStaging)?;
    if receipt.schema_version != SLOT_RECEIPT_SCHEMA_VERSION
        || !valid_slot_id(&receipt.slot_id)
        || !valid_sha256_text(&receipt.manifest_sha256)
        || receipt.payload_bytes == 0
        || receipt.payload_bytes > MAX_PAYLOAD_BYTES
    {
        return Err(ApplicationStageError::UnsafeAbandonedStaging);
    }
    Ok(())
}

fn inspect_abandoned_directory(
    staging_root: &Path,
    directory: &Path,
    identity: RootIdentity,
    entries: &mut usize,
    bytes: &mut u64,
) -> Result<(), ApplicationStageError> {
    for entry in
        fs::read_dir(directory).map_err(|_| ApplicationStageError::UnsafeAbandonedStaging)?
    {
        let entry = entry.map_err(|_| ApplicationStageError::UnsafeAbandonedStaging)?;
        *entries = entries.saturating_add(1);
        if *entries > MAX_TREE_ENTRIES {
            return Err(ApplicationStageError::UnsafeAbandonedStaging);
        }
        let path = entry.path();
        let relative = path
            .strip_prefix(staging_root)
            .map_err(|_| ApplicationStageError::UnsafeAbandonedStaging)?;
        let top_level = relative
            .components()
            .next()
            .and_then(|component| component.as_os_str().to_str())
            .ok_or(ApplicationStageError::UnsafeAbandonedStaging)?;
        if !matches!(
            top_level,
            "bin" | "libexec" | "share" | SLOT_METADATA_DIRECTORY
        ) {
            return Err(ApplicationStageError::UnsafeAbandonedStaging);
        }
        let metadata = fs::symlink_metadata(&path)
            .map_err(|_| ApplicationStageError::UnsafeAbandonedStaging)?;
        if metadata.dev() != identity.device
            || metadata.uid() != identity.owner
            || metadata.permissions().mode() & 0o7000 != 0
        {
            return Err(ApplicationStageError::UnsafeAbandonedStaging);
        }
        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            inspect_abandoned_directory(staging_root, &path, identity, entries, bytes)?;
        } else if metadata.is_file() && metadata.nlink() == 1 {
            *bytes = bytes
                .checked_add(metadata.len())
                .ok_or(ApplicationStageError::UnsafeAbandonedStaging)?;
            if *bytes > MAX_PAYLOAD_BYTES + 1024 * 1024 {
                return Err(ApplicationStageError::UnsafeAbandonedStaging);
            }
        } else {
            return Err(ApplicationStageError::UnsafeAbandonedStaging);
        }
    }
    Ok(())
}

fn valid_sha256_text(value: &str) -> bool {
    value.len() == 64
        && !value.bytes().all(|byte| byte == b'0')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_published_slot(
    slot_path: &Path,
    expected: &VerifiedApplicationBundle,
    trust_root: &ApplicationTrustRoot,
    root_identity: RootIdentity,
) -> Result<(), ApplicationStageError> {
    let actual = load_published_slot(slot_path, trust_root, root_identity)?;
    if actual != *expected {
        return Err(ApplicationStageError::SlotConflict);
    }
    Ok(())
}

fn load_published_slot(
    slot_path: &Path,
    trust_root: &ApplicationTrustRoot,
    root_identity: RootIdentity,
) -> Result<VerifiedApplicationBundle, ApplicationStageError> {
    let metadata =
        fs::symlink_metadata(slot_path).map_err(|_| ApplicationStageError::UnsafeExistingSlot)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.dev() != root_identity.device
        || metadata.uid() != root_identity.owner
        || metadata.permissions().mode() & 0o7777 != 0o555
    {
        return Err(ApplicationStageError::UnsafeExistingSlot);
    }

    let receipt_bytes = read_safe_slot_file(
        &slot_path.join(SLOT_RECEIPT_PATH),
        0o400,
        MAX_SLOT_RECEIPT_BYTES,
        root_identity,
    )?;
    let receipt: SlotReceipt = serde_json::from_slice(&receipt_bytes)
        .map_err(|_| ApplicationStageError::UnsafeExistingSlot)?;
    let manifest_bytes = read_safe_slot_file(
        &slot_path.join(SLOT_MANIFEST_PATH),
        0o400,
        MAX_MANIFEST_BYTES,
        root_identity,
    )?;
    let signature_bytes: [u8; 64] = read_safe_slot_file(
        &slot_path.join(SLOT_SIGNATURE_PATH),
        0o400,
        64,
        root_identity,
    )?
    .try_into()
    .map_err(|_| ApplicationStageError::UnsafeExistingSlot)?;
    let manifest =
        verify_application_manifest_bytes(&manifest_bytes, &signature_bytes, trust_root)?;
    let manifest_sha256 = sha256_hex(&manifest_bytes);
    let payload_bytes = manifest.files.iter().try_fold(0_u64, |total, file| {
        total
            .checked_add(file.size)
            .ok_or(ApplicationStageError::UnsafeExistingSlot)
    })?;
    let expected_slot_id = slot_id_from_manifest(&manifest, &manifest_sha256);
    let actual_slot_id = slot_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(ApplicationStageError::UnsafeExistingSlot)?;
    if receipt.schema_version != SLOT_RECEIPT_SCHEMA_VERSION
        || receipt.slot_id != expected_slot_id
        || actual_slot_id != expected_slot_id
        || receipt.manifest_sha256 != manifest_sha256
        || receipt.payload_bytes != payload_bytes
    {
        return Err(ApplicationStageError::SlotConflict);
    }

    validate_slot_tree(slot_path, &manifest, root_identity)?;
    Ok(VerifiedApplicationBundle {
        manifest,
        manifest_sha256,
        payload_bytes,
    })
}

fn read_safe_slot_file(
    path: &Path,
    expected_mode: u32,
    maximum_size: u64,
    root_identity: RootIdentity,
) -> Result<Vec<u8>, ApplicationStageError> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| ApplicationStageError::UnsafeExistingSlot)?;
    let metadata = file
        .metadata()
        .map_err(|_| ApplicationStageError::UnsafeExistingSlot)?;
    if !metadata.is_file()
        || metadata.dev() != root_identity.device
        || metadata.uid() != root_identity.owner
        || metadata.nlink() != 1
        || metadata.permissions().mode() & 0o7777 != expected_mode
        || metadata.len() == 0
        || metadata.len() > maximum_size
    {
        return Err(ApplicationStageError::UnsafeExistingSlot);
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(maximum_size + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != metadata.len() {
        return Err(ApplicationStageError::UnsafeExistingSlot);
    }
    Ok(bytes)
}

fn validate_slot_tree(
    slot_path: &Path,
    manifest: &ApplicationManifest,
    root_identity: RootIdentity,
) -> Result<(), ApplicationStageError> {
    let expected_files = manifest
        .files
        .iter()
        .map(|file| (file.path.clone(), file))
        .collect::<BTreeMap<_, _>>();
    let expected_directories = payload_directories(manifest);
    let mut observed_files = BTreeSet::new();
    let mut observed_directories = BTreeSet::new();
    let mut entries = 0_usize;
    inspect_slot_directory(
        slot_path,
        slot_path,
        &expected_files,
        &expected_directories,
        &mut observed_files,
        &mut observed_directories,
        &mut entries,
        root_identity,
    )?;
    if observed_files.len() != expected_files.len() || observed_directories != expected_directories
    {
        return Err(ApplicationStageError::UnsafeExistingSlot);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn inspect_slot_directory(
    slot_root: &Path,
    directory: &Path,
    expected_files: &BTreeMap<String, &ApplicationFile>,
    expected_directories: &BTreeSet<String>,
    observed_files: &mut BTreeSet<String>,
    observed_directories: &mut BTreeSet<String>,
    entries: &mut usize,
    root_identity: RootIdentity,
) -> Result<(), ApplicationStageError> {
    for entry in fs::read_dir(directory).map_err(|_| ApplicationStageError::UnsafeExistingSlot)? {
        let entry = entry.map_err(|_| ApplicationStageError::UnsafeExistingSlot)?;
        *entries = entries.saturating_add(1);
        if *entries > MAX_TREE_ENTRIES {
            return Err(ApplicationStageError::UnsafeExistingSlot);
        }
        let path = entry.path();
        let relative = path
            .strip_prefix(slot_root)
            .map_err(|_| ApplicationStageError::UnsafeExistingSlot)?
            .to_str()
            .ok_or(ApplicationStageError::UnsafeExistingSlot)?
            .to_string();
        let metadata =
            fs::symlink_metadata(&path).map_err(|_| ApplicationStageError::UnsafeExistingSlot)?;
        if metadata.dev() != root_identity.device || metadata.uid() != root_identity.owner {
            return Err(ApplicationStageError::UnsafeExistingSlot);
        }
        if relative == SLOT_METADATA_DIRECTORY {
            if !metadata.is_dir() || metadata.permissions().mode() & 0o7777 != 0o500 {
                return Err(ApplicationStageError::UnsafeExistingSlot);
            }
            validate_metadata_directory(&path, root_identity, entries)?;
        } else if metadata.is_dir() {
            if metadata.file_type().is_symlink()
                || metadata.permissions().mode() & 0o7777 != 0o555
                || !expected_directories.contains(&relative)
                || !observed_directories.insert(relative)
            {
                return Err(ApplicationStageError::UnsafeExistingSlot);
            }
            inspect_slot_directory(
                slot_root,
                &path,
                expected_files,
                expected_directories,
                observed_files,
                observed_directories,
                entries,
                root_identity,
            )?;
        } else if metadata.is_file() {
            let expected = expected_files
                .get(&relative)
                .ok_or(ApplicationStageError::UnsafeExistingSlot)?;
            validate_payload_file(&path, expected, root_identity)?;
            if !observed_files.insert(relative) {
                return Err(ApplicationStageError::UnsafeExistingSlot);
            }
        } else {
            return Err(ApplicationStageError::UnsafeExistingSlot);
        }
    }
    Ok(())
}

fn validate_metadata_directory(
    path: &Path,
    root_identity: RootIdentity,
    entries: &mut usize,
) -> Result<(), ApplicationStageError> {
    let expected = BTreeMap::from([
        ("manifest.json", (0o400, MAX_MANIFEST_BYTES)),
        ("manifest.sig", (0o400, 64)),
        ("receipt.json", (0o400, MAX_SLOT_RECEIPT_BYTES)),
    ]);
    let mut observed = BTreeSet::new();
    for entry in fs::read_dir(path).map_err(|_| ApplicationStageError::UnsafeExistingSlot)? {
        let entry = entry.map_err(|_| ApplicationStageError::UnsafeExistingSlot)?;
        *entries = entries.saturating_add(1);
        if *entries > MAX_TREE_ENTRIES {
            return Err(ApplicationStageError::UnsafeExistingSlot);
        }
        let name = entry
            .file_name()
            .to_str()
            .ok_or(ApplicationStageError::UnsafeExistingSlot)?
            .to_string();
        let (mode, maximum) = expected
            .get(name.as_str())
            .ok_or(ApplicationStageError::UnsafeExistingSlot)?;
        let _ = read_safe_slot_file(&entry.path(), *mode, *maximum, root_identity)?;
        if !observed.insert(name) {
            return Err(ApplicationStageError::UnsafeExistingSlot);
        }
    }
    if observed.len() != expected.len() {
        return Err(ApplicationStageError::UnsafeExistingSlot);
    }
    Ok(())
}

fn validate_payload_file(
    path: &Path,
    expected: &ApplicationFile,
    root_identity: RootIdentity,
) -> Result<(), ApplicationStageError> {
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| ApplicationStageError::UnsafeExistingSlot)?;
    let metadata = file
        .metadata()
        .map_err(|_| ApplicationStageError::UnsafeExistingSlot)?;
    if !metadata.is_file()
        || metadata.dev() != root_identity.device
        || metadata.uid() != root_identity.owner
        || metadata.nlink() != 1
        || metadata.permissions().mode() & 0o7777 != immutable_file_mode(expected.mode)
        || metadata.len() != expected.size
    {
        return Err(ApplicationStageError::UnsafeExistingSlot);
    }
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or(ApplicationStageError::UnsafeExistingSlot)?;
        if total > expected.size {
            return Err(ApplicationStageError::UnsafeExistingSlot);
        }
        hasher.update(&buffer[..read]);
    }
    if total != expected.size || hex_lower(hasher.finalize().as_ref()) != expected.sha256 {
        return Err(ApplicationStageError::UnsafeExistingSlot);
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex_lower(Sha256::digest(bytes).as_ref())
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(char::from(HEX[usize::from(byte >> 4)]));
        value.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    value
}

#[cfg(test)]
mod staging_cleanup_tests {
    use std::os::unix::fs::symlink;

    use super::*;

    #[test]
    fn slot_root_lock_excludes_concurrent_staging_and_cleanup() {
        let root = tempfile::tempdir().unwrap();
        let _first = ApplicationSlotStager::open(root.path()).unwrap();
        assert!(matches!(
            ApplicationSlotStager::open(root.path()),
            Err(ApplicationStageError::StagerBusy)
        ));
    }

    #[test]
    fn removes_marker_owned_partial_and_readonly_staging_trees() {
        let root = tempfile::tempdir().unwrap();
        let stager = ApplicationSlotStager::open(root.path()).unwrap();
        let abandoned = root.path().join(".staging-Abc123");
        fs::create_dir(&abandoned).unwrap();
        fs::set_permissions(&abandoned, Permissions::from_mode(0o700)).unwrap();
        create_staging_marker(&abandoned).unwrap();
        let bin = abandoned.join("bin");
        fs::create_dir(&bin).unwrap();
        fs::write(bin.join("partial"), b"partial").unwrap();
        fs::set_permissions(bin.join("partial"), Permissions::from_mode(0o400)).unwrap();
        fs::set_permissions(&bin, Permissions::from_mode(0o500)).unwrap();
        fs::set_permissions(&abandoned, Permissions::from_mode(0o500)).unwrap();

        assert_eq!(
            stager.cleanup_abandoned_staging().unwrap(),
            StagingCleanup { removed: 1 }
        );
        assert!(!abandoned.exists());
        assert_eq!(
            stager.cleanup_abandoned_staging().unwrap(),
            StagingCleanup { removed: 0 }
        );
    }

    #[test]
    fn refuses_links_and_preserves_external_content() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("preserve");
        fs::write(&outside_file, b"preserve").unwrap();
        let stager = ApplicationSlotStager::open(root.path()).unwrap();
        let abandoned = root.path().join(".staging-Def456");
        fs::create_dir(&abandoned).unwrap();
        fs::set_permissions(&abandoned, Permissions::from_mode(0o700)).unwrap();
        create_staging_marker(&abandoned).unwrap();
        symlink(&outside_file, abandoned.join("share")).unwrap();

        assert!(matches!(
            stager.cleanup_abandoned_staging(),
            Err(ApplicationStageError::UnsafeAbandonedStaging)
        ));
        assert_eq!(fs::read(outside_file).unwrap(), b"preserve");
        assert!(abandoned.exists());
    }
}
