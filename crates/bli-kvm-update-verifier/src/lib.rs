use std::{
    collections::{BTreeMap, BTreeSet},
    fs::OpenOptions,
    io::{Cursor, Read},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Component, Path, PathBuf},
};

use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const APPLICATION_FORMAT_V1: &str = "hardened-blikvm-application-v1";
pub const APPLICATION_PRODUCT: &str = "Hardened BliKVM";
pub const APPLICATION_TARGET: &str = "bli-h616-pikvm";
pub const APPLICATION_ARCHITECTURE: &str = "aarch64";
pub const MAX_ARCHIVE_BYTES: u64 = 128 * 1024 * 1024;
pub const MAX_PAYLOAD_BYTES: u64 = 256 * 1024 * 1024;
pub const MAX_MANIFEST_BYTES: u64 = 64 * 1024;
pub const MAX_PAYLOAD_FILES: usize = 4096;

pub const APPLICATION_MANIFEST_PATH: &str = "manifest.json";
pub const APPLICATION_SIGNATURE_PATH: &str = "manifest.sig";
pub const APPLICATION_PAYLOAD_ROOT: &str = "payload";
const MAX_TRAILING_ZERO_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplicationManifest {
    pub format: String,
    pub product: String,
    pub target: String,
    pub architecture: String,
    pub key_id: String,
    pub version: String,
    pub sequence: u64,
    pub minimum_system_sequence: u64,
    pub source_commit: String,
    pub files: Vec<ApplicationFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplicationFile {
    pub path: String,
    pub mode: u32,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone)]
pub struct ApplicationTrustRoot {
    key_id: String,
    verifying_key: VerifyingKey,
}

