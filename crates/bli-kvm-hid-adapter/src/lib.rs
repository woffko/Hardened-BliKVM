//! Stateful translation from fixed USB HID reports to typed KVMD events.
//!
//! The table matches the supported `KEYMAP` in the pinned KVMD 3.291 field
//! image. This crate performs no I/O and does not enable the HID capability.

mod lease;

pub use lease::{
    HidLeaseAcquisition, HidLeaseBatch, HidLeaseError, HidLeaseIdentity, HidLeaseManager,
    MAX_HID_LEASE_TTL,
};

use bli_kvm_core::HidReport;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct KvmdKeyCode {
    usb_usage: u8,
    name: &'static str,
}

impl KvmdKeyCode {
    pub fn from_usb_usage(code: u8) -> Option<Self> {
        if (0xe0..=0xe7).contains(&code) {
            modifier_name(code - 0xe0)
        } else {
            key_name(code)
        }
    }

    pub const fn as_str(self) -> &'static str {
        self.name
    }

    pub const fn usb_usage(self) -> u8 {
        self.usb_usage
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KvmdMouseButton {
    Left,
    Right,
    Middle,
}

impl KvmdMouseButton {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Middle => "middle",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KvmdHidEvent {
    Key {
        key: KvmdKeyCode,
        pressed: bool,
    },
    MouseButton {
        button: KvmdMouseButton,
        pressed: bool,
    },
    MouseMove {
        x: i16,
        y: i16,
    },
    MouseRelative {
        x: i8,
        y: i8,
    },
    MouseWheel {
        x: i8,
        y: i8,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HidTranslationError {
    InvalidReport,
    UnsupportedKeycode,
    UnsupportedMouseButton,
    TooManyKeys,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HidTranslationBatch {
    pub events: Vec<KvmdHidEvent>,
    pub error: Option<HidTranslationError>,
}

impl HidTranslationBatch {
    pub fn accepted(&self) -> bool {
        self.error.is_none()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MouseMode {
    Relative,
    Absolute,
}

#[derive(Debug, Default)]
pub struct HidReportTranslator {
    keyboard: Vec<KvmdKeyCode>,
    mouse_buttons: u8,
    mouse_mode: Option<MouseMode>,
    absolute_position: Option<(u16, u16)>,
}

impl HidReportTranslator {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn translate(&mut self, report: &HidReport) -> HidTranslationBatch {
        if report.validate().is_err() {
            return self.reject(HidTranslationError::InvalidReport);
        }
        match report {
            HidReport::Keyboard { data } => self.translate_keyboard(data),
            HidReport::RelativeMouse { data } => self.translate_relative_mouse(data),
            HidReport::AbsoluteMouse { data } => self.translate_absolute_mouse(data),
        }
    }

    pub fn release_all(&mut self) -> Vec<KvmdHidEvent> {
        let events = self.release_snapshot();
        self.keyboard.clear();
        self.mouse_buttons = 0;
        self.mouse_mode = None;
        self.absolute_position = None;
        events
    }

    pub fn release_snapshot(&self) -> Vec<KvmdHidEvent> {
        let mut events = self
            .keyboard
            .iter()
            .rev()
            .copied()
            .map(|key| KvmdHidEvent::Key {
                key,
                pressed: false,
            })
            .collect::<Vec<_>>();
        for (mask, button) in mouse_buttons() {
            if self.mouse_buttons & mask != 0 {
                events.push(KvmdHidEvent::MouseButton {
                    button,
                    pressed: false,
                });
            }
        }
        events
    }

    fn translate_keyboard(&mut self, data: &[u8; 8]) -> HidTranslationBatch {
        let Some(current) = keyboard_state(data) else {
            return self.reject(HidTranslationError::UnsupportedKeycode);
        };
        let mut events = Vec::new();
        for key in self.keyboard.iter().rev().copied() {
            if !current.contains(&key) {
                events.push(KvmdHidEvent::Key {
                    key,
                    pressed: false,
                });
            }
        }
        for key in current.iter().copied() {
            if !self.keyboard.contains(&key) {
                events.push(KvmdHidEvent::Key { key, pressed: true });
            }
        }
        self.keyboard = current;
        HidTranslationBatch {
            events,
            error: None,
        }
    }

    fn translate_relative_mouse(&mut self, data: &[u8; 4]) -> HidTranslationBatch {
        let mut events = match self.translate_mouse_buttons(data[0]) {
            Ok(events) => events,
            Err(error) => return self.reject(error),
        };
        let x = data[1] as i8;
        let y = data[2] as i8;
        let wheel = data[3] as i8;
        if x != 0 || y != 0 {
            events.push(KvmdHidEvent::MouseRelative { x, y });
        }
        if wheel != 0 {
            events.push(KvmdHidEvent::MouseWheel { x: 0, y: wheel });
        }
        self.mouse_mode = Some(MouseMode::Relative);
        HidTranslationBatch {
            events,
            error: None,
        }
    }

    fn translate_absolute_mouse(&mut self, data: &[u8; 6]) -> HidTranslationBatch {
        let mut events = match self.translate_mouse_buttons(data[0]) {
            Ok(events) => events,
            Err(error) => return self.reject(error),
        };
        let raw_x = u16::from_le_bytes([data[1], data[2]]);
        let raw_y = u16::from_le_bytes([data[3], data[4]]);
        let current = (raw_x, raw_y);
        if self.mouse_mode != Some(MouseMode::Absolute) || self.absolute_position != Some(current) {
            events.push(KvmdHidEvent::MouseMove {
                x: map_absolute(raw_x),
                y: map_absolute(raw_y),
            });
        }
        let wheel = data[5] as i8;
        if wheel != 0 {
            events.push(KvmdHidEvent::MouseWheel { x: 0, y: wheel });
        }
        self.mouse_mode = Some(MouseMode::Absolute);
        self.absolute_position = Some(current);
        HidTranslationBatch {
            events,
            error: None,
        }
    }

    fn translate_mouse_buttons(
        &mut self,
        current: u8,
    ) -> Result<Vec<KvmdHidEvent>, HidTranslationError> {
        if current & !0x07 != 0 {
            return Err(HidTranslationError::UnsupportedMouseButton);
        }
        let mut events = Vec::new();
        for (mask, button) in mouse_buttons() {
            if self.mouse_buttons & mask != 0 && current & mask == 0 {
                events.push(KvmdHidEvent::MouseButton {
                    button,
                    pressed: false,
                });
            }
        }
        for (mask, button) in mouse_buttons() {
            if self.mouse_buttons & mask == 0 && current & mask != 0 {
                events.push(KvmdHidEvent::MouseButton {
                    button,
                    pressed: true,
                });
            }
        }
        self.mouse_buttons = current;
        Ok(events)
    }

    fn reject(&mut self, error: HidTranslationError) -> HidTranslationBatch {
        HidTranslationBatch {
            events: self.release_all(),
            error: Some(error),
        }
    }
}

fn mouse_buttons() -> [(u8, KvmdMouseButton); 3] {
    [
        (0x01, KvmdMouseButton::Left),
        (0x02, KvmdMouseButton::Right),
        (0x04, KvmdMouseButton::Middle),
    ]
}

fn keyboard_state(data: &[u8; 8]) -> Option<Vec<KvmdKeyCode>> {
    let mut keys = Vec::new();
    for bit in 0..8 {
        if data[0] & (1 << bit) != 0 {
            keys.push(modifier_name(bit)?);
        }
    }
    for code in data[2..].iter().copied().filter(|code| *code != 0) {
        keys.push(key_name(code)?);
    }
    Some(keys)
}

fn modifier_name(bit: u8) -> Option<KvmdKeyCode> {
    Some(KvmdKeyCode {
        usb_usage: 0xe0 + bit,
        name: match bit {
            0 => "ControlLeft",
            1 => "ShiftLeft",
            2 => "AltLeft",
            3 => "MetaLeft",
            4 => "ControlRight",
            5 => "ShiftRight",
            6 => "AltRight",
            7 => "MetaRight",
            _ => return None,
        },
    })
}

fn key_name(code: u8) -> Option<KvmdKeyCode> {
    let name = match code {
        4..=29 => LETTER_KEYS[usize::from(code - 4)],
        30..=39 => DIGIT_KEYS[usize::from(code - 30)],
        40..=49 => BASIC_KEYS[usize::from(code - 40)],
        51..=57 => PUNCTUATION_KEYS[usize::from(code - 51)],
        58..=69 => FUNCTION_KEYS[usize::from(code - 58)],
        70 => "PrintScreen",
        71 => "ScrollLock",
        72 => "Pause",
        73 => "Insert",
        74 => "Home",
        75 => "PageUp",
        76 => "Delete",
        77 => "End",
        78 => "PageDown",
        79 => "ArrowRight",
        80 => "ArrowLeft",
        81 => "ArrowDown",
        82 => "ArrowUp",
        83 => "NumLock",
        84 => "NumpadDivide",
        85 => "NumpadMultiply",
        86 => "NumpadSubtract",
        87 => "NumpadAdd",
        88 => "NumpadEnter",
        89..=97 => NUMPAD_DIGITS[usize::from(code - 89)],
        98 => "Numpad0",
        99 => "NumpadDecimal",
        100 => "IntlBackslash",
        101 => "ContextMenu",
        102 => "Power",
        135 => "IntlRo",
        136 => "KanaMode",
        137 => "IntlYen",
        138 => "Convert",
        139 => "NonConvert",
        _ => return None,
    };
    Some(KvmdKeyCode {
        usb_usage: code,
        name,
    })
}

fn map_absolute(value: u16) -> i16 {
    let mapped = (i32::from(value) * 65_535 / 32_767) - 32_768;
    mapped as i16
}

const LETTER_KEYS: [&str; 26] = [
    "KeyA", "KeyB", "KeyC", "KeyD", "KeyE", "KeyF", "KeyG", "KeyH", "KeyI", "KeyJ", "KeyK", "KeyL",
    "KeyM", "KeyN", "KeyO", "KeyP", "KeyQ", "KeyR", "KeyS", "KeyT", "KeyU", "KeyV", "KeyW", "KeyX",
    "KeyY", "KeyZ",
];
const DIGIT_KEYS: [&str; 10] = [
    "Digit1", "Digit2", "Digit3", "Digit4", "Digit5", "Digit6", "Digit7", "Digit8", "Digit9",
    "Digit0",
];
const BASIC_KEYS: [&str; 10] = [
    "Enter",
    "Escape",
    "Backspace",
    "Tab",
    "Space",
    "Minus",
    "Equal",
    "BracketLeft",
    "BracketRight",
    "Backslash",
];
const PUNCTUATION_KEYS: [&str; 7] = [
    "Semicolon",
    "Quote",
    "Backquote",
    "Comma",
    "Period",
    "Slash",
    "CapsLock",
];
const FUNCTION_KEYS: [&str; 12] = [
    "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12",
];
const NUMPAD_DIGITS: [&str; 9] = [
    "Numpad1", "Numpad2", "Numpad3", "Numpad4", "Numpad5", "Numpad6", "Numpad7", "Numpad8",
    "Numpad9",
];

/// Inverse translation for the USB boot keyboard interface.
///
/// Composes the standard 8-byte boot report (modifier byte, reserved byte, six
/// keycodes) from lease events, tracking held keys and modifiers. The report
/// layout is the USB HID boot protocol; no device-specific descriptor guess is
/// involved for the keyboard. Mouse composition waits on the captured device
/// descriptors.
#[derive(Debug, Default)]
pub struct KeyboardReportComposer {
    modifiers: u8,
    keys: Vec<u8>,
}

const BOOT_KEYBOARD_MAX_KEYS: usize = 6;

impl KeyboardReportComposer {
    pub const fn new() -> Self {
        Self {
            modifiers: 0,
            keys: Vec::new(),
        }
    }

    /// Apply one event. Returns `Some(report)` when the keyboard changed and
    /// `None` for events that do not affect the keyboard.
    pub fn apply(&mut self, event: &KvmdHidEvent) -> Result<Option<[u8; 8]>, HidTranslationError> {
        let KvmdHidEvent::Key { key, pressed } = event else {
            return Ok(None);
        };
        let usage = key.usb_usage();
        if (0xe0..=0xe7).contains(&usage) {
            let bit = 1_u8 << (usage - 0xe0);
            if *pressed {
                self.modifiers |= bit;
            } else {
                self.modifiers &= !bit;
            }
        } else if *pressed {
            if !self.keys.contains(&usage) {
                if self.keys.len() >= BOOT_KEYBOARD_MAX_KEYS {
                    return Err(HidTranslationError::TooManyKeys);
                }
                self.keys.push(usage);
            }
        } else {
            self.keys.retain(|held| *held != usage);
        }
        Ok(Some(self.report()))
    }

    /// Release every held key and modifier, returning the empty report.
    pub fn release_all(&mut self) -> [u8; 8] {
        self.modifiers = 0;
        self.keys.clear();
        self.report()
    }

    fn report(&self) -> [u8; 8] {
        let mut report = [0_u8; 8];
        report[0] = self.modifiers;
        for (slot, key) in self.keys.iter().enumerate() {
            report[2 + slot] = *key;
        }
        report
    }
}

/// Inverse translation for the pinned pointer interfaces (`hid.usb1` absolute,
/// `hid.usb2` relative).
///
/// Both layouts and the absolute remapping follow KVMD
/// (`kvmd.plugins.hid.otg.events.make_mouse_report` and
/// `kdmd.plugins.hid.otg.events.MouseMoveEvent`): the absolute interface packs
/// `<BHHbb` (buttons, 16-bit X/Y in `0..=32767`, then wheel Y, wheel X) and the
/// relative interface packs `<Bbbbb` (buttons, signed 8-bit X, Y, wheel Y,
/// wheel X). The KVMD absolute range is remapped with integer floor division:
/// `remap(v, -32768, 32767, 0, 32767)`.
pub const ABSOLUTE_MOUSE_REPORT_LENGTH: usize = 7;
pub const RELATIVE_MOUSE_REPORT_LENGTH: usize = 5;

/// KVMD integer remapping from the mouse-move range to the report range.
pub fn kvmd_absolute_remap(value: i16) -> u16 {
    let in_min = i32::from(i16::MIN);
    let in_max = i32::from(i16::MAX);
    let out_min = 0;
    let out_max = i32::from(i16::MAX);
    let scaled = (i32::from(value) - in_min) * (out_max - out_min);
    let remapped = scaled.div_euclid(in_max - in_min) + out_min;
    remapped.clamp(out_min, out_max) as u16
}

/// Composes reports for the pinned absolute-mouse interface (`hid.usb1`).
#[derive(Debug, Default)]
pub struct AbsolutePointerReportComposer {
    buttons: u8,
    position: (u16, u16),
    wheel: (i8, i8),
}

impl AbsolutePointerReportComposer {
    pub const fn new() -> Self {
        Self {
            buttons: 0,
            position: (0, 0),
            wheel: (0, 0),
        }
    }

    /// Apply one event; `Some(report)` for absolute-mouse events, `None` for
    /// relative-mouse and keyboard events.
    pub fn apply(&mut self, event: &KvmdHidEvent) -> Option<[u8; ABSOLUTE_MOUSE_REPORT_LENGTH]> {
        match event {
            KvmdHidEvent::MouseButton { button, pressed } => {
                self.set_button(button, *pressed);
            }
            KvmdHidEvent::MouseMove { x, y } => {
                self.position = (kvmd_absolute_remap(*x), kvmd_absolute_remap(*y));
            }
            KvmdHidEvent::MouseWheel { x, y } => {
                self.wheel = (*y, *x);
            }
            KvmdHidEvent::MouseRelative { .. } | KvmdHidEvent::Key { .. } => return None,
        }
        Some(self.report())
    }

    fn set_button(&mut self, button: &KvmdMouseButton, pressed: bool) {
        let bit = mouse_button_bit(button);
        if pressed {
            self.buttons |= bit;
        } else {
            self.buttons &= !bit;
        }
    }

    fn report(&self) -> [u8; ABSOLUTE_MOUSE_REPORT_LENGTH] {
        let mut report = [0_u8; ABSOLUTE_MOUSE_REPORT_LENGTH];
        report[0] = self.buttons;
        report[1..3].copy_from_slice(&self.position.0.to_le_bytes());
        report[3..5].copy_from_slice(&self.position.1.to_le_bytes());
        report[5] = self.wheel.0 as u8;
        report[6] = self.wheel.1 as u8;
        report
    }

    /// Release every held button at the current position.
    pub fn release_all(&mut self) -> [u8; ABSOLUTE_MOUSE_REPORT_LENGTH] {
        self.buttons = 0;
        let mut report = self.report();
        self.wheel = (0, 0);
        report[5] = 0;
        report[6] = 0;
        report
    }
}

/// Composes reports for the pinned relative-mouse interface (`hid.usb2`).
#[derive(Debug, Default)]
pub struct RelativePointerReportComposer {
    buttons: u8,
}

impl RelativePointerReportComposer {
    pub const fn new() -> Self {
        Self { buttons: 0 }
    }

    /// Apply one event; `Some(report)` for relative-mouse events, `None` for
    /// absolute-mouse and keyboard events.
    pub fn apply(&mut self, event: &KvmdHidEvent) -> Option<[u8; RELATIVE_MOUSE_REPORT_LENGTH]> {
        let mut report = [0_u8; RELATIVE_MOUSE_REPORT_LENGTH];
        report[0] = self.buttons;
        match event {
            KvmdHidEvent::MouseButton { button, pressed } => {
                let bit = mouse_button_bit(button);
                if *pressed {
                    self.buttons |= bit;
                } else {
                    self.buttons &= !bit;
                }
                report[0] = self.buttons;
            }
            KvmdHidEvent::MouseRelative { x, y } => {
                report[1] = clamp_delta(*x);
                report[2] = clamp_delta(*y);
            }
            KvmdHidEvent::MouseWheel { x, y } => {
                // KVMD writes the vertical wheel before the horizontal one.
                report[3] = clamp_delta(*y);
                report[4] = clamp_delta(*x);
            }
            KvmdHidEvent::MouseMove { .. } | KvmdHidEvent::Key { .. } => return None,
        }
        Some(report)
    }

    /// Release every held button.
    pub fn release_all(&mut self) -> [u8; RELATIVE_MOUSE_REPORT_LENGTH] {
        self.buttons = 0;
        [0_u8; RELATIVE_MOUSE_REPORT_LENGTH]
    }
}

fn mouse_button_bit(button: &KvmdMouseButton) -> u8 {
    match button {
        KvmdMouseButton::Left => 0x01,
        KvmdMouseButton::Right => 0x02,
        KvmdMouseButton::Middle => 0x04,
    }
}

/// KVMD requires pointer deltas in `-127..=127`; `-128` is not representable.
fn clamp_delta(value: i8) -> u8 {
    value.clamp(-127, 127) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(name: &'static str, pressed: bool) -> KvmdHidEvent {
        let usage = match name {
            "ControlLeft" => 0xe0,
            "ShiftLeft" => 0xe1,
            "KeyA" => 4,
            "KeyB" => 5,
            _ => panic!("unsupported test key"),
        };
        KvmdHidEvent::Key {
            key: KvmdKeyCode::from_usb_usage(usage).unwrap(),
            pressed,
        }
    }

    #[test]
    fn keyboard_releases_before_pressing_and_suppresses_unchanged_state() {
        let mut translator = HidReportTranslator::new();
        assert_eq!(
            translator
                .translate(&HidReport::Keyboard {
                    data: [0x02, 0, 4, 0, 0, 0, 0, 0]
                })
                .events,
            vec![key("ShiftLeft", true), key("KeyA", true)]
        );
        assert!(
            translator
                .translate(&HidReport::Keyboard {
                    data: [0x02, 0, 4, 0, 0, 0, 0, 0]
                })
                .events
                .is_empty()
        );
        assert_eq!(
            translator
                .translate(&HidReport::Keyboard {
                    data: [0, 0, 5, 0, 0, 0, 0, 0]
                })
                .events,
            vec![
                key("KeyA", false),
                key("ShiftLeft", false),
                key("KeyB", true)
            ]
        );
    }

    #[test]
    fn unknown_or_invalid_keyboard_report_releases_all_state() {
        let mut translator = HidReportTranslator::new();
        let _ = translator.translate(&HidReport::Keyboard {
            data: [0x01, 0, 4, 0, 0, 0, 0, 0],
        });
        let rejected = translator.translate(&HidReport::Keyboard {
            data: [0, 0, 50, 0, 0, 0, 0, 0],
        });
        assert_eq!(
            rejected.error,
            Some(HidTranslationError::UnsupportedKeycode)
        );
        assert_eq!(
            rejected.events,
            vec![key("KeyA", false), key("ControlLeft", false)]
        );
        assert!(translator.release_all().is_empty());
    }

    #[test]
    fn relative_mouse_translates_buttons_move_and_wheel() {
        let mut translator = HidReportTranslator::new();
        let batch = translator.translate(&HidReport::RelativeMouse {
            data: [0x01, 5, 0xfb, 0xff],
        });
        assert_eq!(
            batch.events,
            vec![
                KvmdHidEvent::MouseButton {
                    button: KvmdMouseButton::Left,
                    pressed: true,
                },
                KvmdHidEvent::MouseRelative { x: 5, y: -5 },
                KvmdHidEvent::MouseWheel { x: 0, y: -1 },
            ]
        );
    }

    #[test]
    fn absolute_mouse_maps_full_coordinate_range_and_mode_switch() {
        let mut translator = HidReportTranslator::new();
        let first = translator.translate(&HidReport::AbsoluteMouse {
            data: [0, 0, 0, 0xff, 0x7f, 0],
        });
        assert_eq!(
            first.events,
            vec![KvmdHidEvent::MouseMove {
                x: i16::MIN,
                y: i16::MAX,
            }]
        );
        assert!(
            translator
                .translate(&HidReport::AbsoluteMouse {
                    data: [0, 0, 0, 0xff, 0x7f, 0]
                })
                .events
                .is_empty()
        );
        let _ = translator.translate(&HidReport::RelativeMouse { data: [0, 0, 0, 0] });
        assert_eq!(
            translator
                .translate(&HidReport::AbsoluteMouse {
                    data: [0, 0, 0, 0xff, 0x7f, 0]
                })
                .events
                .len(),
            1
        );
    }

    #[test]
    fn unsupported_back_button_releases_held_mouse_and_keyboard_state() {
        let mut translator = HidReportTranslator::new();
        let _ = translator.translate(&HidReport::Keyboard {
            data: [0, 0, 4, 0, 0, 0, 0, 0],
        });
        let _ = translator.translate(&HidReport::RelativeMouse { data: [1, 0, 0, 0] });
        let rejected = translator.translate(&HidReport::RelativeMouse {
            data: [0x08, 0, 0, 0],
        });
        assert_eq!(
            rejected.error,
            Some(HidTranslationError::UnsupportedMouseButton)
        );
        assert_eq!(
            rejected.events,
            vec![
                key("KeyA", false),
                KvmdHidEvent::MouseButton {
                    button: KvmdMouseButton::Left,
                    pressed: false,
                }
            ]
        );
    }

    #[test]
    fn keyboard_composer_builds_boot_reports() {
        let mut composer = KeyboardReportComposer::new();
        let key = |usage: u8, pressed: bool| KvmdHidEvent::Key {
            key: KvmdKeyCode::from_usb_usage(usage).unwrap(),
            pressed,
        };

        assert_eq!(
            composer.apply(&key(0x04, true)).unwrap(),
            Some([0, 0, 4, 0, 0, 0, 0, 0])
        );
        assert_eq!(
            composer.apply(&key(0x05, true)).unwrap(),
            Some([0, 0, 4, 5, 0, 0, 0, 0])
        );
        assert_eq!(
            composer.apply(&key(0xe1, true)).unwrap(),
            Some([0x02, 0, 4, 5, 0, 0, 0, 0])
        );
        assert_eq!(
            composer.apply(&key(0x04, false)).unwrap(),
            Some([0x02, 0, 5, 0, 0, 0, 0, 0])
        );
        assert_eq!(
            composer.apply(&key(0x05, false)).unwrap(),
            Some([0x02, 0, 0, 0, 0, 0, 0, 0])
        );
        assert_eq!(composer.release_all(), [0, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn keyboard_composer_rejects_a_seventh_key_and_ignores_mouse() {
        let mut composer = KeyboardReportComposer::new();
        for usage in 0x04..=0x09 {
            let _ = composer.apply(&KvmdHidEvent::Key {
                key: KvmdKeyCode::from_usb_usage(usage).unwrap(),
                pressed: true,
            });
        }
        assert_eq!(
            composer.apply(&KvmdHidEvent::Key {
                key: KvmdKeyCode::from_usb_usage(0x0a).unwrap(),
                pressed: true,
            }),
            Err(HidTranslationError::TooManyKeys)
        );
        assert_eq!(
            composer.apply(&KvmdHidEvent::MouseRelative { x: 1, y: 2 }),
            Ok(None)
        );
    }

    #[test]
    fn relative_composer_builds_hid_usb2_reports() {
        let mut composer = RelativePointerReportComposer::new();
        assert_eq!(
            composer.apply(&KvmdHidEvent::MouseRelative { x: -2, y: 3 }),
            Some([0, 0xfe, 0x03, 0, 0])
        );
        assert_eq!(
            composer.apply(&KvmdHidEvent::MouseButton {
                button: KvmdMouseButton::Left,
                pressed: true,
            }),
            Some([0x01, 0, 0, 0, 0])
        );
        // Vertical wheel precedes the horizontal one in the KVMD layout.
        assert_eq!(
            composer.apply(&KvmdHidEvent::MouseWheel { x: 1, y: -1 }),
            Some([0x01, 0, 0, 0xff, 0x01])
        );
        assert_eq!(
            composer.apply(&KvmdHidEvent::MouseButton {
                button: KvmdMouseButton::Left,
                pressed: false,
            }),
            Some([0x00, 0, 0, 0, 0])
        );
        assert_eq!(composer.release_all(), [0; RELATIVE_MOUSE_REPORT_LENGTH]);
    }

    #[test]
    fn absolute_composer_builds_hid_usb1_reports() {
        let mut composer = AbsolutePointerReportComposer::new();
        assert_eq!(
            composer.apply(&KvmdHidEvent::MouseMove {
                x: i16::MIN,
                y: i16::MAX,
            }),
            Some([0, 0x00, 0x00, 0xff, 0x7f, 0, 0])
        );
        assert_eq!(
            composer.apply(&KvmdHidEvent::MouseMove { x: 0, y: 0 }),
            Some([0, 0xff, 0x3f, 0xff, 0x3f, 0, 0])
        );
        assert_eq!(
            composer.apply(&KvmdHidEvent::MouseWheel { x: 1, y: -1 }),
            Some([0, 0xff, 0x3f, 0xff, 0x3f, 0xff, 0x01])
        );
        assert_eq!(
            composer.apply(&KvmdHidEvent::MouseButton {
                button: KvmdMouseButton::Middle,
                pressed: true,
            }),
            Some([0x04, 0xff, 0x3f, 0xff, 0x3f, 0xff, 0x01])
        );
        assert_eq!(
            composer.release_all(),
            [0x00, 0xff, 0x3f, 0xff, 0x3f, 0x00, 0x00]
        );
    }

    #[test]
    fn pointer_composers_ignore_events_of_the_other_interface() {
        let mut absolute = AbsolutePointerReportComposer::new();
        let mut relative = RelativePointerReportComposer::new();
        assert!(
            absolute
                .apply(&KvmdHidEvent::MouseRelative { x: 1, y: 1 })
                .is_none()
        );
        assert!(
            relative
                .apply(&KvmdHidEvent::MouseMove { x: 1, y: 1 })
                .is_none()
        );
        let key = KvmdHidEvent::Key {
            key: KvmdKeyCode::from_usb_usage(4).unwrap(),
            pressed: true,
        };
        assert!(absolute.apply(&key).is_none());
        assert!(relative.apply(&key).is_none());
    }

    #[test]
    fn kvmd_absolute_remap_matches_kvmd_integer_flooring() {
        assert_eq!(kvmd_absolute_remap(i16::MIN), 0);
        assert_eq!(kvmd_absolute_remap(i16::MAX), 32767);
        // Python: int((0 + 32768) * 32767 // 65535) == 16383
        assert_eq!(kvmd_absolute_remap(0), 16383);
        assert_eq!(kvmd_absolute_remap(-1), 16383);
        assert_eq!(kvmd_absolute_remap(1), 16384);
    }
}
