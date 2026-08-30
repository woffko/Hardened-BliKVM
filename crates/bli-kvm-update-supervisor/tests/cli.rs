use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, Permissions},
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use bli_kvm_update_verifier::{
    APPLICATION_ARCHITECTURE, APPLICATION_FORMAT_V1, APPLICATION_MANIFEST_PATH,
    APPLICATION_PRODUCT, APPLICATION_SIGNATURE_PATH, APPLICATION_TARGET, ApplicationFile,
    ApplicationManifest,
};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::json;
use sha2::{Digest, Sha256};
use tar::{Builder, EntryType, Header};

const KEY_ID: &str = "app-supervisor-cli-test-key-1";

#[test]
fn full_cli_flow_keeps_health_token_out_of_arguments_environment_and_receipts() {
    let root = tempfile::tempdir().unwrap();
    let slots = create_private_directory(root.path(), "slots");
    let state = create_private_directory(root.path(), "state");
    let runtime = create_private_directory(root.path(), "runtime");
    let sources = create_private_directory(root.path(), "sources");
    let signing = SigningKey::from_bytes(&[57; 32]);
    let source1 = write_bundle(&sources, &signing, 1);
    let source2 = write_bundle(&sources, &signing, 2);
    let source3 = write_bundle(&sources, &signing, 3);
    let config = root.path().join("supervisor.json");
    fs::write(
        &config,
        serde_json::to_vec(&json!({
            "schemaVersion": 1,
            "product": APPLICATION_PRODUCT,
            "target": APPLICATION_TARGET,
            "keyId": KEY_ID,
            "publicKeyHex": hex(&signing.verifying_key().to_bytes()),
            "slotRoot": slots,
            "stateRoot": state,
            "runtimeRoot": runtime,
            "systemSequence": 1,
            "healthTimeoutMs": 60_000,
        }))
        .unwrap(),
    )
    .unwrap();
    fs::set_permissions(&config, Permissions::from_mode(0o600)).unwrap();

    let binary = env!("CARGO_BIN_EXE_bli-kvm-update-supervisor");
    let staged1 = command(binary, &config, &["stage", source1.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(staged1.status.success(), "{:?}", staged1.stderr);
    let staged1 = json_output(&staged1);
    assert_eq!(staged1["result"], "published");
    assert_eq!(staged1["abandonedStagingRemoved"], 0);
    let slot1 = staged1["slotId"].as_str().unwrap().to_string();
    let staged2 = command(binary, &config, &["stage", source2.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(staged2.status.success(), "{:?}", staged2.stderr);
    let staged2 = json_output(&staged2);
    assert_eq!(staged2["result"], "published");
    let slot2 = staged2["slotId"].as_str().unwrap().to_string();
    let staged3 = command(binary, &config, &["stage", source3.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(staged3.status.success(), "{:?}", staged3.stderr);
    let staged3 = json_output(&staged3);
    let slot3 = staged3["slotId"].as_str().unwrap().to_string();

    let initialized = command(binary, &config, &["initialize", &slot1])
        .output()
        .unwrap();
    assert!(initialized.status.success(), "{:?}", initialized.stderr);
    assert_eq!(json_output(&initialized)["result"], "initialized");

    let activated = command(binary, &config, &["activate", &slot2])
        .output()
        .unwrap();
    assert!(activated.status.success(), "{:?}", activated.stderr);
    let activation_receipt = json_output(&activated);
    assert_eq!(activation_receipt["result"], "awaiting_health");
    assert_eq!(activation_receipt["credentialReady"], true);
    let token_path = root.path().join("runtime/health-token");
    let token = fs::read(&token_path).unwrap();
    assert_eq!(token.len(), 32);

    let mut confirm = command(binary, &config, &["confirm-stdin"]);
    confirm.stdin(Stdio::piped());
    confirm.stdout(Stdio::piped());
    confirm.stderr(Stdio::piped());
    assert!(
        confirm
            .get_args()
            .all(|argument| argument.as_encoded_bytes() != token)
    );
    assert!(confirm.get_envs().all(|(_, value)| value.is_none()));
    let mut child = confirm.spawn().unwrap();
    child.stdin.take().unwrap().write_all(&token).unwrap();
    let confirmed = child.wait_with_output().unwrap();
    assert!(confirmed.status.success(), "{:?}", confirmed.stderr);
    assert_eq!(json_output(&confirmed)["result"], "confirmed");
    assert!(!String::from_utf8_lossy(&confirmed.stdout).contains(&hex(&token)));
    assert!(!token_path.exists());

    let status = command(binary, &config, &["status"]).output().unwrap();
    assert!(status.status.success(), "{:?}", status.stderr);
    let status = json_output(&status);
    assert_eq!(status["activeSlotId"], slot2);
    assert_eq!(status["floorSequence"], 2);
    let collected = command(binary, &config, &["gc", "0"]).output().unwrap();
    assert!(collected.status.success(), "{:?}", collected.stderr);
    let collected = json_output(&collected);
    assert_eq!(collected["result"], "completed");
    assert_eq!(collected["deletedSlotIds"], json!([slot3]));
    assert!(!slots.join(slot3).exists());
}

fn command(binary: &str, config: &Path, arguments: &[&str]) -> Command {
    let mut command = Command::new(binary);
    command.env_clear();
    command.arg("--config").arg(config).args(arguments);
    command
}

fn json_output(output: &std::process::Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap()
}

fn create_private_directory(root: &Path, name: &str) -> PathBuf {
    let path = root.join(name);
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, Permissions::from_mode(0o700)).unwrap();
    path
}

fn write_bundle(sources: &Path, signing: &SigningKey, sequence: u64) -> PathBuf {
    let payload = payload(sequence);
    let manifest = manifest(sequence, &payload);
    let archive = bundle(signing, &manifest, &payload);
    let source = sources.join(format!("application-{sequence}.tar"));
    fs::write(&source, archive).unwrap();
    fs::set_permissions(&source, Permissions::from_mode(0o600)).unwrap();
    source
}

fn payload(sequence: u64) -> BTreeMap<&'static str, (u32, Vec<u8>)> {
    BTreeMap::from([
        (
            "bin/bli-kvm-server",
            (0o755, format!("server-{sequence}").into_bytes()),
        ),
        (
            "libexec/bli-kvm-control-broker",
            (0o755, format!("control-{sequence}").into_bytes()),
        ),
        (
            "libexec/bli-kvm-platform-broker",
            (0o755, format!("broker-{sequence}").into_bytes()),
        ),
        (
            "libexec/bli-kvm-video-proxy",
            (0o755, format!("video-{sequence}").into_bytes()),
        ),
        (
            "share/web/index.html",
            (0o644, format!("<html>{sequence}</html>").into_bytes()),
        ),
    ])
}

fn manifest(sequence: u64, payload: &BTreeMap<&str, (u32, Vec<u8>)>) -> ApplicationManifest {
    ApplicationManifest {
        format: APPLICATION_FORMAT_V1.into(),
        product: APPLICATION_PRODUCT.into(),
        target: APPLICATION_TARGET.into(),
        architecture: APPLICATION_ARCHITECTURE.into(),
        key_id: KEY_ID.into(),
        version: format!("0.{sequence}.0"),
        sequence,
        minimum_system_sequence: 1,
        source_commit: format!("{sequence:040x}"),
        files: payload
            .iter()
            .map(|(path, (mode, bytes))| ApplicationFile {
                path: (*path).to_string(),
                mode: *mode,
                size: bytes.len() as u64,
                sha256: hex(&Sha256::digest(bytes)),
            })
            .collect(),
    }
}

fn bundle(
    signing_key: &SigningKey,
    manifest: &ApplicationManifest,
    payload: &BTreeMap<&str, (u32, Vec<u8>)>,
) -> Vec<u8> {
    let manifest_bytes = serde_json::to_vec(manifest).unwrap();
    let signature = signing_key.sign(&manifest_bytes).to_bytes();
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
    builder.into_inner().unwrap()
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

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(char::from(HEX[usize::from(byte >> 4)]));
        value.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    value
}
