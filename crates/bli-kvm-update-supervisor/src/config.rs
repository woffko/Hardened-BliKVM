use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};

use bli_kvm_update_manager::{MAXIMUM_HEALTH_TIMEOUT_MS, MINIMUM_HEALTH_TIMEOUT_MS};
use bli_kvm_update_verifier::{APPLICATION_PRODUCT, APPLICATION_TARGET, ApplicationTrustRoot};
use serde::Deserialize;

use crate::SupervisorCliError;

const CONFIG_SCHEMA_VERSION: u32 = 1;
const MAX_CONFIG_BYTES: u64 = 16 * 1024;
const CREDENTIAL_ROOT: &str = "/run/credentials";
const CREDENTIAL_NAME: &str = "update-supervisor-config";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConfigDocument {
    schema_version: u32,
    product: String,
    target: String,
    key_id: String,
    public_key_hex: String,
    slot_root: PathBuf,
    state_root: PathBuf,
    runtime_root: PathBuf,
    system_sequence: u64,
    health_timeout_ms: u64,
}

#[derive(Debug)]
pub(crate) struct LoadedConfig {
    pub(crate) trust_root: ApplicationTrustRoot,
    pub(crate) slot_root: PathBuf,
    pub(crate) state_root: PathBuf,
    pub(crate) runtime_root: PathBuf,
    pub(crate) system_sequence: u64,
    pub(crate) health_timeout_ms: u64,
}

