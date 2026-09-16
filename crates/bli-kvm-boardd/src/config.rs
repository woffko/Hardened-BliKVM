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
        self.fan.validate()
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
