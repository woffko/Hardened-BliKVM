//! Fixed-route, read-only MJPEG bridge between the network server and uStreamer.

mod config;
mod server;

pub use config::{VideoProxyConfig, VideoProxyConfigError};
pub use server::{VideoProxyError, run};
