//! Root-owned helper configuration. Every path, trust key and policy value
//! comes from here, never from the coordinator's requests.

use std::{
    fs::{self, OpenOptions},
    io::Read,
    net::{Ipv4Addr, SocketAddr},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

use bli_kvm_update_runtime::{
    board_readiness::BoardRequirements, native_exec_server::ExecutionProbeConfig,
    native_execution::ExecutionPolicy,
};
use bli_kvm_update_stager::StageQuota;
use bli_kvm_update_verifier::ApplicationTrustRoot;
use serde::Deserialize;

use crate::HelperError;

const CONFIG_SCHEMA_VERSION: u32 = 1;
const MAX_CONFIG_BYTES: u64 = 16 * 1024;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BoardDocument {
    fan_line: u16,
    fan_pwm_period_ms: u64,
    display: bool,
    led: bool,
    button: bool,
    atx_power_led: bool,
    atx_hdd_led: bool,
    external_rtc: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConfigDocument {
    schema_version: u32,
    key_id: String,
    public_key_hex: String,
    coordinator_uid: u32,
    updater_slot_root: PathBuf,
    execution_root: PathBuf,
    ledger_root: PathBuf,
    system_sequence: u64,
    minimum_application_sequence: u64,
    max_slots: usize,
    max_payload_bytes: u64,
    owner_socket: PathBuf,
    video_socket: PathBuf,
    board_socket: PathBuf,
    web_port: u16,
    board: BoardDocument,
}

pub struct HelperConfig {
    pub trust: ApplicationTrustRoot,
    pub coordinator_uid: u32,
    pub updater_slot_root: PathBuf,
    pub execution_root: PathBuf,
    pub ledger_root: PathBuf,
    pub policy: ExecutionPolicy,
    pub probes: ExecutionProbeConfig,
}

fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and does not dereference pointers.
    unsafe { libc::geteuid() }
}

/// Load the configuration. `protected` requires root-owned ancestry of the
/// file, as in production; tests use unprivileged fixtures without it.
pub fn load(path: &Path, protected: bool) -> Result<HelperConfig, HelperError> {
    if !path.is_absolute() || path.file_name().is_none() {
        return Err(HelperError::InvalidConfigPath);
    }
    if fs::canonicalize(path).map_err(|_| HelperError::UnsafeConfigFile)? != path {
        return Err(HelperError::UnsafeConfigFile);
    }
    if protected {
        let parent = path.parent().ok_or(HelperError::UnsafeConfigFile)?;
        bli_kvm_update_stager::validate_protected_directory(parent)
            .map_err(|_| HelperError::UnsafeConfigFile)?;
    }
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| HelperError::UnsafeConfigFile)?;
    let metadata = file.metadata().map_err(|_| HelperError::UnsafeConfigFile)?;
    if !metadata.is_file()
        || metadata.uid() != effective_uid()
        || metadata.nlink() != 1
        || metadata.mode() & 0o7022 != 0
        || metadata.len() == 0
        || metadata.len() > MAX_CONFIG_BYTES
    {
        return Err(HelperError::UnsafeConfigFile);
    }
    let mut bytes = Vec::new();
    file.take(MAX_CONFIG_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| HelperError::UnsafeConfigFile)?;
    if bytes.len() as u64 != metadata.len() {
        return Err(HelperError::UnsafeConfigFile);
    }
    let document: ConfigDocument =
        serde_json::from_slice(&bytes).map_err(|_| HelperError::InvalidConfig)?;
    validate(document)
}

fn decode_public_key(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64
        || value.bytes().all(|byte| byte == b'0')
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    let mut output = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(pair).ok()?;
        output[index] = u8::from_str_radix(text, 16).ok()?;
    }
    Some(output)
}

