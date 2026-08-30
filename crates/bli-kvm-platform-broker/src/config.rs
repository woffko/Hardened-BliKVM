use std::{env, ffi::OsString, path::PathBuf, time::Duration};

use thiserror::Error;

const DEFAULT_BROKER_SOCKET: &str = "/run/hardened-blikvm/platform-broker.sock";
const DEFAULT_KVMD_SOCKET: &str = "/run/kvmd/kvmd.sock";

#[derive(Debug, Clone)]
pub struct BrokerConfig {
    pub broker_socket: PathBuf,
    pub kvmd_socket: PathBuf,
    pub credential_directory: PathBuf,
    pub request_timeout: Duration,
    pub max_connections: usize,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BrokerConfigError {
    #[error("CREDENTIALS_DIRECTORY is required")]
    MissingCredentialDirectory,
    #[error("broker paths must be absolute and distinct")]
    InvalidPath,
    #[error("broker connection limit must be between 1 and 64")]
    InvalidConnectionLimit,
    #[error("broker request timeout must be greater than zero")]
    InvalidTimeout,
}

impl BrokerConfig {
    pub fn from_systemd_environment() -> Result<Self, BrokerConfigError> {
        let credential_directory = env::var_os("CREDENTIALS_DIRECTORY")
            .ok_or(BrokerConfigError::MissingCredentialDirectory)?;
        Self::new(
            DEFAULT_BROKER_SOCKET,
            DEFAULT_KVMD_SOCKET,
            credential_directory,
        )
    }

    pub fn new(
        broker_socket: impl Into<PathBuf>,
        kvmd_socket: impl Into<PathBuf>,
        credential_directory: impl Into<OsString>,
    ) -> Result<Self, BrokerConfigError> {
        let config = Self {
            broker_socket: broker_socket.into(),
            kvmd_socket: kvmd_socket.into(),
            credential_directory: PathBuf::from(credential_directory.into()),
            request_timeout: Duration::from_secs(8),
            max_connections: 8,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), BrokerConfigError> {
        if !self.broker_socket.is_absolute()
            || !self.kvmd_socket.is_absolute()
            || !self.credential_directory.is_absolute()
            || self.broker_socket == self.kvmd_socket
        {
            return Err(BrokerConfigError::InvalidPath);
        }
        if !(1..=64).contains(&self.max_connections) {
            return Err(BrokerConfigError::InvalidConnectionLimit);
        }
        if self.request_timeout.is_zero() {
            return Err(BrokerConfigError::InvalidTimeout);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_relative_or_overlapping_paths() {
        assert_eq!(
            BrokerConfig::new("relative.sock", "/run/kvmd/kvmd.sock", "/run/credentials").err(),
            Some(BrokerConfigError::InvalidPath)
        );
        assert_eq!(
            BrokerConfig::new(
                "/run/kvmd/kvmd.sock",
                "/run/kvmd/kvmd.sock",
                "/run/credentials"
            )
            .err(),
            Some(BrokerConfigError::InvalidPath)
        );
    }
}
