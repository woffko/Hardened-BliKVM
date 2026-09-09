mod config;
mod status_socket;
mod upload_socket;

use std::{
    env,
    ffi::OsString,
    io::{self, Write},
    path::PathBuf,
    process::ExitCode,
};

use bli_kvm_update_manager::{
    ActivationOutcome, ApplicationUpdateManager, ConfirmationOutcome, ManagerStatus,
    PendingPhaseStatus, RecoveryOutcome, RollbackReason, UpdateManagerError,
};
use bli_kvm_update_runtime::{
    ChallengeReconciliation, HealthChallengeStore, SupervisorRecovery, UpdateRuntimeError,
    UpdateSupervisor,
};
use bli_kvm_update_stager::{ApplicationSlotStager, ApplicationStageError, StageDisposition};
use serde_json::{Value, json};
use thiserror::Error;

use config::load_config;

#[derive(Debug, Error)]
enum SupervisorCliError {
    #[error("update status socket failed")]
    StatusSocket(#[from] status_socket::StatusSocketError),
    #[error("command arguments are invalid")]
    InvalidArguments,
    #[error("configuration path is invalid")]
    InvalidConfigPath,
    #[error("configuration file is unsafe")]
    UnsafeConfigFile,
    #[error("configuration document is invalid")]
    InvalidConfig,
    #[error("update manager rejected the command")]
    Manager(#[from] UpdateManagerError),
    #[error("update runtime rejected the command")]
    Runtime(#[from] UpdateRuntimeError),
    #[error("signed application staging rejected the command")]
    Stager(#[from] ApplicationStageError),
    #[error("supervisor I/O failed")]
    Io(#[from] io::Error),
    #[error("supervisor receipt serialization failed")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CliCommand {
    AckConfirmedApply,
    AckConfirmedApplyCredential,
    ValidateConfig,
    Status,
    StatusSocket(String),
    UploadSocket(String, PathBuf),
    Stage(PathBuf),
    GarbageCollect(usize),
    Initialize(String),
    Activate(String),
    WatchdogOnce,
    ConfirmStdin,
}

fn main() -> ExitCode {
    match run(env::args_os()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Hardened BliKVM update supervisor failed: {error}");
            ExitCode::from(error.exit_code())
        }
    }
}

impl SupervisorCliError {
    fn exit_code(&self) -> u8 {
        match self {
            Self::StatusSocket(_) => 96,
            Self::InvalidArguments => 2,
            Self::InvalidConfigPath => 10,
            Self::UnsafeConfigFile => 11,
            Self::InvalidConfig => 12,
            Self::Manager(error) => manager_runtime_exit_code(error),
            Self::Runtime(UpdateRuntimeError::Manager(error)) => manager_runtime_exit_code(error),
            Self::Runtime(UpdateRuntimeError::InvalidHealthResponseLength) => 32,
            Self::Runtime(UpdateRuntimeError::ChallengeMissing) => 33,
            Self::Runtime(UpdateRuntimeError::OperationMismatch) => 34,
            Self::Runtime(UpdateRuntimeError::BootMismatch) => 35,
            Self::Runtime(UpdateRuntimeError::ChallengeExpired) => 36,
            Self::Runtime(UpdateRuntimeError::InvalidHealthResponse) => 37,
            Self::Runtime(_) => 30,
            Self::Stager(_) => 40,
            Self::Io(_) => 50,
            Self::Json(_) => 51,
        }
    }
}

fn manager_runtime_exit_code(error: &UpdateManagerError) -> u8 {
    match error {
        UpdateManagerError::InvalidSlotRoot => 60,
        UpdateManagerError::UnsafeSlotRoot => 61,
        UpdateManagerError::InvalidStateRoot => 62,
        UpdateManagerError::UnsafeStateRoot => 63,
        UpdateManagerError::OverlappingRoots => 64,
        UpdateManagerError::DifferentFilesystems => 65,
        UpdateManagerError::ManagerBusy => 66,
        UpdateManagerError::UnsafeJournal => 67,
        UpdateManagerError::InvalidJournal => 68,
        UpdateManagerError::JournalTooLarge => 69,
        UpdateManagerError::ConcurrentStateChange => 70,
        UpdateManagerError::NotInitialized => 71,
        UpdateManagerError::AlreadyInitialized => 72,
        UpdateManagerError::UnsafePointer => 73,
        UpdateManagerError::InvalidSlotId => 74,
        UpdateManagerError::GenerationExhausted => 75,
        UpdateManagerError::HistoryFull => 76,
        UpdateManagerError::InvalidClock => 77,
        UpdateManagerError::InvalidOperation => 78,
        UpdateManagerError::OperationConflict => 79,
        UpdateManagerError::PendingActivation => 80,
        UpdateManagerError::RecoveryRequired => 81,
        UpdateManagerError::MinimumSystemSequence => 82,
        UpdateManagerError::AntiRollback => 83,
        UpdateManagerError::ConfirmationMismatch => 84,
        UpdateManagerError::SlotProofMismatch => 85,
        UpdateManagerError::InvalidRetention => 86,
        UpdateManagerError::Stager(ApplicationStageError::StagerBusy) => 87,
        UpdateManagerError::Stager(_) => 88,
        UpdateManagerError::Io(_) => 89,
    }
}

fn run(arguments: impl IntoIterator<Item = OsString>) -> Result<(), SupervisorCliError> {
    let (config_path, command) = parse_arguments(arguments)?;
    if let CliCommand::StatusSocket(peer_user) = &command {
        status_socket::serve_stdin(&config_path, peer_user)?;
        return Ok(());
    }
    if let CliCommand::UploadSocket(peer_user, spool) = &command {
        upload_socket::serve_stdin(&config_path, peer_user, spool)?;
        return Ok(());
    }
    let config = load_config(&config_path)?;
    let receipt = match command {
        CliCommand::ValidateConfig => json!({"command": "validate_config", "result": "valid"}),
        CliCommand::Stage(source) => {
            let stager = ApplicationSlotStager::open(config.slot_root)?;
            let abandoned_staging_removed = stager.cleanup_abandoned_staging()?.removed;
            let staged = stager.stage(source, &config.trust_root)?;
            json!({
                "command": "stage",
                "result": match staged.disposition {
                    StageDisposition::Published => "published",
                    StageDisposition::AlreadyPresent => "already_present",
                },
                "slotId": staged.slot_id,
                "version": staged.manifest.version,
                "sequence": staged.manifest.sequence,
                "manifestSha256": staged.manifest_sha256,
                "payloadBytes": staged.payload_bytes,
                "abandonedStagingRemoved": abandoned_staging_removed,
            })
        }
        command => {
            let manager = ApplicationUpdateManager::open(
                config.slot_root,
                config.state_root,
                config.trust_root,
                config.system_sequence,
                config.health_timeout_ms,
            )?;
            match command {
                CliCommand::GarbageCollect(retain) => {
                    let collected = manager.garbage_collect(retain)?;
                    json!({
                        "command": "gc",
                        "result": "completed",
                        "deletedSlotIds": collected.deleted_slot_ids,
                        "retainedUnreferenced": collected.retained_unreferenced,
                        "abandonedStagingRemoved": collected.recovery.abandoned_staging_removed,
                        "slotDeletionsCompleted": collected.recovery.slot_deletions_completed,
                    })
                }
                command => {
                    let challenges = HealthChallengeStore::open(config.runtime_root)?;
                    let supervisor = UpdateSupervisor::new(manager, challenges);
                    match command {
                        CliCommand::Status => status_receipt(&supervisor.status()?),
                        CliCommand::Initialize(slot_id) => {
                            let status = supervisor.initialize(&slot_id)?;
                            json!({
                                "command": "initialize",
                                "result": "initialized",
                                "status": status_receipt(&status),
                            })
                        }
                        CliCommand::Activate(slot_id) => {
                            let activation = supervisor.activate(&slot_id)?;
                            activation_receipt(
                                activation.outcome,
                                activation.credential_path.is_some(),
                            )
                        }
                        CliCommand::WatchdogOnce => watchdog_receipt(supervisor.watchdog_once()?),
                        CliCommand::ConfirmStdin => {
                            let stdin = io::stdin();
                            let mut locked = stdin.lock();
                            confirmation_receipt(supervisor.confirm_from_reader(&mut locked)?)
                        }
                        CliCommand::AckConfirmedApply => {
                            let stdin = io::stdin();
                            let mut locked = stdin.lock();
                            json!({"command": "ack_confirmed_apply", "acknowledged": supervisor.acknowledge_confirmed_apply(&mut locked)?})
                        }
                        CliCommand::AckConfirmedApplyCredential => {
                            let request = config::load_apply_credential(&config_path)?;
                            json!({"command": "ack_confirmed_apply", "acknowledged": supervisor.acknowledge_confirmed_apply(&mut io::Cursor::new(request))?})
                        }
                        CliCommand::Stage(_)
                        | CliCommand::StatusSocket(_)
                        | CliCommand::UploadSocket(_, _)
                        | CliCommand::GarbageCollect(_)
                        | CliCommand::ValidateConfig => {
                            unreachable!("command handled before supervisor creation")
                        }
                    }
                }
            }
        }
    };
    let stdout = io::stdout();
    let mut locked = stdout.lock();
    serde_json::to_writer(&mut locked, &receipt)?;
    locked.write_all(b"\n")?;
    locked.flush()?;
    Ok(())
}

fn parse_arguments(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<(PathBuf, CliCommand), SupervisorCliError> {
    let mut arguments = arguments.into_iter();
    let _program = arguments
        .next()
        .ok_or(SupervisorCliError::InvalidArguments)?;
    if arguments.next().as_deref() != Some(std::ffi::OsStr::new("--config")) {
        return Err(SupervisorCliError::InvalidArguments);
    }
    let config = PathBuf::from(
        arguments
            .next()
            .ok_or(SupervisorCliError::InvalidArguments)?,
    );
    let command = arguments
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or(SupervisorCliError::InvalidArguments)?;
    let command = match command.as_str() {
        "ack-confirmed-apply" => CliCommand::AckConfirmedApply,
        "ack-confirmed-apply-credential" => CliCommand::AckConfirmedApplyCredential,
        "validate-config" => CliCommand::ValidateConfig,
        "status" => CliCommand::Status,
        "status-socket" | "upload-socket" => {
            if arguments.next().as_deref() != Some(std::ffi::OsStr::new("--peer-user")) {
                return Err(SupervisorCliError::InvalidArguments);
            }
            let user = arguments
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or(SupervisorCliError::InvalidArguments)?;
            if !status_socket::valid_peer_name(&user) {
                return Err(SupervisorCliError::InvalidArguments);
            }
            if command == "status-socket" {
                CliCommand::StatusSocket(user)
            } else {
                if arguments.next().as_deref() != Some(std::ffi::OsStr::new("--spool-root")) {
                    return Err(SupervisorCliError::InvalidArguments);
                }
                let spool = PathBuf::from(
                    arguments
                        .next()
                        .ok_or(SupervisorCliError::InvalidArguments)?,
                );
                if !spool.is_absolute() || spool.file_name().is_none() {
                    return Err(SupervisorCliError::InvalidArguments);
                }
                CliCommand::UploadSocket(user, spool)
            }
        }
        "watchdog-once" => CliCommand::WatchdogOnce,
        "confirm-stdin" => CliCommand::ConfirmStdin,
        "gc" => {
            let retain = arguments
                .next()
                .and_then(|value| value.into_string().ok())
                .and_then(|value| value.parse::<usize>().ok())
                .ok_or(SupervisorCliError::InvalidArguments)?;
            CliCommand::GarbageCollect(retain)
        }
        "stage" => CliCommand::Stage(PathBuf::from(
            arguments
                .next()
                .ok_or(SupervisorCliError::InvalidArguments)?,
        )),
        "initialize" | "activate" => {
            let slot_id = arguments
                .next()
                .and_then(|value| value.into_string().ok())
                .ok_or(SupervisorCliError::InvalidArguments)?;
            if command == "initialize" {
                CliCommand::Initialize(slot_id)
            } else {
                CliCommand::Activate(slot_id)
            }
        }
        _ => return Err(SupervisorCliError::InvalidArguments),
    };
    if arguments.next().is_some() {
        return Err(SupervisorCliError::InvalidArguments);
    }
    Ok((config, command))
}

fn status_receipt(status: &ManagerStatus) -> Value {
    json!({
        "command": "status",
        "generation": status.generation,
        "floorSequence": status.floor_sequence,
        "activeSlotId": status.active_slot_id,
        "confirmedSlotId": status.confirmed_slot_id,
        "previousSlotId": status.previous_slot_id,
        "pending": status.pending.as_ref().map(|pending| json!({
            "operationId": pending.operation_id,
            "targetSlotId": pending.target_slot_id,
            "previousSlotId": pending.previous_slot_id,
            "bootId": pending.boot_id,
            "deadlineMonotonicMs": pending.deadline_monotonic_ms,
            "phase": pending_phase(pending.phase),
        })),
        "lastRollback": status.last_rollback.map(rollback_reason),
    })
}

fn activation_receipt(outcome: ActivationOutcome, credential_ready: bool) -> Value {
    match outcome {
        ActivationOutcome::AwaitingHealth {
            operation_id,
            slot_id,
            deadline_monotonic_ms,
        } => json!({
            "command": "activate",
            "result": "awaiting_health",
            "operationId": encode_hex(&operation_id),
            "slotId": slot_id,
            "deadlineMonotonicMs": deadline_monotonic_ms,
            "credentialReady": credential_ready,
        }),
        ActivationOutcome::AlreadyActive => json!({
            "command": "activate",
            "result": "already_active",
            "credentialReady": false,
        }),
        ActivationOutcome::ReplayConfirmed => json!({
            "command": "activate",
            "result": "replay_confirmed",
            "credentialReady": false,
        }),
        ActivationOutcome::ReplayRolledBack { reason } => json!({
            "command": "activate",
            "result": "replay_rolled_back",
            "reason": rollback_reason(reason),
            "credentialReady": false,
        }),
    }
}

fn confirmation_receipt(outcome: ConfirmationOutcome) -> Value {
    match outcome {
        ConfirmationOutcome::Confirmed => {
            json!({"command": "confirm_stdin", "result": "confirmed"})
        }
        ConfirmationOutcome::ReplayConfirmed => {
            json!({"command": "confirm_stdin", "result": "replay_confirmed"})
        }
        ConfirmationOutcome::RolledBack { reason } => json!({
            "command": "confirm_stdin",
            "result": "rolled_back",
            "reason": rollback_reason(reason),
        }),
    }
}

fn watchdog_receipt(recovery: SupervisorRecovery) -> Value {
    let manager = match recovery.manager {
        RecoveryOutcome::Clean => json!({"result": "clean"}),
        RecoveryOutcome::AwaitingHealth {
            slot_id,
            deadline_monotonic_ms,
        } => json!({
            "result": "awaiting_health",
            "slotId": slot_id,
            "deadlineMonotonicMs": deadline_monotonic_ms,
        }),
        RecoveryOutcome::RepairedPointer { slot_id } => {
            json!({"result": "repaired_pointer", "slotId": slot_id})
        }
        RecoveryOutcome::DroppedInvalidPrevious { slot_id } => {
            json!({"result": "dropped_invalid_previous", "slotId": slot_id})
        }
        RecoveryOutcome::RolledBack {
            active_slot_id,
            rejected_slot_id,
            reason,
        } => json!({
            "result": "rolled_back",
            "activeSlotId": active_slot_id,
            "rejectedSlotId": rejected_slot_id,
            "reason": rollback_reason(reason),
        }),
    };
    json!({
        "command": "watchdog_once",
        "manager": manager,
        "challenge": challenge_reconciliation(recovery.challenge),
        "abandonedStagingRemoved": recovery.abandoned_staging_removed,
        "slotDeletionsCompleted": recovery.slot_deletions_completed,
    })
}

fn challenge_reconciliation(value: ChallengeReconciliation) -> &'static str {
    match value {
        ChallengeReconciliation::None => "none",
        ChallengeReconciliation::Ready => "ready",
        ChallengeReconciliation::DiscardedStale => "discarded_stale",
        ChallengeReconciliation::RolledBackMissing => "rolled_back_missing",
        ChallengeReconciliation::RolledBackMismatch => "rolled_back_mismatch",
    }
}

fn pending_phase(value: PendingPhaseStatus) -> &'static str {
    match value {
        PendingPhaseStatus::Prepared => "prepared",
        PendingPhaseStatus::AwaitingHealth => "awaiting_health",
    }
}

fn rollback_reason(value: RollbackReason) -> &'static str {
    match value {
        RollbackReason::InterruptedPrepare => "interrupted_prepare",
        RollbackReason::BootChanged => "boot_changed",
        RollbackReason::HealthTimeout => "health_timeout",
        RollbackReason::ExplicitHealthFailure => "explicit_health_failure",
        RollbackReason::PointerMismatch => "pointer_mismatch",
        RollbackReason::TargetInvalid => "target_invalid",
        RollbackReason::HealthChallengeLost => "health_challenge_lost",
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_fixed_commands_and_nonsecret_positional_values() {
        let parsed = parse_arguments([
            "supervisor".into(),
            "--config".into(),
            "/etc/hardened-blikvm/update.json".into(),
            "activate".into(),
            "app-00000000000000000001-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        ])
        .unwrap();
        assert!(matches!(parsed.1, CliCommand::Activate(_)));
        let staged = parse_arguments([
            "supervisor".into(),
            "--config".into(),
            "/etc/hardened-blikvm/update.json".into(),
            "stage".into(),
            "/var/lib/hardened-blikvm-incoming/application.tar".into(),
        ])
        .unwrap();
        assert!(matches!(staged.1, CliCommand::Stage(_)));
        let collected = parse_arguments([
            "supervisor".into(),
            "--config".into(),
            "/etc/hardened-blikvm/update.json".into(),
            "gc".into(),
            "2".into(),
        ])
        .unwrap();
        assert_eq!(collected.1, CliCommand::GarbageCollect(2));

        for arguments in [
            vec!["supervisor", "status"],
            vec!["supervisor", "--config", "/tmp/c", "unknown"],
            vec![
                "supervisor",
                "--config",
                "/tmp/c",
                "confirm-stdin",
                "secret",
            ],
            vec!["supervisor", "--config", "/tmp/c", "activate"],
        ] {
            assert!(matches!(
                parse_arguments(arguments.into_iter().map(OsString::from)),
                Err(SupervisorCliError::InvalidArguments)
            ));
        }
    }

    #[test]
    fn receipts_never_include_a_health_token_or_public_key() {
        let receipt = activation_receipt(
            ActivationOutcome::AwaitingHealth {
                operation_id: [1; 16],
                slot_id: "app-00000000000000000001-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                deadline_monotonic_ms: 123,
            },
            true,
        );
        let encoded = serde_json::to_string(&receipt).unwrap();
        assert!(!encoded.contains("token"));
        assert!(!encoded.contains("publicKey"));
        assert!(!encoded.contains("credentialPath"));
        assert!(encoded.contains("credentialReady"));
    }

    #[test]
    fn command_failure_layers_have_stable_exit_codes() {
        assert_eq!(
            SupervisorCliError::Manager(UpdateManagerError::ManagerBusy).exit_code(),
            66
        );
        assert_eq!(
            SupervisorCliError::Manager(UpdateManagerError::UnsafeStateRoot).exit_code(),
            63
        );
        assert_eq!(
            SupervisorCliError::Manager(UpdateManagerError::Stager(
                ApplicationStageError::StagerBusy
            ))
            .exit_code(),
            87
        );
        assert_eq!(SupervisorCliError::InvalidArguments.exit_code(), 2);
        assert_eq!(SupervisorCliError::InvalidConfig.exit_code(), 12);
        assert_eq!(
            SupervisorCliError::Runtime(UpdateRuntimeError::InvalidHealthResponseLength)
                .exit_code(),
            32
        );
        assert_eq!(
            SupervisorCliError::Runtime(UpdateRuntimeError::ChallengeMissing).exit_code(),
            33
        );
        assert_eq!(
            SupervisorCliError::Runtime(UpdateRuntimeError::Manager(
                UpdateManagerError::ManagerBusy,
            ))
            .exit_code(),
            66
        );
        assert_eq!(
            SupervisorCliError::Runtime(UpdateRuntimeError::Manager(
                UpdateManagerError::ConfirmationMismatch,
            ))
            .exit_code(),
            84
        );
        assert_eq!(
            SupervisorCliError::Runtime(UpdateRuntimeError::Manager(
                UpdateManagerError::SlotProofMismatch,
            ))
            .exit_code(),
            85
        );
        let nested_stager_busy = SupervisorCliError::Runtime(UpdateRuntimeError::Manager(
            UpdateManagerError::Stager(ApplicationStageError::StagerBusy),
        ));
        assert_eq!(nested_stager_busy.exit_code(), 87);
    }
}
