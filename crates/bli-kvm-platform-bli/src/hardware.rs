//! Source-derived H616 board profile.
//!
//! These values are migration evidence from the pinned vendor revision, not
//! physical acceptance. No code in this module opens or mutates a device.

pub const VENDOR_SOURCE_REVISION: &str = "3e34c04a1dfa84636050585d12d5e585098258ea";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum GpioRole {
    AtxPower,
    AtxReset,
    AtxPowerLed,
    AtxHddLed,
    DisplayButton,
    ActivityLed,
    FanPwm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GpioLineEvidence {
    pub role: GpioRole,
    /// Physical header position passed to the vendor ArmbianIO API.
    pub header_pin: u8,
    /// `/dev/gpiochip0` line offset selected by the vendor Mango Pi table.
    pub chip_offset: u16,
    pub active_high: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtcEvidence {
    pub i2c_bus: u8,
    pub address: u16,
    pub driver: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct H616VendorProfile {
    pub gpio_chip: &'static str,
    pub thermal_zone: &'static str,
    pub gpio_lines: &'static [GpioLineEvidence],
    pub rtc: RtcEvidence,
}

const GPIO_LINES: &[GpioLineEvidence] = &[
    GpioLineEvidence {
        role: GpioRole::AtxPower,
        header_pin: 18,
        chip_offset: 228,
        active_high: true,
    },
    GpioLineEvidence {
        role: GpioRole::AtxReset,
        header_pin: 37,
        chip_offset: 272,
        active_high: true,
    },
    GpioLineEvidence {
        role: GpioRole::AtxPowerLed,
        header_pin: 36,
        chip_offset: 234,
        active_high: true,
    },
    GpioLineEvidence {
        role: GpioRole::AtxHddLed,
        header_pin: 41,
        chip_offset: 233,
        active_high: true,
    },
    GpioLineEvidence {
        role: GpioRole::DisplayButton,
        header_pin: 12,
        chip_offset: 257,
        active_high: true,
    },
    GpioLineEvidence {
        role: GpioRole::ActivityLed,
        header_pin: 32,
        chip_offset: 261,
        active_high: true,
    },
    GpioLineEvidence {
        role: GpioRole::FanPwm,
        header_pin: 15,
        chip_offset: 269,
        active_high: true,
    },
];

pub const H616_VENDOR_PROFILE: H616VendorProfile = H616VendorProfile {
    gpio_chip: "/dev/gpiochip0",
    thermal_zone: "/sys/class/thermal/thermal_zone0/temp",
    gpio_lines: GPIO_LINES,
    rtc: RtcEvidence {
        i2c_bus: 0,
        address: 0x51,
        driver: "pcf8563",
    },
};

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn source_profile_has_unique_roles_and_chip_offsets() {
        let roles = H616_VENDOR_PROFILE
            .gpio_lines
            .iter()
            .map(|line| line.role)
            .collect::<BTreeSet<_>>();
        let offsets = H616_VENDOR_PROFILE
            .gpio_lines
            .iter()
            .map(|line| line.chip_offset)
            .collect::<BTreeSet<_>>();
        assert_eq!(roles.len(), H616_VENDOR_PROFILE.gpio_lines.len());
        assert_eq!(offsets.len(), H616_VENDOR_PROFILE.gpio_lines.len());
    }

    #[test]
    fn source_profile_matches_the_pinned_h616_mapping() {
        assert_eq!(VENDOR_SOURCE_REVISION.len(), 40);
        assert_eq!(H616_VENDOR_PROFILE.gpio_chip, "/dev/gpiochip0");
        assert_eq!(H616_VENDOR_PROFILE.rtc.address, 0x51);
        assert!(H616_VENDOR_PROFILE.gpio_lines.iter().any(|line| {
            line.role == GpioRole::AtxPower && line.header_pin == 18 && line.chip_offset == 228
        }));
    }
}
