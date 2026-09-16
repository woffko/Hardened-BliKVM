//! Operator-supplied board daemon configuration.
//!
//! Every board fact (line offsets, polarity, PWM period, temperature
//! thresholds and curve) comes from this file, which the operator writes from
//! the reviewed gate; nothing here is guessed from the vendor source.

use std::{
    fs,
    path::{Path, PathBuf},
};

use bli_kvm_fan_policy::{FanCurvePoint, FanPolicyConfig};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Largest configuration the daemon will read.
pub const MAX_CONFIG_BYTES: u64 = 64 * 1024;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("board daemon configuration is unavailable")]
    Unavailable,
    #[error("board daemon configuration is too large")]
    TooLarge,
    #[error("board daemon configuration is invalid")]
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FanConfig {
    pub line: u16,
    pub active_low: bool,
    /// Soft-PWM period in milliseconds, reviewed at the fan gate.
    pub pwm_period_ms: u64,
    pub turn_off_millidegrees: i32,
    pub turn_on_millidegrees: i32,
    pub critical_millidegrees: i32,
    pub minimum_on_ms: u64,
    pub minimum_off_ms: u64,
    pub sensor_timeout_ms: u64,
    pub minimum_duty_percent: u8,
    /// Upper bound the daemon accepts for a manual duty.
    pub maximum_duty_percent: u8,
    pub curve: Vec<CurvePoint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CurvePoint {
    pub at_or_above_millidegrees: i32,
    pub duty_percent: u8,
}

/// One reviewed GPIO line: offset and polarity from the gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LineSpec {
    pub offset: u16,
    pub active_low: bool,
}

/// One controller initialization step: command, payload and optional settle
/// delay. The sequence is a board fact and comes from the operator, not from a
/// guessed datasheet default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InitStep {
    pub command: u8,
    #[serde(default)]
    pub data: Vec<u8>,
    #[serde(default)]
    pub delay_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DisplayConfig {
    pub spidev: PathBuf,
    pub clock_hz: u32,
    pub data_command: LineSpec,
    pub reset: LineSpec,
    pub backlight: LineSpec,
    pub reset_assert_ms: u64,
    pub reset_settle_ms: u64,
    pub backlight_settle_ms: u64,
    pub init: Vec<InitStep>,
}

