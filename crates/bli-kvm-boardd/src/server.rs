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

/// Display operations the control surface exposes. The daemon holds this as a
/// trait object so the panel runtime stays behind the board configuration and
/// the host tests can use a mock.
pub trait DisplayControl: Send {
    fn test_pattern(&mut self) -> Result<(), &'static str>;
    fn set_backlight(&mut self, on: bool) -> Result<(), &'static str>;
    fn is_initialized(&self) -> bool;
    fn backlight_on(&self) -> bool;
}

/// Display button sampling the daemon drives from its sampling thread.
pub trait ButtonControl: Send {
    /// Sample the line once; `true` means a wake cycle was requested.
    fn poll(&mut self, now_ms: u64) -> Result<bool, &'static str>;
    fn wakeups(&self) -> u64;
}

/// ATX host LED inputs the status view reports.
pub trait AtxLedControl: Send {
    fn sample(&mut self) -> Result<(), &'static str>;
    fn power_present(&self) -> bool;
    fn power_on(&self) -> bool;
    fn hdd_present(&self) -> bool;
    fn hdd_on(&self) -> bool;
}

/// Activity LED operations the control surface exposes.
pub trait LedControl: Send {
    fn set_enabled(&mut self, enabled: bool, now_ms: u64) -> Result<(), &'static str>;
    fn enabled(&self) -> bool;
    fn level(&self) -> bool;
}

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
    DisplayTest,
    DisplayBacklightSet { on: bool },
    LedEnabledSet { enabled: bool },
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
    pub display_present: bool,
    pub display_initialized: bool,
    pub display_backlight_on: bool,
    pub led_present: bool,
    pub led_enabled: bool,
    pub led_level: bool,
    pub button_present: bool,
    pub button_wakeups: u64,
    pub atx_power_led_present: bool,
    pub atx_power_led_on: bool,
    pub atx_hdd_led_present: bool,
    pub atx_hdd_led_on: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_rtc_seconds: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_rtc_offset_seconds: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_rtc_error: Option<&'static str>,
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
    external_rtc: Option<i64>,
    external_rtc_offset: Option<i64>,
    external_rtc_error: Option<&'static str>,
    display: Option<Box<dyn DisplayControl>>,
    led: Option<Box<dyn LedControl>>,
    button: Option<Box<dyn ButtonControl>>,
    atx_leds: Option<Box<dyn AtxLedControl>>,
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
        display: Option<Box<dyn DisplayControl>>,
        led: Option<Box<dyn LedControl>>,
        button: Option<Box<dyn ButtonControl>>,
        atx_leds: Option<Box<dyn AtxLedControl>>,
    ) -> Self {
        Self {
            display,
            led,
            button,
            atx_leds,
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
            external_rtc: None,
            external_rtc_offset: None,
            external_rtc_error: None,
        }
    }

    pub fn has_display(&self) -> bool {
        self.display.is_some()
    }

    fn display(&mut self) -> Result<&mut (dyn DisplayControl + 'static), BoarddError> {
        match self.display.as_mut() {
            Some(display) => Ok(display.as_mut()),
            None => Err(BoarddError::InvalidState),
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
            display_present: self.display.is_some(),
            display_initialized: self
                .display
                .as_ref()
                .is_some_and(|display| display.is_initialized()),
            display_backlight_on: self
                .display
                .as_ref()
                .is_some_and(|display| display.backlight_on()),
            led_present: self.led.is_some(),
            led_enabled: self.led.as_ref().is_some_and(|led| led.enabled()),
            led_level: self.led.as_ref().is_some_and(|led| led.level()),
            button_present: self.button.is_some(),
            button_wakeups: self.button.as_ref().map_or(0, |button| button.wakeups()),
            atx_power_led_present: self
                .atx_leds
                .as_ref()
                .is_some_and(|leds| leds.power_present()),
            atx_power_led_on: self.atx_leds.as_ref().is_some_and(|leds| leds.power_on()),
            atx_hdd_led_present: self
                .atx_leds
                .as_ref()
                .is_some_and(|leds| leds.hdd_present()),
            atx_hdd_led_on: self.atx_leds.as_ref().is_some_and(|leds| leds.hdd_on()),
            external_rtc_seconds: self.external_rtc,
            external_rtc_offset_seconds: self.external_rtc_offset,
            external_rtc_error: self.external_rtc_error,
        }
    }

    /// Record one external RTC observation for the status view.
    pub fn note_external_rtc(&mut self, result: Result<(i64, i64), &'static str>) {
        match result {
            Ok((seconds, offset)) => {
                self.external_rtc = Some(seconds);
                self.external_rtc_offset = Some(offset);
                self.external_rtc_error = None;
            }
            Err(error) => {
                self.external_rtc = None;
                self.external_rtc_offset = None;
                self.external_rtc_error = Some(error);
            }
        }
    }

    pub fn has_led(&self) -> bool {
        self.led.is_some()
    }

    pub fn set_led_enabled(&mut self, enabled: bool) -> Result<(), BoarddError> {
        let Some(led) = self.led.as_mut() else {
            return Err(BoarddError::InvalidState);
        };
        led.set_enabled(enabled, unix_millis_now())
            .map_err(|kind| match kind {
                "invalid_state" => BoarddError::InvalidState,
                _ => BoarddError::BackendUnavailable,
            })
    }

    /// Sample the display button; a wake cycle lights the panel again.
    pub fn tick_button(&mut self) {
        let Some(button) = self.button.as_mut() else {
            return;
        };
        if !matches!(button.poll(unix_millis_now()), Ok(true)) {
            return;
        }
        if let Some(display) = self.display.as_mut()
            && !display.backlight_on()
        {
            let _ = display.set_backlight(true);
        }
    }

    /// Sample the ATX host LED inputs; called by the sampling thread.
    pub fn tick_atx_leds(&mut self) {
        let Some(atx_leds) = self.atx_leds.as_mut() else {
            return;
        };
        let _ = atx_leds.sample();
    }

    /// Drive the LED pulse forward; called by the sampling thread.
    pub fn tick_led(&mut self) {
        let Some(led) = self.led.as_mut() else {
            return;
        };
        let enabled = led.enabled();
        let _ = led.set_enabled(enabled, unix_millis_now());
    }

    pub fn draw_display_test(&mut self) -> Result<(), BoarddError> {
        let display = self.display()?;
        display.test_pattern().map_err(|kind| match kind {
            "invalid_state" => BoarddError::InvalidState,
            _ => BoarddError::BackendUnavailable,
        })
    }

    pub fn set_display_backlight(&mut self, on: bool) -> Result<(), BoarddError> {
        let display = self.display()?;
        display.set_backlight(on).map_err(|kind| match kind {
            "invalid_state" => BoarddError::InvalidState,
            _ => BoarddError::BackendUnavailable,
        })
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
        Request::DisplayTest => match state.draw_display_test() {
            Ok(()) => Response::ok(state),
            Err(error) => Response::error(-error_code(&error), error.to_string_static()),
        },
        Request::DisplayBacklightSet { on } => match state.set_display_backlight(on) {
            Ok(()) => Response::ok(state),
            Err(error) => Response::error(-error_code(&error), error.to_string_static()),
        },
        Request::LedEnabledSet { enabled } => match state.set_led_enabled(enabled) {
            Ok(()) => Response::ok(state),
            Err(error) => Response::error(-error_code(&error), error.to_string_static()),
        },
    }
}

