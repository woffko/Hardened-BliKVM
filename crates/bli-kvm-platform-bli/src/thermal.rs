use std::path::Path;

use thiserror::Error;

use crate::sysfs::{BoundedSysfsReadError, read_bounded_ascii};

const H616_THERMAL_PATH: &str = "/sys/class/thermal/thermal_zone0/temp";
const MAX_TEMPERATURE_BYTES: u64 = 16;
const MIN_TEMPERATURE_MILLIDEGREES: i32 = -40_000;
const MAX_TEMPERATURE_MILLIDEGREES: i32 = 150_000;

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ThermalReadError {
    #[error("thermal observation is unavailable")]
    Unavailable,
    #[error("thermal observation source is unsafe")]
    UnsafeSource,
    #[error("thermal observation value is invalid")]
    InvalidValue,
}

impl ThermalReadError {
    pub fn class(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::UnsafeSource => "unsafe_source",
            Self::InvalidValue => "invalid_value",
        }
    }
}

pub fn read_h616_temperature_millidegrees() -> Result<i32, ThermalReadError> {
    read_temperature(Path::new(H616_THERMAL_PATH))
}

fn read_temperature(path: &Path) -> Result<i32, ThermalReadError> {
    let text = read_bounded_ascii(path, MAX_TEMPERATURE_BYTES).map_err(|error| match error {
        BoundedSysfsReadError::Unavailable => ThermalReadError::Unavailable,
        BoundedSysfsReadError::UnsafeSource => ThermalReadError::UnsafeSource,
        BoundedSysfsReadError::InvalidValue => ThermalReadError::InvalidValue,
    })?;
    if text.is_empty()
        || text.starts_with('+')
        || text
            .strip_prefix('-')
            .unwrap_or(&text)
            .bytes()
            .any(|byte| !byte.is_ascii_digit())
    {
        return Err(ThermalReadError::InvalidValue);
    }
    let value = text
        .parse::<i32>()
        .map_err(|_| ThermalReadError::InvalidValue)?;
    if !(MIN_TEMPERATURE_MILLIDEGREES..=MAX_TEMPERATURE_MILLIDEGREES).contains(&value) {
        return Err(ThermalReadError::InvalidValue);
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
    };

    use tempfile::tempdir;

    use super::*;

    fn fixture(value: &[u8]) -> (tempfile::TempDir, std::path::PathBuf) {
        let directory = tempdir().unwrap();
        let path = directory.path().join("temp");
        fs::write(&path, value).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        (directory, path)
    }

    #[test]
    fn reads_only_bounded_millidegrees() {
        for (text, expected) in [
            (b"42125\n".as_slice(), 42_125),
            (b"-40000", -40_000),
            (b"150000\r\n", 150_000),
        ] {
            let (_directory, path) = fixture(text);
            assert_eq!(read_temperature(&path), Ok(expected));
        }
        for text in [
            b"".as_slice(),
            b"150001\n",
            b"42.1\n",
            b"+42000\n",
            b"42000 extra\n",
            b"-40001\n",
            b"42000\nextra",
            b"\xff\n",
            b"12345678901234567",
        ] {
            let (_directory, path) = fixture(text);
            assert_eq!(read_temperature(&path), Err(ThermalReadError::InvalidValue));
        }
    }

    #[test]
    fn rejects_links_and_group_or_world_writable_sources() {
        let (directory, path) = fixture(b"42000\n");
        let linked = directory.path().join("linked");
        symlink(&path, &linked).unwrap();
        assert_eq!(
            read_temperature(&linked),
            Err(ThermalReadError::UnsafeSource)
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o622)).unwrap();
        assert_eq!(read_temperature(&path), Err(ThermalReadError::UnsafeSource));

        let (_directory, path) = fixture(b"42000\n");
        let hard_link = path.with_extension("hard-link");
        fs::hard_link(&path, &hard_link).unwrap();
        assert_eq!(read_temperature(&path), Err(ThermalReadError::UnsafeSource));
    }
}
