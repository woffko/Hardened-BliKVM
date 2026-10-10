//! Control surface: typed JSON lines over a Unix socket.

use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::{
        fs::{FileTypeExt, MetadataExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use bli_kvm_fan_policy::{FanCommand, FanDutyPercent, FanObservation};
use bli_kvm_fan_supervisor::{FanActuator, FanSupervisor, FanSupervisorState};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Largest request line the daemon accepts.
pub const MAX_REQUEST_BYTES: usize = 4 * 1024;
const CONNECTION_BUDGET: Duration = Duration::from_secs(2);

struct DeadlineStream {
    stream: UnixStream,
    deadline: Instant,
}

fn remaining_time(deadline: Instant) -> std::io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or_else(|| std::io::ErrorKind::TimedOut.into())
}

impl Read for DeadlineStream {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.stream
            .set_read_timeout(Some(remaining_time(self.deadline)?))?;
        self.stream.read(bytes)
    }
}

impl Write for DeadlineStream {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.stream
            .set_write_timeout(Some(remaining_time(self.deadline)?))?;
        self.stream.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        remaining_time(self.deadline)?;
        self.stream.flush()
    }
}

/// Display operations the control surface exposes. The daemon holds this as a
/// trait object so the panel runtime stays behind the board configuration and
/// the host tests can use a mock.
pub trait DisplayControl: Send {
    fn test_pattern(&mut self) -> Result<(), &'static str>;
    /// Draw the status screen (host name, address, temperature and so on).
    fn status(&mut self) -> Result<(), &'static str>;
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
    display_status_interval_ms: u64,
    last_display_status_ms: u64,
    /// The owner's file with the display's sleep time, read on every tick.
    display_sleep_path: Option<PathBuf>,
    display_sleep_seconds: u32,
    /// When the display last had activity: the first tick, the button, a
    /// new sleep time, a request to light it.
    display_activity_ms: Option<u64>,
    /// The display went dark: its backlight is off and nothing is drawn
    /// until it is lit again.
    display_dark: bool,
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
            display_status_interval_ms: 5_000,
            last_display_status_ms: 0,
            display_sleep_path: None,
            display_sleep_seconds: 0,
            display_activity_ms: None,
            display_dark: false,
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

    /// Set the automatic status refresh interval in milliseconds.
    pub fn configure_display_status(&mut self, interval_ms: u64) {
        if interval_ms > 0 {
            self.display_status_interval_ms = interval_ms;
        }
    }

    /// Follow the display's sleep time in the board owner's file
    /// (`bli_kvm_core::DISPLAY_SLEEP_PATH`): read on every tick, as Hardened
    /// NanoKVM does, so that a new choice takes effect within a sample.
    pub fn configure_display_sleep(&mut self, path: impl Into<PathBuf>) {
        self.display_sleep_path = Some(path.into());
    }

    /// Whether the display went dark after its sleep time.
    pub fn display_dark(&self) -> bool {
        self.display_dark
    }

    /// Redraw the status screen when the refresh interval has elapsed, and
    /// darken the display once it has had no activity for its sleep time.
    pub fn tick_display(&mut self, now_ms: u64) {
        if self.display.is_none() {
            return;
        }
        let activity = *self.display_activity_ms.get_or_insert(now_ms);
        if let Some(path) = self.display_sleep_path.as_deref() {
            let seconds = bli_kvm_core::load_display_sleep(path);
            if seconds != self.display_sleep_seconds {
                // A new choice starts the time over and lights a dark display.
                self.display_sleep_seconds = seconds;
                self.light_display(now_ms);
                return;
            }
        }
        if !self.display_dark
            && self.display_sleep_seconds > 0
            && now_ms.saturating_sub(activity) >= u64::from(self.display_sleep_seconds) * 1_000
        {
            self.darken_display();
            return;
        }
        if self.display_dark {
            return;
        }
        if now_ms.saturating_sub(self.last_display_status_ms) < self.display_status_interval_ms {
            return;
        }
        self.last_display_status_ms = now_ms;
        let _ = self.draw_display_status();
    }

