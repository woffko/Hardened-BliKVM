#![cfg(feature = "gadget-hid-profile")]

use std::{
    fs,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

use bli_kvm_control_client::ControlClient;
use bli_kvm_control_protocol::ControlResponse;
use bli_kvm_core::{HidLeaseRequest, HidReport, PlatformControlRequestV2, TargetAttestation};

struct OwnedChild(Child);

impl Drop for OwnedChild {
    fn drop(&mut self) {
        // Reap the exact child even when an assertion fails.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn process_signals_release_native_hid_before_successful_exit() {
    for (signal, fail_release) in [
        (libc::SIGTERM, false),
        (libc::SIGINT, false),
        (libc::SIGTERM, true),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("control.sock");
        let ledger = directory.path().join("ledger.json");
        let hidg = directory.path().join("hidg");
        fs::create_dir(&hidg).unwrap();
        let keyboard = hidg.join("hidg0");
        fs::write(&keyboard, []).unwrap();
        let mut child = OwnedChild(
            Command::new(env!("CARGO_BIN_EXE_bli-kvm-control-broker"))
                .env_clear()
                .env("BLI_KVM_CONTROL_SOCKET", &socket)
                .env("BLI_KVM_GADGET_HID_PROFILE", "1")
                .env("BLI_KVM_GADGET_HID_LEDGER", &ledger)
                .env("BLI_KVM_GADGET_HIDG_DIR", &hidg)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while !socket.exists() {
            assert!(
                child.0.try_wait().unwrap().is_none(),
                "broker exited before ready"
            );
            assert!(Instant::now() < deadline, "broker socket startup timed out");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let client = ControlClient::new(&socket).unwrap();
        let operation = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            client
                .execute_v2(
                    operation,
                    PlatformControlRequestV2::TargetAttest {
                        attestation: TargetAttestation {
                            session_binding: 9,
                            target_binding: [0x11; 32],
                            focus_binding: [0x22; 16],
                            ttl_seconds: 30,
                        },
                    }
                )
                .await
                .unwrap(),
            ControlResponse::Applied,
        );
        let ControlResponse::LeaseGranted { lease_id } = client
            .execute_v2(
                operation,
                PlatformControlRequestV2::HidLease {
                    lease: HidLeaseRequest::Acquire {
                        session_binding: 9,
                        target_binding: [0x11; 32],
                        ttl_seconds: 30,
                    },
                },
            )
            .await
            .unwrap()
        else {
            panic!("lease not granted")
        };
        assert_eq!(
            client
                .execute_v2(
                    operation,
                    PlatformControlRequestV2::HidLease {
                        lease: HidLeaseRequest::Report {
                            lease_id,
                            session_binding: 9,
                            target_binding: [0x11; 32],
                            report: HidReport::Keyboard {
                                data: [0, 0, 4, 0, 0, 0, 0, 0]
                            },
                        },
                    }
                )
                .await
                .unwrap(),
            ControlResponse::Applied
        );
        assert_eq!(fs::read(&keyboard).unwrap(), [0, 0, 4, 0, 0, 0, 0, 0]);
        if fail_release {
            fs::remove_file(&keyboard).unwrap();
        }
        let pid = i32::try_from(child.0.id()).unwrap();
        // SAFETY: kill does not access memory; this PID belongs to our unreaped
        // child. Only this isolated fixture process receives the selected signal.
        assert_eq!(unsafe { libc::kill(pid, signal) }, 0);
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "broker signal shutdown timed out"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        assert_eq!(
            status.success(),
            !fail_release,
            "unexpected shutdown: {status}"
        );
        let pending = bli_kvm_hid_transition::HidReleaseLedger::new(&ledger)
            .unwrap()
            .load()
            .unwrap();
        assert_eq!(!pending.is_empty(), fail_release);
        if !fail_release {
            assert_eq!(fs::read(&keyboard).unwrap(), [0; 8]);
        }
        assert!(!socket.exists());
    }
}
