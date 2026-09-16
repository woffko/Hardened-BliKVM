//! ST7789 panel: reset and backlight sequencing, the operator's controller
//! initialization, and a bounded test pattern.
//!
//! The controller commands come from `bli-kvm-st7789-model`; the initialization
//! sequence, the line offsets and the timing come from the operator
//! configuration, so nothing here is a guessed datasheet default.

use bli_kvm_platform_bli::{GpioDriver, GpioRole, LineDirection, LineRequest};
use bli_kvm_st7789_model::{
    BYTES_PER_PIXEL, DISPLAY_HEIGHT, DISPLAY_PIXEL_COUNT, DISPLAY_WIDTH, DisplayWindow,
    MAX_PIXEL_CHUNK_PIXELS, PixelStream, Rgb565, WindowPhase,
};
use thiserror::Error;

use crate::{config::DisplayConfig, spi::SpiBus};

/// Consumer label the daemon requests the panel lines with.
const DISPLAY_CONSUMER: &str = "hardened-blikvm-boardd";

#[derive(Debug, Error)]
pub enum DisplayError {
    #[error("display configuration is invalid")]
    InvalidConfiguration,
    #[error("display bus failed: {0}")]
    Spi(#[from] crate::spi::SpiError),
    #[error("display line failed")]
    Line,
    #[error("display pixel stream failed")]
    PixelStream,
    #[error("display is not initialized")]
    NotInitialized,
}

/// Minimal sleep abstraction so sequencing is testable without waiting.
pub trait Sleeper {
    fn sleep_ms(&mut self, milliseconds: u64);
}

#[derive(Debug, Default)]
pub struct ThreadSleeper;

impl Sleeper for ThreadSleeper {
    fn sleep_ms(&mut self, milliseconds: u64) {
        if milliseconds > 0 {
            std::thread::sleep(std::time::Duration::from_millis(milliseconds));
        }
    }
}

/// Test pattern colours, one per vertical bar.
const BARS: [Rgb565; 6] = [
    Rgb565::RED,
    Rgb565::GREEN,
    Rgb565::BLUE,
    Rgb565::WHITE,
    Rgb565::BLACK,
    Rgb565::GREEN,
];

pub struct Display<B, G, S>
where
    B: SpiBus,
    G: GpioDriver,
    S: Sleeper,
{
    bus: B,
    driver: G,
    sleeper: S,
    config: DisplayConfig,
    data_command: LineRequest,
    reset: LineRequest,
    backlight: LineRequest,
    initialized: bool,
    backlight_on: bool,
}

impl<B, G, S> Display<B, G, S>
where
    B: SpiBus,
    G: GpioDriver,
    S: Sleeper,
{
    pub fn new(bus: B, driver: G, sleeper: S, config: DisplayConfig) -> Self {
        Self {
            bus,
            driver,
            sleeper,
            data_command: line(GpioRole::DisplayDataCommand, config.data_command),
            reset: line(GpioRole::DisplayReset, config.reset),
            backlight: line(GpioRole::DisplayBacklight, config.backlight),
            config,
            initialized: false,
            backlight_on: false,
        }
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    pub fn backlight_on(&self) -> bool {
        self.backlight_on
    }

    /// Request the panel lines, pulse reset, run the configured initialization
    /// and only then enable the backlight.
    pub fn initialize(&mut self) -> Result<(), DisplayError> {
        for line in [&self.data_command, &self.reset, &self.backlight] {
            self.driver
                .request_line(line)
                .map_err(|_| DisplayError::Line)?;
        }
        self.set_line(&self.backlight.clone(), false)?;
        self.set_line(&self.reset.clone(), true)?;
        self.sleeper.sleep_ms(self.config.reset_assert_ms);
        self.set_line(&self.reset.clone(), false)?;
        self.sleeper.sleep_ms(self.config.reset_settle_ms);

        let steps = self.config.init.clone();
        for step in steps {
            self.write(false, &[step.command])?;
            if !step.data.is_empty() {
                self.write(true, &step.data)?;
            }
            self.sleeper.sleep_ms(step.delay_ms);
        }
        self.set_line(&self.backlight.clone(), true)?;
        self.sleeper.sleep_ms(self.config.backlight_settle_ms);
        self.backlight_on = true;
        self.initialized = true;
        Ok(())
    }

    pub fn set_backlight(&mut self, on: bool) -> Result<(), DisplayError> {
        if !self.initialized && on {
            return Err(DisplayError::NotInitialized);
        }
        self.set_line(&self.backlight.clone(), on)?;
        if !on {
            self.sleeper.sleep_ms(self.config.backlight_settle_ms);
        }
        self.backlight_on = on;
        Ok(())
    }

    /// Draw the fixed test pattern across the full panel in bounded chunks.
    pub fn draw_color_bars(&mut self) -> Result<(), DisplayError> {
        if !self.initialized {
            return Err(DisplayError::NotInitialized);
        }
        let window = DisplayWindow::FULL;
        for phase in window.program().phases() {
            match phase {
                WindowPhase::Command(command) => self.write(false, &[command])?,
                WindowPhase::AddressData(data) => self.write(true, &data)?,
            }
        }

        let mut stream = PixelStream::new(window);
        let mut encoded = [0_u8; MAX_PIXEL_CHUNK_PIXELS * BYTES_PER_PIXEL];
        let mut row = vec![Rgb565::BLACK; DISPLAY_WIDTH as usize];
        for y in 0..DISPLAY_HEIGHT {
            let bar = usize::from(y) * BARS.len() / usize::from(DISPLAY_HEIGHT);
            let colour = BARS[bar.min(BARS.len() - 1)];
            for pixel in row.iter_mut() {
                *pixel = colour;
            }
            let written = stream
                .encode_chunk(&row, &mut encoded)
                .map_err(|_| DisplayError::PixelStream)?;
            self.write(true, &encoded[..written])?;
        }
        if !stream.is_complete() {
            return Err(DisplayError::PixelStream);
        }
        Ok(())
    }

    fn set_line(&mut self, line: &LineRequest, active: bool) -> Result<(), DisplayError> {
        self.driver
            .write_line(line, active)
            .map_err(|_| DisplayError::Line)
    }

    fn write(&mut self, data: bool, bytes: &[u8]) -> Result<(), DisplayError> {
        if bytes.is_empty() {
            return Ok(());
        }
        self.set_line(&self.data_command.clone(), data)?;
        self.bus.write(data, bytes)?;
        Ok(())
    }
}

impl<B, G, S> crate::server::DisplayControl for Display<B, G, S>
where
    B: SpiBus,
    G: GpioDriver + Send,
    S: Sleeper + Send,
{
    fn test_pattern(&mut self) -> Result<(), &'static str> {
        self.draw_color_bars().map_err(|error| match error {
            DisplayError::NotInitialized => "invalid_state",
            _ => "backend_unavailable",
        })
    }

    fn set_backlight(&mut self, on: bool) -> Result<(), &'static str> {
        Display::set_backlight(self, on).map_err(|error| match error {
            DisplayError::NotInitialized => "invalid_state",
            _ => "backend_unavailable",
        })
    }