impl DisplayConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        if !self.spidev.is_absolute() {
            return Err(ConfigError::Invalid);
        }
        if self.clock_hz < 1_000 || self.clock_hz > 50_000_000 {
            return Err(ConfigError::Invalid);
        }
        let offsets = [
            self.data_command.offset,
            self.reset.offset,
            self.backlight.offset,
        ];
        if offsets[0] == offsets[1] || offsets[1] == offsets[2] || offsets[0] == offsets[2] {
            return Err(ConfigError::Invalid);
        }
        if !(1..=100).contains(&self.reset_assert_ms)
            || !(1..=1_000).contains(&self.reset_settle_ms)
            || !(1..=1_000).contains(&self.backlight_settle_ms)
        {
            return Err(ConfigError::Invalid);
        }
        if self.init.is_empty() || self.init.len() > 64 {
            return Err(ConfigError::Invalid);
        }
        for step in &self.init {
            if step.data.len() > 128 || step.delay_ms > 1_000 {
                return Err(ConfigError::Invalid);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BoarddConfig {
    /// Control socket the server and operator tools use.
    pub socket: PathBuf,
    /// GPIO character device that owns the board lines.
    pub gpio_chip: PathBuf,
    /// Milliseconds between automatic temperature evaluations.
    pub sample_interval_ms: u64,
    pub fan: FanConfig,
    pub display: DisplayConfig,
}

impl FanConfig {
    fn validate(&self) -> Result<(), ConfigError> {
        if self.pwm_period_ms < 5 || self.pwm_period_ms > 1000 {
            return Err(ConfigError::Invalid);
        }
        if !(1..=100).contains(&self.minimum_duty_percent)
            || self.maximum_duty_percent > 100
            || self.minimum_duty_percent > self.maximum_duty_percent
        {
            return Err(ConfigError::Invalid);
        }
        if self.turn_off_millidegrees >= self.turn_on_millidegrees
            || self.turn_on_millidegrees >= self.critical_millidegrees
        {
            return Err(ConfigError::Invalid);
        }
        if !(1_000..=600_000).contains(&self.minimum_on_ms)
            || !(1_000..=600_000).contains(&self.minimum_off_ms)
            || !(1_000..=300_000).contains(&self.sensor_timeout_ms)
        {
            return Err(ConfigError::Invalid);
        }
        if self.curve.len() > 8 {
            return Err(ConfigError::Invalid);
        }
        if self.curve.is_empty() {
            return Err(ConfigError::Invalid);
        }
        let mut previous: Option<i32> = None;
        for point in &self.curve {
            if point.duty_percent < self.minimum_duty_percent
                || point.duty_percent > self.maximum_duty_percent
            {
                return Err(ConfigError::Invalid);
            }
            if let Some(previous) = previous
                && point.at_or_above_millidegrees <= previous
            {
                return Err(ConfigError::Invalid);
            }
            previous = Some(point.at_or_above_millidegrees);
        }
        Ok(())
    }

    pub fn policy_config(&self) -> FanPolicyConfig {
        FanPolicyConfig {
            turn_off_millidegrees: self.turn_off_millidegrees,
            turn_on_millidegrees: self.turn_on_millidegrees,
            critical_millidegrees: self.critical_millidegrees,
            minimum_on_ms: self.minimum_on_ms,
            minimum_off_ms: self.minimum_off_ms,
            sensor_timeout_ms: self.sensor_timeout_ms,
            minimum_duty_percent: self.minimum_duty_percent,
            curve: self
                .curve
                .iter()
                .map(|point| FanCurvePoint {
                    at_or_above_millidegrees: point.at_or_above_millidegrees,
                    duty_percent: point.duty_percent,
                })
                .collect(),
        }
    }
}

impl BoarddConfig {
    pub fn from_path(path: &Path) -> Result<Self, ConfigError> {
        if !path.is_absolute() {
            return Err(ConfigError::Invalid);
        }
        let metadata = fs::metadata(path).map_err(|_| ConfigError::Unavailable)?;
        if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_CONFIG_BYTES {
            return Err(ConfigError::TooLarge);
        }
        let bytes = fs::read(path).map_err(|_| ConfigError::Unavailable)?;
        if bytes.len() as u64 > MAX_CONFIG_BYTES {
            return Err(ConfigError::TooLarge);
        }
        let config: Self = serde_json::from_slice(&bytes).map_err(|_| ConfigError::Invalid)?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConfigError> {
        if !self.socket.is_absolute() || !self.gpio_chip.is_absolute() {
            return Err(ConfigError::Invalid);
        }
        if self.sample_interval_ms < 250 || self.sample_interval_ms > 60_000 {
            return Err(ConfigError::Invalid);
        }
        self.fan.validate()?;
        self.display.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> BoarddConfig {
        BoarddConfig {
            socket: PathBuf::from("/run/hardened-blikvm-boardd/control.sock"),
            gpio_chip: PathBuf::from("/dev/gpiochip0"),
            sample_interval_ms: 1_000,
            fan: FanConfig {
                line: 269,
                active_low: false,
                pwm_period_ms: 40,
                turn_off_millidegrees: 45_000,
                turn_on_millidegrees: 50_000,
                critical_millidegrees: 80_000,
                minimum_on_ms: 2_000,
                minimum_off_ms: 2_000,
                sensor_timeout_ms: 5_000,
                minimum_duty_percent: 20,
                maximum_duty_percent: 100,
                curve: vec![
                    CurvePoint {
                        at_or_above_millidegrees: 50_000,
                        duty_percent: 30,
                    },
                    CurvePoint {
                        at_or_above_millidegrees: 70_000,
                        duty_percent: 80,
                    },
                ],
            },
            display: DisplayConfig {
                spidev: PathBuf::from("/dev/spidev1.0"),
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
                        data: Vec::new(),
                        delay_ms: 150,
                    },
                    InitStep {
                        command: 0x11,
                        data: Vec::new(),
                        delay_ms: 120,
                    },
                    InitStep {
                        command: 0x3a,
                        data: vec![0x55],
                        delay_ms: 0,
                    },
                    InitStep {
                        command: 0x29,
                        data: Vec::new(),
                        delay_ms: 0,
                    },
                ],
            },
        }
    }

    #[test]
    fn accepts_a_reviewed_configuration() {
        assert_eq!(config().validate(), Ok(()));
    }

    #[test]
    fn rejects_unreviewed_or_inconsistent_values() {
        let mut invalid = config();
        invalid.socket = PathBuf::from("relative.sock");
        assert_eq!(invalid.validate(), Err(ConfigError::Invalid));

        let mut invalid = config();
        invalid.fan.pwm_period_ms = 0;
        assert_eq!(invalid.validate(), Err(ConfigError::Invalid));

        let mut invalid = config();
        invalid.fan.curve[1].duty_percent = 10;
        assert_eq!(invalid.validate(), Err(ConfigError::Invalid));

        let mut invalid = config();
        invalid.fan.curve.reverse();
        assert_eq!(invalid.validate(), Err(ConfigError::Invalid));

        let mut invalid = config();
        invalid.fan.minimum_duty_percent = 90;
        invalid.fan.maximum_duty_percent = 50;
        assert_eq!(invalid.validate(), Err(ConfigError::Invalid));

        let mut invalid = config();
        invalid.display.reset.offset = 259;
        assert_eq!(invalid.validate(), Err(ConfigError::Invalid));

        let mut invalid = config();
        invalid.display.init.clear();
        assert_eq!(invalid.validate(), Err(ConfigError::Invalid));

        let mut invalid = config();
        invalid.display.clock_hz = 0;
        assert_eq!(invalid.validate(), Err(ConfigError::Invalid));
    }

    #[test]
    fn load_reads_a_bounded_absolute_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("boardd.json");
        fs::write(&path, serde_json::to_vec(&config()).unwrap()).unwrap();
        assert_eq!(BoarddConfig::from_path(&path), Ok(config()));

        let oversized = directory.path().join("oversized.json");
        fs::write(&oversized, vec![b'{'; (MAX_CONFIG_BYTES + 1) as usize]).unwrap();
        assert_eq!(
            BoarddConfig::from_path(&oversized),
            Err(ConfigError::TooLarge)
        );

        assert_eq!(
            BoarddConfig::from_path(Path::new("relative.json")),
            Err(ConfigError::Invalid)
        );
    }
}
