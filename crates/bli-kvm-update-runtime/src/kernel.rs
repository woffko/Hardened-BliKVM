use std::{
    fs::OpenOptions,
    io::Read,
    mem::MaybeUninit,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
};

use bli_kvm_update_manager::ClockSnapshot;

use crate::UpdateRuntimeError;

const BOOT_ID_PATH: &str = "/proc/sys/kernel/random/boot_id";
const MAX_BOOT_ID_BYTES: u64 = 64;

#[derive(Debug, Default, Clone, Copy)]
pub struct LinuxKernelEvidence;

impl LinuxKernelEvidence {
    pub fn new() -> Self {
        Self
    }

    pub fn snapshot(&self) -> Result<ClockSnapshot, UpdateRuntimeError> {
        let before = read_boot_id(Path::new(BOOT_ID_PATH))?;
        let monotonic_ms = read_boottime_ms()?;
        let after = read_boot_id(Path::new(BOOT_ID_PATH))?;
        if before != after {
            return Err(UpdateRuntimeError::BootIdentityChanged);
        }
        Ok(ClockSnapshot {
            boot_id: before,
            monotonic_ms,
        })
    }
}

fn read_boot_id(path: &Path) -> Result<[u8; 16], UpdateRuntimeError> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| UpdateRuntimeError::InvalidBootIdentity)?;
    let metadata = file
        .metadata()
        .map_err(|_| UpdateRuntimeError::InvalidBootIdentity)?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.permissions().mode() & 0o022 != 0 {
        return Err(UpdateRuntimeError::InvalidBootIdentity);
    }
    let mut bytes = Vec::with_capacity(37);
    file.take(MAX_BOOT_ID_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| UpdateRuntimeError::InvalidBootIdentity)?;
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    parse_boot_id(&bytes)
}

fn parse_boot_id(bytes: &[u8]) -> Result<[u8; 16], UpdateRuntimeError> {
    if bytes.len() != 36
        || [8, 13, 18, 23].iter().any(|index| bytes[*index] != b'-')
        || bytes.iter().enumerate().any(|(index, byte)| {
            !(matches!(index, 8 | 13 | 18 | 23)
                || byte.is_ascii_digit()
                || (b'a'..=b'f').contains(byte))
        })
    {
        return Err(UpdateRuntimeError::InvalidBootIdentity);
    }
    let mut output = [0_u8; 16];
    let mut high_nibble: Option<u8> = None;
    let mut output_index = 0_usize;
    for byte in bytes.iter().copied().filter(|byte| *byte != b'-') {
        let nibble = match byte {
            b'0'..=b'9' => byte - b'0',
            b'a'..=b'f' => byte - b'a' + 10,
            _ => return Err(UpdateRuntimeError::InvalidBootIdentity),
        };
        if let Some(high) = high_nibble.take() {
            output[output_index] = (high << 4) | nibble;
            output_index += 1;
        } else {
            high_nibble = Some(nibble);
        }
    }
    if output_index != output.len() || high_nibble.is_some() || output.iter().all(|byte| *byte == 0)
    {
        return Err(UpdateRuntimeError::InvalidBootIdentity);
    }
    Ok(output)
}

fn read_boottime_ms() -> Result<u64, UpdateRuntimeError> {
    let mut value = MaybeUninit::<libc::timespec>::uninit();
    // SAFETY: clock_gettime initializes the provided timespec on success and
    // CLOCK_BOOTTIME requires no additional pointer or lifetime invariants.
    let result = unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, value.as_mut_ptr()) };
    if result != 0 {
        return Err(UpdateRuntimeError::InvalidBootClock);
    }
    // SAFETY: the successful clock_gettime call initialized the timespec.
    let value = unsafe { value.assume_init() };
    let seconds = u64::try_from(value.tv_sec).map_err(|_| UpdateRuntimeError::InvalidBootClock)?;
    let nanoseconds =
        u64::try_from(value.tv_nsec).map_err(|_| UpdateRuntimeError::InvalidBootClock)?;
    if nanoseconds >= 1_000_000_000 {
        return Err(UpdateRuntimeError::InvalidBootClock);
    }
    seconds
        .checked_mul(1_000)
        .and_then(|milliseconds| milliseconds.checked_add(nanoseconds / 1_000_000))
        .ok_or(UpdateRuntimeError::InvalidBootClock)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_only_canonical_nonzero_kernel_boot_ids() {
        assert_eq!(
            parse_boot_id(b"00112233-4455-6677-8899-aabbccddeeff").unwrap(),
            [
                0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
                0xee, 0xff,
            ]
        );
        for invalid in [
            b"00112233445566778899aabbccddeeff".as_slice(),
            b"00112233-4455-6677-8899-AABBCCDDEEFF".as_slice(),
            b"00000000-0000-0000-0000-000000000000".as_slice(),
            b"00112233-4455-6677-8899-aabbccddeezz".as_slice(),
        ] {
            assert!(matches!(
                parse_boot_id(invalid),
                Err(UpdateRuntimeError::InvalidBootIdentity)
            ));
        }
    }

    #[test]
    fn reads_live_kernel_evidence_without_wall_clock() {
        let snapshot = LinuxKernelEvidence::new().snapshot().unwrap();
        assert!(snapshot.boot_id.iter().any(|byte| *byte != 0));
        assert!(snapshot.monotonic_ms > 0);
    }
}
