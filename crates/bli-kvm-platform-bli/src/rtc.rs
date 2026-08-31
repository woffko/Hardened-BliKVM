use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use thiserror::Error;

use crate::sysfs::{BoundedSysfsReadError, read_bounded_ascii};

const H616_EXTERNAL_RTC_NAME_PATH: &str = "/sys/class/rtc/rtc1/name";
const H616_EXTERNAL_RTC_EPOCH_PATH: &str = "/sys/class/rtc/rtc1/since_epoch";
const H616_EXTERNAL_RTC_IDENTITY: &str = "rtc-pcf8563 0-0051";
const MAX_RTC_IDENTITY_BYTES: u64 = 64;
const MAX_RTC_EPOCH_BYTES: u64 = 16;
const MIN_RTC_UNIX_SECONDS: i64 = 946_684_800;
const MAX_RTC_UNIX_SECONDS: i64 = 4_102_444_799;
const MAX_RTC_OFFSET_SECONDS: i64 = 315_576_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RtcReadout {
    pub unix_seconds: i64,
    /// External RTC Unix seconds minus the current system Unix seconds.
    pub offset_from_system_seconds: i64,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum RtcReadError {
    #[error("external RTC observation is unavailable")]
    Unavailable,
    #[error("external RTC observation source is unsafe")]
    UnsafeSource,
    #[error("external RTC identity is invalid")]
    InvalidIdentity,
    #[error("external RTC observation value is invalid")]
    InvalidValue,
    #[error("system clock is invalid for RTC comparison")]
    InvalidSystemClock,
}

impl RtcReadError {
    pub fn class(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::UnsafeSource => "unsafe_source",
            Self::InvalidIdentity => "invalid_identity",
            Self::InvalidValue => "invalid_value",
            Self::InvalidSystemClock => "invalid_system_clock",
        }
    }
}

pub fn read_h616_external_rtc() -> Result<RtcReadout, RtcReadError> {
    let system_unix_seconds = i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| RtcReadError::InvalidSystemClock)?
            .as_secs(),
    )
    .map_err(|_| RtcReadError::InvalidSystemClock)?;
    read_rtc(
        Path::new(H616_EXTERNAL_RTC_NAME_PATH),
        Path::new(H616_EXTERNAL_RTC_EPOCH_PATH),
        system_unix_seconds,
    )
}

fn read_rtc(
    identity_path: &Path,
    epoch_path: &Path,
    system_unix_seconds: i64,
) -> Result<RtcReadout, RtcReadError> {
    let identity =
        read_bounded_ascii(identity_path, MAX_RTC_IDENTITY_BYTES).map_err(map_sysfs_error)?;
    if identity != H616_EXTERNAL_RTC_IDENTITY {
        return Err(RtcReadError::InvalidIdentity);
    }
    if !(MIN_RTC_UNIX_SECONDS..=MAX_RTC_UNIX_SECONDS).contains(&system_unix_seconds) {
        return Err(RtcReadError::InvalidSystemClock);
    }
    let epoch = read_bounded_ascii(epoch_path, MAX_RTC_EPOCH_BYTES).map_err(map_sysfs_error)?;
    if epoch.bytes().any(|byte| !byte.is_ascii_digit()) {
        return Err(RtcReadError::InvalidValue);
    }
    let unix_seconds = epoch
        .parse::<i64>()
        .map_err(|_| RtcReadError::InvalidValue)?;
    if !(MIN_RTC_UNIX_SECONDS..=MAX_RTC_UNIX_SECONDS).contains(&unix_seconds) {
        return Err(RtcReadError::InvalidValue);
    }
    let offset_from_system_seconds = unix_seconds
        .checked_sub(system_unix_seconds)
        .filter(|offset| offset.unsigned_abs() <= MAX_RTC_OFFSET_SECONDS as u64)
        .ok_or(RtcReadError::InvalidValue)?;
    Ok(RtcReadout {
        unix_seconds,
        offset_from_system_seconds,
    })
}

fn map_sysfs_error(error: BoundedSysfsReadError) -> RtcReadError {
    match error {
        BoundedSysfsReadError::Unavailable => RtcReadError::Unavailable,
        BoundedSysfsReadError::UnsafeSource => RtcReadError::UnsafeSource,
        BoundedSysfsReadError::InvalidValue => RtcReadError::InvalidValue,
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::PermissionsExt};

    use tempfile::tempdir;

    use super::*;

    fn fixture(
        identity: &[u8],
        epoch: &[u8],
    ) -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let directory = tempdir().unwrap();
        let identity_path = directory.path().join("name");
        let epoch_path = directory.path().join("since_epoch");
        fs::write(&identity_path, identity).unwrap();
        fs::write(&epoch_path, epoch).unwrap();
        fs::set_permissions(&identity_path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(&epoch_path, fs::Permissions::from_mode(0o600)).unwrap();
        (directory, identity_path, epoch_path)
    }

    #[test]
    fn reads_only_the_expected_bounded_external_rtc() {
        let (_directory, identity, epoch) = fixture(b"rtc-pcf8563 0-0051\n", b"1759489393\n");
        assert_eq!(
            read_rtc(&identity, &epoch, 1_788_137_388),
            Ok(RtcReadout {
                unix_seconds: 1_759_489_393,
                offset_from_system_seconds: -28_647_995,
            })
        );
    }

    #[test]
    fn rejects_wrong_identity_epoch_and_system_clock() {
        let (_directory, identity, epoch) = fixture(b"rtc-pcf8563 0-0052", b"1759489393");
        assert_eq!(
            read_rtc(&identity, &epoch, 1_788_137_388),
            Err(RtcReadError::InvalidIdentity)
        );

        let (_directory, identity, epoch) = fixture(b"rtc-pcf8563 0-0051", b"invalid");
        assert_eq!(
            read_rtc(&identity, &epoch, 1_788_137_388),
            Err(RtcReadError::InvalidValue)
        );

        let (_directory, identity, epoch) = fixture(b"rtc-pcf8563 0-0051", b"1759489393");
        assert_eq!(
            read_rtc(&identity, &epoch, MIN_RTC_UNIX_SECONDS - 1),
            Err(RtcReadError::InvalidSystemClock)
        );
        assert_eq!(
            read_rtc(&identity, &epoch, MAX_RTC_UNIX_SECONDS),
            Err(RtcReadError::InvalidValue)
        );
    }
}
