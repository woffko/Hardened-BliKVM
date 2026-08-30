use std::{
    fs::OpenOptions,
    io::{Cursor, Read},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::PathBuf,
};

use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const SYSTEM_FORMAT_V1: &str = "hardened-blikvm-system-v1";
pub const SYSTEM_PRODUCT: &str = "Hardened BliKVM";
pub const SYSTEM_TARGET: &str = "bli-h616-pikvm";
pub const SYSTEM_ARCHITECTURE: &str = "aarch64";

pub const SYSTEM_MANIFEST_PATH: &str = "manifest.json";
pub const SYSTEM_SIGNATURE_PATH: &str = "manifest.sig";
pub const SYSTEM_PAYLOAD_DIRECTORY: &str = "payload/";

pub const MAX_SYSTEM_ARCHIVE_BYTES: u64 = 10 * 1024 * 1024 * 1024;
pub const MAX_ROOTFS_IMAGE_BYTES: u64 = 8 * 1024 * 1024 * 1024;
pub const MAX_BOOT_IMAGE_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_SYSTEM_MANIFEST_BYTES: u64 = 64 * 1024;

const MIN_ROOTFS_IMAGE_BYTES: u64 = 2048;
const MIN_BOOT_IMAGE_BYTES: u64 = 512;
const MAX_TRAILING_ZERO_BYTES: u64 = 64 * 1024;
const EXT4_MAGIC_OFFSET: u64 = 1024 + 56;
const VFAT_SIGNATURE_OFFSET: u64 = 510;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemComponentRole {
    Boot,
    Rootfs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemContentType {
    Ext4,
    Vfat,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SystemComponent {
    pub role: SystemComponentRole,
    pub path: String,
    pub content_type: SystemContentType,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SystemManifest {
    pub format: String,
    pub product: String,
    pub target: String,
    pub architecture: String,
    pub key_id: String,
    pub version: String,
    pub sequence: u64,
    pub minimum_recovery_sequence: u64,
    pub minimum_boot_control_sequence: u64,
    pub layout_id: String,
    pub source_commit: String,
    pub components: Vec<SystemComponent>,
}

#[derive(Debug, Clone)]
pub struct SystemTrustRoot {
    key_id: String,
    verifying_key: VerifyingKey,
}

impl SystemTrustRoot {
    pub fn new(key_id: impl Into<String>, public_key: [u8; 32]) -> Result<Self, SystemVerifyError> {
        let key_id = key_id.into();
        if !valid_key_id(&key_id) {
            return Err(SystemVerifyError::InvalidTrustRoot);
        }
        let verifying_key = VerifyingKey::from_bytes(&public_key)
            .map_err(|_| SystemVerifyError::InvalidTrustRoot)?;
        Ok(Self {
            key_id,
            verifying_key,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemVerificationPolicy {
    layout_id: String,
    recovery_sequence: u64,
    boot_control_sequence: u64,
    rootfs_bytes: u64,
    boot_bytes: Option<u64>,
}

impl SystemVerificationPolicy {
    pub fn new(
        layout_id: impl Into<String>,
        recovery_sequence: u64,
        boot_control_sequence: u64,
        rootfs_bytes: u64,
        boot_bytes: Option<u64>,
    ) -> Result<Self, SystemVerifyError> {
        let layout_id = layout_id.into();
        if !valid_layout_id(&layout_id)
            || recovery_sequence == 0
            || boot_control_sequence == 0
            || !(MIN_ROOTFS_IMAGE_BYTES..=MAX_ROOTFS_IMAGE_BYTES).contains(&rootfs_bytes)
            || boot_bytes
                .is_some_and(|size| !(MIN_BOOT_IMAGE_BYTES..=MAX_BOOT_IMAGE_BYTES).contains(&size))
        {
            return Err(SystemVerifyError::InvalidPolicy);
        }
        Ok(Self {
            layout_id,
            recovery_sequence,
            boot_control_sequence,
            rootfs_bytes,
            boot_bytes,
        })
    }

    pub fn layout_id(&self) -> &str {
        &self.layout_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedSystemBundle {
    pub manifest: SystemManifest,
    pub manifest_sha256: String,
    pub payload_bytes: u64,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum SystemVerifyError {
    #[error("system image trust root is invalid")]
    InvalidTrustRoot,
    #[error("system image layout policy is invalid")]
    InvalidPolicy,
    #[error("system image archive path is invalid")]
    InvalidArchivePath,
    #[error("system image archive file is unsafe")]
    UnsafeArchiveFile,
    #[error("system image archive exceeded its size limit")]
    ArchiveTooLarge,
    #[error("system image archive is invalid")]
    InvalidArchive,
    #[error("system image archive entry type is forbidden")]
    ForbiddenEntryType,
    #[error("system image archive entry metadata is invalid")]
    InvalidEntryMetadata,
    #[error("system image archive entry path or order is invalid")]
    InvalidEntryPath,
    #[error("system image manifest is missing, non-canonical, or invalid")]
    InvalidManifest,
    #[error("system image signature is missing or invalid")]
    InvalidSignature,
    #[error("system image manifest shape is invalid")]
    InvalidManifestShape,
    #[error("system image does not match the local layout policy")]
    PolicyMismatch,
    #[error("system image payload exceeded its size limit")]
    PayloadTooLarge,
    #[error("system image payload does not match the manifest")]
    PayloadMismatch,
    #[error("system image payload has an invalid filesystem signature")]
    InvalidFilesystemImage,
    #[error("system image archive I/O failed")]
    Io,
}

pub fn verify_system_file(
    path: impl Into<PathBuf>,
    trust_root: &SystemTrustRoot,
    policy: &SystemVerificationPolicy,
) -> Result<VerifiedSystemBundle, SystemVerifyError> {
    let path = path.into();
    if !path.is_absolute() || path.file_name().is_none() {
        return Err(SystemVerifyError::InvalidArchivePath);
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| SystemVerifyError::UnsafeArchiveFile)?;
    let metadata = file.metadata().map_err(|_| SystemVerifyError::Io)?;
    if !metadata.is_file()
        || metadata.uid() != effective_uid()
        || metadata.nlink() != 1
        || metadata.permissions().mode() & 0o022 != 0
    {
        return Err(SystemVerifyError::UnsafeArchiveFile);
    }
    if metadata.len() == 0 || metadata.len() > MAX_SYSTEM_ARCHIVE_BYTES {
        return Err(SystemVerifyError::ArchiveTooLarge);
    }
    let verified = verify_system_reader(&mut file, metadata.len(), trust_root, policy)?;
    let after = file
        .metadata()
        .map_err(|_| SystemVerifyError::UnsafeArchiveFile)?;
    if !same_archive_identity(&metadata, &after) {
        return Err(SystemVerifyError::UnsafeArchiveFile);
    }
    Ok(verified)
}

pub fn verify_system_bytes(
    bytes: &[u8],
    trust_root: &SystemTrustRoot,
    policy: &SystemVerificationPolicy,
) -> Result<VerifiedSystemBundle, SystemVerifyError> {
    if bytes.is_empty() || bytes.len() as u64 > MAX_SYSTEM_ARCHIVE_BYTES {
        return Err(SystemVerifyError::ArchiveTooLarge);
    }
    verify_system_reader(Cursor::new(bytes), bytes.len() as u64, trust_root, policy)
}

pub fn verify_system_reader(
    reader: impl Read,
    archive_size: u64,
    trust_root: &SystemTrustRoot,
    policy: &SystemVerificationPolicy,
) -> Result<VerifiedSystemBundle, SystemVerifyError> {
    if archive_size == 0 || archive_size > MAX_SYSTEM_ARCHIVE_BYTES {
        return Err(SystemVerifyError::ArchiveTooLarge);
    }
    let mut archive = tar::Archive::new(reader);
    let mut entries = archive
        .entries()
        .map_err(|_| SystemVerifyError::InvalidArchive)?
        .raw(true);

    let mut manifest_entry = entries
        .next()
        .ok_or(SystemVerifyError::InvalidManifest)?
        .map_err(|_| SystemVerifyError::InvalidArchive)?;
    validate_entry(
        &manifest_entry,
        SYSTEM_MANIFEST_PATH,
        false,
        manifest_entry.size(),
    )?;
    if manifest_entry.size() == 0 || manifest_entry.size() > MAX_SYSTEM_MANIFEST_BYTES {
        return Err(SystemVerifyError::InvalidManifest);
    }
    let manifest_size = manifest_entry.size();
    let manifest_bytes = read_exact_bounded(&mut manifest_entry, manifest_size)?;
    drop(manifest_entry);

    let mut signature_entry = entries
        .next()
        .ok_or(SystemVerifyError::InvalidSignature)?
        .map_err(|_| SystemVerifyError::InvalidArchive)?;
    validate_entry(&signature_entry, SYSTEM_SIGNATURE_PATH, false, 64)?;
    let signature_bytes: [u8; 64] = read_exact_bounded(&mut signature_entry, 64)?
        .try_into()
        .map_err(|_| SystemVerifyError::InvalidSignature)?;
    drop(signature_entry);

    let manifest =
        verify_system_manifest_bytes(&manifest_bytes, &signature_bytes, trust_root, policy)?;

    let payload_directory = entries
        .next()
        .ok_or(SystemVerifyError::InvalidArchive)?
        .map_err(|_| SystemVerifyError::InvalidArchive)?;
    validate_entry(&payload_directory, SYSTEM_PAYLOAD_DIRECTORY, true, 0)?;
    drop(payload_directory);

    let mut total_payload_bytes = 0_u64;
    for component in &manifest.components {
        let expected_path = format!("payload/{}", component.path);
        let mut entry = entries
            .next()
            .ok_or(SystemVerifyError::PayloadMismatch)?
            .map_err(|_| SystemVerifyError::InvalidArchive)?;
        validate_entry(&entry, &expected_path, false, component.size)?;
        total_payload_bytes = total_payload_bytes
            .checked_add(component.size)
            .ok_or(SystemVerifyError::PayloadTooLarge)?;
        if total_payload_bytes > MAX_ROOTFS_IMAGE_BYTES + MAX_BOOT_IMAGE_BYTES {
            return Err(SystemVerifyError::PayloadTooLarge);
        }
        let actual_sha256 = hash_component(&mut entry, component)?;
        if actual_sha256 != component.sha256 {
            return Err(SystemVerifyError::PayloadMismatch);
        }
    }

    if entries.next().is_some() {
        return Err(SystemVerifyError::InvalidEntryPath);
    }

    let mut trailing = Vec::new();
    archive
        .into_inner()
        .take(MAX_TRAILING_ZERO_BYTES + 1)
        .read_to_end(&mut trailing)
        .map_err(|_| SystemVerifyError::Io)?;
    if trailing.len() as u64 > MAX_TRAILING_ZERO_BYTES || trailing.iter().any(|byte| *byte != 0) {
        return Err(SystemVerifyError::InvalidArchive);
    }

    Ok(VerifiedSystemBundle {
        manifest,
        manifest_sha256: sha256_hex(&manifest_bytes),
        payload_bytes: total_payload_bytes,
    })
}

pub fn verify_system_manifest_bytes(
    manifest_bytes: &[u8],
    signature_bytes: &[u8; 64],
    trust_root: &SystemTrustRoot,
    policy: &SystemVerificationPolicy,
) -> Result<SystemManifest, SystemVerifyError> {
    if manifest_bytes.is_empty() || manifest_bytes.len() as u64 > MAX_SYSTEM_MANIFEST_BYTES {
        return Err(SystemVerifyError::InvalidManifest);
    }
    let manifest: SystemManifest =
        serde_json::from_slice(manifest_bytes).map_err(|_| SystemVerifyError::InvalidManifest)?;
    let canonical =
        serde_json::to_vec(&manifest).map_err(|_| SystemVerifyError::InvalidManifest)?;
    if canonical != manifest_bytes {
        return Err(SystemVerifyError::InvalidManifest);
    }
    validate_manifest(&manifest, trust_root, policy)?;
    let signature = Signature::from_bytes(signature_bytes);
    trust_root
        .verifying_key
        .verify_strict(manifest_bytes, &signature)
        .map_err(|_| SystemVerifyError::InvalidSignature)?;
    Ok(manifest)
}

fn validate_entry<R: Read>(
    entry: &tar::Entry<'_, R>,
    expected_path: &str,
    directory: bool,
    expected_size: u64,
) -> Result<(), SystemVerifyError> {
    let path_bytes = entry.path_bytes();
    let actual_path = std::str::from_utf8(path_bytes.as_ref())
        .map_err(|_| SystemVerifyError::InvalidEntryPath)?;
    if actual_path != expected_path {
        return Err(SystemVerifyError::InvalidEntryPath);
    }
    let expected_type = if directory { b'5' } else { b'0' };
    if entry.header().entry_type().as_byte() != expected_type {
        return Err(SystemVerifyError::ForbiddenEntryType);
    }
    let header = entry.header();
    let expected_mode = if directory { 0o755 } else { 0o644 };
    if header
        .mode()
        .map_err(|_| SystemVerifyError::InvalidEntryMetadata)?
        != expected_mode
        || header
            .uid()
            .map_err(|_| SystemVerifyError::InvalidEntryMetadata)?
            != 0
        || header
            .gid()
            .map_err(|_| SystemVerifyError::InvalidEntryMetadata)?
            != 0
        || header
            .mtime()
            .map_err(|_| SystemVerifyError::InvalidEntryMetadata)?
            != 0
        || entry.size() != expected_size
    {
        return Err(SystemVerifyError::InvalidEntryMetadata);
    }
    Ok(())
}

fn read_exact_bounded(reader: &mut impl Read, expected: u64) -> Result<Vec<u8>, SystemVerifyError> {
    let capacity = usize::try_from(expected).map_err(|_| SystemVerifyError::InvalidArchive)?;
    let mut bytes = Vec::with_capacity(capacity);
    reader
        .take(expected.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| SystemVerifyError::Io)?;
    if bytes.len() as u64 != expected {
        return Err(SystemVerifyError::InvalidArchive);
    }
    Ok(bytes)
}

fn hash_component(
    reader: &mut impl Read,
    component: &SystemComponent,
) -> Result<String, SystemVerifyError> {
    let (signature_offset, expected_signature) = match component.content_type {
        SystemContentType::Ext4 => (EXT4_MAGIC_OFFSET, [0x53, 0xef]),
        SystemContentType::Vfat => (VFAT_SIGNATURE_OFFSET, [0x55, 0xaa]),
    };
    let mut observed_signature = [0_u8; 2];
    let mut signature_bytes_seen = 0_usize;
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|_| SystemVerifyError::Io)?;
        if read == 0 {
            break;
        }
        let start = total;
        total = total
            .checked_add(read as u64)
            .ok_or(SystemVerifyError::PayloadTooLarge)?;
        if total > component.size {
            return Err(SystemVerifyError::InvalidArchive);
        }
        for index in 0..2_u64 {
            let offset = signature_offset + index;
            if (start..total).contains(&offset) {
                observed_signature[index as usize] = buffer[(offset - start) as usize];
                signature_bytes_seen += 1;
            }
        }
        hasher.update(&buffer[..read]);
    }
    if total != component.size {
        return Err(SystemVerifyError::InvalidArchive);
    }
    if signature_bytes_seen != 2 || observed_signature != expected_signature {
        return Err(SystemVerifyError::InvalidFilesystemImage);
    }
    Ok(hex_lower(hasher.finalize().as_ref()))
}

fn validate_manifest(
    manifest: &SystemManifest,
    trust_root: &SystemTrustRoot,
    policy: &SystemVerificationPolicy,
) -> Result<(), SystemVerifyError> {
    if manifest.format != SYSTEM_FORMAT_V1
        || manifest.product != SYSTEM_PRODUCT
        || manifest.target != SYSTEM_TARGET
        || manifest.architecture != SYSTEM_ARCHITECTURE
        || manifest.key_id != trust_root.key_id
        || !valid_key_id(&manifest.key_id)
        || !valid_version(&manifest.version)
        || manifest.sequence == 0
        || manifest.minimum_recovery_sequence == 0
        || manifest.minimum_boot_control_sequence == 0
        || !valid_layout_id(&manifest.layout_id)
        || !valid_commit(&manifest.source_commit)
    {
        return Err(SystemVerifyError::InvalidManifestShape);
    }
    if manifest.layout_id != policy.layout_id
        || manifest.minimum_recovery_sequence > policy.recovery_sequence
        || manifest.minimum_boot_control_sequence > policy.boot_control_sequence
    {
        return Err(SystemVerifyError::PolicyMismatch);
    }
    validate_components(&manifest.components, policy)
}

fn validate_components(
    components: &[SystemComponent],
    policy: &SystemVerificationPolicy,
) -> Result<(), SystemVerifyError> {
    let expected_len = 1 + usize::from(policy.boot_bytes.is_some());
    if components.len() != expected_len {
        return Err(SystemVerifyError::PolicyMismatch);
    }
    let mut previous_path: Option<&str> = None;
    let mut rootfs_seen = false;
    let mut boot_seen = false;
    for component in components {
        if previous_path.is_some_and(|path| component.path.as_str() <= path)
            || component.size == 0
            || !valid_sha256(&component.sha256)
        {
            return Err(SystemVerifyError::InvalidManifestShape);
        }
        previous_path = Some(component.path.as_str());
        match (
            component.role,
            component.content_type,
            component.path.as_str(),
        ) {
            (SystemComponentRole::Boot, SystemContentType::Vfat, "boot.img") => {
                let expected = policy.boot_bytes.ok_or(SystemVerifyError::PolicyMismatch)?;
                if boot_seen || component.size != expected {
                    return Err(SystemVerifyError::PolicyMismatch);
                }
                boot_seen = true;
            }
            (SystemComponentRole::Rootfs, SystemContentType::Ext4, "rootfs.img") => {
                if rootfs_seen || component.size != policy.rootfs_bytes {
                    return Err(SystemVerifyError::PolicyMismatch);
                }
                rootfs_seen = true;
            }
            _ => return Err(SystemVerifyError::InvalidManifestShape),
        }
    }
    if !rootfs_seen || boot_seen != policy.boot_bytes.is_some() {
        return Err(SystemVerifyError::PolicyMismatch);
    }
    Ok(())
}

fn valid_key_id(value: &str) -> bool {
    (5..=64).contains(&value.len()) && value.starts_with("sys-") && valid_identifier_tail(value)
}

fn valid_layout_id(value: &str) -> bool {
    (8..=64).contains(&value.len()) && value.starts_with("layout-") && valid_identifier_tail(value)
}

fn valid_identifier_tail(value: &str) -> bool {
    !value.ends_with('.')
        && !value.contains("..")
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

fn valid_version(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_digit())
        && value
            .bytes()
            .last()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && !value.contains("..")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'+' | b'-'))
}

fn valid_commit(value: &str) -> bool {
    value.len() == 40
        && !value.bytes().all(|byte| byte == b'0')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && !value.bytes().all(|byte| byte == b'0')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and does not dereference pointers.
    unsafe { libc::geteuid() }
}

fn same_archive_identity(before: &std::fs::Metadata, after: &std::fs::Metadata) -> bool {
    before.dev() == after.dev()
        && before.ino() == after.ino()
        && before.uid() == after.uid()
        && before.nlink() == after.nlink()
        && before.permissions().mode() == after.permissions().mode()
        && before.len() == after.len()
        && before.mtime() == after.mtime()
        && before.mtime_nsec() == after.mtime_nsec()
        && before.ctime() == after.ctime()
        && before.ctime_nsec() == after.ctime_nsec()
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
