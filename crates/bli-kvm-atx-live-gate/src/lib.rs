//! Live ATX waiting-route gate.
//!
//! Runs the persisted ATX state machine against the real KVMD internal socket
//! with Power and Reset physically disconnected. It proves the three fixed
//! `wait=1` routes, duplicate replay, conflict rejection, five/thirty-second
//! cooldowns, restart with a Pending journal and explicit reconciliation. It
//! performs real KVMD clicks and is meant to be run only under the reviewed
//! test conditions.

use std::{
    path::Path,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use bli_kvm_atx_policy::{
    AtxCompletion, AtxDecision, AtxEligibility, AtxOperation, AtxPolicyEngine, AtxRejection,
};
use bli_kvm_atx_transition::{AtxTransitionEngine, AtxTransitionOutcome, PowerLedObservation};
use bli_kvm_core::AtxControlAction;
use bli_kvm_kvmd_atx_client::KvmdAtxClient;
use bli_kvm_kvmd_client::KvmdCredentials;

const SESSION: u64 = 9;
const BINDINGS: [u8; 32] = [0x44; 32];

#[derive(Debug)]
pub enum GateError {
    Setup(&'static str),
    Phase(&'static str),
}

impl std::fmt::Display for GateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Setup(_) => "ATX live gate setup failed",
            Self::Phase(_) => "ATX live gate phase failed",
        })
    }
}

impl std::error::Error for GateError {}

fn open(
    socket: &Path,
    journal: &Path,
    username: &[u8],
    password: &[u8],
) -> Result<AtxTransitionEngine, GateError> {
    let client = KvmdAtxClient::new(socket).map_err(|_| GateError::Setup("kvmd-client"))?;
    let credentials =
        KvmdCredentials::new(username, password).map_err(|_| GateError::Setup("credentials"))?;
    AtxTransitionEngine::open(journal, client, credentials).map_err(|_| GateError::Setup("engine"))
}

fn operation(seed: u8, action: AtxControlAction, issued_at_ms: u64) -> AtxOperation {
    AtxOperation::new([seed; 16], SESSION, BINDINGS, issued_at_ms, action)
        .expect("operation inputs are non-zero")
}

fn eligibility(issued_at_ms: u64, now_ms: u64) -> AtxEligibility {
    AtxEligibility::reviewed_for_confirmation(SESSION, BINDINGS, issued_at_ms, now_ms, true, true)
        .expect("a fresh explicit confirmation is eligible")
}

