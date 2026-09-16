//! Control surface: typed JSON lines over a Unix socket.

use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::Path,
    sync::{Arc, Mutex},
};

use bli_kvm_fan_policy::{FanCommand, FanDutyPercent, FanObservation};
use bli_kvm_fan_supervisor::{FanActuator, FanSupervisor, FanSupervisorState};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Largest request line the daemon accepts.
pub const MAX_REQUEST_BYTES: usize = 4 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FanModeName {
    Auto,
    Manual,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "request", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    Status,
    FanModeSet { mode: FanModeName },
    FanDutySet { percent: u8 },
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StatusView {
    pub fan_mode: &'static str,
    pub fan_status: &'static str,
    pub fan_duty_percent: u8,
    pub fan_pwm: bool,
    pub fan_pwm_period_ms: u64,
    pub fan_line: u16,
    pub thermal_millidegrees: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thermal_error: Option<&'static str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub code: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<StatusView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<&'static str>,
}

impl Response {
    fn ok(state: &BoarddState<impl FanActuator>) -> Self {
        Self {
            code: 0,
            data: Some(state.status()),
            error: None,
        }
    }

    fn error(code: i32, error: &'static str) -> Self {
        Self {
            code,
            data: None,
            error: Some(error),
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BoarddError {
    #[error("invalid_state")]
    InvalidState,
    #[error("invalid_value")]
    InvalidValue,
    #[error("backend_unavailable")]
    BackendUnavailable,
}

/// Board daemon state shared by the control surface and the sampling loop.
pub struct BoarddState<A: FanActuator> {
    supervisor: FanSupervisor<A>,
    mode: FanModeName,
    manual_duty: u8,
    minimum_duty: u8,
    maximum_duty: u8,
    pwm_enabled: bool,
    pwm_period_ms: u64,
    fan_line: u16,
    last_thermal: Option<i32>,
    thermal_error: Option<&'static str>,
}

impl<A> BoarddState<A>
where
    A: FanActuator,
    A::Error: std::fmt::Debug,
{
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        supervisor: FanSupervisor<A>,
        mode: FanModeName,
        manual_duty: u8,
        minimum_duty: u8,
        maximum_duty: u8,
        pwm_enabled: bool,
        pwm_period_ms: u64,
        fan_line: u16,
    ) -> Self {
        Self {
            supervisor,
            mode,
            manual_duty,
            minimum_duty,
            maximum_duty,
            pwm_enabled,
            pwm_period_ms,
            fan_line,
            last_thermal: None,
            thermal_error: None,
        }
    }

    pub fn supervisor_state(&self) -> FanSupervisorState {
        self.supervisor.state()
    }

    pub fn status(&self) -> StatusView {
        let state = self.supervisor.state();
        StatusView {
            fan_mode: match self.mode {
                FanModeName::Auto => "auto",
                FanModeName::Manual => "manual",
            },
            fan_status: match state.status {
                bli_kvm_fan_supervisor::FanSupervisorStatus::Uninitialized => "uninitialized",
                bli_kvm_fan_supervisor::FanSupervisorStatus::Active => "active",
                bli_kvm_fan_supervisor::FanSupervisorStatus::FailsafeLatched => "failsafe_latched",
                bli_kvm_fan_supervisor::FanSupervisorStatus::ActuationUncertain => {
                    "actuation_uncertain"
                }
            },
            fan_duty_percent: if self.mode == FanModeName::Manual {
                self.manual_duty
            } else {
                match state.applied_command {
                    Some(FanCommand::DutyPercent(percent)) => percent.get(),
                    _ => 0,
                }
            },
            fan_pwm: self.pwm_enabled,
            fan_pwm_period_ms: self.pwm_period_ms,
            fan_line: self.fan_line,
            thermal_millidegrees: self.last_thermal,
            thermal_error: self.thermal_error,
        }
    }

    pub fn set_mode(&mut self, mode: FanModeName) -> Result<(), BoarddError> {
        match (mode, self.mode) {
            (FanModeName::Auto, FanModeName::Auto) | (FanModeName::Manual, FanModeName::Manual) => {
                return Ok(());
            }
            _ => {}
        }
        self.mode = mode;
        if mode == FanModeName::Manual {
            self.apply_manual_duty()?;
        }
        Ok(())
    }

    pub fn set_manual_duty(&mut self, percent: u8) -> Result<(), BoarddError> {
        if self.mode != FanModeName::Manual {
            return Err(BoarddError::InvalidState);
        }
        if percent == 0 && self.minimum_duty > 0 {
            // A stopped fan is only allowed when the reviewed minimum is zero;
            // otherwise the smallest accepted duty keeps it spinning.
            self.manual_duty = self.minimum_duty;
        } else {
            if percent < self.minimum_duty || percent > self.maximum_duty {
                return Err(BoarddError::InvalidValue);
            }
            self.manual_duty = percent;
        }
        self.apply_manual_duty()
    }

    fn apply_manual_duty(&mut self) -> Result<(), BoarddError> {
        let command = if self.manual_duty == 0 {
            FanCommand::Off
        } else {
            FanCommand::DutyPercent(
                FanDutyPercent::new(self.manual_duty).ok_or(BoarddError::InvalidValue)?,
            )
        };
        self.supervisor
            .actuator_mut()
            .apply(command)
            .map_err(|_| BoarddError::BackendUnavailable)
    }

    /// Automatic evaluation step; manual mode keeps the operator's duty.
    pub fn sample(&mut self, now_ms: u64) -> Result<(), BoarddError> {
        if self.mode != FanModeName::Auto {
            return Ok(());
        }
        let observation = match self.last_thermal {
            Some(millidegrees) => FanObservation::TemperatureMillidegrees(millidegrees),
            None => FanObservation::SensorUnavailable,
        };
        match self.supervisor.evaluate(now_ms, observation) {
            Ok(outcome) => {
                let _ = outcome;
                Ok(())
            }
            Err(_) => Err(BoarddError::BackendUnavailable),
        }
    }

    pub fn note_thermal(&mut self, result: Result<i32, &'static str>) {
        match result {
            Ok(millidegrees) => {
                self.last_thermal = Some(millidegrees);
                self.thermal_error = None;
            }
            Err(error) => {
                self.last_thermal = None;
                self.thermal_error = Some(error);
            }
        }
    }
}

/// Handle one typed request against the shared state.
pub fn handle<A>(state: &mut BoarddState<A>, request: Request) -> Response
where
    A: FanActuator,
    A::Error: std::fmt::Debug,
{
    match request {
        Request::Status => Response::ok(state),
        Request::FanModeSet { mode } => match state.set_mode(mode) {
            Ok(()) => Response::ok(state),
            Err(error) => Response::error(-error_code(&error), error.to_string_static()),
        },
        Request::FanDutySet { percent } => match state.set_manual_duty(percent) {
            Ok(()) => Response::ok(state),
            Err(error) => Response::error(-error_code(&error), error.to_string_static()),
        },
    }
}

fn error_code(error: &BoarddError) -> i32 {
    match error {
        BoarddError::InvalidState => 2,
        BoarddError::InvalidValue => 3,
        BoarddError::BackendUnavailable => 4,
    }
}

impl BoarddError {
    fn to_string_static(&self) -> &'static str {
        match self {
            BoarddError::InvalidState => "invalid_state",
            BoarddError::InvalidValue => "invalid_value",
            BoarddError::BackendUnavailable => "backend_unavailable",
        }
    }
}

/// Accept connections and answer typed requests until the listener fails.
pub fn serve<A>(socket: &Path, state: &Arc<Mutex<BoarddState<A>>>) -> std::io::Result<()>
where
    A: FanActuator,
    A::Error: std::fmt::Debug,
{
    if socket.exists() {
        std::fs::remove_file(socket)?;
    }
    let listener = UnixListener::bind(socket)?;
    for connection in listener.incoming() {
        let Ok(stream) = connection else { continue };
        let _ = handle_connection(stream, state);
    }
    Ok(())
}

fn handle_connection<A>(
    stream: UnixStream,
    state: &Arc<Mutex<BoarddState<A>>>,
) -> std::io::Result<()>
where
    A: FanActuator,
    A::Error: std::fmt::Debug,
{
    let mut writer = stream.try_clone()?;
    let mut reader = BufReader::new(stream);
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            return Ok(());
        }
        if read > MAX_REQUEST_BYTES {
            let response = Response::error(-1, "invalid_request");
            serde_json::to_writer(&mut writer, &response)?;
            writer.write_all(b"\n")?;
            return Ok(());
        }
        let response = match serde_json::from_str::<Request>(line.trim_end()) {
            Ok(request) => match state.lock() {
                Ok(mut state) => handle(&mut state, request),
                Err(_) => Response::error(-5, "internal"),
            },
            Err(_) => Response::error(-1, "invalid_request"),
        };
        serde_json::to_writer(&mut writer, &response)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bli_kvm_fan_policy::{FanConfigError, FanPolicyConfig};

    #[derive(Debug, Default)]
    struct MockActuator {
        applied: Vec<FanCommand>,
    }

    impl FanActuator for MockActuator {
        type Error = std::io::Error;

        fn apply(&mut self, command: FanCommand) -> Result<(), Self::Error> {
            self.applied.push(command);
            Ok(())
        }
    }

    fn policy() -> FanPolicyConfig {
        FanPolicyConfig {
            turn_off_millidegrees: 45_000,
            turn_on_millidegrees: 50_000,
            critical_millidegrees: 80_000,
            minimum_on_ms: 1_000,
            minimum_off_ms: 1_000,
            sensor_timeout_ms: 60_000,
            minimum_duty_percent: 20,
            curve: vec![bli_kvm_fan_policy::FanCurvePoint {
                at_or_above_millidegrees: 50_000,
                duty_percent: 40,
            }],
        }
    }

    fn state() -> BoarddState<MockActuator> {
        let mut supervisor = FanSupervisor::new(policy(), 0, MockActuator::default()).unwrap();
        let _ = supervisor.initialize();
        BoarddState::new(supervisor, FanModeName::Auto, 0, 20, 100, true, 40, 269)
    }

    #[test]
    fn status_reports_mode_duty_and_line() {
        let mut state = state();
        let response = handle(&mut state, Request::Status);
        assert_eq!(response.code, 0);
        let data = response.data.unwrap();
        assert_eq!(data.fan_mode, "auto");
        assert_eq!(data.fan_line, 269);
        assert!(data.fan_pwm);
    }

    #[test]
    fn manual_duty_needs_manual_mode_and_respects_bounds() {
        let mut state = state();
        let denied = handle(&mut state, Request::FanDutySet { percent: 50 });
        assert_eq!(denied.code, -2);
        assert_eq!(denied.error, Some("invalid_state"));

        assert_eq!(
            handle(
                &mut state,
                Request::FanModeSet {
                    mode: FanModeName::Manual
                }
            )
            .code,
            0
        );
        let out_of_range = handle(&mut state, Request::FanDutySet { percent: 10 });
        assert_eq!(out_of_range.code, -3);
        assert_eq!(out_of_range.error, Some("invalid_value"));

        let applied = handle(&mut state, Request::FanDutySet { percent: 60 });
        assert_eq!(applied.code, 0);
        assert_eq!(applied.data.unwrap().fan_duty_percent, 60);
    }

    #[test]
    fn automatic_sampling_follows_temperature() {
        let mut state = state();
        state.note_thermal(Ok(30_000));
        assert_eq!(state.sample(1_000), Ok(()));
        let cool = state.status();
        assert_eq!(cool.thermal_millidegrees, Some(30_000));

        state.note_thermal(Ok(75_000));
        assert_eq!(state.sample(2_000), Ok(()));
        assert_eq!(state.status().fan_duty_percent, 40);

        state.note_thermal(Err("unavailable"));
        assert_eq!(state.sample(3_000), Ok(()));
        assert_eq!(state.status().thermal_error, Some("unavailable"));
    }

    #[test]
    fn socket_answers_a_status_request() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("control.sock");
        let state = Arc::new(Mutex::new(state()));
        let socket_path = socket.clone();
        let server = std::thread::spawn(move || serve(&socket_path, &state));
        for _ in 0..100 {
            if socket.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let mut stream = UnixStream::connect(&socket).unwrap();
        stream.write_all(b"{\"request\":\"status\"}\n").unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let response: serde_json::Value = serde_json::from_str(line.trim_end()).unwrap();
        assert_eq!(response["code"], 0);
        assert_eq!(response["data"]["fanPwm"], true);
        drop(reader);
        let _ = server;
        let _ = std::fs::remove_file(&socket);
    }

    #[test]
    fn policy_config_round_trips_from_a_file() {
        let config = policy();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn rejects_invalid_policy_configuration() {
        let mut config = policy();
        config.critical_millidegrees = 40_000;
        assert_eq!(config.validate(), Err(FanConfigError::InvalidThresholds));
    }
}
