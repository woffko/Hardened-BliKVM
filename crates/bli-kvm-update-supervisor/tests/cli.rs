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

#[path = "support/activation_socket.rs"]
mod activation_socket;
#[path = "support/http_upload.rs"]
mod http_upload;

#[test]
fn upload_worker_stages_retries_and_rejects_without_activation() {
    use bli_kvm_update_protocol::{
        decode_frame, encode_frame,
        upload::{
            UploadDisposition, UploadRequest, UploadRequestEnvelope, UploadResponse,
            decode_response, encode_request,
        },
    };
    use std::{
        io::Read,
        net::Shutdown,
        os::{fd::OwnedFd, unix::net::UnixStream},
        time::Duration,
    };
    let uid = nix::unistd::Uid::effective();
    // Production expressly rejects root peers; run the positive socket case
    // only with an actual unprivileged test identity.
    if uid.is_root() {
        return;
    }
    let user = nix::unistd::User::from_uid(uid).unwrap().unwrap().name;
    let root = tempfile::tempdir().unwrap();
    let slots = create_private_directory(root.path(), "slots");
    let state = create_private_directory(root.path(), "state");
    let runtime = create_private_directory(root.path(), "runtime");
    let spool = create_private_directory(root.path(), "spool");
    let sources = create_private_directory(root.path(), "sources");
    let signing = SigningKey::from_bytes(&[57; 32]);
    let source = write_bundle(&sources, &signing, 1);
    let config = root.path().join("supervisor.json");
    fs::write(
        &config,
        serde_json::to_vec(&json!({
            "schemaVersion": 1, "product": APPLICATION_PRODUCT, "target": APPLICATION_TARGET,
            "keyId": KEY_ID, "publicKeyHex": hex(&signing.verifying_key().to_bytes()),
            "slotRoot": slots, "stateRoot": state, "runtimeRoot": runtime,
            "systemSequence": 1, "healthTimeoutMs": 60_000,
        }))
        .unwrap(),
    )
    .unwrap();
    fs::set_permissions(&config, Permissions::from_mode(0o600)).unwrap();
    let connect = || {
        let (client, worker) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let fd: OwnedFd = worker.into();
        let child = command(
            env!("CARGO_BIN_EXE_bli-kvm-update-supervisor"),
            &config,
            &[
                "upload-socket",
                "--peer-user",
                &user,
                "--spool-root",
                spool.to_str().unwrap(),
            ],
        )
        .stdin(Stdio::from(fd))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
        (client, child)
    };
    let metadata = |size: usize| {
        encode_frame(
            &encode_request(&UploadRequestEnvelope {
                schema_version: 1,
                request_id: 71,
                request: UploadRequest::Stage {
                    archive_bytes: size as u64,
                },
            })
            .unwrap(),
        )
        .unwrap()
    };
    let upload = |body: &[u8]| {
        let (mut client, mut child) = connect();
        client
            .write_all(
                &encode_frame(
                    &encode_request(&UploadRequestEnvelope {
                        schema_version: 1,
                        request_id: 71,
                        request: UploadRequest::Stage {
                            archive_bytes: body.len() as u64,
                        },
                    })
                    .unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
        client.write_all(body).unwrap();
        client.shutdown(Shutdown::Write).unwrap();
        let mut bytes = Vec::new();
        client.read_to_end(&mut bytes).unwrap();
        assert!(child.wait().unwrap().success());
        let response = decode_response(decode_frame(&bytes).unwrap()).unwrap();
        assert_eq!(response.request_id, 71);
        assert_eq!(fs::read_dir(&spool).unwrap().count(), 0);
        assert_eq!(fs::read_dir(&state).unwrap().count(), 0);
        assert_eq!(fs::read_dir(&runtime).unwrap().count(), 0);
        assert!(!slots.join("current").exists());
        response.response
    };
    let body = fs::read(&source).unwrap();
    let UploadResponse::Staged { receipt: first } = upload(&body) else {
        panic!("must publish")
    };
    assert_eq!(first.disposition, UploadDisposition::Published);
    let UploadResponse::Staged { receipt: second } = upload(&body) else {
        panic!("must retry")
    };
    assert_eq!(second.disposition, UploadDisposition::AlreadyPresent);
    assert_eq!(first.slot, second.slot);
    assert!(matches!(upload(&[0; 1024]), UploadResponse::Error { .. }));

    // The process owns admission while waiting for a partial body, but the
    // slot lock remains available to watchdog/status. Cancellation publishes
    // nothing and drops the anonymous spool and admission guard.
    let body2 = fs::read(write_bundle(&sources, &signing, 2)).unwrap();
    let list_slots = || {
        fs::read_dir(&slots)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<BTreeSet<_>>()
    };
    let before = list_slots();
    let (mut client, mut child) = connect();
    client.write_all(&metadata(body2.len())).unwrap();
    client.write_all(&body2[..body2.len() / 2]).unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        use std::os::unix::fs::MetadataExt;
        // Observe the child's anonymous spool without competing for its lock.
        let has_spool = fs::read_dir(format!("/proc/{}/fd", child.id()))
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|entry| fs::metadata(entry.path()).ok())
            .any(|metadata| metadata.is_file() && metadata.nlink() == 0);
        if has_spool {
            break;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("worker did not acquire admission");
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    let stager = bli_kvm_update_stager::ApplicationSlotStager::open(&slots).unwrap();
    assert_eq!(list_slots(), before);
    drop(stager);
    drop(client);
    assert!(!child.wait().unwrap().success());
    assert_eq!(list_slots(), before);
    assert_eq!(fs::read_dir(&spool).unwrap().count(), 0);
    assert!(bli_kvm_update_supervisor::upload_storage::UploadStorage::open(&spool).is_ok());

    // Complete upload plus write EOF permits publication even if the client
    // cannot receive its receipt. A retry must recover the published identity.
    let (mut client, mut child) = connect();
    client.write_all(&metadata(body2.len())).unwrap();
    client.write_all(&body2).unwrap();
    client.shutdown(Shutdown::Read).unwrap();
    client.shutdown(Shutdown::Write).unwrap();
    assert!(!child.wait().unwrap().success());
    drop(client);
    let UploadResponse::Staged { receipt } = upload(&body2) else {
        panic!("must recover receipt")
    };
    assert_eq!(receipt.disposition, UploadDisposition::AlreadyPresent);
}

#[test]
fn validate_config_does_not_open_or_create_state_roots() {
    let root = tempfile::tempdir().unwrap();
    let config = root.path().join("config.json");
    fs::write(
        &config,
        serde_json::to_vec(&json!({
            "schemaVersion": 1, "product": APPLICATION_PRODUCT, "target": APPLICATION_TARGET,
            "keyId": KEY_ID,
            "publicKeyHex": "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
            "slotRoot": root.path().join("absent-slots"),
            "stateRoot": root.path().join("absent-state"),
            "runtimeRoot": root.path().join("absent-runtime"),
            "systemSequence": 1, "healthTimeoutMs": 60_000,
        }))
        .unwrap(),
    )
    .unwrap();
    fs::set_permissions(&config, Permissions::from_mode(0o600)).unwrap();
    let binary = env!("CARGO_BIN_EXE_bli-kvm-update-supervisor");
    let result = command(binary, &config, &["validate-config"])
        .output()
        .unwrap();
    assert!(result.status.success());
    assert_eq!(
        json_output(&result),
        json!({"command": "validate_config", "result": "valid"})
    );
    assert_eq!(fs::read_dir(root.path()).unwrap().count(), 1);
    let extra = command(binary, &config, &["validate-config", "extra"])
        .output()
        .unwrap();
    assert_eq!(extra.status.code(), Some(2));
}

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

    // Inspection must work without opening or initializing the credential root.
    fs::remove_dir(&runtime).unwrap();
    let journal_path = state.join("application-state.json");
    let initial_journal = fs::read(&journal_path).unwrap();
    let inspected = command(binary, &config, &["inspect-recovery"])
        .output()
        .unwrap();
    assert!(inspected.status.success(), "{:?}", inspected.stderr);
    assert_eq!(json_output(&inspected)["selection"], "confirmed");
    assert_eq!(json_output(&inspected)["schemaVersion"], 1);
    assert_eq!(json_output(&inspected)["confirmedSlotId"], slot1);
    assert!(!runtime.exists());
    assert_eq!(fs::read(&journal_path).unwrap(), initial_journal);
    fs::create_dir(&runtime).unwrap();
    fs::set_permissions(&runtime, Permissions::from_mode(0o700)).unwrap();

    // A different process holding only the runtime lock must fence activation,
    // even though this worker can acquire its manager's slot/state locks.
    let runtime_owner = bli_kvm_update_runtime::HealthChallengeStore::open(&runtime).unwrap();
    let selected_before = fs::read_link(slots.join("current")).unwrap();
    let busy = command(binary, &config, &["activate", &slot2])
        .output()
        .unwrap();
    assert_eq!(busy.status.code(), Some(66));
    assert!(busy.stdout.is_empty());
    assert_eq!(fs::read(&journal_path).unwrap(), initial_journal);
    assert_eq!(
        fs::read_link(slots.join("current")).unwrap(),
        selected_before
    );
    assert_eq!(fs::read_dir(&runtime).unwrap().count(), 0);
    let inspected = command(binary, &config, &["inspect-recovery"])
        .output()
        .unwrap();
    assert!(inspected.status.success(), "{:?}", inspected.stderr);
    drop(runtime_owner);

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

    // A damaged candidate must not prevent inspection of the confirmed anchor.
    let candidate_binary = slots.join(&slot2).join("bin/bli-kvm-server");
    let candidate_bytes = fs::read(&candidate_binary).unwrap();
    let candidate_mode = fs::metadata(&candidate_binary).unwrap().permissions();
    fs::set_permissions(&candidate_binary, Permissions::from_mode(0o700)).unwrap();
    fs::write(&candidate_binary, b"damaged").unwrap();
    fs::set_permissions(&candidate_binary, candidate_mode.clone()).unwrap();
    let pending_journal = fs::read(&journal_path).unwrap();
    let challenge_path = runtime.join("health-challenge.json");
    let challenge = fs::read(&challenge_path).unwrap();
    let selected = fs::read_link(slots.join("current")).unwrap();
    let inspected = command(binary, &config, &["inspect-recovery"])
        .output()
        .unwrap();
    assert!(inspected.status.success(), "{:?}", inspected.stderr);
    let receipt = json_output(&inspected);
    assert_eq!(receipt["selection"], "pending_target");
    assert_eq!(receipt["pending"]["targetSlotId"], slot2);
    assert_eq!(receipt["confirmedSlotId"], slot1);
    assert!(
        !command(binary, &config, &["status"])
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(fs::read(&journal_path).unwrap(), pending_journal);
    assert_eq!(fs::read(&token_path).unwrap(), token);
    assert_eq!(fs::read(&challenge_path).unwrap(), challenge);
    assert_eq!(fs::read_link(slots.join("current")).unwrap(), selected);
    fs::set_permissions(&candidate_binary, Permissions::from_mode(0o700)).unwrap();
    fs::write(&candidate_binary, candidate_bytes).unwrap();
    fs::set_permissions(&candidate_binary, candidate_mode).unwrap();

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

    let apply_path = runtime.join("confirmed-apply-request.json");
    let apply_request = fs::read(&apply_path).unwrap();
    let mut acknowledge = command(binary, &config, &["ack-confirmed-apply"]);
    acknowledge
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = acknowledge.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&apply_request)
        .unwrap();
    let acknowledged = child.wait_with_output().unwrap();
    assert!(acknowledged.status.success(), "{:?}", acknowledged.stderr);
    assert_eq!(json_output(&acknowledged)["acknowledged"], true);
    assert!(!apply_path.exists());
    for _ in 0..3 {
        assert!(
            command(binary, &config, &["watchdog-once"])
                .output()
                .unwrap()
                .status
                .success()
        );
        assert!(!apply_path.exists());
    }

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

#[test]
fn native_migration_is_explicit_and_boot_without_the_helper_changes_nothing() {
    let root = tempfile::tempdir().unwrap();
    let slots = create_private_directory(root.path(), "slots");
    let state = create_private_directory(root.path(), "state");
    let runtime = create_private_directory(root.path(), "runtime");
    let sources = create_private_directory(root.path(), "sources");
    let signing = SigningKey::from_bytes(&[58; 32]);
    let mut native = payload(1);
    native.insert("libexec/bli-kvm-boardd", (0o755, b"boardd-1".to_vec()));
    let source = sources.join("native-1.tar");
    fs::write(&source, bundle(&signing, &manifest(1, &native), &native)).unwrap();
    fs::set_permissions(&source, Permissions::from_mode(0o600)).unwrap();
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
    let run = |arguments: &[&str]| command(binary, &config, arguments).output().unwrap();

    let staged = run(&["stage", source.to_str().unwrap()]);
    assert!(staged.status.success(), "{:?}", staged.stderr);
    let slot = json_output(&staged)["slotId"].as_str().unwrap().to_string();
    assert!(run(&["initialize", &slot]).status.success());
    let journal_path = state.join("application-state.json");
    let schema = || json_output(&run(&["inspect-recovery"]))["schemaVersion"].clone();
    assert_eq!(schema(), 1);

    // Migration is a separate explicit command and starts no service.
    let migrated = run(&["migrate-native-idle"]);
    assert!(migrated.status.success(), "{:?}", migrated.stderr);
    assert_eq!(json_output(&migrated)["result"], "native_idle");
    assert_eq!(schema(), 3);
    let migrated_journal = fs::read(&journal_path).unwrap();
    let replay = run(&["migrate-native-idle"]);
    assert!(replay.status.success(), "{:?}", replay.stderr);
    assert_eq!(fs::read(&journal_path).unwrap(), migrated_journal);

    // Without the root helper socket, native boot fails before any mutation.
    let selected = fs::read_link(slots.join("current")).unwrap();
    let booted = run(&["boot-native"]);
    assert!(!booted.status.success());
    assert_eq!(booted.status.code(), Some(50));
    assert!(booted.stdout.is_empty());
    assert_eq!(fs::read(&journal_path).unwrap(), migrated_journal);
    assert_eq!(fs::read_link(slots.join("current")).unwrap(), selected);
    assert_eq!(fs::read_dir(&runtime).unwrap().count(), 0);
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
