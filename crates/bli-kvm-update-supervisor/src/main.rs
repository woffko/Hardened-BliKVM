mod config;

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
use serde_json::{Value, json};
use thiserror::Error;

use config::load_config;

#[derive(Debug, Error)]
enum SupervisorCliError {
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
    #[error("supervisor I/O failed")]
    Io(#[from] io::Error),
    #[error("supervisor receipt serialization failed")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CliCommand {
    Status,
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
            if matches!(error, SupervisorCliError::InvalidArguments) {
                ExitCode::from(2)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}

fn run(arguments: impl IntoIterator<Item = OsString>) -> Result<(), SupervisorCliError> {
    let (config_path, command) = parse_arguments(arguments)?;
    let config = load_config(&config_path)?;
    let manager = ApplicationUpdateManager::open(
        config.slot_root,
        config.state_root,
        config.trust_root,
        config.system_sequence,
        config.health_timeout_ms,
    )?;
    let challenges = HealthChallengeStore::open(config.runtime_root)?;
    let supervisor = UpdateSupervisor::new(manager, challenges);
    let receipt = match command {
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
            activation_receipt(activation.outcome, activation.credential_path.is_some())
        }
        CliCommand::WatchdogOnce => watchdog_receipt(supervisor.watchdog_once()?),
        CliCommand::ConfirmStdin => {
            let stdin = io::stdin();
            let mut locked = stdin.lock();
            confirmation_receipt(supervisor.confirm_from_reader(&mut locked)?)
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
        "status" => CliCommand::Status,
        "watchdog-once" => CliCommand::WatchdogOnce,
        "confirm-stdin" => CliCommand::ConfirmStdin,
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
}
