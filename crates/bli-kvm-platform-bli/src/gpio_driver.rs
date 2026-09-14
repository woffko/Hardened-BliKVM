//! Transactional GPIO line application over a driver seam.
//!
//! This module performs no I/O. It defines the driver interface the board
//! daemon uses and enforces the ownership transaction: either every planned
//! line is requested, or the already-requested lines are released in reverse
//! order before the failure is reported. That keeps the board from ending up
//! with a half-acquired line set during a handoff. The concrete libgpiod
//! adapter is a separate, physically gated slice.

use crate::gpio::LineRequest;

/// Minimal line driver seam. The libgpiod adapter implements this; tests use a
/// deterministic mock.
pub trait GpioDriver {
    type Error: std::fmt::Debug;

    fn request_line(&mut self, request: &LineRequest) -> Result<(), Self::Error>;

    /// Drive the requested line. `active` is the logical level; the driver is
    /// responsible for the polarity recorded in the request.
    fn write_line(&mut self, request: &LineRequest, active: bool) -> Result<(), Self::Error>;

    fn release_line(&mut self, request: &LineRequest) -> Result<(), Self::Error>;
}

#[derive(Debug, PartialEq, Eq)]
pub enum ApplyError<E> {
    /// Requesting a line failed; any earlier lines were released.
    Driver(E),
    /// Requesting a line failed and releasing an earlier line also failed.
    RollbackFailed { apply: E, rollback: E },
}

/// Request every planned line, releasing earlier lines in reverse order if a
/// later request fails.
pub fn apply_plan<D: GpioDriver>(
    driver: &mut D,
    plan: &[LineRequest],
) -> Result<(), ApplyError<D::Error>> {
    let mut applied: Vec<&LineRequest> = Vec::with_capacity(plan.len());
    for request in plan {
        match driver.request_line(request) {
            Ok(()) => applied.push(request),
            Err(apply) => {
                for done in applied.iter().rev() {
                    if let Err(rollback) = driver.release_line(done) {
                        return Err(ApplyError::RollbackFailed { apply, rollback });
                    }
                }
                return Err(ApplyError::Driver(apply));
            }
        }
    }
    Ok(())
}

/// Release every planned line in reverse request order.
pub fn release_plan<D: GpioDriver>(driver: &mut D, plan: &[LineRequest]) -> Result<(), D::Error> {
    for request in plan.iter().rev() {
        driver.release_line(request)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpio::{LineOwner, plan_requests};
    use crate::hardware::GpioRole;

    #[derive(Default)]
    struct MockDriver {
        fail_request_at: Option<usize>,
        fail_release_at: Option<usize>,
        fail_write_at: Option<usize>,
        requests: Vec<u16>,
        writes: Vec<(u16, bool)>,
        releases: Vec<u16>,
    }

    impl MockDriver {
        fn failing_request(index: usize) -> Self {
            Self {
                fail_request_at: Some(index),
                ..Self::default()
            }
        }

        fn failing_release(index: usize) -> Self {
            Self {
                fail_release_at: Some(index),
                ..Self::default()
            }
        }
    }

    impl GpioDriver for MockDriver {
        type Error = u16;

        fn request_line(&mut self, request: &LineRequest) -> Result<(), Self::Error> {
            if self.fail_request_at == Some(self.requests.len()) {
                return Err(request.offset);
            }
            self.requests.push(request.offset);
            Ok(())
        }

        fn write_line(&mut self, request: &LineRequest, active: bool) -> Result<(), Self::Error> {
            if self.fail_write_at == Some(self.writes.len()) {
                return Err(request.offset);
            }
            self.writes.push((request.offset, active));
            Ok(())
        }

        fn release_line(&mut self, request: &LineRequest) -> Result<(), Self::Error> {
            if self.fail_release_at == Some(self.releases.len()) {
                return Err(request.offset);
            }
            self.releases.push(request.offset);
            Ok(())
        }
    }

    fn kvmd_plan() -> Vec<LineRequest> {
        plan_requests(LineOwner::Kvmd, &[GpioRole::AtxPower, GpioRole::AtxReset]).unwrap()
    }

    #[test]
    fn apply_requests_every_line_in_order() {
        let mut driver = MockDriver::default();
        let plan = kvmd_plan();
        assert_eq!(apply_plan(&mut driver, &plan), Ok(()));
        assert_eq!(driver.requests, vec![228, 272]);
        assert!(driver.releases.is_empty());
    }

    #[test]
    fn apply_rolls_back_earlier_lines_on_failure() {
        let mut driver = MockDriver::failing_request(1);
        let plan = kvmd_plan();
        assert_eq!(apply_plan(&mut driver, &plan), Err(ApplyError::Driver(272)));
        assert_eq!(driver.requests, vec![228]);
        assert_eq!(driver.releases, vec![228]);
    }

    #[test]
    fn apply_reports_a_failed_rollback() {
        let mut driver = MockDriver {
            fail_request_at: Some(2),
            fail_release_at: Some(1),
            ..MockDriver::default()
        };
        let mut plan = kvmd_plan();
        plan.push(plan_requests(LineOwner::Kvmd, &[GpioRole::AtxPowerLed]).unwrap()[0]);
        assert_eq!(
            apply_plan(&mut driver, &plan),
            Err(ApplyError::RollbackFailed {
                apply: 234,
                rollback: 228,
            })
        );
        assert_eq!(driver.requests, vec![228, 272]);
        assert_eq!(driver.releases, vec![272]);
    }

    #[test]
    fn release_happens_in_reverse_order() {
        let mut driver = MockDriver::default();
        let plan = kvmd_plan();
        assert_eq!(release_plan(&mut driver, &plan), Ok(()));
        assert_eq!(driver.releases, vec![272, 228]);
    }

    #[test]
    fn release_reports_the_first_failure() {
        let mut driver = MockDriver::failing_release(0);
        let plan = kvmd_plan();
        assert_eq!(release_plan(&mut driver, &plan), Err(272));
        assert!(driver.releases.is_empty());
    }
}