impl ApplicationTrustRoot {
    pub fn new(key_id: impl Into<String>, public_key: [u8; 32]) -> Result<Self, UpdateVerifyError> {
        let key_id = key_id.into();
        if !valid_key_id(&key_id) {
            return Err(UpdateVerifyError::InvalidTrustRoot);
        }
        let verifying_key = VerifyingKey::from_bytes(&public_key)
            .map_err(|_| UpdateVerifyError::InvalidTrustRoot)?;
        Ok(Self {
            key_id,
            verifying_key,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedApplicationBundle {
    pub manifest: ApplicationManifest,
    pub manifest_sha256: String,
    pub payload_bytes: u64,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum UpdateVerifyError {
    #[error("application trust root is invalid")]
    InvalidTrustRoot,
    #[error("application archive path is invalid")]
    InvalidArchivePath,
    #[error("application archive file is unsafe")]
    UnsafeArchiveFile,
    #[error("application archive exceeded its size limit")]
    ArchiveTooLarge,
    #[error("application archive is invalid")]
    InvalidArchive,
    #[error("application archive entry type is forbidden")]
    ForbiddenEntryType,
    #[error("application archive entry metadata is invalid")]
    InvalidEntryMetadata,
    #[error("application archive path is invalid")]
    InvalidEntryPath,
    #[error("application archive contains duplicate or unsorted entries")]
    DuplicateOrUnsortedEntry,
    #[error("application manifest is missing or too large")]
    InvalidManifest,
    #[error("application signature is missing or invalid")]
    InvalidSignature,
    #[error("application manifest shape is invalid")]
    InvalidManifestShape,
    #[error("application payload exceeded its size limit")]
    PayloadTooLarge,
    #[error("application payload does not match the manifest")]
    PayloadMismatch,
    #[error("application archive I/O failed")]
    Io,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ObservedFile {
    mode: u32,
    size: u64,
    sha256: String,
}

pub fn verify_application_file(
    path: impl Into<PathBuf>,
    trust_root: &ApplicationTrustRoot,
) -> Result<VerifiedApplicationBundle, UpdateVerifyError> {
    let path = path.into();
    if !path.is_absolute() || path.file_name().is_none() {
        return Err(UpdateVerifyError::InvalidArchivePath);
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| UpdateVerifyError::UnsafeArchiveFile)?;
    let metadata = file.metadata().map_err(|_| UpdateVerifyError::Io)?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o022 != 0 {
        return Err(UpdateVerifyError::UnsafeArchiveFile);
    }
    if metadata.len() == 0 || metadata.len() > MAX_ARCHIVE_BYTES {
        return Err(UpdateVerifyError::ArchiveTooLarge);
    }
    verify_application_reader(file, metadata.len(), trust_root)
}

pub fn verify_application_bytes(
    bytes: &[u8],
    trust_root: &ApplicationTrustRoot,
) -> Result<VerifiedApplicationBundle, UpdateVerifyError> {
    if bytes.is_empty() || bytes.len() as u64 > MAX_ARCHIVE_BYTES {
        return Err(UpdateVerifyError::ArchiveTooLarge);
    }
    verify_application_reader(Cursor::new(bytes), bytes.len() as u64, trust_root)
}

pub fn verify_application_reader(
    reader: impl Read,
    archive_size: u64,
    trust_root: &ApplicationTrustRoot,
) -> Result<VerifiedApplicationBundle, UpdateVerifyError> {
    if archive_size == 0 || archive_size > MAX_ARCHIVE_BYTES {
        return Err(UpdateVerifyError::ArchiveTooLarge);
    }
    let mut archive = tar::Archive::new(reader);
    let entries = archive
        .entries()
        .map_err(|_| UpdateVerifyError::InvalidArchive)?
        .raw(true);
    let mut previous_path: Option<String> = None;
    let mut manifest_bytes: Option<Vec<u8>> = None;
    let mut signature_bytes: Option<[u8; 64]> = None;
    let mut payload_files = BTreeMap::new();
    let mut payload_directories = BTreeSet::new();
    let mut total_payload_bytes = 0_u64;
    let mut entry_count = 0_usize;

    for entry in entries {
        let mut entry = entry.map_err(|_| UpdateVerifyError::InvalidArchive)?;
        entry_count = entry_count.saturating_add(1);
        if entry_count > MAX_PAYLOAD_FILES.saturating_mul(2).saturating_add(16) {
            return Err(UpdateVerifyError::InvalidArchive);
        }
        let entry_type = entry.header().entry_type();
        if !(entry_type.is_file() || entry_type.is_dir()) {
            return Err(UpdateVerifyError::ForbiddenEntryType);
        }
        let path = normalized_entry_path(entry.path_bytes().as_ref(), entry_type.is_dir())?;
        if previous_path
            .as_ref()
            .is_some_and(|previous| path <= *previous)
        {
            return Err(UpdateVerifyError::DuplicateOrUnsortedEntry);
        }
        previous_path = Some(path.clone());
        validate_header(entry.header(), entry_type.is_dir())?;

        if entry_type.is_dir() {
            if path == APPLICATION_PAYLOAD_ROOT {
                payload_directories.insert(path);
                continue;
            }
            let relative = path
                .strip_prefix("payload/")
                .ok_or(UpdateVerifyError::InvalidEntryPath)?;
            validate_application_payload_path(relative)?;
            payload_directories.insert(path);
            continue;
        }

        let size = entry.size();
        match path.as_str() {
            APPLICATION_MANIFEST_PATH => {
                if entry.header().mode().ok() != Some(0o644)
                    || size == 0
                    || size > MAX_MANIFEST_BYTES
                    || manifest_bytes.is_some()
                {
                    return Err(UpdateVerifyError::InvalidManifest);
                }
                manifest_bytes = Some(read_exact_bounded(&mut entry, size)?);
            }
            APPLICATION_SIGNATURE_PATH => {
                if entry.header().mode().ok() != Some(0o644)
                    || size != 64
                    || signature_bytes.is_some()
                {
                    return Err(UpdateVerifyError::InvalidSignature);
                }
                let bytes = read_exact_bounded(&mut entry, size)?;
                signature_bytes = Some(
                    bytes
                        .try_into()
                        .map_err(|_| UpdateVerifyError::InvalidSignature)?,
                );
            }
            _ => {
                let relative = path
                    .strip_prefix("payload/")
                    .ok_or(UpdateVerifyError::InvalidEntryPath)?;
                validate_application_payload_path(relative)?;
                total_payload_bytes = total_payload_bytes
                    .checked_add(size)
                    .ok_or(UpdateVerifyError::PayloadTooLarge)?;
                if total_payload_bytes > MAX_PAYLOAD_BYTES
                    || payload_files.len() >= MAX_PAYLOAD_FILES
                {
                    return Err(UpdateVerifyError::PayloadTooLarge);
                }
                let sha256 = hash_entry(&mut entry, size)?;
                let mode = entry
                    .header()
                    .mode()
                    .map_err(|_| UpdateVerifyError::InvalidEntryMetadata)?;
                if payload_files
                    .insert(relative.to_string(), ObservedFile { mode, size, sha256 })
                    .is_some()
                {
                    return Err(UpdateVerifyError::DuplicateOrUnsortedEntry);
                }
            }
        }
    }

    let mut trailing = Vec::new();
    archive
        .into_inner()
        .take(MAX_TRAILING_ZERO_BYTES + 1)
        .read_to_end(&mut trailing)
        .map_err(|_| UpdateVerifyError::Io)?;
    if trailing.len() as u64 > MAX_TRAILING_ZERO_BYTES || trailing.iter().any(|byte| *byte != 0) {
        return Err(UpdateVerifyError::InvalidArchive);
    }

    let manifest_bytes = manifest_bytes.ok_or(UpdateVerifyError::InvalidManifest)?;
    let signature_bytes = signature_bytes.ok_or(UpdateVerifyError::InvalidSignature)?;
    let manifest =
        verify_application_manifest_bytes(&manifest_bytes, &signature_bytes, trust_root)?;
    validate_payload_tree(&manifest, &payload_files, &payload_directories)?;

    Ok(VerifiedApplicationBundle {
        manifest,
        manifest_sha256: sha256_hex(&manifest_bytes),
        payload_bytes: total_payload_bytes,
    })
}

pub fn verify_application_manifest_bytes(
    manifest_bytes: &[u8],
    signature_bytes: &[u8; 64],
    trust_root: &ApplicationTrustRoot,
) -> Result<ApplicationManifest, UpdateVerifyError> {
    if manifest_bytes.is_empty() || manifest_bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(UpdateVerifyError::InvalidManifest);
    }
    let manifest: ApplicationManifest =
        serde_json::from_slice(manifest_bytes).map_err(|_| UpdateVerifyError::InvalidManifest)?;
    validate_manifest(&manifest, trust_root)?;
    let signature = Signature::from_bytes(signature_bytes);
    trust_root
        .verifying_key
        .verify_strict(manifest_bytes, &signature)
        .map_err(|_| UpdateVerifyError::InvalidSignature)?;
    Ok(manifest)
}

fn validate_header(header: &tar::Header, directory: bool) -> Result<(), UpdateVerifyError> {
    let mode = header
        .mode()
        .map_err(|_| UpdateVerifyError::InvalidEntryMetadata)?;
    let uid = header
        .uid()
        .map_err(|_| UpdateVerifyError::InvalidEntryMetadata)?;
    let gid = header
        .gid()
        .map_err(|_| UpdateVerifyError::InvalidEntryMetadata)?;
    let mtime = header
        .mtime()
        .map_err(|_| UpdateVerifyError::InvalidEntryMetadata)?;
    if uid != 0 || gid != 0 || mtime != 0 || (directory && mode != 0o755) {
        return Err(UpdateVerifyError::InvalidEntryMetadata);
    }
    Ok(())
}

fn normalized_entry_path(raw: &[u8], directory: bool) -> Result<String, UpdateVerifyError> {
    let value = std::str::from_utf8(raw).map_err(|_| UpdateVerifyError::InvalidEntryPath)?;
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
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-'))
    {
        return Err(UpdateVerifyError::InvalidEntryPath);
    }
    validate_relative_payload_path(value)?;
    Ok(value.to_string())
}

fn validate_relative_payload_path(value: &str) -> Result<(), UpdateVerifyError> {
    let path = Path::new(value);
    if value.is_empty()
        || value.contains('\0')
        || value.contains('\\')
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-'))
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || value.split('/').any(|component| component.is_empty())
        || value.contains("..")
    {
        return Err(UpdateVerifyError::InvalidEntryPath);
    }
    Ok(())
}

fn validate_application_payload_path(value: &str) -> Result<(), UpdateVerifyError> {
    validate_relative_payload_path(value)?;
    let top_level = value
        .split('/')
        .next()
        .ok_or(UpdateVerifyError::InvalidEntryPath)?;
    if !matches!(top_level, "bin" | "libexec" | "share") {
        return Err(UpdateVerifyError::InvalidEntryPath);
    }
    Ok(())
}

fn read_exact_bounded(reader: &mut impl Read, expected: u64) -> Result<Vec<u8>, UpdateVerifyError> {
    let mut bytes = Vec::with_capacity(expected as usize);
    reader
        .take(expected.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| UpdateVerifyError::Io)?;
    if bytes.len() as u64 != expected {
        return Err(UpdateVerifyError::InvalidArchive);
    }
    Ok(bytes)
}

fn hash_entry(reader: &mut impl Read, expected: u64) -> Result<String, UpdateVerifyError> {
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|_| UpdateVerifyError::Io)?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or(UpdateVerifyError::PayloadTooLarge)?;
        if total > expected {
            return Err(UpdateVerifyError::InvalidArchive);
        }
        hasher.update(&buffer[..read]);
    }
    if total != expected {
        return Err(UpdateVerifyError::InvalidArchive);
    }
    Ok(hex_lower(hasher.finalize().as_ref()))
}

fn validate_manifest(
    manifest: &ApplicationManifest,
    trust_root: &ApplicationTrustRoot,
) -> Result<(), UpdateVerifyError> {
    if manifest.format != APPLICATION_FORMAT_V1
        || manifest.product != APPLICATION_PRODUCT
        || manifest.target != APPLICATION_TARGET
        || manifest.architecture != APPLICATION_ARCHITECTURE
        || manifest.key_id != trust_root.key_id
        || !valid_key_id(&manifest.key_id)
        || !valid_version(&manifest.version)
        || manifest.sequence == 0
        || manifest.minimum_system_sequence == 0
        || !valid_commit(&manifest.source_commit)
        || manifest.files.is_empty()
        || manifest.files.len() > MAX_PAYLOAD_FILES
    {
        return Err(UpdateVerifyError::InvalidManifestShape);
    }
    let mut previous: Option<&str> = None;
    let mut total = 0_u64;
    for file in &manifest.files {
        validate_application_payload_path(&file.path)
            .map_err(|_| UpdateVerifyError::InvalidManifestShape)?;
        if previous.is_some_and(|value| file.path.as_str() <= value)
            || !matches!(file.mode, 0o644 | 0o755)
            || file.size == 0
            || !valid_sha256(&file.sha256)
        {
            return Err(UpdateVerifyError::InvalidManifestShape);
        }
        previous = Some(file.path.as_str());
        total = total
            .checked_add(file.size)
            .ok_or(UpdateVerifyError::PayloadTooLarge)?;
        if total > MAX_PAYLOAD_BYTES {
            return Err(UpdateVerifyError::PayloadTooLarge);
        }
    }
    for (path, mode) in [
        ("bin/bli-kvm-server", 0o755),
        ("libexec/bli-kvm-platform-broker", 0o755),
        ("libexec/bli-kvm-video-proxy", 0o755),
        ("libexec/bli-kvm-control-broker", 0o755),
        ("share/web/index.html", 0o644),
    ] {
        if !manifest
            .files
            .iter()
            .any(|file| file.path == path && file.mode == mode)
        {
            return Err(UpdateVerifyError::InvalidManifestShape);
        }
    }
    Ok(())
}

fn validate_payload_tree(
    manifest: &ApplicationManifest,
    observed: &BTreeMap<String, ObservedFile>,
    directories: &BTreeSet<String>,
) -> Result<(), UpdateVerifyError> {
    if manifest.files.len() != observed.len() {
        return Err(UpdateVerifyError::PayloadMismatch);
    }
    for file in &manifest.files {
        let Some(actual) = observed.get(&file.path) else {
            return Err(UpdateVerifyError::PayloadMismatch);
        };
        if actual.mode != file.mode || actual.size != file.size || actual.sha256 != file.sha256 {
            return Err(UpdateVerifyError::PayloadMismatch);
        }
    }
    let mut expected_directories = BTreeSet::from([APPLICATION_PAYLOAD_ROOT.to_string()]);
    for file in &manifest.files {
        let mut path = PathBuf::from(APPLICATION_PAYLOAD_ROOT);
        let components = Path::new(&file.path).components().collect::<Vec<_>>();
        for component in components.iter().take(components.len().saturating_sub(1)) {
            path.push(component.as_os_str());
            expected_directories.insert(
                path.to_str()
                    .ok_or(UpdateVerifyError::InvalidEntryPath)?
                    .to_string(),
            );
        }
    }
    if *directories != expected_directories {
        return Err(UpdateVerifyError::PayloadMismatch);
    }
    Ok(())
}

fn valid_key_id(value: &str) -> bool {
    (5..=64).contains(&value.len())
        && value.starts_with("app-")
        && !value.ends_with('.')
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
mod tests {
    use ed25519_dalek::{Signer, SigningKey};
    use tar::{Builder, EntryType, Header};

    use super::*;

    const KEY_ID: &str = "app-test-key-1";

    fn payload() -> BTreeMap<&'static str, (u32, Vec<u8>)> {
        BTreeMap::from([
            ("bin/bli-kvm-server", (0o755, b"server".to_vec())),
            (
                "libexec/bli-kvm-control-broker",
                (0o755, b"control".to_vec()),
            ),
            (
                "libexec/bli-kvm-platform-broker",
                (0o755, b"broker".to_vec()),
            ),
            ("libexec/bli-kvm-video-proxy", (0o755, b"video".to_vec())),
            ("share/web/index.html", (0o644, b"<html></html>".to_vec())),
        ])
    }

    fn manifest(payload: &BTreeMap<&str, (u32, Vec<u8>)>) -> ApplicationManifest {
        ApplicationManifest {
            format: APPLICATION_FORMAT_V1.into(),
            product: APPLICATION_PRODUCT.into(),
            target: APPLICATION_TARGET.into(),
            architecture: APPLICATION_ARCHITECTURE.into(),
            key_id: KEY_ID.into(),
            version: "0.2.0-dev".into(),
            sequence: 2,
            minimum_system_sequence: 1,
            source_commit: "1111111111111111111111111111111111111111".into(),
            files: payload
                .iter()
                .map(|(path, (mode, bytes))| ApplicationFile {
                    path: (*path).to_string(),
                    mode: *mode,
                    size: bytes.len() as u64,
                    sha256: sha256_hex(bytes),
                })
                .collect(),
        }
    }

    fn header(path: &str, mode: u32, size: u64, entry_type: EntryType) -> Header {
        let mut header = Header::new_ustar();
        header.set_path(path).unwrap();
        header.set_mode(mode);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_size(size);
        header.set_entry_type(entry_type);
        header.set_cksum();
        header
    }

    fn payload_directories(payload: &BTreeMap<&str, (u32, Vec<u8>)>) -> BTreeSet<String> {
        let mut directories = BTreeSet::from(["payload".to_string()]);
        for path in payload.keys() {
            let mut current = PathBuf::from("payload");
            let components = Path::new(path).components().collect::<Vec<_>>();
            for component in components.iter().take(components.len() - 1) {
                current.push(component.as_os_str());
                directories.insert(current.to_str().unwrap().to_string());
            }
        }
        directories
    }

    fn bundle(
        signing_key: &SigningKey,
        manifest: &ApplicationManifest,
        payload: &BTreeMap<&str, (u32, Vec<u8>)>,
        extra: Option<(&str, EntryType)>,
    ) -> Vec<u8> {
        bundle_with_signature(signing_key, manifest, payload, extra, None)
    }

    fn bundle_with_signature(
        signing_key: &SigningKey,
        manifest: &ApplicationManifest,
        payload: &BTreeMap<&str, (u32, Vec<u8>)>,
        extra: Option<(&str, EntryType)>,
        signature_override: Option<[u8; 64]>,
    ) -> Vec<u8> {
        let manifest_bytes = serde_json::to_vec(manifest).unwrap();
        let signature =
            signature_override.unwrap_or_else(|| signing_key.sign(&manifest_bytes).to_bytes());
        let mut builder = Builder::new(Vec::new());
        builder.mode(tar::HeaderMode::Deterministic);
        builder
            .append(
                &header(
                    APPLICATION_MANIFEST_PATH,
                    0o644,
                    manifest_bytes.len() as u64,
                    EntryType::Regular,
                ),
                manifest_bytes.as_slice(),
            )
            .unwrap();
        builder
            .append(
                &header(APPLICATION_SIGNATURE_PATH, 0o644, 64, EntryType::Regular),
                signature.as_slice(),
            )
            .unwrap();
        let directories = payload_directories(payload);
        let mut entries = directories.clone();
        entries.extend(payload.keys().map(|path| format!("payload/{path}")));
        for path in entries {
            if directories.contains(&path) {
                builder
                    .append(&header(&path, 0o755, 0, EntryType::Directory), &[][..])
                    .unwrap();
            } else {
                let relative = path.strip_prefix("payload/").unwrap();
                let (mode, bytes) = payload.get(relative).unwrap();
                builder
                    .append(
                        &header(&path, *mode, bytes.len() as u64, EntryType::Regular),
                        bytes.as_slice(),
                    )
                    .unwrap();
            }
        }
        if let Some((path, entry_type)) = extra {
            builder
                .append(&header(path, 0o644, 0, entry_type), &[][..])
                .unwrap();
        }
        builder.into_inner().unwrap()
    }

    fn keys() -> (SigningKey, ApplicationTrustRoot) {
        let signing = SigningKey::from_bytes(&[7; 32]);
        let trust = ApplicationTrustRoot::new(KEY_ID, signing.verifying_key().to_bytes()).unwrap();
        (signing, trust)
    }

    #[test]
    fn verifies_signed_exact_deterministic_payload_tree() {
        let (signing, trust) = keys();
        let payload = payload();
        let manifest = manifest(&payload);
        let archive = bundle(&signing, &manifest, &payload, None);
        let verified = verify_application_bytes(&archive, &trust).unwrap();
        assert_eq!(verified.manifest, manifest);
        assert_eq!(verified.payload_bytes, 6 + 7 + 6 + 5 + 13);
        assert_eq!(verified.manifest_sha256.len(), 64);
    }

    #[test]
    fn rejects_wrong_key_manifest_tamper_and_payload_tamper() {
        let (signing, trust) = keys();
        let payload = payload();
        let manifest = manifest(&payload);
        let archive = bundle(&signing, &manifest, &payload, None);
        let wrong = SigningKey::from_bytes(&[8; 32]);
        let wrong_trust =
            ApplicationTrustRoot::new(KEY_ID, wrong.verifying_key().to_bytes()).unwrap();
        assert_eq!(
            verify_application_bytes(&archive, &wrong_trust),
            Err(UpdateVerifyError::InvalidSignature)
        );

        let mut changed_manifest = manifest.clone();
        changed_manifest.sequence = 3;
        let original_signature = signing
            .sign(&serde_json::to_vec(&manifest).unwrap())
            .to_bytes();
        let changed = bundle_with_signature(
            &signing,
            &changed_manifest,
            &payload,
            None,
            Some(original_signature),
        );
        assert_eq!(
            verify_application_bytes(&changed, &trust),
            Err(UpdateVerifyError::InvalidSignature)
        );
        let mut tampered = payload.clone();
        tampered.insert("bin/bli-kvm-server", (0o755, b"tampered".to_vec()));
        let tampered_bundle = bundle(&signing, &manifest, &tampered, None);
        assert_eq!(
            verify_application_bytes(&tampered_bundle, &trust),
            Err(UpdateVerifyError::PayloadMismatch)
        );
    }

    #[test]
    fn rejects_forbidden_entry_types_extra_files_and_bad_paths() {
        let (signing, trust) = keys();
        let payload = payload();
        let manifest = manifest(&payload);
        let symlink = bundle(
            &signing,
            &manifest,
            &payload,
            Some(("payload/z-link", EntryType::Symlink)),
        );
        assert_eq!(
            verify_application_bytes(&symlink, &trust),
            Err(UpdateVerifyError::ForbiddenEntryType)
        );
        let hardlink = bundle(
            &signing,
            &manifest,
            &payload,
            Some(("payload/z-hardlink", EntryType::Link)),
        );
        assert_eq!(
            verify_application_bytes(&hardlink, &trust),
            Err(UpdateVerifyError::ForbiddenEntryType)
        );
        let mut pax = Builder::new(Vec::new());
        pax.append_pax_extensions([("comment", b"forbidden".as_slice())])
            .unwrap();
        let pax = pax.into_inner().unwrap();
        assert_eq!(
            verify_application_bytes(&pax, &trust),
            Err(UpdateVerifyError::ForbiddenEntryType)
        );
        let duplicate = bundle(
            &signing,
            &manifest,
            &payload,
            Some(("payload/share/web/index.html", EntryType::Regular)),
        );
        assert_eq!(
            verify_application_bytes(&duplicate, &trust),
            Err(UpdateVerifyError::DuplicateOrUnsortedEntry)
        );
        let mut trailing = bundle(&signing, &manifest, &payload, None);
        trailing.extend_from_slice(b"nonzero-trailing-data");
        assert_eq!(
            verify_application_bytes(&trailing, &trust),
            Err(UpdateVerifyError::InvalidArchive)
        );
        assert_eq!(
            validate_relative_payload_path("../escape"),
            Err(UpdateVerifyError::InvalidEntryPath)
        );
        assert_eq!(
            validate_relative_payload_path("dir\\file"),
            Err(UpdateVerifyError::InvalidEntryPath)
        );
        assert_eq!(
            validate_application_payload_path("other/authorized-but-out-of-slot"),
            Err(UpdateVerifyError::InvalidEntryPath)
        );
        let mut outside_slot = payload.clone();
        outside_slot.insert(
            "other/authorized-but-out-of-slot",
            (0o644, b"signed but forbidden".to_vec()),
        );
        let outside_manifest = self::manifest(&outside_slot);
        let outside_bundle = bundle(&signing, &outside_manifest, &outside_slot, None);
        assert_eq!(
            verify_application_bytes(&outside_bundle, &trust),
            Err(UpdateVerifyError::InvalidEntryPath)
        );
    }

    #[test]
    fn rejects_unsorted_manifest_and_unsigned_or_wrong_domain_key() {
        let (signing, trust) = keys();
        let payload = payload();
        let mut unsorted_manifest = manifest(&payload);
        unsorted_manifest.files.swap(0, 1);
        let archive = bundle(&signing, &unsorted_manifest, &payload, None);
        assert_eq!(
            verify_application_bytes(&archive, &trust),
            Err(UpdateVerifyError::InvalidManifestShape)
        );
        let manifest_bytes = serde_json::to_vec(&manifest(&payload)).unwrap();
        let mut unsigned = Builder::new(Vec::new());
        unsigned.mode(tar::HeaderMode::Deterministic);
        unsigned
            .append(
                &header(
                    APPLICATION_MANIFEST_PATH,
                    0o644,
                    manifest_bytes.len() as u64,
                    EntryType::Regular,
                ),
                manifest_bytes.as_slice(),
            )
            .unwrap();
        let unsigned = unsigned.into_inner().unwrap();
        assert_eq!(
            verify_application_bytes(&unsigned, &trust),
            Err(UpdateVerifyError::InvalidSignature)
        );
        assert!(matches!(
            ApplicationTrustRoot::new("system-test-key", signing.verifying_key().to_bytes()),
            Err(UpdateVerifyError::InvalidTrustRoot)
        ));

        let mut wrong_target = manifest(&payload);
        wrong_target.target = "other-board".into();
        let archive = bundle(&signing, &wrong_target, &payload, None);
        assert_eq!(
            verify_application_bytes(&archive, &trust),
            Err(UpdateVerifyError::InvalidManifestShape)
        );

        let mut missing_required = manifest(&payload);
        missing_required
            .files
            .retain(|file| file.path != "libexec/bli-kvm-control-broker");
        let archive = bundle(&signing, &missing_required, &payload, None);
        assert_eq!(
            verify_application_bytes(&archive, &trust),
            Err(UpdateVerifyError::InvalidManifestShape)
        );
    }
}