/// Runs every live phase and returns the fixed report lines.
pub async fn run_live_checks(
    socket: &Path,
    journal: &Path,
    username: &[u8],
    password: &[u8],
    start_ms: u64,
) -> Result<Vec<(String, String)>, GateError> {
    let mut now = start_ms;
    let mut engine = open(socket, journal, username, password)?;

    // 1. Live power_short through the fixed waiting route.
    let short = operation(1, AtxControlAction::PowerShort, now);
    let started = Instant::now();
    match engine.execute(short, eligibility(now, now), now).await {
        Ok(AtxTransitionOutcome::Applied) => {}
        _ => return Err(GateError::Phase("power_short")),
    }
    let short_elapsed_ms = started.elapsed().as_millis() as u64;

    // 2. Duplicate operation ID replays without another KVMD call.
    now += 1_000;
    match engine.execute(short, eligibility(now, now), now).await {
        Ok(AtxTransitionOutcome::ReplayedApplied) => {}
        _ => return Err(GateError::Phase("duplicate_replay")),
    }

    // 3. Conflicting action under the same ID is rejected.
    let conflict = operation(1, AtxControlAction::Reset, now);
    match engine.execute(conflict, eligibility(now, now), now).await {
        Ok(AtxTransitionOutcome::Rejected(AtxRejection::OperationIdConflict)) => {}
        _ => return Err(GateError::Phase("conflict_rejected")),
    }

    // 4. Five-second cooldown blocks another power_short.
    let second = operation(2, AtxControlAction::PowerShort, now);
    match engine.execute(second, eligibility(now, now), now).await {
        Ok(AtxTransitionOutcome::Rejected(AtxRejection::CooldownActive)) => {}
        _ => return Err(GateError::Phase("cooldown_active")),
    }

    // 5. power_long after the thirty-second global cooldown.
    now += 31_000;
    let long = operation(3, AtxControlAction::PowerLong, now);
    match engine.execute(long, eligibility(now, now), now).await {
        Ok(AtxTransitionOutcome::Applied) => {}
        _ => return Err(GateError::Phase("power_long")),
    }

    // 6. reset after another thirty seconds.
    now += 31_000;
    let reset = operation(4, AtxControlAction::Reset, now);
    match engine.execute(reset, eligibility(now, now), now).await {
        Ok(AtxTransitionOutcome::Applied) => {}
        _ => return Err(GateError::Phase("reset")),
    }

    // 7. A Pending journal survives a restart and blocks new work until an
    //    explicit reconciliation records the outcome.
    now += 31_000;
    let pending = operation(5, AtxControlAction::PowerShort, now);
    {
        let mut policy = AtxPolicyEngine::open(journal).map_err(|_| GateError::Setup("policy"))?;
        match policy
            .prepare(pending, eligibility(now, now), now)
            .map_err(|_| GateError::Setup("prepare"))?
        {
            AtxDecision::Permit => {}
            _ => return Err(GateError::Phase("pending_prepare")),
        }
    }
    drop(engine);
    let mut restarted = open(socket, journal, username, password)?;
    let blocked = operation(6, AtxControlAction::PowerShort, now);
    match restarted.execute(blocked, eligibility(now, now), now).await {
        Ok(AtxTransitionOutcome::Rejected(AtxRejection::IndeterminatePending)) => {}
        _ => return Err(GateError::Phase("restart_pending")),
    }
    restarted
        .resolve_pending(
            [5; 16],
            AtxCompletion::Applied,
            PowerLedObservation::Off,
            now + 1_000,
        )
        .map_err(|_| GateError::Phase("reconciliation"))?;
    match restarted
        .execute(pending, eligibility(now + 1_000, now + 1_000), now + 1_000)
        .await
    {
        Ok(AtxTransitionOutcome::ReplayedApplied) => {}
        _ => return Err(GateError::Phase("reconciled_replay")),
    }

    Ok(vec![
        ("power_short".into(), "ok".into()),
        (
            "power_short_elapsed_ms".into(),
            short_elapsed_ms.to_string(),
        ),
        ("duplicate_replay".into(), "ok".into()),
        ("conflict_rejected".into(), "ok".into()),
        ("cooldown_active".into(), "ok".into()),
        ("power_long".into(), "ok".into()),
        ("reset".into(), "ok".into()),
        ("restart_pending".into(), "ok".into()),
        ("reconciliation".into(), "ok".into()),
        ("physical_actions_invoked".into(), "yes".into()),
    ])
}

pub fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::UnixListener,
    };

    use super::*;

    async fn serve(socket: &Path, count: usize) -> tokio::task::JoinHandle<Vec<String>> {
        let listener = UnixListener::bind(socket).unwrap();
        tokio::spawn(async move {
            let mut requests = Vec::new();
            for _ in 0..count {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut chunk = [0_u8; 1024];
                    let read = stream.read(&mut chunk).await.unwrap();
                    request.extend_from_slice(&chunk[..read]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}")
                    .await
                    .unwrap();
                requests.push(String::from_utf8(request).unwrap());
            }
            requests
        })
    }

    #[tokio::test]
    async fn every_live_phase_reaches_only_fixed_routes() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("kvmd.sock");
        let journal = directory.path().join("journal.json");
        let server = serve(&socket, 3).await;
        let facts = run_live_checks(
            &socket,
            &journal,
            b"test-user",
            b"not-a-real-password",
            1_000_000,
        )
        .await
        .unwrap();
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 3);
        assert!(requests[0].starts_with("POST /atx/click?button=power&wait=1 HTTP/1.1\r\n"));
        assert!(requests[1].starts_with("POST /atx/click?button=power_long&wait=1 HTTP/1.1\r\n"));
        assert!(requests[2].starts_with("POST /atx/click?button=reset&wait=1 HTTP/1.1\r\n"));
        let named: std::collections::HashMap<_, _> = facts.into_iter().collect();
        assert_eq!(named.get("power_short"), Some(&"ok".to_string()));
        assert_eq!(
            named.get("physical_actions_invoked"),
            Some(&"yes".to_string())
        );
        assert_eq!(named.get("restart_pending"), Some(&"ok".to_string()));
        assert_eq!(named.get("reconciliation"), Some(&"ok".to_string()));
    }
}
