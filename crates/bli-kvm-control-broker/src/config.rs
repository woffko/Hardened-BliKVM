use std::{env, error::Error, ffi::OsString, fmt, path::PathBuf, time::Duration};

const DEFAULT_CONTROL_SOCKET: &str = "/run/hardened-blikvm-control/control.sock";

#[derive(Debug, Clone)]
pub struct ControlBrokerConfig {
    pub socket_path: PathBuf,
    pub request_timeout: Duration,
    pub max_connections: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlBrokerConfigError {
    InvalidSocketPath,
    InvalidTimeout,
    InvalidConnectionLimit,
}

impl fmt::Display for ControlBrokerConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidSocketPath => "control broker socket path must be absolute",
            Self::InvalidTimeout => "control broker timeout must be greater than zero",
            Self::InvalidConnectionLimit => {
                "control broker connection limit must be between 1 and 16"
            }
        })
    }
}

impl Error for ControlBrokerConfigError {}

impl ControlBrokerConfig {
    pub fn from_environment() -> Result<Self, ControlBrokerConfigError> {
        Self::new(
            env::var_os("BLI_KVM_CONTROL_SOCKET")
                .unwrap_or_else(|| OsString::from(DEFAULT_CONTROL_SOCKET)),
        )
    }

    pub fn new(socket_path: impl Into<PathBuf>) -> Result<Self, ControlBrokerConfigError> {
        let config = Self {
            socket_path: socket_path.into(),
            request_timeout: Duration::from_secs(3),
            max_connections: 4,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ControlBrokerConfigError> {
        if !self.socket_path.is_absolute() {
            return Err(ControlBrokerConfigError::InvalidSocketPath);
        }
        if self.request_timeout.is_zero() {
            return Err(ControlBrokerConfigError::InvalidTimeout);
        }
        if !(1..=16).contains(&self.max_connections) {
            return Err(ControlBrokerConfigError::InvalidConnectionLimit);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_relative_unbounded_or_zero_configuration() {
        assert!(matches!(
            ControlBrokerConfig::new("control.sock"),
            Err(ControlBrokerConfigError::InvalidSocketPath)
        ));
        let mut config = ControlBrokerConfig::new("/run/control.sock").unwrap();
        config.max_connections = 17;
        assert_eq!(
            config.validate(),
            Err(ControlBrokerConfigError::InvalidConnectionLimit)
        );
        config.max_connections = 1;
        config.request_timeout = Duration::ZERO;
        assert_eq!(
            config.validate(),
            Err(ControlBrokerConfigError::InvalidTimeout)
        );
    }
}