fn unix_millis_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
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
        BoarddState::new(
            supervisor,
            FanModeName::Auto,
            0,
            20,
            100,
            true,
            40,
            269,
            None,
            None,
            None,
            None,
        )
    }

    #[derive(Default)]
    struct MockDisplay {
        initialized: bool,
        backlight: bool,
        fail: bool,
        patterns: usize,
    }

    impl DisplayControl for MockDisplay {
        fn test_pattern(&mut self) -> Result<(), &'static str> {
            if self.fail {
                return Err("backend_unavailable");
            }
            if !self.initialized {
                return Err("invalid_state");
            }
            self.patterns += 1;
            Ok(())
        }

        fn set_backlight(&mut self, on: bool) -> Result<(), &'static str> {
            if self.fail {
                return Err("backend_unavailable");
            }
            if !self.initialized && on {
                return Err("invalid_state");
            }
            self.backlight = on;
            Ok(())
        }

        fn is_initialized(&self) -> bool {
            self.initialized
        }

        fn backlight_on(&self) -> bool {
            self.backlight
        }
    }

    fn state_with_display(display: MockDisplay) -> BoarddState<MockActuator> {
        let mut supervisor = FanSupervisor::new(policy(), 0, MockActuator::default()).unwrap();
        let _ = supervisor.initialize();
        BoarddState::new(
            supervisor,
            FanModeName::Auto,
            0,
            20,
            100,
            true,
            40,
            269,
            Some(Box::new(display)),
            None,
            None,
            None,
        )
    }

    #[test]
    fn display_requests_reach_the_panel_runtime() {
        let mut state = state_with_display(MockDisplay {
            initialized: true,
            ..MockDisplay::default()
        });
        assert!(state.status().display_present);
        assert_eq!(handle(&mut state, Request::DisplayTest).code, 0);
        let response = handle(&mut state, Request::DisplayBacklightSet { on: false });
        assert_eq!(response.code, 0);
        let data = response.data.unwrap();
        assert!(!data.display_backlight_on);
        assert!(data.display_initialized);
    }

    #[derive(Default)]
    struct MockLed {
        enabled: bool,
        level: bool,
        fail: bool,
    }

    impl LedControl for MockLed {
        fn set_enabled(&mut self, enabled: bool, _now_ms: u64) -> Result<(), &'static str> {
            if self.fail {
                return Err("backend_unavailable");
            }
            self.enabled = enabled;
            self.level = enabled;
            Ok(())
        }

        fn enabled(&self) -> bool {
            self.enabled
        }

        fn level(&self) -> bool {
            self.level
        }
    }

    fn state_with_led(led: MockLed) -> BoarddState<MockActuator> {
        let mut supervisor = FanSupervisor::new(policy(), 0, MockActuator::default()).unwrap();
        let _ = supervisor.initialize();
        BoarddState::new(
            supervisor,
            FanModeName::Auto,
            0,
            20,
            100,
            true,
            40,
            269,
            None,
            Some(Box::new(led)),
            None,
            None,
        )
    }

    #[test]
    fn led_request_reaches_the_pulse_runtime() {
        let mut state = state_with_led(MockLed {
            enabled: true,
            level: true,
            ..MockLed::default()
        });
        assert!(state.status().led_present);
        assert!(state.status().led_enabled);
        let response = handle(&mut state, Request::LedEnabledSet { enabled: false });
        assert_eq!(response.code, 0);
        let data = response.data.unwrap();
        assert!(!data.led_enabled);
        assert!(!data.led_level);

        let mut without = self::state();
        assert!(!without.status().led_present);
        assert_eq!(
            handle(&mut without, Request::LedEnabledSet { enabled: true }).code,
            -2
        );

        let mut broken = state_with_led(MockLed {
            fail: true,
            ..MockLed::default()
        });
        assert_eq!(
            handle(&mut broken, Request::LedEnabledSet { enabled: true }).code,
            -4
        );
    }

    #[derive(Default)]
    struct MockButton {
        wakeups: u64,
        wake_next: bool,
        fail: bool,
    }

    impl ButtonControl for MockButton {
        fn poll(&mut self, _now_ms: u64) -> Result<bool, &'static str> {
            if self.fail {
                return Err("backend_unavailable");
            }
            if self.wake_next {
                self.wakeups += 1;
                self.wake_next = false;
                return Ok(true);
            }
            Ok(false)
        }

        fn wakeups(&self) -> u64 {
            self.wakeups
        }
    }

    fn state_with_button(
        button: MockButton,
        display: Option<MockDisplay>,
    ) -> BoarddState<MockActuator> {
        let mut supervisor = FanSupervisor::new(policy(), 0, MockActuator::default()).unwrap();
        let _ = supervisor.initialize();
        BoarddState::new(
            supervisor,
            FanModeName::Auto,
            0,
            20,
            100,
            true,
            40,
            269,
            display.map(|display| Box::new(display) as Box<dyn DisplayControl>),
            None,
            Some(Box::new(button)),
            None,
        )
    }

    #[test]
    fn external_rtc_observation_reaches_the_status() {
        let mut state = state();
        assert_eq!(state.status().external_rtc_seconds, None);
        state.note_external_rtc(Ok((1_760_000_000, -12)));
        let data = state.status();
        assert_eq!(data.external_rtc_seconds, Some(1_760_000_000));
        assert_eq!(data.external_rtc_offset_seconds, Some(-12));
        assert_eq!(data.external_rtc_error, None);

        state.note_external_rtc(Err("unavailable"));
        let data = state.status();
        assert_eq!(data.external_rtc_seconds, None);
        assert_eq!(data.external_rtc_error, Some("unavailable"));
    }

    #[test]
    fn a_button_wake_lights_the_panel_again() {
        let mut state = state_with_button(
            MockButton {
                wake_next: true,
                ..MockButton::default()
            },
            Some(MockDisplay {
                initialized: true,
                backlight: false,
                ..MockDisplay::default()
            }),
        );
        assert!(state.status().button_present);
        assert!(!state.status().display_backlight_on);
        state.tick_button();
        let data = state.status();
        assert_eq!(data.button_wakeups, 1);
        assert!(data.display_backlight_on, "wake must light the panel");

        // A wake without a panel is counted and never panics.
        let mut bare = state_with_button(
            MockButton {
                wake_next: true,
                ..MockButton::default()
            },
            None,
        );
        bare.tick_button();
        assert_eq!(bare.status().button_wakeups, 1);
        assert!(!bare.status().display_present);
    }

    #[test]
    fn display_requests_fail_closed_without_or_on_a_broken_panel() {
        let mut state = state();
        assert!(!state.status().display_present);
        assert_eq!(handle(&mut state, Request::DisplayTest).code, -2);

        let mut broken = state_with_display(MockDisplay {
            initialized: true,
            fail: true,
            ..MockDisplay::default()
        });
        assert_eq!(handle(&mut broken, Request::DisplayTest).code, -4);

        let mut uninitialized = state_with_display(MockDisplay::default());
        assert_eq!(handle(&mut uninitialized, Request::DisplayTest).code, -2);
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
