//! Frame admission: skip unchanged captures and enforce a frame-rate ceiling.
//!
//! A desktop is mostly static, so comparing the compressed capture bytes
//! avoids decoding and encoding identical frames. A periodic refresh still
//! admits an unchanged frame so a newly joined or lossy viewer converges.

use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GatePolicy {
    /// Minimum spacing between admitted frames (the frame-rate ceiling).
    pub min_interval: Duration,
    /// Admit an unchanged frame at least this often.
    pub refresh_interval: Duration,
}

impl GatePolicy {
    /// Valid policies cap at 60 fps and refresh within 10 seconds.
    pub fn new(max_fps: u32, refresh_interval: Duration) -> Option<Self> {
        if !(1..=60).contains(&max_fps)
            || refresh_interval.is_zero()
            || refresh_interval > Duration::from_secs(10)
        {
            return None;
        }
        let min_interval = Duration::from_secs(1) / max_fps;
        (refresh_interval >= min_interval).then_some(Self {
            min_interval,
            refresh_interval,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameDecision {
    /// Encode this frame.
    Encode,
    /// Identical to the last admitted frame and no refresh is due.
    Unchanged,
    /// Arrived before the frame-rate ceiling allows another frame.
    TooSoon,
}

#[derive(Debug)]
pub struct FrameGate {
    policy: GatePolicy,
    last: Option<(Instant, [u8; 32])>,
}

impl FrameGate {
    pub fn new(policy: GatePolicy) -> Self {
        Self { policy, last: None }
    }

    /// Decide for one captured frame; `now` must not go backwards.
    pub fn admit(&mut self, jpeg: &[u8], now: Instant) -> FrameDecision {
        let digest: [u8; 32] = Sha256::digest(jpeg).into();
        if let Some((at, previous)) = self.last {
            let elapsed = now.saturating_duration_since(at);
            if elapsed < self.policy.min_interval {
                return FrameDecision::TooSoon;
            }
            if previous == digest && elapsed < self.policy.refresh_interval {
                return FrameDecision::Unchanged;
            }
        }
        self.last = Some((now, digest));
        FrameDecision::Encode
    }

    /// Forget history so the next frame is admitted, e.g. after a viewer
    /// requested a key frame.
    pub fn reset(&mut self) {
        self.last = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policies_are_bounded() {
        assert!(GatePolicy::new(0, Duration::from_secs(1)).is_none());
        assert!(GatePolicy::new(61, Duration::from_secs(1)).is_none());
        assert!(GatePolicy::new(30, Duration::ZERO).is_none());
        assert!(GatePolicy::new(30, Duration::from_secs(11)).is_none());
        // Refresh cannot be shorter than the frame interval.
        assert!(GatePolicy::new(1, Duration::from_millis(500)).is_none());
        let policy = GatePolicy::new(30, Duration::from_secs(2)).unwrap();
        assert_eq!(policy.min_interval, Duration::from_secs(1) / 30);
    }

    #[test]
    fn unchanged_frames_are_skipped_until_refresh_and_rate_is_capped() {
        let policy = GatePolicy::new(10, Duration::from_secs(1)).unwrap();
        let mut gate = FrameGate::new(policy);
        let start = Instant::now();
        assert_eq!(gate.admit(b"a", start), FrameDecision::Encode);
        assert_eq!(
            gate.admit(b"b", start + Duration::from_millis(50)),
            FrameDecision::TooSoon
        );
        assert_eq!(
            gate.admit(b"a", start + Duration::from_millis(150)),
            FrameDecision::Unchanged
        );
        assert_eq!(
            gate.admit(b"b", start + Duration::from_millis(200)),
            FrameDecision::Encode
        );
        assert_eq!(
            gate.admit(b"b", start + Duration::from_millis(1100)),
            FrameDecision::Unchanged
        );
        assert_eq!(
            gate.admit(b"b", start + Duration::from_millis(1200)),
            FrameDecision::Encode
        );
        gate.reset();
        assert_eq!(
            gate.admit(b"b", start + Duration::from_millis(1210)),
            FrameDecision::Encode
        );
    }
}