pub(crate) fn load_config(path: &Path) -> Result<LoadedConfig, SupervisorCliError> {
    if !path.is_absolute() || path.file_name().is_none() {
        return Err(SupervisorCliError::InvalidConfigPath);
    }
    let canonical = fs::canonicalize(path).map_err(|_| SupervisorCliError::UnsafeConfigFile)?;
    if canonical != path {
        return Err(SupervisorCliError::UnsafeConfigFile);
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| SupervisorCliError::UnsafeConfigFile)?;
    let metadata = file
        .metadata()
        .map_err(|_| SupervisorCliError::UnsafeConfigFile)?;
    let owner = metadata.uid();
    let trusted_owner = if path.starts_with(CREDENTIAL_ROOT) {
        trusted_credential(&file, path, effective_uid())?
    } else {
        trusted_config_owner(owner)
    };
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.permissions().mode() & 0o022 != 0
        || metadata.permissions().mode() & 0o7000 != 0
        || !trusted_owner
        || metadata.len() == 0
        || metadata.len() > MAX_CONFIG_BYTES
    {
        return Err(SupervisorCliError::UnsafeConfigFile);
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_CONFIG_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != metadata.len() {
        return Err(SupervisorCliError::UnsafeConfigFile);
    }
    let document: ConfigDocument =
        serde_json::from_slice(&bytes).map_err(|_| SupervisorCliError::InvalidConfig)?;
    validate_document(document)
}

// Unlike an ordinary root-owned config, systemd's credential copy belongs to
// the service UID. Accept it only beneath root-controlled ancestors and on a
// read-only credential mount. Mode 0400 alone is not a trust boundary.
fn trusted_credential(file: &File, path: &Path, uid: u32) -> Result<bool, SupervisorCliError> {
    trusted_named_credential(file, path, uid, CREDENTIAL_NAME)
}

fn trusted_named_credential(
    file: &File,
    path: &Path,
    uid: u32,
    name: &str,
) -> Result<bool, SupervisorCliError> {
    let Some(directory) = path.parent() else {
        return Ok(false);
    };
    if path.file_name() != Some(std::ffi::OsStr::new(name))
        || directory.parent() != Some(Path::new(CREDENTIAL_ROOT))
    {
        return Ok(false);
    }
    let directory_file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(directory)
        .map_err(|_| SupervisorCliError::UnsafeConfigFile)?;
    let metadata = file.metadata()?;
    let parent_metadata = directory_file.metadata()?;
    if !metadata.is_file()
        || metadata.uid() != uid
        || metadata.nlink() != 1
        || metadata.permissions().mode() & 0o7777 != 0o400
        || !parent_metadata.is_dir()
        || parent_metadata.uid() != uid
        || parent_metadata.permissions().mode() & 0o7077 != 0
        || metadata.dev() != parent_metadata.dev()
        || !read_only_mount(file)?
        || !read_only_mount(&directory_file)?
    {
        return Ok(false);
    }
    for ancestor in Path::new(CREDENTIAL_ROOT).ancestors() {
        let metadata = fs::symlink_metadata(ancestor)?;
        if !metadata.is_dir() || metadata.uid() != 0 || metadata.permissions().mode() & 0o022 != 0 {
            return Ok(false);
        }
    }
    Ok(true)
}

pub(crate) fn load_apply_credential(config_path: &Path) -> Result<Vec<u8>, SupervisorCliError> {
    const NAME: &str = "confirmed-apply-request";
    let directory = config_path
        .parent()
        .ok_or(SupervisorCliError::UnsafeConfigFile)?;
    let path = directory.join(NAME);
    if fs::canonicalize(&path).map_err(|_| SupervisorCliError::UnsafeConfigFile)? != path {
        return Err(SupervisorCliError::UnsafeConfigFile);
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)?;
    if !trusted_named_credential(&file, &path, effective_uid(), NAME)? {
        return Err(SupervisorCliError::UnsafeConfigFile);
    }
    let limit = bli_kvm_update_runtime::MAX_APPLY_REQUEST_BYTES as u64;
    let size = file.metadata()?.len();
    if size == 0 || size > limit {
        return Err(SupervisorCliError::UnsafeConfigFile);
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 != size {
        return Err(SupervisorCliError::UnsafeConfigFile);
    }
    Ok(bytes)
}

fn read_only_mount(file: &File) -> Result<bool, SupervisorCliError> {
    let mut result = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: fd is borrowed from a live File, and result points to writable
    // storage of the required type. It is read only after fstatvfs succeeds.
    if unsafe { libc::fstatvfs(file.as_raw_fd(), result.as_mut_ptr()) } != 0 {
        return Err(SupervisorCliError::Io(std::io::Error::last_os_error()));
    }
    // SAFETY: fstatvfs returned success and initialized the structure.
    Ok(unsafe { result.assume_init() }.f_flag & libc::ST_RDONLY != 0)
}

fn validate_document(document: ConfigDocument) -> Result<LoadedConfig, SupervisorCliError> {
    if document.schema_version != CONFIG_SCHEMA_VERSION
        || document.product != APPLICATION_PRODUCT
        || document.target != APPLICATION_TARGET
        || document.system_sequence == 0
        || !(MINIMUM_HEALTH_TIMEOUT_MS..=MAXIMUM_HEALTH_TIMEOUT_MS)
            .contains(&document.health_timeout_ms)
        || [
            &document.slot_root,
            &document.state_root,
            &document.runtime_root,
        ]
        .iter()
        .any(|path| !path.is_absolute() || path.file_name().is_none())
        || document.slot_root.starts_with(&document.state_root)
        || document.state_root.starts_with(&document.slot_root)
        || document.slot_root.starts_with(&document.runtime_root)
        || document.runtime_root.starts_with(&document.slot_root)
        || document.state_root.starts_with(&document.runtime_root)
        || document.runtime_root.starts_with(&document.state_root)
    {
        return Err(SupervisorCliError::InvalidConfig);
    }
    let public_key = decode_public_key(&document.public_key_hex)?;
    let trust_root = ApplicationTrustRoot::new(document.key_id, public_key)
        .map_err(|_| SupervisorCliError::InvalidConfig)?;
    Ok(LoadedConfig {
        trust_root,
        slot_root: document.slot_root,
        state_root: document.state_root,
        runtime_root: document.runtime_root,
        system_sequence: document.system_sequence,
        health_timeout_ms: document.health_timeout_ms,
    })
}

fn decode_public_key(value: &str) -> Result<[u8; 32], SupervisorCliError> {
    if value.len() != 64
        || value.bytes().all(|byte| byte == b'0')
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(SupervisorCliError::InvalidConfig);
    }
    let mut output = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        output[index] = (decode_nibble(pair[0])? << 4) | decode_nibble(pair[1])?;
    }
    Ok(output)
}

fn decode_nibble(value: u8) -> Result<u8, SupervisorCliError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err(SupervisorCliError::InvalidConfig),
    }
}

fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and does not dereference pointers.
    unsafe { libc::geteuid() }
}

