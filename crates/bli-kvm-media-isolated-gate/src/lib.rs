//! Isolated virtual-media transition gate.
//!
//! Runs the persisted media state machine against local mock KVMD sockets and a
//! private temp catalog. It proves attach, eject, catalog selection, duplicate
//! replay, backend-failure persistence with reconnect, and restart with a
//! Pending journal plus reconciliation. It never contacts the real KVMD.

use std::path::Path;

use bli_kvm_core::{VirtualMediaCommand, VirtualMediaControlMode};
use bli_kvm_kvmd_client::KvmdCredentials;
use bli_kvm_kvmd_media_client::KvmdMediaClient;
use bli_kvm_media_catalog::MediaCatalog;
use bli_kvm_media_policy::{
    MediaCompletion, MediaDecision, MediaEligibility, MediaObservedState, MediaOperation,
    MediaPolicyEngine, MediaRejection,
};
use bli_kvm_media_transition::{MediaTransitionEngine, MediaTransitionOutcome};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixListener,
};

const CATALOG: &str = r#"{
    "schemaVersion": 1,
    "entries": [{
        "imageId": 7,
        "imageName": "rescue.iso",
        "size": 1,
        "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "cdromAllowed": true,
        "removableAllowed": false,
        "writableAllowed": false
    }]
}"#;

#[derive(Debug)]
pub enum GateError {
    Setup(&'static str),
    Phase(&'static str),
}

impl std::fmt::Display for GateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("isolated media gate failed")
    }
}

impl std::error::Error for GateError {}

