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
//!
//! The target agent's channel (`docs/baselines/work-plan-20261014-target-agent.md`)
//! is the one descriptor of this project's own: while the operator's switch
//! is on, `hid.usb1` carries the absolute mouse as report 1, unchanged but
//! for its ID, and a vendor-defined collection (usage page 0xFF00) as report
//! 2, 63 bytes each way. The board's USB controller has four IN endpoints,
//! and the keyboard, both mice and the virtual media take them all, so the
//! channel cannot be a function of its own. The absolute mouse is the one
//! interface no firmware drives (it is not a boot device), and its OUT
//! endpoint, unused by the mouse, carries the agent's reports to the board.

use crate::{Function, HidConfig};

pub const HID_KEYBOARD_NAME: &str = "hid.usb0";
pub const HID_ABSOLUTE_MOUSE_NAME: &str = "hid.usb1";
pub const HID_RELATIVE_MOUSE_NAME: &str = "hid.usb2";

pub const HID_KEYBOARD_REPORT_LENGTH: u16 = 8;
pub const HID_ABSOLUTE_MOUSE_REPORT_LENGTH: u16 = 7;
pub const HID_RELATIVE_MOUSE_REPORT_LENGTH: u16 = 5;
/// `hid.usb1`'s report length while it carries the agent's collection.
pub const HID_AGENT_REPORT_LENGTH: u16 = 64;
/// The absolute mouse's report ID while the agent's collection is there.
pub const ABSOLUTE_MOUSE_REPORT_ID: u8 = 1;
/// The agent's report ID.
pub const AGENT_REPORT_ID: u8 = 2;

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

/// The absolute mouse as report 1, then the agent's collection as report 2.
const AGENT_ABSOLUTE_MOUSE_DESCRIPTOR: &[u8] = &[
    0x05, 0x01, 0x09, 0x02, 0xa1, 0x01, 0x85, 0x01, 0x09, 0x01, 0xa1, 0x00, 0x05, 0x09, 0x19, 0x01,
    0x29, 0x08, 0x15, 0x00, 0x25, 0x01, 0x95, 0x08, 0x75, 0x01, 0x81, 0x02, 0x05, 0x01, 0x09, 0x30,
    0x09, 0x31, 0x16, 0x00, 0x00, 0x26, 0xff, 0x7f, 0x75, 0x10, 0x95, 0x02, 0x81, 0x02, 0x09, 0x38,
    0x15, 0x81, 0x25, 0x7f, 0x75, 0x08, 0x95, 0x01, 0x81, 0x06, 0x05, 0x0c, 0x0a, 0x38, 0x02, 0x15,
    0x81, 0x25, 0x7f, 0x75, 0x08, 0x95, 0x01, 0x81, 0x06, 0xc0, 0xc0,
    // Usage page 0xFF00, usage 1, an application collection, report 2:
    // usage 2, 63 bytes in; usage 3, 63 bytes out.
    0x06, 0x00, 0xff, 0x09, 0x01, 0xa1, 0x01, 0x85, 0x02, 0x09, 0x02, 0x15, 0x00, 0x26, 0xff, 0x00,
    0x75, 0x08, 0x95, 0x3f, 0x81, 0x02, 0x09, 0x03, 0x15, 0x00, 0x26, 0xff, 0x00, 0x75, 0x08, 0x95,
    0x3f, 0x91, 0x02, 0xc0,
];

/// The report ID the absolute mouse's reports need with this descriptor of
/// `hid.usb1`: none with the pinned one, 1 with the agent's; `None` for any
/// other, which no report may be written to.
pub fn absolute_mouse_report_id(descriptor: &[u8]) -> Option<Option<u8>> {
    if descriptor == ABSOLUTE_MOUSE_DESCRIPTOR {
        Some(None)
    } else if descriptor == AGENT_ABSOLUTE_MOUSE_DESCRIPTOR {
        Some(Some(ABSOLUTE_MOUSE_REPORT_ID))
    } else {
        None
    }
}

/// `hid.usb1` as the absolute mouse alone, or with the agent's collection.
pub fn absolute_mouse_function(agent: bool) -> HidConfig {
    HidConfig {
        name: HID_ABSOLUTE_MOUSE_NAME.to_string(),
        subclass: 0,
        protocol: 0,
        report_length: if agent {
            HID_AGENT_REPORT_LENGTH
        } else {
            HID_ABSOLUTE_MOUSE_REPORT_LENGTH
        },
        report_descriptor: if agent {
            AGENT_ABSOLUTE_MOUSE_DESCRIPTOR.to_vec()
        } else {
            ABSOLUTE_MOUSE_DESCRIPTOR.to_vec()
        },
    }
}

/// The three pinned HID functions in gadget order.
pub fn pinned_hid_functions() -> Vec<Function> {
    hid_functions(false)
}

