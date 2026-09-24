//! Typed, read-only owner IPC compatibility/readback probes.
//! These do not attest HID reconciliation or measured capture health.

use crate::board_readiness::BoardRequirements;
use bli_kvm_control_protocol::{
    CONTROL_SCHEMA_VERSION_V2, ControlRequestEnvelopeV2, ControlResponse, decode_response_v2,
    encode_request_v2,
};
use bli_kvm_core::{PlatformControlRequestV2, VideoSettingsChange};
use rand_core::{OsRng, RngCore};
use std::{io, path::Path, time::Instant};

#[derive(Debug, PartialEq, Eq)]
pub struct OwnerReadback {
    pub hid_only: bool,
    pub fps: u16,
    pub quality: u8,
    pub resolution: String,
    pub power_led_on: bool,
    pub hdd_led_on: bool,
}

fn rejected() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "owner readiness response rejected",
    )
}

fn exchange(
    socket: &Path,
    pid: u32,
    request_id: u64,
    request: PlatformControlRequestV2,
    deadline: Instant,
) -> io::Result<ControlResponse> {
    if Instant::now() >= deadline {
        return Err(io::ErrorKind::TimedOut.into());
    }
    let mut nonce = [0_u8; 16];
    OsRng
        .try_fill_bytes(&mut nonce)
        .map_err(|_| io::Error::other("owner probe identity unavailable"))?;
    let operation_id: String = nonce.iter().map(|byte| format!("{byte:02x}")).collect();
    let request = ControlRequestEnvelopeV2 {
        schema_version: CONTROL_SCHEMA_VERSION_V2,
        request_id,
        operation_id,
        request,
    };
    let payload = encode_request_v2(&request).map_err(|_| rejected())?;
    let response = crate::native_ipc::exchange_control_frame(socket, pid, &payload, deadline)?;
    let response = decode_response_v2(&response).map_err(|_| rejected())?;
    if response.request_id != request.request_id || response.operation_id != request.operation_id {
        return Err(rejected());
    }
    if Instant::now() >= deadline {
        return Err(io::ErrorKind::TimedOut.into());
    }
    Ok(response.response)
}

