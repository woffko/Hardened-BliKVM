//! Isolated-backend HID transition gate.
//!
//! This gate runs the eligibility-gated HID control policy against a local
//! mock KVMD Unix socket. It never talks to the real KVMD or host and proves
//! startup-ledger replay, lease apply/release, expiry, identity mismatch and
//! backend-failure release-all on the target device.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use bli_kvm_core::HidReport;
use bli_kvm_hid_adapter::{HidLeaseIdentity, KvmdHidEvent, KvmdKeyCode};
use bli_kvm_hid_policy::{HidControlPolicy, HidDecision, HidEligibility, HidRejection};
use bli_kvm_hid_transition::{HidReleaseLedger, HidTransitionEngine};
use bli_kvm_kvmd_client::KvmdCredentials;
use bli_kvm_kvmd_hid_client::KvmdHidClient;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixListener,
};

const KEY_A_USAGE: u8 = 4;

#[derive(Debug)]
pub enum GateError {
    Setup(&'static str),
    Phase(&'static str),
    Io(std::io::Error),
}

impl std::fmt::Display for GateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Setup(phase) => {
                let _ = phase;
                "isolated gate setup failed"
            }
            Self::Phase(phase) => {
                let _ = phase;
                "isolated gate phase failed"
            }
            Self::Io(_) => "isolated gate I/O failed",
        })
    }
}

impl std::error::Error for GateError {}

impl From<std::io::Error> for GateError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

fn key_event(pressed: bool) -> KvmdHidEvent {
    KvmdHidEvent::Key {
        key: KvmdKeyCode::from_usb_usage(KEY_A_USAGE).expect("KeyA is in the pinned table"),
        pressed,
    }
}

fn identity(seed: u8, lease_id: [u8; 16]) -> HidLeaseIdentity {
    HidLeaseIdentity::new(lease_id, u64::from(seed), [seed; 32]).expect("identity is non-zero")
}

fn policy(ledger: &Path, socket: &Path) -> Result<HidControlPolicy, GateError> {
    let engine = HidTransitionEngine::new(
        KvmdHidClient::new(socket).map_err(|_| GateError::Setup("kvmd-client"))?,
        KvmdCredentials::new("hid-gate-user", "hid-gate-not-a-real-password")
            .map_err(|_| GateError::Setup("credentials"))?,
        HidReleaseLedger::new(ledger).map_err(|_| GateError::Setup("ledger"))?,
    )
    .map_err(|_| GateError::Setup("engine"))?;
    Ok(HidControlPolicy::new(engine))
}