fn trusted_config_owner(owner: u32) -> bool {
    owner == 0 || (cfg!(debug_assertions) && owner == effective_uid())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use ed25519_dalek::SigningKey;
    use serde_json::json;

    use super::*;

    #[test]
    fn owner_read_only_file_on_writable_mount_is_not_a_credential() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(CREDENTIAL_NAME);
        fs::write(&path, b"public config").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
        let file = File::open(&path).unwrap();
        assert!(!read_only_mount(&file).unwrap());
        assert!(!trusted_credential(&file, &path, effective_uid()).unwrap());
        assert!(
            !trusted_credential(
                &file,
                Path::new("/run/credentials/config.json"),
                effective_uid()
            )
            .unwrap()
        );
    }

    #[test]
    fn apply_credential_rejects_an_ordinary_read_only_sibling() {
        let directory = tempfile::tempdir().unwrap();
        let config_path = directory.path().join(CREDENTIAL_NAME);
        let request = directory.path().join("confirmed-apply-request");
        fs::write(&request, b"public request").unwrap();
        fs::set_permissions(&request, fs::Permissions::from_mode(0o400)).unwrap();
        assert!(matches!(
            load_apply_credential(&config_path),
            Err(SupervisorCliError::UnsafeConfigFile)
        ));
    }

    #[test]
    fn apply_credential_requires_the_fixed_sibling_and_rejects_symlinks() {
        let directory = tempfile::tempdir().unwrap();
        let config_path = directory.path().join(CREDENTIAL_NAME);
        let alternate = directory.path().join("alternate-request");
        fs::write(&alternate, b"public request").unwrap();
        fs::set_permissions(&alternate, fs::Permissions::from_mode(0o400)).unwrap();
        assert!(matches!(
            load_apply_credential(&config_path),
            Err(SupervisorCliError::UnsafeConfigFile)
        ));
        symlink(&alternate, directory.path().join("confirmed-apply-request")).unwrap();
        assert!(matches!(
            load_apply_credential(&config_path),
            Err(SupervisorCliError::UnsafeConfigFile)
        ));
    }

    fn valid_document(root: &Path) -> serde_json::Value {
        let signing = SigningKey::from_bytes(&[47; 32]);
        json!({
            "schemaVersion": 1,
            "product": APPLICATION_PRODUCT,
            "target": APPLICATION_TARGET,
            "keyId": "app-supervisor-test-key-1",
            "publicKeyHex": encode_hex(&signing.verifying_key().to_bytes()),
            "slotRoot": root.join("slots"),
            "stateRoot": root.join("state"),
            "runtimeRoot": root.join("runtime"),
            "systemSequence": 1,
            "healthTimeoutMs": 60_000,
        })
    }

    fn write_config(directory: &Path, document: &serde_json::Value) -> PathBuf {
        let path = directory.join("supervisor.json");
        fs::write(&path, serde_json::to_vec(document).unwrap()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        path
    }

    #[test]
    fn loads_only_exact_public_trust_and_distinct_absolute_roots() {
        let directory = tempfile::tempdir().unwrap();
        let path = write_config(directory.path(), &valid_document(directory.path()));
        let loaded = load_config(&path).unwrap();
        assert_eq!(loaded.system_sequence, 1);
        assert_eq!(loaded.health_timeout_ms, 60_000);

        let mut invalid = valid_document(directory.path());
        invalid["publicKeyHex"] = serde_json::Value::String("00".repeat(32));
        let path = write_config(directory.path(), &invalid);
        assert!(matches!(
            load_config(&path),
            Err(SupervisorCliError::InvalidConfig)
        ));

        let mut overlapping = valid_document(directory.path());
        overlapping["stateRoot"] = overlapping["slotRoot"].clone();
        let path = write_config(directory.path(), &overlapping);
        assert!(matches!(
            load_config(&path),
            Err(SupervisorCliError::InvalidConfig)
        ));
    }

    #[test]
    fn rejects_writable_or_symlinked_configuration() {
        let directory = tempfile::tempdir().unwrap();
        let path = write_config(directory.path(), &valid_document(directory.path()));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o620)).unwrap();
        assert!(matches!(
            load_config(&path),
            Err(SupervisorCliError::UnsafeConfigFile)
        ));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let linked = directory.path().join("linked.json");
        symlink(path, &linked).unwrap();
        assert!(matches!(
            load_config(&linked),
            Err(SupervisorCliError::UnsafeConfigFile)
        ));
    }

    fn encode_hex(bytes: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut value = String::with_capacity(bytes.len() * 2);
        for byte in bytes {
            value.push(char::from(HEX[usize::from(byte >> 4)]));
            value.push(char::from(HEX[usize::from(byte & 0x0f)]));
        }
        value
    }
}
