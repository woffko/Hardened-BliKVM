use super::*;
use bli_kvm_update_protocol::{
    activation::{
        self, ActivationErrorCode, ActivationRequest, ActivationRequestEnvelope, ActivationResponse,
    },
    decode_frame, encode_frame,
};
use std::{
    io::Read,
    net::Shutdown,
    os::{
        fd::OwnedFd,
        unix::{fs::MetadataExt, net::UnixStream},
    },
    time::Duration,
};

#[test]
fn activation_exit_success_means_new_transition_even_when_receipt_is_lost() {
    let uid = nix::unistd::Uid::effective();
    if uid.is_root() {
        return;
    }
    let user = nix::unistd::User::from_uid(uid).unwrap().unwrap().name;
    for lose_receipt in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let slots = create_private_directory(root.path(), "slots");
        let state = create_private_directory(root.path(), "state");
        let runtime = create_private_directory(root.path(), "runtime");
        let sources = create_private_directory(root.path(), "sources");
        let signing = SigningKey::from_bytes(&[57; 32]);
        let trust = bli_kvm_update_verifier::ApplicationTrustRoot::new(
            KEY_ID,
            signing.verifying_key().to_bytes(),
        )
        .unwrap();
        let first = bli_kvm_update_stager::ApplicationSlotStager::open(&slots)
            .unwrap()
            .stage(write_bundle(&sources, &signing, 1), &trust)
            .unwrap();
        let second = bli_kvm_update_stager::ApplicationSlotStager::open(&slots)
            .unwrap()
            .stage(write_bundle(&sources, &signing, 2), &trust)
            .unwrap();
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
        let binary = env!("CARGO_BIN_EXE_bli-kvm-update-supervisor");
        let initialized = command(binary, &config, &["initialize", &first.slot_id])
            .output()
            .unwrap();
        assert!(initialized.status.success());
        let generation = json_output(&initialized)["status"]["generation"]
            .as_u64()
            .unwrap();
        let journal_path = state.join("application-state.json");
        let before = fs::read(&journal_path).unwrap();
        let exchange = |slot: &str, generation, lost: bool| {
            let (mut stream, worker) = UnixStream::pair().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let fd: OwnedFd = worker.into();
            let child = command(
                binary,
                &config,
                &["activation-socket", "--peer-user", &user],
            )
            .stdin(Stdio::from(fd))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
            let request = ActivationRequestEnvelope {
                schema_version: 1,
                request_id: 19,
                request: ActivationRequest::Activate {
                    slot_id: slot.into(),
                    expected_generation: generation,
                },
            };
            stream
                .write_all(&encode_frame(&activation::encode_request(&request).unwrap()).unwrap())
                .unwrap();
            stream
                .shutdown(if lost {
                    Shutdown::Both
                } else {
                    Shutdown::Write
                })
                .unwrap();
            let mut bytes = Vec::new();
            if !lost {
                stream.read_to_end(&mut bytes).unwrap();
            }
            let exit = child.wait_with_output().unwrap().status;
            let response = if lost {
                None
            } else {
                let value = activation::decode_response(decode_frame(&bytes).unwrap()).unwrap();
                assert_eq!(value.request_id, 19);
                Some(value.response)
            };
            (exit, response)
        };
        for (slot, expected, code) in [
            (
                &second.slot_id,
                generation + 1,
                ActivationErrorCode::StaleGeneration,
            ),
            (
                &first.slot_id,
                generation,
                ActivationErrorCode::AlreadyActive,
            ),
        ] {
            let (exit, response) = exchange(slot, expected, false);
            assert_eq!(exit.code(), Some(97));
            assert_eq!(response, Some(ActivationResponse::Error { code }));
            assert_eq!(fs::read(&journal_path).unwrap(), before);
            assert_eq!(fs::read_dir(&runtime).unwrap().count(), 0);
        }
        let (exit, response) = exchange(&second.slot_id, generation, lose_receipt);
        assert!(
            exit.success(),
            "new accepted activation must permit fixed apply dispatch despite lost reply"
        );
        if !lose_receipt {
            let Some(ActivationResponse::AwaitingHealth { receipt }) = response else {
                panic!("missing activation receipt")
            };
            assert_eq!(receipt.target_slot_id, second.slot_id);
        }
        let after = fs::read(&journal_path).unwrap();
        let status = command(binary, &config, &["status"]).output().unwrap();
        assert!(status.status.success());
        let status = json_output(&status);
        assert_eq!(status["activeSlotId"], second.slot_id);
        assert_eq!(status["confirmedSlotId"], first.slot_id);
        let pending_generation = status["generation"].as_u64().unwrap();
        let token_identity = fs::metadata(runtime.join("health-token")).unwrap().ino();
        for (expected, code) in [
            (generation, ActivationErrorCode::StaleGeneration),
            (pending_generation, ActivationErrorCode::PendingActivation),
        ] {
            let (exit, response) = exchange(&second.slot_id, expected, false);
            assert_eq!(exit.code(), Some(97));
            assert_eq!(response, Some(ActivationResponse::Error { code }));
            assert_eq!(fs::read(&journal_path).unwrap(), after);
            assert_eq!(
                fs::metadata(runtime.join("health-token")).unwrap().ino(),
                token_identity
            );
        }
    }
}

