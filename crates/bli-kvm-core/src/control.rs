use serde::{Deserialize, Serialize};
use thiserror::Error;

const HID_MOUSE_BUTTON_MASK: u8 = 0x1f;
const HID_ABSOLUTE_MAX: u16 = 0x7fff;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum HidReport {
    Keyboard { data: [u8; 8] },
    RelativeMouse { data: [u8; 4] },
    AbsoluteMouse { data: [u8; 6] },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AtxControlAction {
    PowerShort,
    PowerLong,
    Reset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum VirtualMediaCommand {
    Select {
        image_id: u64,
        mode: VirtualMediaControlMode,
        writable: bool,
    },
    SetConnected {
        connected: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VirtualMediaControlMode {
    CdRom,
    RemovableDisk,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PlatformControlRequest {
    Hid { report: HidReport },
    Atx { action: AtxControlAction },
    VirtualMedia { command: VirtualMediaCommand },
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ControlValidationError {
    #[error("keyboard report is invalid")]
    InvalidKeyboardReport,
    #[error("mouse report is invalid")]
    InvalidMouseReport,
    #[error("virtual-media command is invalid")]
    InvalidVirtualMediaCommand,
}

impl PlatformControlRequest {
    pub fn validate(&self) -> Result<(), ControlValidationError> {
        match self {
            Self::Hid { report } => report.validate(),
            Self::Atx { .. } => Ok(()),
            Self::VirtualMedia { command } => command.validate(),
        }
    }
}

impl HidReport {
    pub fn validate(&self) -> Result<(), ControlValidationError> {
        match self {
            Self::Keyboard { data } => validate_keyboard_report(data),
            Self::RelativeMouse { data } => validate_relative_mouse_report(data),
            Self::AbsoluteMouse { data } => validate_absolute_mouse_report(data),
        }
    }
}

impl VirtualMediaCommand {
    pub fn validate(&self) -> Result<(), ControlValidationError> {
        match self {
            Self::Select {
                image_id,
                mode,
                writable,
            } if *image_id == 0 || (*mode == VirtualMediaControlMode::CdRom && *writable) => {
                Err(ControlValidationError::InvalidVirtualMediaCommand)
            }
            Self::Select { .. } | Self::SetConnected { .. } => Ok(()),
        }
    }
}

fn validate_keyboard_report(data: &[u8; 8]) -> Result<(), ControlValidationError> {
    if data[1] != 0 {
        return Err(ControlValidationError::InvalidKeyboardReport);
    }
    let mut seen = [false; 256];
    for keycode in data[2..].iter().copied().filter(|keycode| *keycode != 0) {
        if keycode < 4 || seen[usize::from(keycode)] {
            return Err(ControlValidationError::InvalidKeyboardReport);
        }
        seen[usize::from(keycode)] = true;
    }
    Ok(())
}

fn validate_relative_mouse_report(data: &[u8; 4]) -> Result<(), ControlValidationError> {
    if data[0] & !HID_MOUSE_BUTTON_MASK != 0 {
        Err(ControlValidationError::InvalidMouseReport)
    } else {
        Ok(())
    }
}

fn validate_absolute_mouse_report(data: &[u8; 6]) -> Result<(), ControlValidationError> {
    let x = u16::from_le_bytes([data[1], data[2]]);
    let y = u16::from_le_bytes([data[3], data[4]]);
    if data[0] & !HID_MOUSE_BUTTON_MASK != 0 || x > HID_ABSOLUTE_MAX || y > HID_ABSOLUTE_MAX {
        Err(ControlValidationError::InvalidMouseReport)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_fixed_hid_report_shapes_and_reserved_fields() {
        assert_eq!(
            HidReport::Keyboard {
                data: [0x02, 0, 4, 5, 0, 0, 0, 0]
            }
            .validate(),
            Ok(())
        );
        for invalid in [
            [0, 1, 4, 0, 0, 0, 0, 0],
            [0, 0, 1, 0, 0, 0, 0, 0],
            [0, 0, 4, 4, 0, 0, 0, 0],
        ] {
            assert_eq!(
                HidReport::Keyboard { data: invalid }.validate(),
                Err(ControlValidationError::InvalidKeyboardReport)
            );
        }
        assert_eq!(
            HidReport::RelativeMouse {
                data: [0x1f, 127, 129, 0]
            }
            .validate(),
            Ok(())
        );
        assert_eq!(
            HidReport::RelativeMouse {
                data: [0x20, 0, 0, 0]
            }
            .validate(),
            Err(ControlValidationError::InvalidMouseReport)
        );
        assert_eq!(
            HidReport::AbsoluteMouse {
                data: [1, 0xff, 0x7f, 0, 0, 0]
            }
            .validate(),
            Ok(())
        );
        assert_eq!(
            HidReport::AbsoluteMouse {
                data: [1, 0, 0x80, 0, 0, 0]
            }
            .validate(),
            Err(ControlValidationError::InvalidMouseReport)
        );
    }

    #[test]
    fn media_uses_only_nonzero_opaque_ids_and_read_only_cdrom() {
        assert_eq!(
            VirtualMediaCommand::Select {
                image_id: 7,
                mode: VirtualMediaControlMode::CdRom,
                writable: false,
            }
            .validate(),
            Ok(())
        );
        assert_eq!(
            VirtualMediaCommand::Select {
                image_id: 0,
                mode: VirtualMediaControlMode::RemovableDisk,
                writable: false,
            }
            .validate(),
            Err(ControlValidationError::InvalidVirtualMediaCommand)
        );
        assert_eq!(
            VirtualMediaCommand::Select {
                image_id: 1,
                mode: VirtualMediaControlMode::CdRom,
                writable: true,
            }
            .validate(),
            Err(ControlValidationError::InvalidVirtualMediaCommand)
        );
    }
}
