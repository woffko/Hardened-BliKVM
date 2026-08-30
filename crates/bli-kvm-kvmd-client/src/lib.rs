//! Bounded, read-only client for the local KVMD Unix socket.
//!
//! This crate belongs in the transition broker, never in the network-facing
//! server. It intentionally exposes no HID, ATX, media, or GPIO mutation.

mod client;
mod credentials;
mod models;

pub use client::{KvmdClientError, KvmdInventoryError, KvmdReadClient, KvmdReadEndpoint};
pub use credentials::{CredentialError, KvmdCredentials};
pub use models::{
    KvmdAtxLeds, KvmdAtxState, KvmdEndpointState, KvmdExtrasInfo, KvmdHidState, KvmdInfoState,
    KvmdInventory, KvmdMsdDrive, KvmdMsdState, KvmdStreamerInfo, KvmdStreamerSource,
    KvmdStreamerState, KvmdSystemInfo, KvmdVersionInfo,
};