/// Uses only the closed readback requests below, with fresh operation IDs and
/// no replay. Surround with the native slot identity fence. Requirements must
/// derive from reviewed system configuration. Preserved user settings may be
/// either HID mode; desired video values must satisfy the existing core contract.
pub fn probe_readbacks(
    socket: &Path,
    pid: u32,
    requirements: BoardRequirements,
    deadline: Instant,
) -> io::Result<OwnerReadback> {
    let ControlResponse::HidOnlyState { enabled: hid_only } = exchange(
        socket,
        pid,
        1,
        PlatformControlRequestV2::HidOnlyState {},
        deadline,
    )?
    else {
        return Err(rejected());
    };
    let ControlResponse::VideoState {
        fps,
        quality,
        resolution: Some(resolution),
    } = exchange(
        socket,
        pid,
        2,
        PlatformControlRequestV2::VideoSettingsState {},
        deadline,
    )?
    else {
        return Err(rejected());
    };
    VideoSettingsChange::new(Some(fps), Some(quality))
        .with_resolution(&resolution)
        .validate()
        .map_err(|_| rejected())?;
    let ControlResponse::BoardStatus {
        power_led_present,
        power_led_on,
        hdd_led_present,
        hdd_led_on,
    } = exchange(
        socket,
        pid,
        3,
        PlatformControlRequestV2::BoardStatus {},
        deadline,
    )?
    else {
        return Err(rejected());
    };
    if (requirements.atx_power_led && !power_led_present)
        || (requirements.atx_hdd_led && !hdd_led_present)
        || (!power_led_present && power_led_on)
        || (!hdd_led_present && hdd_led_on)
    {
        return Err(rejected());
    }
    let ControlResponse::HidReconciliationState {
        pending_releases: 0,
    } = exchange(
        socket,
        pid,
        4,
        PlatformControlRequestV2::HidReconciliationState {},
        deadline,
    )?
    else {
        return Err(rejected());
    };
    Ok(OwnerReadback {
        hid_only,
        fps,
        quality,
        resolution,
        power_led_on,
        hdd_led_on,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bli_kvm_control_protocol::{
        ControlRejectionCode, ControlResponseEnvelope, decode_frame, decode_request_v2,
        encode_frame,
    };
    use std::{
        io::{Read, Write},
        os::unix::net::UnixListener,
        thread,
        time::Duration,
    };

    #[test]
    fn real_protocol_readbacks_require_exact_type_version_and_correlation() {
        // Each rejection must stop at that request, rather than sending the next
        // probe. Fixtures use the actual owner protocol decoder and frame encoder.
        for case in 0..10 {
            let directory = tempfile::tempdir().unwrap();
            let socket = directory.path().join("owner.sock");
            let listener = UnixListener::bind(&socket).unwrap();
            let count = if case == 0 || case == 9 {
                4
            } else if case == 8 {
                3
            } else if case == 6 || case == 7 {
                2
            } else {
                1
            };
            let worker = thread::spawn(move || {
                for id in 1..=count {
                    let mut stream = listener.accept().unwrap().0;
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut frame = Vec::new();
                    stream.read_to_end(&mut frame).unwrap();
                    let request = decode_request_v2(decode_frame(&frame).unwrap()).unwrap();
                    assert_eq!(request.request_id, id);
                    let response = match (id, request.request) {
                        (1, PlatformControlRequestV2::HidOnlyState {}) => {
                            ControlResponse::HidOnlyState { enabled: false }
                        }
                        (2, PlatformControlRequestV2::VideoSettingsState {}) => {
                            ControlResponse::VideoState {
                                fps: if case == 6 { 0 } else { 40 },
                                quality: 80,
                                resolution: if case == 7 {
                                    None
                                } else {
                                    Some("1280x720".into())
                                },
                            }
                        }
                        (3, PlatformControlRequestV2::BoardStatus {}) => {
                            ControlResponse::BoardStatus {
                                power_led_present: case != 8,
                                power_led_on: false,
                                hdd_led_present: true,
                                hdd_led_on: false,
                            }
                        }
                        (4, PlatformControlRequestV2::HidReconciliationState {}) => {
                            ControlResponse::HidReconciliationState {
                                pending_releases: u64::from(case == 9),
                            }
                        }
                        _ => panic!("unexpected or mutating readiness request"),
                    };
                    let mut reply = ControlResponseEnvelope {
                        schema_version: CONTROL_SCHEMA_VERSION_V2,
                        request_id: id,
                        operation_id: request.operation_id,
                        response,
                    };
                    match case {
                        1 => reply.schema_version = 1,
                        2 => reply.request_id += 1,
                        3 => {
                            let replacement = if reply.operation_id.starts_with('a') {
                                "b"
                            } else {
                                "a"
                            };
                            reply.operation_id.replace_range(..1, replacement);
                        }
                        4 => {
                            reply.response = ControlResponse::Rejected {
                                code: ControlRejectionCode::BackendUnavailable,
                            }
                        }
                        5 => reply.response = ControlResponse::Applied,
                        _ => {}
                    }
                    // Intentionally bypass response validation for malformed cases.
                    stream
                        .write_all(&encode_frame(&serde_json::to_vec(&reply).unwrap()).unwrap())
                        .unwrap();
                }
            });
            let requirements = BoardRequirements {
                fan_line: 269,
                fan_pwm_period_ms: 40,
                display: true,
                led: true,
                button: true,
                atx_power_led: true,
                atx_hdd_led: true,
                external_rtc: true,
            };
            let result = probe_readbacks(
                &socket,
                std::process::id(),
                requirements,
                Instant::now() + Duration::from_secs(2),
            );
            if case == 0 {
                assert_eq!(
                    result.unwrap(),
                    OwnerReadback {
                        hid_only: false,
                        fps: 40,
                        quality: 80,
                        resolution: "1280x720".into(),
                        power_led_on: false,
                        hdd_led_on: false
                    }
                );
            } else {
                assert!(result.is_err(), "case {case}");
            }
            worker.join().unwrap();
        }
    }
}