async fn serve(
    socket: &Path,
    statuses: &[&'static str],
) -> Result<tokio::task::JoinHandle<Vec<String>>, GateError> {
    let listener = UnixListener::bind(socket).map_err(GateError::Io)?;
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

async fn serve_repeat(
    socket: &Path,
    count: usize,
    status: &'static str,
) -> Result<tokio::task::JoinHandle<Vec<String>>, GateError> {
    let statuses = vec![status; count];
    serve(socket, &statuses).await
}

async fn phase_mouse(root: &Path, eligible: HidEligibility, now: Instant) -> Result<(), GateError> {
    let socket = root.join("mouse.sock");
    let ledger = root.join("mouse-ledger.json");
    let server = serve(&socket, &["200 OK"; 5]).await?;
    let mut gate = policy(&ledger, &socket)?;
    let owner = identity(6, [6; 16]);
    if gate
        .acquire(owner, eligible, now, Duration::from_secs(30))
        .await
        != HidDecision::Applied
    {
        return Err(GateError::Phase("mouse_acquire"));
    }
    let reports = [
        HidReport::AbsoluteMouse {
            data: [0, 0x00, 0x40, 0x00, 0x20, 0],
        },
        HidReport::RelativeMouse {
            data: [0, 10, 246, 0],
        },
        HidReport::RelativeMouse { data: [0, 0, 0, 5] },
        HidReport::RelativeMouse {
            data: [0x01, 0, 0, 0],
        },
        HidReport::RelativeMouse { data: [0, 0, 0, 0] },
    ];
    for report in &reports {
        if gate.report(owner, eligible, now, report).await != HidDecision::Applied {
            return Err(GateError::Phase("mouse_report"));
        }
    }
    let requests = server.await.map_err(|_| GateError::Phase("mouse_server"))?;
    if requests.len() != 5
        || !requests[0].contains("send_mouse_move?")
        || !requests[1].contains("send_mouse_relative?delta_x=10&delta_y=-10")
        || !requests[2].contains("send_mouse_wheel?delta_x=0&delta_y=5")
        || !requests[3].contains("send_mouse_button?button=left&state=1")
        || !requests[4].contains("send_mouse_button?button=left&state=0")
    {
        return Err(GateError::Phase("mouse_routes"));
    }
    if gate.release(owner).await != HidDecision::Applied {
        return Err(GateError::Phase("mouse_release"));
    }
    Ok(())
}

async fn phase_rollover(
    root: &Path,
    eligible: HidEligibility,
    now: Instant,
) -> Result<(), GateError> {
    let socket = root.join("rollover.sock");
    let ledger = root.join("rollover-ledger.json");
    let server = serve(&socket, &["200 OK"; 24]).await?;
    let mut gate = policy(&ledger, &socket)?;
    let owner = identity(7, [7; 16]);
    if gate
        .acquire(owner, eligible, now, Duration::from_secs(30))
        .await
        != HidDecision::Applied
    {
        return Err(GateError::Phase("rollover_acquire"));
    }
    for report in [
        HidReport::Keyboard {
            data: [0, 0, 4, 5, 6, 7, 8, 9],
        },
        HidReport::Keyboard {
            data: [0, 0, 10, 11, 12, 13, 14, 15],
        },
    ] {
        if gate.report(owner, eligible, now, &report).await != HidDecision::Applied {
            return Err(GateError::Phase("rollover_report"));
        }
    }
    if gate.release(owner).await != HidDecision::Applied {
        return Err(GateError::Phase("rollover_release"));
    }
    let requests = server
        .await
        .map_err(|_| GateError::Phase("rollover_server"))?;
    if requests.len() != 24 {
        return Err(GateError::Phase("rollover_count"));
    }
    for (index, request) in requests.iter().enumerate() {
        let expected = match index {
            0..=5 | 12..=17 => "state=1",
            _ => "state=0",
        };
        if !request.contains(expected) {
            return Err(GateError::Phase("rollover_order"));
        }
    }
    Ok(())
}

async fn phase_soak(
    root: &Path,
    eligible: HidEligibility,
    now: Instant,
    events: usize,
) -> Result<(), GateError> {
    let socket = root.join("soak.sock");
    let ledger = root.join("soak-ledger.json");
    let server = serve_repeat(&socket, events, "200 OK").await?;
    let mut gate = policy(&ledger, &socket)?;
    let owner = identity(8, [8; 16]);
    if gate
        .acquire(owner, eligible, now, Duration::from_secs(30))
        .await
        != HidDecision::Applied
    {
        return Err(GateError::Phase("soak_acquire"));
    }
    for index in 0..events {
        let data = if index % 2 == 0 {
            [0, 0, KEY_A_USAGE, 0, 0, 0, 0, 0]
        } else {
            [0; 8]
        };
        if gate
            .report(owner, eligible, now, &HidReport::Keyboard { data })
            .await
            != HidDecision::Applied
        {
            return Err(GateError::Phase("soak_report"));
        }
    }
    let requests = server.await.map_err(|_| GateError::Phase("soak_server"))?;
    if requests.len() != events {
        return Err(GateError::Phase("soak_count"));
    }
    if gate.pending_release_count() != 0 {
        return Err(GateError::Phase("soak_pending"));
    }
    Ok(())
}

/// Runs every isolated phase and returns the fixed report lines.
pub async fn run_isolated_checks(
    soak_events: usize,
) -> Result<Vec<(&'static str, &'static str)>, GateError> {
    let root = std::env::temp_dir().join(format!("bli-kvm-hid-gate-{}", std::process::id()));
    std::fs::create_dir_all(&root)?;
    let eligible = HidEligibility::verified_for_test_profile();
    let now = Instant::now();

    // Startup ledger replay: a persisted release must be flushed before any
    // new lease and must reach the isolated backend.
    let replay_socket = root.join("replay.sock");
    let replay_ledger = root.join("replay-ledger.json");
    {
        let ledger =
            HidReleaseLedger::new(&replay_ledger).map_err(|_| GateError::Setup("replay-ledger"))?;
        ledger
            .store(&[key_event(false)])
            .map_err(|_| GateError::Setup("replay-store"))?;
    }
    let server = serve(&replay_socket, &["200 OK"]).await?;
    let mut gate = policy(&replay_ledger, &replay_socket)?;
    if gate.pending_release_count() != 1 {
        return Err(GateError::Phase("startup_pending"));
    }
    if gate.disconnect().await != HidDecision::Applied {
        return Err(GateError::Phase("startup_replay"));
    }
    let requests = server.await.map_err(|_| GateError::Phase("server"))?;
    if requests.len() != 1 || !requests[0].contains("key=KeyA&state=0") {
        return Err(GateError::Phase("startup_replay_route"));
    }

    // Lease apply and release reach only the fixed key route.
    let apply_socket = root.join("apply.sock");
    let apply_ledger = root.join("apply-ledger.json");
    let server = serve(&apply_socket, &["200 OK", "200 OK"]).await?;
    let mut gate = policy(&apply_ledger, &apply_socket)?;
    let owner = identity(1, [1; 16]);
    if gate
        .acquire(owner, eligible, now, Duration::from_secs(30))
        .await
        != HidDecision::Applied
    {
        return Err(GateError::Phase("acquire"));
    }
    if gate
        .report(
            owner,
            eligible,
            now,
            &bli_kvm_core::HidReport::Keyboard {
                data: [0, 0, KEY_A_USAGE, 0, 0, 0, 0, 0],
            },
        )
        .await
        != HidDecision::Applied
    {
        return Err(GateError::Phase("report"));
    }
    if gate.release(owner).await != HidDecision::Applied {
        return Err(GateError::Phase("release"));
    }
    let requests = server.await.map_err(|_| GateError::Phase("server"))?;
    if requests.len() != 2
        || !requests[0].contains("key=KeyA&state=1")
        || !requests[1].contains("key=KeyA&state=0")
    {
        return Err(GateError::Phase("apply_routes"));
    }

    // Expiry releases a held key.
    let expiry_socket = root.join("expiry.sock");
    let expiry_ledger = root.join("expiry-ledger.json");
    let server = serve(&expiry_socket, &["200 OK", "200 OK"]).await?;
    let mut gate = policy(&expiry_ledger, &expiry_socket)?;
    let owner = identity(2, [2; 16]);
    if gate
        .acquire(owner, eligible, now, Duration::from_secs(30))
        .await
        != HidDecision::Applied
    {
        return Err(GateError::Phase("expiry_acquire"));
    }
    if gate
        .report(
            owner,
            eligible,
            now,
            &bli_kvm_core::HidReport::Keyboard {
                data: [0, 0, KEY_A_USAGE, 0, 0, 0, 0, 0],
            },
        )
        .await
        != HidDecision::Applied
    {
        return Err(GateError::Phase("expiry_report"));
    }
    if gate.expire(now + Duration::from_secs(31)).await != HidDecision::Applied {
        return Err(GateError::Phase("expiry"));
    }
    let requests = server.await.map_err(|_| GateError::Phase("server"))?;
    if requests.len() != 2 || !requests[1].contains("key=KeyA&state=0") {
        return Err(GateError::Phase("expiry_release"));
    }

    // Identity mismatch is rejected without backend I/O.
    let mismatch_socket = root.join("absent.sock");
    let mismatch_ledger = root.join("mismatch-ledger.json");
    let mut gate = policy(&mismatch_ledger, &mismatch_socket)?;
    if gate
        .acquire(identity(3, [3; 16]), eligible, now, Duration::from_secs(30))
        .await
        != HidDecision::Applied
    {
        return Err(GateError::Phase("mismatch_acquire"));
    }
    if gate
        .report(
            identity(4, [4; 16]),
            eligible,
            now,
            &bli_kvm_core::HidReport::Keyboard {
                data: [0, 0, KEY_A_USAGE, 0, 0, 0, 0, 0],
            },
        )
        .await
        != HidDecision::Reject(HidRejection::LeaseMismatch)
    {
        return Err(GateError::Phase("mismatch"));
    }
    if mismatch_socket.exists() {
        return Err(GateError::Phase("mismatch_backend_contacted"));
    }

    // Backend failure revokes the lease and completes release-all.
    let failure_socket = root.join("failure.sock");
    let failure_ledger = root.join("failure-ledger.json");
    let server = serve(&failure_socket, &["500 Error", "200 OK"]).await?;
    let mut gate = policy(&failure_ledger, &failure_socket)?;
    let owner = identity(5, [5; 16]);
    if gate
        .acquire(owner, eligible, now, Duration::from_secs(30))
        .await
        != HidDecision::Applied
    {
        return Err(GateError::Phase("failure_acquire"));
    }
    if gate
        .report(
            owner,
            eligible,
            now,
            &bli_kvm_core::HidReport::Keyboard {
                data: [0, 0, KEY_A_USAGE, 0, 0, 0, 0, 0],
            },
        )
        .await
        != HidDecision::Reject(HidRejection::BackendUnavailable)
    {
        return Err(GateError::Phase("failure_report"));
    }
    let requests = server.await.map_err(|_| GateError::Phase("server"))?;
    if requests.len() != 2 || !requests[1].contains("key=KeyA&state=0") {
        return Err(GateError::Phase("failure_release_all"));
    }
    if gate.pending_release_count() != 0 {
        return Err(GateError::Phase("failure_pending"));
    }

    phase_mouse(&root, eligible, now).await?;
    phase_rollover(&root, eligible, now).await?;
    phase_soak(&root, eligible, now, soak_events).await?;

    let _ = std::fs::remove_dir_all(&root);
    Ok(vec![
        ("isolated_backend", "yes"),
        ("startup_replay", "ok"),
        ("lease_apply", "ok"),
        ("expiry_release", "ok"),
        ("identity_mismatch", "ok"),
        ("failure_release_all", "ok"),
        ("mouse_modes", "ok"),
        ("rollover", "ok"),
        ("soak", "ok"),
        ("physical_actions_invoked", "no"),
    ])
}

/// Path helper used by tests.
pub fn default_root() -> PathBuf {
    std::env::temp_dir().join(format!("bli-kvm-hid-gate-{}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn every_isolated_phase_passes() {
        let facts = run_isolated_checks(200).await.unwrap();
        let values: Vec<&str> = facts.iter().map(|(_, value)| *value).collect();
        assert!(values.contains(&"ok"));
        assert_eq!(facts.len(), 10);
        let named: std::collections::HashMap<_, _> = facts.into_iter().collect();
        for key in [
            "startup_replay",
            "lease_apply",
            "expiry_release",
            "identity_mismatch",
            "failure_release_all",
            "mouse_modes",
            "rollover",
            "soak",
        ] {
            assert_eq!(named.get(key), Some(&"ok"), "phase {key}");
        }
        assert_eq!(named.get("physical_actions_invoked"), Some(&"no"));
    }
}
