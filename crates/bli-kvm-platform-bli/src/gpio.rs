//! Pure GPIO line-request planning for the H616 board daemon.
//!
//! This module performs no I/O. It encodes which subsystem currently owns each
//! reviewed line and refuses to plan a request for a line the requesting owner
//! does not hold, so the Rust board daemon cannot take over KVMD or legacy
//! sysfs lines before the reviewed physical handoff. The libgpiod driver that
//! consumes a plan is a separate, later slice.

use crate::hardware::{GpioRole, H616_VENDOR_PROFILE};

/// Fixed consumer label; never taken from caller input.
pub const BOARD_CONSUMER: &str = "hardened-blikvm-board";

/// Current live owner of a reviewed line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineOwner {
    Kvmd,
    LegacySysfs,
    /// Confirmed live but not claimed by any service (input only).
    Unused,
    /// The hardened Rust board daemon; holds nothing before the reviewed handoff.
    HardenedBoard,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineDirection {
    Input,
    Output,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinePlanningError {
    /// The role is not present in the reviewed board profile.
    UnknownRole,
    /// The requesting owner does not currently hold the line.
    OwnershipDenied,
    /// The same role was requested twice.
    DuplicateRole,
    /// The caller asked for a plan larger than the reviewed limit.
    TooManyLines,
}

/// Reviewed limit: the H616 profile has seven lines.
pub const MAX_PLANNED_LINES: usize = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineRequest {
    pub chip: &'static str,
    pub offset: u16,
    pub consumer: &'static str,
    pub role: GpioRole,
    pub direction: LineDirection,
    /// Source-derived polarity; not a physical acceptance.
    pub active_low: bool,
}

/// Current live owner, per the read-only line-info baseline.
pub fn current_owner(role: GpioRole) -> LineOwner {
    match role {
        GpioRole::AtxPower | GpioRole::AtxReset => LineOwner::Kvmd,
        GpioRole::AtxPowerLed | GpioRole::AtxHddLed => LineOwner::Kvmd,
        GpioRole::DisplayButton => LineOwner::Unused,
        GpioRole::ActivityLed | GpioRole::FanPwm => LineOwner::LegacySysfs,
        GpioRole::DisplayDataCommand | GpioRole::DisplayReset | GpioRole::DisplayBacklight => {
            LineOwner::LegacySysfs
        }
    }
}

/// Direction is a board fact, not caller input.
pub fn role_direction(role: GpioRole) -> LineDirection {
    match role {
        GpioRole::AtxPowerLed | GpioRole::AtxHddLed | GpioRole::DisplayButton => {
            LineDirection::Input
        }
        GpioRole::AtxPower
        | GpioRole::AtxReset
        | GpioRole::ActivityLed
        | GpioRole::FanPwm
        | GpioRole::DisplayDataCommand
        | GpioRole::DisplayReset
        | GpioRole::DisplayBacklight => LineDirection::Output,
    }
}

fn profile_line(role: GpioRole) -> Option<&'static crate::hardware::GpioLineEvidence> {
    H616_VENDOR_PROFILE
        .gpio_lines
        .iter()
        .find(|line| line.role == role)
}

/// Build a request plan for `owner`, refusing anything the owner does not hold.
pub fn plan_requests(
    owner: LineOwner,
    roles: &[GpioRole],
) -> Result<Vec<LineRequest>, LinePlanningError> {
    if roles.len() > MAX_PLANNED_LINES {
        return Err(LinePlanningError::TooManyLines);
    }

    let mut planned = Vec::with_capacity(roles.len());
    let mut seen = Vec::new();
    for role in roles {
        if seen.contains(role) {
            return Err(LinePlanningError::DuplicateRole);
        }
        seen.push(*role);

        let line = profile_line(*role).ok_or(LinePlanningError::UnknownRole)?;
        if current_owner(*role) != owner {
            return Err(LinePlanningError::OwnershipDenied);
        }
        planned.push(LineRequest {
            chip: H616_VENDOR_PROFILE.gpio_chip,
            offset: line.chip_offset,
            consumer: BOARD_CONSUMER,
            role: *role,
            direction: role_direction(*role),
            active_low: !line.active_high,
        });
    }
    Ok(planned)
}

