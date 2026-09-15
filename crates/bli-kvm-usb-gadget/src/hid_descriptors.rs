//! Pinned HID report descriptors captured read-only from the board.
//!
//! Provenance: read on 2026-09-15 from the stock `kvmd` gadget on the H616
//! board (`tools/capture_device_hid_descriptors.py`, receipt
//! `evidence/private/device-hid-descriptors-20260915-a.json`). These are
//! protocol facts and must not be edited by hand; `the descriptors are checked
//! against the recorded SHA-256 in the tests.
//!
//! Function parameters:
//! - `hid.usb0` boot keyboard: subclass 1, protocol 1, report length 8;
//! - `hid.usb1` absolute mouse: subclass 0, protocol 0, report length 7;
//! - `hid.usb2` relative mouse: subclass 1, protocol 2, report length 5.
//!
//! The two pointer interfaces are easy to mix up: `hid.usb1` reports absolute
//! 16-bit X/Y in `0..=32767` (KVMD packs it as `<BHHbb`, 7 bytes) while
//! `hid.usb2` reports signed 8-bit relative deltas (KVMD packs it as `<Bbbbb`,
//! 5 bytes). See `kvmd.plugins.hid.otg.events.make_mouse_report`.

use crate::{Function, HidConfig};

pub const HID_KEYBOARD_NAME: &str = "hid.usb0";
pub const HID_ABSOLUTE_MOUSE_NAME: &str = "hid.usb1";
pub const HID_RELATIVE_MOUSE_NAME: &str = "hid.usb2";

pub const HID_KEYBOARD_REPORT_LENGTH: u16 = 8;
pub const HID_ABSOLUTE_MOUSE_REPORT_LENGTH: u16 = 7;
pub const HID_RELATIVE_MOUSE_REPORT_LENGTH: u16 = 5;

const KEYBOARD_DESCRIPTOR: &[u8] = &[
    0x05, 0x01, 0x09, 0x06, 0xa1, 0x01, 0x05, 0x07, 0x19, 0xe0, 0x29, 0xe7, 0x15, 0x00, 0x25, 0x01,
    0x75, 0x01, 0x95, 0x08, 0x81, 0x02, 0x95, 0x01, 0x75, 0x08, 0x81, 0x01, 0x95, 0x05, 0x75, 0x01,
    0x05, 0x08, 0x19, 0x01, 0x29, 0x05, 0x91, 0x02, 0x95, 0x01, 0x75, 0x03, 0x91, 0x01, 0x95, 0x06,
    0x75, 0x08, 0x15, 0x00, 0x26, 0xff, 0x00, 0x05, 0x07, 0x19, 0x00, 0x2a, 0xff, 0x00, 0x81, 0x00,
    0xc0,
];
const RELATIVE_MOUSE_DESCRIPTOR: &[u8] = &[
    0x05, 0x01, 0x09, 0x02, 0xa1, 0x01, 0x09, 0x01, 0xa1, 0x00, 0x05, 0x09, 0x19, 0x01, 0x29, 0x08,
    0x15, 0x00, 0x25, 0x01, 0x95, 0x08, 0x75, 0x01, 0x81, 0x02, 0x05, 0x01, 0x09, 0x30, 0x09, 0x31,
    0x09, 0x38, 0x15, 0x81, 0x25, 0x7f, 0x75, 0x08, 0x95, 0x03, 0x81, 0x06, 0x05, 0x0c, 0x0a, 0x38,
    0x02, 0x15, 0x81, 0x25, 0x7f, 0x75, 0x08, 0x95, 0x01, 0x81, 0x06, 0xc0, 0xc0,
];

