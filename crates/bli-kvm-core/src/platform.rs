use crate::CapabilityManifest;

/// Supplies a capability manifest without performing hardware I/O.
///
/// Runtime probes and mutations will live behind separate, narrower traits so
/// that displaying a page can never implicitly acquire privileged devices.
pub trait CapabilityProvider: Send + Sync {
    fn capabilities(&self) -> CapabilityManifest;
}