fn validate(document: ConfigDocument) -> Result<HelperConfig, HelperError> {
    let roots = [
        &document.updater_slot_root,
        &document.execution_root,
        &document.ledger_root,
    ];
    let overlapping = roots.iter().enumerate().any(|(index, left)| {
        roots
            .iter()
            .skip(index + 1)
            .any(|right| left.starts_with(right) || right.starts_with(left))
    });
    let paths = roots.into_iter().chain([
        &document.owner_socket,
        &document.video_socket,
        &document.board_socket,
    ]);
    let policy = ExecutionPolicy {
        system_sequence: document.system_sequence,
        minimum_application_sequence: document.minimum_application_sequence,
        quota: StageQuota {
            max_slots: document.max_slots,
            max_payload_bytes: document.max_payload_bytes,
        },
    };
    if document.schema_version != CONFIG_SCHEMA_VERSION
        || document.coordinator_uid == 0
        || overlapping
        || paths
            .into_iter()
            .any(|path| !path.is_absolute() || path.file_name().is_none())
        || document.system_sequence == 0
        || document.minimum_application_sequence == 0
        || !(1..=32).contains(&document.max_slots)
        || document.max_payload_bytes == 0
        || document.web_port < 1024
        || document.board.fan_pwm_period_ms == 0
    {
        return Err(HelperError::InvalidConfig);
    }
    let public_key =
        decode_public_key(&document.public_key_hex).ok_or(HelperError::InvalidConfig)?;
    let trust = ApplicationTrustRoot::new(document.key_id, public_key)
        .map_err(|_| HelperError::InvalidConfig)?;
    let board = document.board;
    Ok(HelperConfig {
        trust,
        coordinator_uid: document.coordinator_uid,
        updater_slot_root: document.updater_slot_root,
        execution_root: document.execution_root,
        ledger_root: document.ledger_root,
        policy,
        probes: ExecutionProbeConfig {
            owner_socket: document.owner_socket,
            video_socket: document.video_socket,
            board_socket: document.board_socket,
            web_endpoint: SocketAddr::from((Ipv4Addr::LOCALHOST, document.web_port)),
            board: BoardRequirements {
                fan_line: board.fan_line,
                fan_pwm_period_ms: board.fan_pwm_period_ms,
                display: board.display,
                led: board.led,
                button: board.button,
                atx_power_led: board.atx_power_led,
                atx_hdd_led: board.atx_hdd_led,
                external_rtc: board.external_rtc,
            },
        },
    })
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{PermissionsExt, symlink};

    use super::*;

    fn document() -> serde_json::Value {
        serde_json::json!({
            "schemaVersion": 1,
            "keyId": "app-test",
            "publicKeyHex": "3b6a27bcceb6a42d62a3a8d02a6f0d73653215771de243a63ac048a18b59da29",
            "coordinatorUid": 998,
            "updaterSlotRoot": "/var/lib/hardened-blikvm-slots-v2",
            "executionRoot": "/var/lib/hardened-blikvm-exec",
            "ledgerRoot": "/var/lib/hardened-blikvm-exec-ledger",
            "systemSequence": 2,
            "minimumApplicationSequence": 14,
            "maxSlots": 3,
            "maxPayloadBytes": 134217728,
            "ownerSocket": "/run/hardened-blikvm-gadget/control.sock",
            "videoSocket": "/run/hardened-blikvm-video/mjpeg.sock",
            "boardSocket": "/run/hardened-blikvm-boardd/control.sock",
            "webPort": 18080,
            "board": {
                "fanLine": 229,
                "fanPwmPeriodMs": 10,
                "display": true,
                "led": true,
                "button": true,
                "atxPowerLed": true,
                "atxHddLed": true,
                "externalRtc": true
            }
        })
    }

    fn write(directory: &Path, value: &serde_json::Value) -> PathBuf {
        let path = fs::canonicalize(directory)
            .unwrap()
            .join("exec-helper.json");
        let _ = fs::remove_file(&path);
        fs::write(&path, serde_json::to_vec(value).unwrap()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        path
    }

    #[test]
    fn valid_configuration_derives_policy_probes_and_loopback_endpoint() {
        let directory = tempfile::tempdir().unwrap();
        let config = load(&write(directory.path(), &document()), false).unwrap();
        assert_eq!(config.coordinator_uid, 998);
        assert_eq!(config.policy.minimum_application_sequence, 14);
        assert_eq!(config.policy.quota.max_slots, 3);
        assert_eq!(
            config.probes.web_endpoint,
            SocketAddr::from((Ipv4Addr::LOCALHOST, 18080))
        );
        assert!(config.probes.board.external_rtc);
    }

    #[test]
    fn invalid_documents_are_refused() {
        let directory = tempfile::tempdir().unwrap();
        let mutations: [(&str, serde_json::Value); 12] = [
            ("schemaVersion", 2.into()),
            ("coordinatorUid", 0.into()),
            (
                "executionRoot",
                "/var/lib/hardened-blikvm-slots-v2/exec".into(),
            ),
            ("ledgerRoot", "relative".into()),
            ("ownerSocket", "/".into()),
            ("systemSequence", 0.into()),
            ("minimumApplicationSequence", 0.into()),
            ("maxSlots", 33.into()),
            ("maxPayloadBytes", 0.into()),
            ("webPort", 80.into()),
            ("publicKeyHex", "0".repeat(64).into()),
            ("unexpected", true.into()),
        ];
        for (field, value) in mutations {
            let mut invalid = document();
            invalid[field] = value;
            let path = write(directory.path(), &invalid);
            assert!(
                matches!(load(&path, false), Err(HelperError::InvalidConfig)),
                "{field}"
            );
        }
        let mut invalid = document();
        invalid["board"]["fanPwmPeriodMs"] = 0.into();
        assert!(matches!(
            load(&write(directory.path(), &invalid), false),
            Err(HelperError::InvalidConfig)
        ));
    }

    #[test]
    fn unsafe_files_and_paths_are_refused() {
        let directory = tempfile::tempdir().unwrap();
        let path = write(directory.path(), &document());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o620)).unwrap();
        assert!(matches!(
            load(&path, false),
            Err(HelperError::UnsafeConfigFile)
        ));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

        let link = path.with_file_name("linked.json");
        symlink(&path, &link).unwrap();
        assert!(matches!(
            load(&link, false),
            Err(HelperError::UnsafeConfigFile)
        ));
        let hard = path.with_file_name("hard.json");
        fs::hard_link(&path, &hard).unwrap();
        assert!(matches!(
            load(&path, false),
            Err(HelperError::UnsafeConfigFile)
        ));
        fs::remove_file(&hard).unwrap();

        assert!(matches!(
            load(Path::new("relative.json"), false),
            Err(HelperError::InvalidConfigPath)
        ));
        // A temporary directory is not a root-owned protected ancestry.
        assert!(matches!(
            load(&path, true),
            Err(HelperError::UnsafeConfigFile)
        ));
        fs::write(&path, vec![b' '; MAX_CONFIG_BYTES as usize + 1]).unwrap();
        assert!(matches!(
            load(&path, false),
            Err(HelperError::UnsafeConfigFile)
        ));
    }
}