const ABSOLUTE_MOUSE_DESCRIPTOR: &[u8] = &[
    0x05, 0x01, 0x09, 0x02, 0xa1, 0x01, 0x09, 0x01, 0xa1, 0x00, 0x05, 0x09, 0x19, 0x01, 0x29, 0x08,
    0x15, 0x00, 0x25, 0x01, 0x95, 0x08, 0x75, 0x01, 0x81, 0x02, 0x05, 0x01, 0x09, 0x30, 0x09, 0x31,
    0x16, 0x00, 0x00, 0x26, 0xff, 0x7f, 0x75, 0x10, 0x95, 0x02, 0x81, 0x02, 0x09, 0x38, 0x15, 0x81,
    0x25, 0x7f, 0x75, 0x08, 0x95, 0x01, 0x81, 0x06, 0x05, 0x0c, 0x0a, 0x38, 0x02, 0x15, 0x81, 0x25,
    0x7f, 0x75, 0x08, 0x95, 0x01, 0x81, 0x06, 0xc0, 0xc0,
];
/// The three pinned HID functions in gadget order.
pub fn pinned_hid_functions() -> Vec<Function> {
    vec![
        Function::Hid(HidConfig {
            name: HID_KEYBOARD_NAME.to_string(),
            subclass: 1,
            protocol: 1,
            report_length: HID_KEYBOARD_REPORT_LENGTH,
            report_descriptor: KEYBOARD_DESCRIPTOR.to_vec(),
        }),
        Function::Hid(HidConfig {
            name: HID_ABSOLUTE_MOUSE_NAME.to_string(),
            subclass: 0,
            protocol: 0,
            report_length: HID_ABSOLUTE_MOUSE_REPORT_LENGTH,
            report_descriptor: ABSOLUTE_MOUSE_DESCRIPTOR.to_vec(),
        }),
        Function::Hid(HidConfig {
            name: HID_RELATIVE_MOUSE_NAME.to_string(),
            subclass: 1,
            protocol: 2,
            report_length: HID_RELATIVE_MOUSE_REPORT_LENGTH,
            report_descriptor: RELATIVE_MOUSE_DESCRIPTOR.to_vec(),
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn sha256(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    }

    #[test]
    fn descriptors_match_the_captured_hashes() {
        assert_eq!(
            sha256(KEYBOARD_DESCRIPTOR),
            "5333cc3a23db1dd7a9e8303e2169a54a7070f69822fa503014834652c24410b6"
        );
        assert_eq!(
            sha256(ABSOLUTE_MOUSE_DESCRIPTOR),
            "3a71a5a23705ee80a711e143d1242e34cb5d85592d4b5e6ac3f20bf8b9830a12"
        );
        assert_eq!(
            sha256(RELATIVE_MOUSE_DESCRIPTOR),
            "92155ddf022d4091f5e15eea5871107cd6ff7a4e6ada0ef931bd14b86c5632d0"
        );
    }

    #[test]
    fn pinned_functions_keep_the_captured_parameters() {
        let functions = pinned_hid_functions();
        assert_eq!(functions.len(), 3);
        let expected = [
            (HID_KEYBOARD_NAME, 1_u8, 1_u8, HID_KEYBOARD_REPORT_LENGTH),
            (
                HID_ABSOLUTE_MOUSE_NAME,
                0,
                0,
                HID_ABSOLUTE_MOUSE_REPORT_LENGTH,
            ),
            (
                HID_RELATIVE_MOUSE_NAME,
                1,
                2,
                HID_RELATIVE_MOUSE_REPORT_LENGTH,
            ),
        ];
        for (function, (name, subclass, protocol, length)) in functions.iter().zip(expected) {
            match function {
                Function::Hid(hid) => {
                    assert_eq!(hid.name, name);
                    assert_eq!(hid.subclass, subclass);
                    assert_eq!(hid.protocol, protocol);
                    assert_eq!(hid.report_length, length);
                    assert!(!hid.report_descriptor.is_empty());
                }
                other => panic!("expected a HID function, got {other:?}"),
            }
        }
    }

    #[test]
    fn pinned_functions_validate_in_a_gadget_spec() {
        let directory = tempfile::tempdir().unwrap();
        let image = directory.path().join("image.img");
        std::fs::write(&image, b"image").unwrap();
        let mut spec = crate::GadgetSpec {
            id_vendor: 0x1d6b,
            id_product: 0x0104,
            bcd_usb: 0x0200,
            bcd_device: 0x0100,
            strings: crate::GadgetStrings {
                manufacturer: "Hardened BLi-KVM".into(),
                product: "BLi-KVM Composite Device".into(),
                serial: "BLK-test".into(),
                configuration: "BLi-KVM configuration".into(),
            },
            udc: "musb-hdrc.2.auto".into(),
            functions: vec![crate::Function::MassStorage(crate::MassStorageConfig {
                file: image,
                read_only: true,
                cdrom: false,
                removable: true,
            })],
        };
        spec.functions.extend(pinned_hid_functions());
        assert_eq!(crate::apply(directory.path(), &spec), Ok(()));
    }
}