/// Build a request for an actuator line owned by the hardened board after the
/// reviewed physical handoff. Only the ATX actuator roles are allowed; every
/// other line stays behind the ownership planner.
pub fn hardened_board_request(role: GpioRole) -> Result<LineRequest, LinePlanningError> {
    match role {
        GpioRole::AtxPower | GpioRole::AtxReset => {}
        _ => return Err(LinePlanningError::OwnershipDenied),
    }
    let line = profile_line(role).ok_or(LinePlanningError::UnknownRole)?;
    Ok(LineRequest {
        chip: H616_VENDOR_PROFILE.gpio_chip,
        offset: line.chip_offset,
        consumer: BOARD_CONSUMER,
        role,
        direction: LineDirection::Output,
        active_low: !line.active_high,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_matches_the_read_only_line_baseline() {
        assert_eq!(current_owner(GpioRole::AtxPower), LineOwner::Kvmd);
        assert_eq!(current_owner(GpioRole::AtxReset), LineOwner::Kvmd);
        assert_eq!(current_owner(GpioRole::AtxPowerLed), LineOwner::Kvmd);
        assert_eq!(current_owner(GpioRole::AtxHddLed), LineOwner::Kvmd);
        assert_eq!(current_owner(GpioRole::DisplayButton), LineOwner::Unused);
        assert_eq!(current_owner(GpioRole::ActivityLed), LineOwner::LegacySysfs);
        assert_eq!(current_owner(GpioRole::FanPwm), LineOwner::LegacySysfs);
    }

    #[test]
    fn hardened_board_request_allows_only_atx_actuators() {
        let power = hardened_board_request(GpioRole::AtxPower).unwrap();
        assert_eq!(power.offset, 228);
        assert_eq!(power.direction, LineDirection::Output);
        assert_eq!(power.consumer, BOARD_CONSUMER);
        let reset = hardened_board_request(GpioRole::AtxReset).unwrap();
        assert_eq!(reset.offset, 272);
        assert_eq!(
            hardened_board_request(GpioRole::ActivityLed),
            Err(LinePlanningError::OwnershipDenied)
        );
        assert_eq!(
            hardened_board_request(GpioRole::AtxPowerLed),
            Err(LinePlanningError::OwnershipDenied)
        );
    }

    #[test]
    fn hardened_board_is_denied_every_line_until_handoff() {
        for role in [
            GpioRole::AtxPower,
            GpioRole::AtxReset,
            GpioRole::AtxPowerLed,
            GpioRole::AtxHddLed,
            GpioRole::DisplayButton,
            GpioRole::ActivityLed,
            GpioRole::FanPwm,
        ] {
            assert_eq!(
                plan_requests(LineOwner::HardenedBoard, &[role]),
                Err(LinePlanningError::OwnershipDenied),
                "{role:?}"
            );
        }
    }

    #[test]
    fn kvmd_can_only_plan_its_own_lines() {
        let plan = plan_requests(LineOwner::Kvmd, &[GpioRole::AtxPower, GpioRole::AtxReset])
            .expect("kvmd owns power and reset");
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0].offset, 228);
        assert_eq!(plan[0].direction, LineDirection::Output);
        assert_eq!(plan[0].consumer, BOARD_CONSUMER);
        assert!(!plan[0].active_low);
        assert_eq!(
            plan_requests(LineOwner::Kvmd, &[GpioRole::ActivityLed]),
            Err(LinePlanningError::OwnershipDenied)
        );
        assert_eq!(
            plan_requests(LineOwner::Kvmd, &[GpioRole::FanPwm]),
            Err(LinePlanningError::OwnershipDenied)
        );
    }

    #[test]
    fn legacy_sysfs_owns_activity_and_fan_only() {
        let plan = plan_requests(
            LineOwner::LegacySysfs,
            &[GpioRole::ActivityLed, GpioRole::FanPwm],
        )
        .expect("legacy sysfs owns activity and fan");
        assert_eq!(plan.len(), 2);
        assert!(
            plan.iter()
                .all(|line| line.direction == LineDirection::Output)
        );
        assert_eq!(
            plan_requests(LineOwner::LegacySysfs, &[GpioRole::AtxPower]),
            Err(LinePlanningError::OwnershipDenied)
        );
    }

    #[test]
    fn unused_lines_are_claimed_by_no_owner() {
        for owner in [LineOwner::Kvmd, LineOwner::LegacySysfs, LineOwner::Unused] {
            let result = plan_requests(owner, &[GpioRole::DisplayButton]);
            if owner == LineOwner::Unused {
                let plan = result.expect("unused owner may plan the unused input");
                assert_eq!(plan[0].direction, LineDirection::Input);
            } else {
                assert_eq!(result, Err(LinePlanningError::OwnershipDenied));
            }
        }
    }

    #[test]
    fn duplicate_and_oversized_plans_are_rejected() {
        assert_eq!(
            plan_requests(LineOwner::Kvmd, &[GpioRole::AtxPower, GpioRole::AtxPower]),
            Err(LinePlanningError::DuplicateRole)
        );
        let too_many = [
            GpioRole::AtxPower,
            GpioRole::AtxReset,
            GpioRole::AtxPowerLed,
            GpioRole::AtxHddLed,
            GpioRole::ActivityLed,
            GpioRole::FanPwm,
            GpioRole::DisplayButton,
            GpioRole::AtxPower,
        ];
        assert_eq!(
            plan_requests(LineOwner::Kvmd, &too_many),
            Err(LinePlanningError::TooManyLines)
        );
    }
}