#[test]
fn native_profile_refuses_legacy_activation_and_admission_without_the_helper() {
    let uid = nix::unistd::Uid::effective();
    if uid.is_root() {
        return;
    }
    let user = nix::unistd::User::from_uid(uid).unwrap().unwrap().name;
    let root = tempfile::tempdir().unwrap();
    let slots = create_private_directory(root.path(), "slots");
    let state = create_private_directory(root.path(), "state");
    let runtime = create_private_directory(root.path(), "runtime");
    let sources = create_private_directory(root.path(), "sources");
    let signing = SigningKey::from_bytes(&[59; 32]);
    let trust = bli_kvm_update_verifier::ApplicationTrustRoot::new(
        KEY_ID,
        signing.verifying_key().to_bytes(),
    )
    .unwrap();
    let stager = bli_kvm_update_stager::ApplicationSlotStager::open(&slots).unwrap();
    let first = stager
        .stage(write_bundle(&sources, &signing, 1), &trust)
        .unwrap();
    let second = stager
        .stage(write_bundle(&sources, &signing, 2), &trust)
        .unwrap();
    drop(stager);
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
    let binary = env!("CARGO_BIN_EXE_bli-kvm-update-supervisor");
    let initialized = command(binary, &config, &["initialize", &first.slot_id])
        .output()
        .unwrap();
    assert!(initialized.status.success());
    let generation = json_output(&initialized)["status"]["generation"]
        .as_u64()
        .unwrap();
    let journal_path = state.join("application-state.json");
    let before = fs::read(&journal_path).unwrap();
    for request in [
        ActivationRequest::Activate {
            slot_id: second.slot_id.clone(),
            expected_generation: generation,
        },
        ActivationRequest::Admit {
            slot_id: second.slot_id.clone(),
            expected_generation: generation,
        },
    ] {
        let (mut stream, worker) = UnixStream::pair().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let fd: OwnedFd = worker.into();
        let child = command(
            binary,
            &config,
            &[
                "activation-socket",
                "--peer-user",
                &user,
                "--native-profile",
            ],
        )
        .stdin(Stdio::from(fd))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
        let envelope = ActivationRequestEnvelope {
            schema_version: 1,
            request_id: 23,
            request,
        };
        stream
            .write_all(&encode_frame(&activation::encode_request(&envelope).unwrap()).unwrap())
            .unwrap();
        stream.shutdown(Shutdown::Write).unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).unwrap();
        let exit = child.wait_with_output().unwrap().status;
        assert!(!exit.success());
        let response = activation::decode_response(decode_frame(&bytes).unwrap()).unwrap();
        assert_eq!(response.request_id, 23);
        assert!(matches!(
            response.response,
            ActivationResponse::Error { .. }
        ));
        assert_eq!(fs::read(&journal_path).unwrap(), before);
        assert_eq!(fs::read_dir(&runtime).unwrap().count(), 0);
    }
    let invalid = command(
        binary,
        &config,
        &["activation-socket", "--peer-user", &user, "--native"],
    )
    .output()
    .unwrap();
    assert_eq!(invalid.status.code(), Some(2));
}