    /// Light the display, start its time over and draw the status at once:
    /// a dark panel keeps the frame it had.
    fn light_display(&mut self, now_ms: u64) {
        self.display_activity_ms = Some(now_ms);
        if let Some(display) = self.display.as_mut()
            && !display.backlight_on()
            && display.set_backlight(true).is_err()
        {
            return;
        }
        self.display_dark = false;
        self.last_display_status_ms = now_ms;
        let _ = self.draw_display_status();
    }

    /// Turn the backlight off; a failed write is tried again on the next tick.
    fn darken_display(&mut self) {
        if let Some(display) = self.display.as_mut()
            && display.set_backlight(false).is_ok()
        {
            self.display_dark = true;
        }
    }

    pub fn draw_display_status(&mut self) -> Result<(), BoarddError> {
        let display = self.display()?;
        display.status().map_err(|kind| match kind {
            "invalid_state" => BoarddError::InvalidState,
            _ => BoarddError::BackendUnavailable,
        })
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

    /// Latch the existing supervisor failsafe before board-specific worker drain.
    pub fn prepare_shutdown(
        &mut self,
    ) -> Result<&mut A, bli_kvm_fan_supervisor::FanSupervisorError<A::Error>> {
        self.supervisor.prepare_shutdown()?;
        Ok(self.supervisor.actuator_mut())
    }

    pub fn status(&self) -> StatusView {
        let mut state = self.supervisor.state();
        if self.ensure_fan_runtime().is_err() {
            state.status = bli_kvm_fan_supervisor::FanSupervisorStatus::ActuationUncertain;
        }
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
    pub fn tick_button(&mut self, now_ms: u64) {
        let Some(button) = self.button.as_mut() else {
            return;
        };
        if !matches!(button.poll(now_ms), Ok(true)) {
            return;
        }
        if self.display.is_none() {
            return;
        }
        // A wake cycle lights the panel, starts its sleep time over and
        // redraws the status, so a stale or undefined frame is replaced the
        // moment the panel is lit again.
        self.light_display(now_ms);
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

    /// Light or darken the display on request: lit, its sleep time starts
    /// over; darkened, it stays dark until the button or a new choice.
    pub fn set_display_backlight(&mut self, on: bool) -> Result<(), BoarddError> {
        let display = self.display()?;
        display.set_backlight(on).map_err(|kind| match kind {
            "invalid_state" => BoarddError::InvalidState,
            _ => BoarddError::BackendUnavailable,
        })?;
        self.display_dark = !on;
        if on {
            self.display_activity_ms = Some(unix_millis_now());
        }
        Ok(())
    }

    pub fn set_mode(&mut self, mode: FanModeName) -> Result<(), BoarddError> {
        self.ensure_fan_runtime()?;
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
        self.ensure_fan_runtime()?;
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
        self.ensure_fan_runtime()?;
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

    /// Read-only runtime check, including when manual mode skips policy sampling.
    pub fn ensure_fan_runtime(&self) -> Result<(), BoarddError> {
        self.supervisor
            .actuator()
            .check_runtime()
            .map_err(|_| BoarddError::BackendUnavailable)
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
    serve_until(socket, state, || false)
}

/// Stop admission when requested. Active transport reads retain their finite
/// connection deadline; already executing hardware operations must finish.
pub fn serve_until<A>(
    socket: &Path,
    state: &Arc<Mutex<BoarddState<A>>>,
    stopping: impl Fn() -> bool,
) -> std::io::Result<()>
where
    A: FanActuator,
    A::Error: std::fmt::Debug,
{
    // Never unlink a live listener or an unrelated node to claim its pathname.
    let listener = UnixListener::bind(socket)?;
    let owned = std::fs::symlink_metadata(socket)?;
    listener.set_nonblocking(true)?;
    while !stopping() {
        match listener.accept() {
            Ok((stream, _)) => {
                if !stopping() {
                    let _ = handle_connection_until(
                        stream,
                        state,
                        Instant::now() + CONNECTION_BUDGET,
                        &stopping,
                    );
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    drop(listener);
    match std::fs::symlink_metadata(socket) {
        Ok(current)
            if current.file_type().is_socket()
                && current.dev() == owned.dev()
                && current.ino() == owned.ino() =>
        {
            std::fs::remove_file(socket)?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        _ => return Err(std::io::Error::other("boardd socket ownership changed")),
    }
    Ok(())
}

#[cfg(test)]
fn handle_connection_before<A>(
    stream: UnixStream,
    state: &Arc<Mutex<BoarddState<A>>>,
    deadline: Instant,
) -> std::io::Result<()>
where
    A: FanActuator,
    A::Error: std::fmt::Debug,
{
    handle_connection_until(stream, state, deadline, &|| false)
}

fn handle_connection_until<A>(
    stream: UnixStream,
    state: &Arc<Mutex<BoarddState<A>>>,
    deadline: Instant,
    stopping: &impl Fn() -> bool,
) -> std::io::Result<()>
where
    A: FanActuator,
    A::Error: std::fmt::Debug,
{
    let mut writer = DeadlineStream {
        stream: stream.try_clone()?,
        deadline,
    };
    let mut reader = BufReader::new(DeadlineStream { stream, deadline });
    loop {
        if stopping() {
            return Ok(());
        }
        remaining_time(deadline)?;
        let mut line = String::new();
        let read = read_bounded_request_line(&mut reader, &mut line)?;
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
                Ok(mut state) => {
                    if stopping() {
                        return Ok(());
                    }
                    remaining_time(deadline)?;
                    handle(&mut state, request)
                }
                Err(_) => Response::error(-5, "internal"),
            },
            Err(_) => Response::error(-1, "invalid_request"),
        };
        serde_json::to_writer(&mut writer, &response)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
    }
}

fn read_bounded_request_line(
    reader: &mut impl BufRead,
    line: &mut String,
) -> std::io::Result<usize> {
    // Retain one overflow byte so the existing invalid_request response closes
    // oversized connections. Never allocate the full untrusted line first.
    reader.take((MAX_REQUEST_BYTES + 1) as u64).read_line(line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bli_kvm_fan_policy::{FanConfigError, FanPolicyConfig};

    #[test]
    fn listener_stops_and_removes_its_owned_socket() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("board.sock");
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker_path = path.clone();
        let worker = std::thread::spawn(move || {
            serve_until(&worker_path, &Arc::new(Mutex::new(state())), || {
                worker_stop.load(Ordering::SeqCst)
            })
        });
        let deadline = Instant::now() + Duration::from_secs(1);
        while !path.exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        stop.store(true, Ordering::SeqCst);
        worker.join().unwrap().unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn occupied_socket_and_unrelated_file_are_not_unlinked_on_startup() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("occupied.sock");
        let _listener = UnixListener::bind(&path).unwrap();
        let inode = std::fs::symlink_metadata(&path).unwrap().ino();
        let state = Arc::new(Mutex::new(state()));
        assert!(serve_until(&path, &state, || true).is_err());
        assert_eq!(std::fs::symlink_metadata(&path).unwrap().ino(), inode);
        let file = root.path().join("unrelated");
        std::fs::write(&file, "preserved").unwrap();
        assert!(serve_until(&file, &state, || true).is_err());
        assert_eq!(std::fs::read_to_string(file).unwrap(), "preserved");
    }

    #[test]
    fn stopped_connection_does_not_execute_queued_mutation() {
        let (server, mut client) = UnixStream::pair().unwrap();
        client
            .write_all(b"{\"request\":\"fan_mode_set\",\"mode\":\"manual\"}\n")
            .unwrap();
        let state = Arc::new(Mutex::new(state()));
        handle_connection_until(
            server,
            &state,
            Instant::now() + Duration::from_secs(1),
            &|| true,
        )
        .unwrap();
        assert_eq!(state.lock().unwrap().mode, FanModeName::Auto);
    }

    #[test]
    fn shutdown_uses_the_existing_full_duty_supervisor_failsafe() {
        let mut state = state();
        let actuator = state.prepare_shutdown().unwrap();
        assert_eq!(
            actuator.applied.last(),
            Some(&FanCommand::DutyPercent(FanDutyPercent::new(100).unwrap(),))
        );
        assert_eq!(state.status().fan_status, "failsafe_latched");
    }

    #[test]
    fn idle_connection_cannot_hold_the_serial_handler_indefinitely() {
        let (server, _client) = UnixStream::pair().unwrap();
        let state = Arc::new(Mutex::new(state()));
        let start = Instant::now();
        assert!(
            handle_connection_before(server, &state, start + Duration::from_millis(50)).is_err()
        );
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn trickled_input_does_not_renew_the_connection_deadline() {
        let (server, mut client) = UnixStream::pair().unwrap();
        client
            .set_write_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let sender = std::thread::spawn(move || {
            for _ in 0..30 {
                if client.write_all(b" ").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        let state = Arc::new(Mutex::new(state()));
        let start = Instant::now();
        assert!(
            handle_connection_before(server, &state, start + Duration::from_millis(60)).is_err()
        );
        assert!(start.elapsed() < Duration::from_millis(250));
        sender.join().unwrap();
    }

    #[test]
    fn backpressured_writer_obeys_the_same_deadline() {
        let (mut stream, _peer) = UnixStream::pair().unwrap();
        stream.set_nonblocking(true).unwrap();
        let mut full = false;
        for _ in 0..1024 {
            match stream.write(&[0; 8192]) {
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    full = true;
                    break;
                }
                Err(error) => panic!("prefill failed: {error}"),
            }
        }
        assert!(full);
        stream.set_nonblocking(false).unwrap();
        let start = Instant::now();
        let mut writer = DeadlineStream {
            stream,
            deadline: start + Duration::from_millis(50),
        };
        assert!(writer.write_all(b"reply").is_err());
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn expired_request_waiting_for_state_cannot_change_fan_mode() {
        let (server, mut client) = UnixStream::pair().unwrap();
        client
            .set_write_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        client
            .write_all(b"{\"request\":\"fan_mode_set\",\"mode\":\"manual\"}\n")
            .unwrap();
        let state = Arc::new(Mutex::new(state()));
        let held = state.lock().unwrap();
        let worker_state = state.clone();
        let deadline = Instant::now() + Duration::from_millis(40);
        let worker =
            std::thread::spawn(move || handle_connection_before(server, &worker_state, deadline));
        std::thread::sleep(Duration::from_millis(80));
        drop(held);
        assert!(worker.join().unwrap().is_err());
        assert_eq!(state.lock().unwrap().mode, FanModeName::Auto);
    }

    #[test]
    fn oversized_unterminated_request_is_bounded_before_allocation() {
        let mut source =
            std::io::BufReader::new(std::io::repeat(b'x').take((MAX_REQUEST_BYTES * 1024) as u64));
        let mut line = String::new();
        let count = read_bounded_request_line(&mut source, &mut line).unwrap();
        assert_eq!(count, MAX_REQUEST_BYTES + 1);
        assert_eq!(line.len(), MAX_REQUEST_BYTES + 1);
    }

    #[test]
    fn bounded_lines_preserve_separate_requests_and_exact_size_boundary() {
        let mut bytes = vec![b' '; MAX_REQUEST_BYTES - 1];
        bytes.push(b'\n');
        bytes.extend_from_slice(b"{\"request\":\"status\"}\n");
        let mut source = std::io::Cursor::new(bytes);
        let mut line = String::new();
        assert_eq!(
            read_bounded_request_line(&mut source, &mut line).unwrap(),
            MAX_REQUEST_BYTES
        );
        line.clear();
        read_bounded_request_line(&mut source, &mut line).unwrap();
        assert!(matches!(
            serde_json::from_str::<Request>(line.trim_end()).unwrap(),
            Request::Status
        ));
        line.clear();
        assert_eq!(
            read_bounded_request_line(&mut source, &mut line).unwrap(),
            0
        );
    }

    #[derive(Debug, Default)]
    struct MockActuator {
        applied: Vec<FanCommand>,
        failed: bool,
    }

    impl FanActuator for MockActuator {
        type Error = std::io::Error;

        fn apply(&mut self, command: FanCommand) -> Result<(), Self::Error> {
            self.applied.push(command);
            Ok(())
        }

        fn check_runtime(&self) -> Result<(), Self::Error> {
            if self.failed {
                Err(std::io::Error::other("fixture runtime failure"))
            } else {
                Ok(())
            }
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

    #[test]
    fn runtime_failure_is_visible_without_new_commands_in_auto_and_manual_modes() {
        for mode in [FanModeName::Auto, FanModeName::Manual] {
            let mut state = state();
            state.set_mode(mode).unwrap();
            state.supervisor.actuator_mut().failed = true;
            let before = state.supervisor.actuator().applied.len();
            assert_eq!(state.status().fan_status, "actuation_uncertain");
            assert!(state.ensure_fan_runtime().is_err());
            assert!(state.sample(1_000).is_err());
            assert!(
                state.set_mode(mode).is_err(),
                "same-mode mutation cannot report success"
            );
            assert!(state.set_manual_duty(50).is_err());
            assert_eq!(state.supervisor.actuator().applied.len(), before);
        }
    }

    #[derive(Default)]
    struct MockDisplay {
        initialized: bool,
        backlight: bool,
        fail: bool,
        patterns: usize,
    }

    impl DisplayControl for MockDisplay {
        fn status(&mut self) -> Result<(), &'static str> {
            if self.fail {
                return Err("backend_unavailable");
            }
            if !self.initialized {
                return Err("invalid_state");
            }
            self.patterns += 1;
            Ok(())
        }

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
        state.tick_button(unix_millis_now());
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
        bare.tick_button(unix_millis_now());
        assert_eq!(bare.status().button_wakeups, 1);
        assert!(!bare.status().display_present);
    }

    struct CountingDisplay {
        backlight: bool,
        draws: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl DisplayControl for CountingDisplay {
        fn status(&mut self) -> Result<(), &'static str> {
            self.draws.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }

        fn test_pattern(&mut self) -> Result<(), &'static str> {
            Ok(())
        }

        fn set_backlight(&mut self, on: bool) -> Result<(), &'static str> {
            self.backlight = on;
            Ok(())
        }

        fn is_initialized(&self) -> bool {
            true
        }

        fn backlight_on(&self) -> bool {
            self.backlight
        }
    }

    struct SharedButton {
        wake: Arc<std::sync::atomic::AtomicBool>,
        wakeups: u64,
    }

    impl ButtonControl for SharedButton {
        fn poll(&mut self, _now_ms: u64) -> Result<bool, &'static str> {
            let wake = self.wake.swap(false, std::sync::atomic::Ordering::SeqCst);
            self.wakeups += u64::from(wake);
            Ok(wake)
        }

        fn wakeups(&self) -> u64 {
            self.wakeups
        }
    }

    /// A lit panel, its button, and the owner's sleep file in `directory`.
    fn sleeping_state(
        directory: &Path,
    ) -> (
        BoarddState<MockActuator>,
        Arc<std::sync::atomic::AtomicUsize>,
        Arc<std::sync::atomic::AtomicBool>,
        PathBuf,
    ) {
        let draws = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let wake = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut supervisor = FanSupervisor::new(policy(), 0, MockActuator::default()).unwrap();
        let _ = supervisor.initialize();
        let mut state = BoarddState::new(
            supervisor,
            FanModeName::Auto,
            0,
            20,
            100,
            true,
            40,
            269,
            Some(Box::new(CountingDisplay {
                backlight: true,
                draws: Arc::clone(&draws),
            })),
            None,
            Some(Box::new(SharedButton {
                wake: Arc::clone(&wake),
                wakeups: 0,
            })),
            None,
        );
        let path = directory.join("display-sleep.env");
        state.configure_display_sleep(&path);
        (state, draws, wake, path)
    }

    #[test]
    fn the_display_goes_dark_after_its_sleep_time_and_the_button_lights_it() {
        use std::sync::atomic::Ordering;
        let directory = tempfile::tempdir().unwrap();
        let (mut state, draws, wake, path) = sleeping_state(directory.path());
        std::fs::write(&path, bli_kvm_core::render_display_sleep(15)).unwrap();
        let start = 1_000_000;
        // The first tick reads the choice and draws the status.
        state.tick_display(start);
        assert!(!state.display_dark());
        assert!(state.status().display_backlight_on);
        state.tick_display(start + 14_999);
        assert!(!state.display_dark());
        state.tick_display(start + 15_000);
        assert!(state.display_dark());
        assert!(!state.status().display_backlight_on);
        // Dark, nothing is drawn.
        let drawn = draws.load(Ordering::SeqCst);
        for second in 1..=30 {
            state.tick_display(start + 15_000 + second * 1_000);
        }
        assert_eq!(draws.load(Ordering::SeqCst), drawn);
        assert!(state.display_dark());

        // The button lights it and draws at once; the time starts over.
        wake.store(true, Ordering::SeqCst);
        state.tick_button(start + 50_000);
        assert!(!state.display_dark());
        assert!(state.status().display_backlight_on);
        assert_eq!(draws.load(Ordering::SeqCst), drawn + 1);
        assert_eq!(state.status().button_wakeups, 1);
        state.tick_display(start + 64_999);
        assert!(!state.display_dark());
        state.tick_display(start + 65_000);
        assert!(state.display_dark());

        // A new choice lights it; never keeps it lit.
        std::fs::write(&path, bli_kvm_core::render_display_sleep(0)).unwrap();
        state.tick_display(start + 66_000);
        assert!(!state.display_dark());
        assert!(state.status().display_backlight_on);
        state.tick_display(start + 10_000_000);
        assert!(!state.display_dark());
    }

    #[test]
    fn without_the_owners_file_the_display_stays_lit() {
        let directory = tempfile::tempdir().unwrap();
        let (mut state, _draws, _wake, path) = sleeping_state(directory.path());
        state.tick_display(1_000_000);
        state.tick_display(1_000_000 + 7_200_000);
        assert!(!state.display_dark());
        // A time that is not one of the choices is never, too.
        std::fs::write(&path, "sleep_seconds=20\n").unwrap();
        state.tick_display(1_000_000 + 7_201_000);
        state.tick_display(1_000_000 + 7_300_000);
        assert!(!state.display_dark());
        assert!(state.status().display_backlight_on);
    }

    #[test]
    fn a_display_darkened_on_request_stays_dark_until_the_button() {
        use std::sync::atomic::Ordering;
        let directory = tempfile::tempdir().unwrap();
        let (mut state, draws, wake, _path) = sleeping_state(directory.path());
        state.tick_display(1_000_000);
        assert_eq!(
            handle(&mut state, Request::DisplayBacklightSet { on: false }).code,
            0
        );
        assert!(state.display_dark());
        let drawn = draws.load(Ordering::SeqCst);
        state.tick_display(1_000_000 + 60_000);
        assert!(state.display_dark());
        assert_eq!(draws.load(Ordering::SeqCst), drawn);
        wake.store(true, Ordering::SeqCst);
        state.tick_button(1_000_000 + 61_000);
        assert!(!state.display_dark());
        assert!(state.status().display_backlight_on);
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
