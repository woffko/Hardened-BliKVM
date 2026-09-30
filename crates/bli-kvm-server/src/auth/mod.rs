//! Authentication model adapted from Hardened NanoKVM.
//!
//! BLi-KVM deliberately omits legacy default accounts and fixed-key frontend
//! password obfuscation. Root-account synchronization belongs to the future
//! privileged broker and is not part of the network process.

pub mod cookie;
pub mod middleware;
pub mod password;
pub mod rate_limit;
pub mod routes;
pub mod session;
pub(crate) mod token;
