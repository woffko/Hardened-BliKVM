use std::{env, ffi::OsString, path::PathBuf, time::Duration};

use thiserror::Error;

const DEFAULT_PROXY_SOCKET: &str = "/run/hardened-blikvm-video/mjpeg.sock";
const DEFAULT_UPSTREAM_SOCKET: &str = "/run/kvmd/ustreamer.sock";

#[derive(Debug, Clone)]
pub struct VideoProxyConfig {
    pub proxy_socket: PathBuf,
    pub upstream_socket: PathBuf,
    pub request_timeout: Duration,
    pub idle_timeout: Duration,
    pub max_connections: usize,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum VideoProxyConfigError {
    #[error("video proxy paths must be absolute and distinct")]
    InvalidPath,
    #[error("video proxy connection limit must be between 1 and 8")]
    InvalidConnectionLimit,
    #[error("video proxy timeouts must be greater than zero")]
    InvalidTimeout,
}

impl VideoProxyConfig {
    pub fn from_environment() -> Result<Self, VideoProxyConfigError> {
        Self::new(
            env::var_os("BLI_KVM_VIDEO_PROXY_SOCKET")
                .unwrap_or_else(|| OsString::from(DEFAULT_PROXY_SOCKET)),
            env::var_os("BLI_KVM_USTREAMER_SOCKET")
                .unwrap_or_else(|| OsString::from(DEFAULT_UPSTREAM_SOCKET)),
        )
    }

    pub fn new(
        proxy_socket: impl Into<PathBuf>,
        upstream_socket: impl Into<PathBuf>,
    ) -> Result<Self, VideoProxyConfigError> {
        let config = Self {
            proxy_socket: proxy_socket.into(),
            upstream_socket: upstream_socket.into(),
            request_timeout: Duration::from_secs(5),
            idle_timeout: Duration::from_secs(15),
            max_connections: 2,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), VideoProxyConfigError> {
        if !self.proxy_socket.is_absolute()
            || !self.upstream_socket.is_absolute()
            || self.proxy_socket == self.upstream_socket
        {
            return Err(VideoProxyConfigError::InvalidPath);
        }
        if !(1..=8).contains(&self.max_connections) {
            return Err(VideoProxyConfigError::InvalidConnectionLimit);
        }
        if self.request_timeout.is_zero() || self.idle_timeout.is_zero() {
            return Err(VideoProxyConfigError::InvalidTimeout);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_relative_overlapping_or_unbounded_configuration() {
        assert_eq!(
            VideoProxyConfig::new("relative.sock", "/run/kvmd/ustreamer.sock").err(),
            Some(VideoProxyConfigError::InvalidPath)
        );
        assert_eq!(
            VideoProxyConfig::new("/run/kvmd/ustreamer.sock", "/run/kvmd/ustreamer.sock").err(),
            Some(VideoProxyConfigError::InvalidPath)
        );
        let mut config = VideoProxyConfig::new("/run/proxy.sock", "/run/upstream.sock").unwrap();
        config.max_connections = 9;
        assert_eq!(
            config.validate(),
            Err(VideoProxyConfigError::InvalidConnectionLimit)
        );
    }
}