async fn serve(
    socket: &Path,
    statuses: &[&'static str],
) -> Result<tokio::task::JoinHandle<Vec<String>>, GateError> {
    let listener = UnixListener::bind(socket).map_err(|_| GateError::Setup("socket"))?;
    let statuses = statuses
        .iter()
        .map(|value| value.to_string())
        .collect::<Vec<_>>();
    Ok(tokio::spawn(async move {
        let mut requests = Vec::new();
        for status in statuses {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let mut request = Vec::new();
            loop {
                let mut chunk = [0_u8; 1024];
                let Ok(read) = stream.read(&mut chunk).await else {
                    break;
                };
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let body = if status.starts_with("200") {
                r#"{"ok":true}"#
            } else {
                r#"{"ok":false}"#
            };
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
            requests.push(String::from_utf8_lossy(&request).into_owned());
        }
        requests
    }))
}

fn catalog(root: &Path) -> Result<MediaCatalog, GateError> {
    let path = root.join("catalog.json");
    std::fs::write(&path, CATALOG).map_err(|_| GateError::Setup("catalog-write"))?;
    MediaCatalog::open(&path).map_err(|_| GateError::Setup("catalog"))
}

fn engine(root: &Path, socket: &Path) -> Result<MediaTransitionEngine, GateError> {
    MediaTransitionEngine::open(
        root.join("journal.json"),
        KvmdMediaClient::new(socket).map_err(|_| GateError::Setup("client"))?,
        catalog(root)?,
        KvmdCredentials::new("media-gate-user", "media-gate-not-a-real-password")
            .map_err(|_| GateError::Setup("credentials"))?,
    )
    .map_err(|_| GateError::Setup("engine"))
}

fn observed(busy: bool, connected: bool) -> MediaObservedState {
    MediaObservedState {
        enabled: true,
        online: true,
        busy,
        connected,
    }
}

fn operation(
    seed: u8,
    command: VirtualMediaCommand,
    issued_at_ms: u64,
) -> Result<MediaOperation, GateError> {
    MediaOperation::new([seed; 16], 9, [0x44; 32], issued_at_ms, command)
        .map_err(|_| GateError::Setup("operation"))
}

fn eligible() -> MediaEligibility {
    MediaEligibility::verified_for_test_profile()
}

async fn apply(
    machine: &mut MediaTransitionEngine,
    operation: MediaOperation,
    observed: MediaObservedState,
    now_ms: u64,
) -> Result<MediaTransitionOutcome, GateError> {
    machine
        .execute(operation, eligible(), observed, now_ms)
        .await
        .map_err(|_| GateError::Phase("execute"))
}

/// Runs every isolated phase and returns the fixed report lines.
pub async fn run_isolated_checks() -> Result<Vec<(&'static str, &'static str)>, GateError> {
    let root = std::env::temp_dir().join(format!("bli-kvm-media-gate-{}", std::process::id()));
    std::fs::create_dir_all(&root).map_err(|_| GateError::Setup("root"))?;

    // 1. Attach.
    let socket = root.join("attach.sock");
    let server = serve(&socket, &["200 OK"]).await?;
    let mut machine = engine(&root, &socket)?;
    let attach = operation(
        1,
        VirtualMediaCommand::SetConnected { connected: true },
        1_000,
    )?;
    if apply(&mut machine, attach, observed(false, false), 1_000).await?
        != MediaTransitionOutcome::Applied
    {
        return Err(GateError::Phase("attach"));
    }
    let requests = server
        .await
        .map_err(|_| GateError::Phase("attach-server"))?;
    if requests.len() != 1 || !requests[0].contains("set_connected?connected=1") {
        return Err(GateError::Phase("attach-route"));
    }

    // 2. Eject after the mutation cooldown.
    let _ = std::fs::remove_file(&socket);
    let server = serve(&socket, &["200 OK"]).await?;
    let eject = operation(
        2,
        VirtualMediaCommand::SetConnected { connected: false },
        4_000,
    )?;
    if apply(&mut machine, eject, observed(false, true), 4_000).await?
        != MediaTransitionOutcome::Applied
    {
        return Err(GateError::Phase("eject"));
    }
    let requests = server.await.map_err(|_| GateError::Phase("eject-server"))?;
    if requests.len() != 1 || !requests[0].contains("set_connected?connected=0") {
        return Err(GateError::Phase("eject-route"));
    }

    // 3. Catalog selection, then duplicate replay without another call.
    let _ = std::fs::remove_file(&socket);
    let server = serve(&socket, &["200 OK"]).await?;
    let select = operation(
        3,
        VirtualMediaCommand::Select {
            image_id: 7,
            mode: VirtualMediaControlMode::CdRom,
            writable: false,
        },
        7_000,
    )?;
    if apply(&mut machine, select, observed(false, false), 7_000).await?
        != MediaTransitionOutcome::Applied
    {
        return Err(GateError::Phase("select"));
    }
    if apply(&mut machine, select, observed(false, false), 9_500).await?
        != MediaTransitionOutcome::ReplayedApplied
    {
        return Err(GateError::Phase("select-replay"));
    }
    let requests = server
        .await
        .map_err(|_| GateError::Phase("select-server"))?;
    if requests.len() != 1 || !requests[0].contains("set_params?image=rescue.iso") {
        return Err(GateError::Phase("select-route"));
    }

    // 4. Backend failure is persisted, then a healthy backend recovers.
    let _ = std::fs::remove_file(&socket);
    let server = serve(&socket, &["500 Error"]).await?;
    let failing = operation(
        4,
        VirtualMediaCommand::SetConnected { connected: true },
        12_000,
    )?;
    if apply(&mut machine, failing, observed(false, false), 12_000).await?
        != MediaTransitionOutcome::BackendFailed
    {
        return Err(GateError::Phase("backend-failure"));
    }
    if apply(&mut machine, failing, observed(false, false), 14_500).await?
        != MediaTransitionOutcome::ReplayedRejected
    {
        return Err(GateError::Phase("backend-failure-replay"));
    }
    let requests = server
        .await
        .map_err(|_| GateError::Phase("failure-server"))?;
    if requests.len() != 1 {
        return Err(GateError::Phase("backend-failure-retry"));
    }
    let _ = std::fs::remove_file(&socket);
    let server = serve(&socket, &["200 OK"]).await?;
    let recovery = operation(
        5,
        VirtualMediaCommand::SetConnected { connected: true },
        17_000,
    )?;
    if apply(&mut machine, recovery, observed(false, false), 17_000).await?
        != MediaTransitionOutcome::Applied
    {
        return Err(GateError::Phase("backend-recovery"));
    }
    let requests = server
        .await
        .map_err(|_| GateError::Phase("recovery-server"))?;
    if requests.len() != 1 || !requests[0].contains("set_connected?connected=1") {
        return Err(GateError::Phase("backend-recovery-route"));
    }

    // 5. Restart with a Pending journal blocks new work until reconciliation.
    let pending = operation(
        6,
        VirtualMediaCommand::SetConnected { connected: false },
        20_000,
    )?;
    {
        let mut policy = MediaPolicyEngine::open(root.join("journal.json"))
            .map_err(|_| GateError::Setup("policy"))?;
        let decision = policy
            .prepare(pending, eligible(), observed(false, true), 20_000)
            .map_err(|_| GateError::Setup("prepare"))?;
        if decision != MediaDecision::Permit {
            return Err(GateError::Phase("pending-prepare"));
        }
    }
    drop(machine);
    let mut restarted = engine(&root, &socket)?;
    let blocked = operation(
        7,
        VirtualMediaCommand::SetConnected { connected: true },
        20_000,
    )?;
    if restarted
        .execute(blocked, eligible(), observed(false, false), 20_000)
        .await
        .map_err(|_| GateError::Phase("pending-execute"))?
        != MediaTransitionOutcome::Rejected(MediaRejection::IndeterminatePending)
    {
        return Err(GateError::Phase("pending-block"));
    }
    restarted
        .resolve_pending([6; 16], MediaCompletion::Applied, 21_000)
        .map_err(|_| GateError::Phase("reconciliation"))?;
    if restarted
        .execute(pending, eligible(), observed(false, true), 23_000)
        .await
        .map_err(|_| GateError::Phase("pending-replay"))?
        != MediaTransitionOutcome::ReplayedApplied
    {
        return Err(GateError::Phase("reconciled-replay"));
    }

    let _ = std::fs::remove_dir_all(&root);
    Ok(vec![
        ("isolated_backend", "yes"),
        ("attach", "ok"),
        ("eject", "ok"),
        ("select", "ok"),
        ("duplicate_replay", "ok"),
        ("backend_recovery", "ok"),
        ("pending_reconciliation", "ok"),
        ("physical_actions_invoked", "no"),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn every_isolated_media_phase_passes() {
        let facts = run_isolated_checks().await.unwrap();
        let named: std::collections::HashMap<_, _> = facts.into_iter().collect();
        for key in [
            "attach",
            "eject",
            "select",
            "duplicate_replay",
            "backend_recovery",
            "pending_reconciliation",
        ] {
            assert_eq!(named.get(key), Some(&"ok"), "phase {key}");
        }
        assert_eq!(named.get("physical_actions_invoked"), Some(&"no"));
    }
}