    fn is_initialized(&self) -> bool {
        self.initialized
    }

    fn backlight_on(&self) -> bool {
        self.backlight_on
    }
}

fn line(role: GpioRole, spec: crate::config::LineSpec) -> LineRequest {
    LineRequest {
        chip: "gpiochip0",
        offset: spec.offset,
        consumer: DISPLAY_CONSUMER,
        role,
        direction: LineDirection::Output,
        active_low: spec.active_low,
    }
}

/// Total bytes one full-frame pattern transfer must contain.
pub const fn full_frame_bytes() -> usize {
    DISPLAY_PIXEL_COUNT as usize * BYTES_PER_PIXEL
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use crate::config::{InitStep, LineSpec};

    type Transfers = Arc<Mutex<Vec<(bool, Vec<u8>)>>>;
    type LineWrites = Arc<Mutex<Vec<(u16, bool)>>>;

    #[derive(Default)]
    struct MockBus {
        transfers: Transfers,
        fail: bool,
    }

    impl SpiBus for MockBus {
        fn write(&mut self, data: bool, bytes: &[u8]) -> Result<(), crate::spi::SpiError> {
            if self.fail {
                return Err(crate::spi::SpiError::Transfer);
            }
            self.transfers.lock().unwrap().push((data, bytes.to_vec()));
            Ok(())
        }
    }

    #[derive(Default)]
    struct MockDriver {
        writes: LineWrites,
    }

    impl GpioDriver for MockDriver {
        type Error = std::io::Error;

        fn request_line(&mut self, _request: &LineRequest) -> Result<(), Self::Error> {
            Ok(())
        }

        fn write_line(&mut self, request: &LineRequest, active: bool) -> Result<(), Self::Error> {
            self.writes.lock().unwrap().push((request.offset, active));
            Ok(())
        }

        fn release_line(&mut self, _request: &LineRequest) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    #[derive(Default)]
    struct MockSleeper {
        slept: Arc<Mutex<u64>>,
    }

    impl Sleeper for MockSleeper {
        fn sleep_ms(&mut self, milliseconds: u64) {
            *self.slept.lock().unwrap() += milliseconds;
        }
    }

    fn config() -> DisplayConfig {
        DisplayConfig {
            spidev: std::path::PathBuf::from("/dev/spidev1.0"),
            clock_hz: 40_000_000,
            data_command: LineSpec {
                offset: 259,
                active_low: false,
            },
            reset: LineSpec {
                offset: 262,
                active_low: false,
            },
            backlight: LineSpec {
                offset: 260,
                active_low: false,
            },
            reset_assert_ms: 10,
            reset_settle_ms: 120,
            backlight_settle_ms: 50,
            init: vec![
                InitStep {
                    command: 0x01,
                    data: vec![],
                    delay_ms: 150,
                },
                InitStep {
                    command: 0x3a,
                    data: vec![0x55],
                    delay_ms: 0,
                },
                InitStep {
                    command: 0x29,
                    data: vec![],
                    delay_ms: 0,
                },
            ],
        }
    }

    type TestDisplay = Display<MockBus, MockDriver, MockSleeper>;

    fn display() -> (TestDisplay, Transfers, LineWrites) {
        let bus = MockBus::default();
        let driver = MockDriver::default();
        let transfers = Arc::clone(&bus.transfers);
        let writes = Arc::clone(&driver.writes);
        (
            Display::new(bus, driver, MockSleeper::default(), config()),
            transfers,
            writes,
        )
    }

    #[test]
    fn initialize_sequences_reset_then_commands_then_backlight() {
        let (mut display, transfers, writes) = display();
        display.initialize().unwrap();
        assert!(display.is_initialized());
        assert!(display.backlight_on());

        let transfers = transfers.lock().unwrap().clone();
        let commands: Vec<u8> = transfers
            .iter()
            .filter(|(data, _)| !*data)
            .map(|(_, bytes)| bytes[0])
            .collect();
        assert_eq!(commands, vec![0x01, 0x3a, 0x29]);
        let colour_mode = transfers
            .iter()
            .find(|(data, bytes)| *data && bytes == &[0x55])
            .is_some();
        assert!(colour_mode, "colour mode payload missing: {transfers:?}");

        let writes = writes.lock().unwrap().clone();
        assert_eq!(writes[0], (260, false), "backlight must start off");
        assert_eq!(writes[1], (262, true), "reset must be asserted");
        assert!(writes.contains(&(262, false)), "reset must be released");
        assert_eq!(writes.last().copied(), Some((260, true)));
    }

    #[test]
    fn backlight_needs_initialization_and_follows_the_request() {
        let (mut display, _transfers, _writes) = display();
        assert_eq!(
            display.set_backlight(true).unwrap_err().to_string(),
            "display is not initialized"
        );
        display.initialize().unwrap();
        display.set_backlight(false).unwrap();
        assert!(!display.backlight_on());
    }

    #[test]
    fn failing_bus_keeps_the_backlight_off() {
        let bus = MockBus {
            fail: true,
            ..MockBus::default()
        };
        let mut display =
            Display::new(bus, MockDriver::default(), MockSleeper::default(), config());
        assert!(display.initialize().is_err());
        assert!(!display.is_initialized());
        assert!(!display.backlight_on());
    }

    #[test]
    fn test_pattern_writes_one_full_frame_in_bounded_chunks() {
        let (mut display, transfers, _writes) = display();
        display.initialize().unwrap();
        transfers.lock().unwrap().clear();
        display.draw_color_bars().unwrap();

        let transfers = transfers.lock().unwrap().clone();
        let data_bytes: usize = transfers
            .iter()
            .filter(|(data, _)| *data)
            .map(|(_, bytes)| bytes.len())
            .sum();
        // Window address data (4 bytes twice) plus the frame.
        assert_eq!(data_bytes, 8 + full_frame_bytes());
        for (_, bytes) in transfers.iter().filter(|(data, _)| *data) {
            assert!(bytes.len() <= MAX_PIXEL_CHUNK_PIXELS * BYTES_PER_PIXEL);
        }
        let commands: Vec<u8> = transfers
            .iter()
            .filter(|(data, _)| !*data)
            .map(|(_, bytes)| bytes[0])
            .collect();
        assert_eq!(commands, vec![0x2a, 0x2b, 0x2c]);
    }
}