/// The three HID functions in gadget order, `hid.usb1` with the agent's
/// collection when `agent` is set.
pub fn hid_functions(agent: bool) -> Vec<Function> {
    vec![
        Function::Hid(HidConfig {
            name: HID_KEYBOARD_NAME.to_string(),
            subclass: 1,
            protocol: 1,
            report_length: HID_KEYBOARD_REPORT_LENGTH,
            report_descriptor: KEYBOARD_DESCRIPTOR.to_vec(),
        }),
        Function::Hid(absolute_mouse_function(agent)),
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

    /// Each report's ID (0 without one) and its bits.
    type Reports = Vec<(u8, u32)>;

    /// The reports in and out, from the descriptor's short items.
    fn report_bits(descriptor: &[u8]) -> (Reports, Reports) {
        let (mut size, mut count, mut id) = (0_u32, 0_u32, 0_u8);
        let (mut input, mut output) = (Reports::new(), Reports::new());
        let mut at = 0;
        while at < descriptor.len() {
            let prefix = descriptor[at];
            let length = [0, 1, 2, 4][usize::from(prefix & 3)];
            let mut value = 0_u32;
            for (shift, byte) in descriptor[at + 1..at + 1 + length].iter().enumerate() {
                value |= u32::from(*byte) << (8 * shift);
            }
            let add =
                |reports: &mut Vec<(u8, u32)>| match reports.iter_mut().find(|(r, _)| *r == id) {
                    Some((_, bits)) => *bits += size * count,
                    None => reports.push((id, size * count)),
                };
            match prefix & 0xfc {
                0x74 => size = value,
                0x94 => count = value,
                0x84 => id = value as u8,
                0x80 => add(&mut input),
                0x90 => add(&mut output),
                _ => {}
            }
            at += 1 + length;
        }
        (input, output)
    }

    #[test]
    fn the_agents_descriptor_is_the_mouse_with_an_id_and_the_agents_collection() {
        assert_eq!(
            sha256(AGENT_ABSOLUTE_MOUSE_DESCRIPTOR),
            "3f6733401d98ad19a11d06b15187d9afe6d5bebd7b49e9977673e6c4f755f73d"
        );
        // The mouse's items are the pinned ones, its ID after the first
        // collection.
        let mut mouse = ABSOLUTE_MOUSE_DESCRIPTOR[..6].to_vec();
        mouse.extend_from_slice(&[0x85, ABSOLUTE_MOUSE_REPORT_ID]);
        mouse.extend_from_slice(&ABSOLUTE_MOUSE_DESCRIPTOR[6..]);
        assert!(AGENT_ABSOLUTE_MOUSE_DESCRIPTOR.starts_with(&mouse));
        assert_eq!(
            &AGENT_ABSOLUTE_MOUSE_DESCRIPTOR[mouse.len()..][..3],
            &[0x06, 0x00, 0xff]
        );
        // The pinned mouse: one input report of 7 bytes without an ID.
        assert_eq!(
            report_bits(ABSOLUTE_MOUSE_DESCRIPTOR),
            (vec![(0, 56)], vec![])
        );
        // With the agent: the mouse's 7 bytes as report 1, the agent's 63
        // each way as report 2; with their IDs they fit the report length.
        assert_eq!(
            report_bits(AGENT_ABSOLUTE_MOUSE_DESCRIPTOR),
            (
                vec![(ABSOLUTE_MOUSE_REPORT_ID, 56), (AGENT_REPORT_ID, 504)],
                vec![(AGENT_REPORT_ID, 504)]
            )
        );
        assert_eq!(1 + 504 / 8, usize::from(HID_AGENT_REPORT_LENGTH));
    }

    #[test]
    fn the_absolute_mouse_report_id_follows_the_descriptor() {
        assert_eq!(
            absolute_mouse_report_id(ABSOLUTE_MOUSE_DESCRIPTOR),
            Some(None)
        );
        assert_eq!(
            absolute_mouse_report_id(AGENT_ABSOLUTE_MOUSE_DESCRIPTOR),
            Some(Some(1))
        );
        assert_eq!(absolute_mouse_report_id(RELATIVE_MOUSE_DESCRIPTOR), None);
        assert_eq!(absolute_mouse_report_id(&[]), None);
        let agent = hid_functions(true);
        let plain = hid_functions(false);
        assert_eq!(agent[0], plain[0]);
        assert_eq!(agent[2], plain[2]);
        assert_eq!(plain, pinned_hid_functions());
        let Function::Hid(mouse) = &agent[1] else {
            panic!("{:?}", agent[1]);
        };
        assert_eq!(
            (
                mouse.name.as_str(),
                mouse.subclass,
                mouse.protocol,
                mouse.report_length
            ),
            (HID_ABSOLUTE_MOUSE_NAME, 0, 0, HID_AGENT_REPORT_LENGTH)
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
