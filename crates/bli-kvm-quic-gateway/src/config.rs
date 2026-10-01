//! The gateway's settings, from its unit's environment.

use std::{env, net::SocketAddr, path::PathBuf, time::Duration};

#[derive(Debug, Clone)]
pub struct Config {
    /// The UDP address the endpoint listens on.
    pub bind: SocketAddr,
    /// The web server's link socket.
    pub link_socket: PathBuf,
    /// The H.264 encoder's frame socket.
    pub frame_socket: PathBuf,
    /// Connections served at once, counted from the first packet.
    pub max_connections: usize,
    /// From the first packet to a redeemed token.
    pub hello_timeout: Duration,
    pub idle_timeout: Duration,
    pub keep_alive: Duration,
    /// How long a certificate is valid, and how long one is used.
    pub certificate_days: u32,
    pub certificate_use: Duration,
    /// How often a session is sent its counters.
    pub counters_interval: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            bind: SocketAddr::from(([0, 0, 0, 0, 0, 0, 0, 0], 443)),
            link_socket: PathBuf::from("/run/hardened-blikvm-quic-link/link.sock"),
            frame_socket: PathBuf::from("/run/hardened-blikvm-h264/frames.sock"),
            max_connections: 8,
            hello_timeout: Duration::from_secs(5),
            idle_timeout: Duration::from_secs(30),
            keep_alive: Duration::from_secs(5),
            certificate_days: 13,
            certificate_use: Duration::from_secs(7 * 24 * 3600),
            counters_interval: Duration::from_secs(2),
        }
    }
}

impl Config {
    /// `BLI_KVM_QUIC_BIND`, `BLI_KVM_QUIC_LINK_SOCKET` and
    /// `BLI_KVM_QUIC_FRAME_SOCKET` replace the defaults; nothing else is
    /// adjustable on a board.
    pub fn from_env() -> Result<Self, String> {
        let mut config = Self::default();
        if let Some(value) = env::var_os("BLI_KVM_QUIC_BIND") {
            config.bind = value
                .to_str()
                .and_then(|text| text.parse().ok())
                .ok_or("BLI_KVM_QUIC_BIND must be an address and a port")?;
        }
        if let Some(value) = env::var_os("BLI_KVM_QUIC_LINK_SOCKET") {
            let path = PathBuf::from(value);
            if !path.is_absolute() {
                return Err("BLI_KVM_QUIC_LINK_SOCKET must be an absolute path".into());
            }
            config.link_socket = path;
        }
        if let Some(value) = env::var_os("BLI_KVM_QUIC_FRAME_SOCKET") {
            let path = PathBuf::from(value);
            if !path.is_absolute() {
                return Err("BLI_KVM_QUIC_FRAME_SOCKET must be an absolute path".into());
            }
            config.frame_socket = path;
        }
        Ok(config)
    }
}
