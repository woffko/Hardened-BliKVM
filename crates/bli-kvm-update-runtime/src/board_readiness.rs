//! Read-only board daemon readiness against reviewed system requirements.

use serde::Deserialize;
use std::{io, path::Path, time::Instant};

/// Values must come from the reviewed system configuration, not candidate data
/// or the response being checked. Optional peripherals are explicitly selected.
#[derive(Debug, Clone, Copy)]
pub struct BoardRequirements {
    pub fan_line: u16,
    pub fan_pwm_period_ms: u64,
    pub display: bool,
    pub led: bool,
    pub button: bool,
    pub atx_power_led: bool,
    pub atx_hdd_led: bool,
    pub external_rtc: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FanMode {
    Auto,
    Manual,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FanStatus {
    Uninitialized,
    Active,
    FailsafeLatched,
    ActuationUncertain,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BoardStatus {
    #[serde(rename = "fanMode")]
    _fan_mode: FanMode,
    fan_status: FanStatus,
    fan_duty_percent: u8,
    fan_pwm: bool,
    fan_pwm_period_ms: u64,
    fan_line: u16,
    thermal_millidegrees: Option<i32>,
    thermal_error: Option<String>,
    display_present: bool,
    display_initialized: bool,
    display_backlight_on: bool,
    led_present: bool,
    led_enabled: bool,
    led_level: bool,
    button_present: bool,
    button_wakeups: u64,
    atx_power_led_present: bool,
    atx_power_led_on: bool,
    atx_hdd_led_present: bool,
    atx_hdd_led_on: bool,
    external_rtc_seconds: Option<i64>,
    external_rtc_offset_seconds: Option<i64>,
    external_rtc_error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    code: i32,
    data: Option<BoardStatus>,
    error: Option<String>,
}

fn not_ready() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "board daemon readiness rejected",
    )
}

fn validate(response: &[u8], expected: BoardRequirements) -> io::Result<()> {
    let response: Response = serde_json::from_slice(response).map_err(|_| not_ready())?;
    if response.code != 0 || response.error.is_some() {
        return Err(not_ready());
    }
    let state = response.data.ok_or_else(not_ready)?;
    // Both typed fan modes are supported; status/duty here are daemon reports,
    // not proof of measured RPM or the physical PWM waveform.
    if !matches!(
        state.fan_status,
        FanStatus::Active | FanStatus::FailsafeLatched
    ) || !state.fan_pwm
        || state.fan_duty_percent > 100
        // FanSupervisor latches failsafe only after applying maximum duty.
        || (matches!(state.fan_status, FanStatus::FailsafeLatched)
            && state.fan_duty_percent != 100)
        || state.fan_line != expected.fan_line
        || state.fan_pwm_period_ms != expected.fan_pwm_period_ms
        || state.thermal_millidegrees.is_none()
        || state.thermal_error.is_some()
        || (expected.display && (!state.display_present || !state.display_initialized))
        || (expected.led && !state.led_present)
        || (expected.button && !state.button_present)
        || (expected.atx_power_led && !state.atx_power_led_present)
        || (expected.atx_hdd_led && !state.atx_hdd_led_present)
        || (expected.external_rtc
            && (state.external_rtc_seconds.is_none()
                || state.external_rtc_offset_seconds.is_none()
                || state.external_rtc_error.is_some()))
        || (!state.display_present && (state.display_initialized || state.display_backlight_on))
        || (!state.led_present && (state.led_enabled || state.led_level))
        || (!state.button_present && state.button_wakeups != 0)
        || (!state.atx_power_led_present && state.atx_power_led_on)
        || (!state.atx_hdd_led_present && state.atx_hdd_led_on)
    {
        return Err(not_ready());
    }
    Ok(())
}

/// Probe only Status; never changes fan duty, LEDs, panel or ATX outputs.
/// The enclosing native identity fence must verify the supplied PID's slot
/// executable and start-time before and after this operation.
pub fn probe(
    socket: &Path,
    expected_pid: u32,
    expected: BoardRequirements,
    deadline: Instant,
) -> io::Result<()> {
    let response = crate::native_ipc::exchange_line(
        socket,
        expected_pid,
        b"{\"request\":\"status\"}\n",
        deadline,
    )?;
    validate(&response, expected)?;
    if Instant::now() >= deadline {
        return Err(io::ErrorKind::TimedOut.into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bli_kvm_boardd::server::{Response, StatusView};

    fn requirements() -> BoardRequirements {
        BoardRequirements {
            fan_line: 269,
            fan_pwm_period_ms: 40,
            display: true,
            led: true,
            button: true,
            atx_power_led: true,
            atx_hdd_led: true,
            external_rtc: true,
        }
    }

    fn response() -> Vec<u8> {
        serde_json::to_vec(&Response {
            code: 0,
            error: None,
            data: Some(StatusView {
                fan_mode: "auto",
                fan_status: "active",
                fan_duty_percent: 40,
                fan_pwm: true,
                fan_pwm_period_ms: 40,
                fan_line: 269,
                thermal_millidegrees: Some(50_000),
                thermal_error: None,
                display_present: true,
                display_initialized: true,
                display_backlight_on: false,
                led_present: true,
                led_enabled: true,
                led_level: false,
                button_present: true,
                button_wakeups: 0,
                atx_power_led_present: true,
                atx_power_led_on: false,
                atx_hdd_led_present: true,
                atx_hdd_led_on: false,
                external_rtc_seconds: Some(1_000),
                external_rtc_offset_seconds: Some(0),
                external_rtc_error: None,
            }),
        })
        .unwrap()
    }

    #[test]
    fn observed_h616_status_without_external_rtc_is_ready_only_when_rtc_is_optional() {
        // Status captured read-only from the H616 test board on 2026-09-24.
        let observed = br#"{"code":0,"data":{"fanMode":"auto","fanStatus":"active","fanDutyPercent":30,"fanPwm":true,"fanPwmPeriodMs":40,"fanLine":269,"thermalMillidegrees":50606,"displayPresent":true,"displayInitialized":true,"displayBacklightOn":true,"ledPresent":true,"ledEnabled":true,"ledLevel":true,"buttonPresent":true,"buttonWakeups":0,"atxPowerLedPresent":true,"atxPowerLedOn":false,"atxHddLedPresent":true,"atxHddLedOn":false,"externalRtcError":"unavailable"}}"#;
        assert!(validate(observed, requirements()).is_err());
        validate(
            observed,
            BoardRequirements {
                external_rtc: false,
                ..requirements()
            },
        )
        .unwrap();
    }

    #[test]
    fn real_board_serializer_contract_rejects_unhealthy_or_incompatible_states() {
        let bytes = response();
        validate(&bytes, requirements()).unwrap();
        for (key, value) in [
            ("fanStatus", serde_json::json!("actuation_uncertain")),
            ("fanStatus", serde_json::json!("uninitialized")),
            ("fanMode", serde_json::json!("unknown")),
            ("fanDutyPercent", serde_json::json!(101)),
            ("fanPwm", serde_json::json!(false)),
            ("fanLine", serde_json::json!(1)),
            ("fanPwmPeriodMs", serde_json::json!(1)),
            ("thermalMillidegrees", serde_json::Value::Null),
            ("thermalError", serde_json::json!("unavailable")),
            ("displayInitialized", serde_json::json!(false)),
            ("ledPresent", serde_json::json!(false)),
            ("buttonPresent", serde_json::json!(false)),
            ("atxPowerLedPresent", serde_json::json!(false)),
            ("atxHddLedPresent", serde_json::json!(false)),
            ("externalRtcError", serde_json::json!("unavailable")),
        ] {
            let mut changed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            changed["data"][key] = value;
            assert!(
                validate(&serde_json::to_vec(&changed).unwrap(), requirements()).is_err(),
                "{key}"
            );
        }
        for bytes in [
            b"{\"code\":0}".as_slice(),
            b"{\"code\":1}",
            b"{\"code\":0,\"code\":0}",
        ] {
            assert!(validate(bytes, requirements()).is_err());
        }
    }

    #[test]
    fn latched_failsafe_requires_maximum_reported_duty() {
        let mut state: serde_json::Value = serde_json::from_slice(&response()).unwrap();
        state["data"]["fanStatus"] = serde_json::json!("failsafe_latched");
        for duty in [0, 40, 99, 100, 101] {
            state["data"]["fanDutyPercent"] = serde_json::json!(duty);
            assert_eq!(
                validate(&serde_json::to_vec(&state).unwrap(), requirements()).is_ok(),
                duty == 100
            );
        }
    }

    #[test]
    fn peer_bound_probe_sends_only_the_real_status_request() {
        use std::{
            io::{Read, Write},
            os::unix::net::UnixListener,
            thread,
            time::Duration,
        };
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("board.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let worker = thread::spawn(move || {
            let mut stream = listener.accept().unwrap().0;
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            stream.read_to_end(&mut request).unwrap();
            assert!(matches!(
                serde_json::from_slice::<bli_kvm_boardd::server::Request>(&request).unwrap(),
                bli_kvm_boardd::server::Request::Status
            ));
            let mut response = response();
            response.push(b'\n');
            stream.write_all(&response).unwrap();
        });
        probe(
            &socket,
            std::process::id(),
            requirements(),
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap();
        worker.join().unwrap();
    }
}
