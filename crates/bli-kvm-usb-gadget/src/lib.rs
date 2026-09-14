//! Idempotent Linux USB gadget (configfs) state machine.
//!
//! This is the Rust replacement for the vendor `enable-gadget.sh` /
//! `kvmd-msd.sh` shell scripts (see `docs/vendor-rust-migration.md`). It builds
//! the gadget from typed, validated values, binds the UDC last, and tears the
//! gadget down in reverse order. Applying an already-applied spec is a no-op.
//!
//! The steps are produced by a pure planner (`bind_steps`/`remove_steps`) so
//! ordering and idempotency can be asserted without touching real configfs;
//! `apply`/`remove` execute the same steps against a configfs root. HID
//! functions and their report descriptors are a later slice.

use std::path::{Path, PathBuf};

use thiserror::Error;

/// Gadget directory name under the configfs root.
pub const GADGET_NAME: &str = "hardened";
/// Function name used in the gadget configuration.
pub const MSD_FUNCTION: &str = "mass_storage.usb0";
/// Device string language id.
pub const LANG: &str = "0x409";

const MAX_STRING_LEN: usize = 126;
const MAX_UDC_LEN: usize = 64;
/// The vendor default serial; never shipped by the hardened gadget.
const LEGACY_SERIAL: &str = "CAFEBABE";

#[derive(Debug, Error, PartialEq, Eq)]
pub enum GadgetError {
    #[error("gadget I/O failed")]
    Io(String),
    #[error("invalid gadget string {0}")]
    InvalidString(&'static str),
    #[error("invalid UDC controller name")]
    InvalidUdc,
    #[error("invalid mass-storage image path")]
    InvalidImagePath,
    #[error("mass-storage image is not a regular file")]
    ImageMissing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GadgetStrings {
    pub manufacturer: String,
    pub product: String,
    pub serial: String,
    pub configuration: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MassStorageConfig {
    pub file: PathBuf,
    pub read_only: bool,
    pub cdrom: bool,
    pub removable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GadgetSpec {
    pub id_vendor: u16,
    pub id_product: u16,
    pub bcd_usb: u16,
    pub bcd_device: u16,
    pub strings: GadgetStrings,
    pub udc: String,
    pub mass_storage: MassStorageConfig,
}

/// One configfs operation, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    WriteFile { path: PathBuf, value: String },
    Symlink { link: PathBuf, target: PathBuf },
    RemoveFile { path: PathBuf },
    RemoveDir { path: PathBuf },
}

fn gadget_path() -> PathBuf {
    PathBuf::from(GADGET_NAME)
}

fn hex16(value: u16) -> String {
    format!("{value:04x}")
}

fn bool_value(value: bool) -> String {
    if value {
        "1".to_string()
    } else {
        "0".to_string()
    }
}

fn validate_string(value: &str, field: &'static str) -> Result<(), GadgetError> {
    if value.is_empty()
        || value.chars().count() > MAX_STRING_LEN
        || !value.chars().all(|ch| (' '..='~').contains(&ch))
    {
        return Err(GadgetError::InvalidString(field));
    }
    Ok(())
}

fn validate_spec(spec: &GadgetSpec) -> Result<(), GadgetError> {
    validate_string(&spec.strings.manufacturer, "manufacturer")?;
    validate_string(&spec.strings.product, "product")?;
    validate_string(&spec.strings.serial, "serial")?;
    validate_string(&spec.strings.configuration, "configuration")?;
    if spec.strings.serial.eq_ignore_ascii_case(LEGACY_SERIAL) {
        return Err(GadgetError::InvalidString("serial"));
    }
    if spec.udc.is_empty()
        || spec.udc.len() > MAX_UDC_LEN
        || !spec
            .udc
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | ':'))
    {
        return Err(GadgetError::InvalidUdc);
    }
    let file = &spec.mass_storage.file;
    if !file.is_absolute()
        || file
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(GadgetError::InvalidImagePath);
    }
    let metadata = std::fs::metadata(file).map_err(|_| GadgetError::ImageMissing)?;
    if !metadata.is_file() {
        return Err(GadgetError::ImageMissing);
    }
    Ok(())
}

/// Ordered plan that materializes the gadget and binds the UDC last.
pub fn bind_steps(spec: &GadgetSpec) -> Vec<Step> {
    let gadget = gadget_path();
    let strings = gadget.join("strings").join(LANG);
    let config = gadget.join("configs").join("c.1");
    let function = gadget.join("functions").join(MSD_FUNCTION);
    let lun = function.join("lun.0");

    vec![
        Step::WriteFile {
            path: gadget.join("idVendor"),
            value: hex16(spec.id_vendor),
        },
        Step::WriteFile {
            path: gadget.join("idProduct"),
            value: hex16(spec.id_product),
        },
        Step::WriteFile {
            path: gadget.join("bcdUSB"),
            value: hex16(spec.bcd_usb),
        },
        Step::WriteFile {
            path: gadget.join("bcdDevice"),
            value: hex16(spec.bcd_device),
        },
        Step::WriteFile {
            path: strings.join("manufacturer"),
            value: spec.strings.manufacturer.clone(),
        },
        Step::WriteFile {
            path: strings.join("product"),
            value: spec.strings.product.clone(),
        },
        Step::WriteFile {
            path: strings.join("serialnumber"),
            value: spec.strings.serial.clone(),
        },
        Step::WriteFile {
            path: config.join("strings").join(LANG).join("configuration"),
            value: spec.strings.configuration.clone(),
        },
        Step::WriteFile {
            path: lun.join("file"),
            value: spec.mass_storage.file.to_string_lossy().into_owned(),
        },
        Step::WriteFile {
            path: lun.join("ro"),
            value: bool_value(spec.mass_storage.read_only),
        },
        Step::WriteFile {
            path: lun.join("cdrom"),
            value: bool_value(spec.mass_storage.cdrom),
        },
        Step::WriteFile {
            path: lun.join("removable"),
            value: bool_value(spec.mass_storage.removable),
        },
        Step::Symlink {
            link: config.join(MSD_FUNCTION),
            target: PathBuf::from("../../functions").join(MSD_FUNCTION),
        },
        Step::WriteFile {
            path: gadget.join("UDC"),
            value: spec.udc.clone(),
        },
    ]
}

/// Ordered plan that unbinds first and removes the tree last.
pub fn remove_steps() -> Vec<Step> {
    let gadget = gadget_path();
    let config = gadget.join("configs").join("c.1");
    vec![
        Step::WriteFile {
            path: gadget.join("UDC"),
            value: String::new(),
        },
        Step::RemoveFile {
            path: config.join(MSD_FUNCTION),
        },
        Step::RemoveDir {
            path: gadget.join("functions").join(MSD_FUNCTION),
        },
        Step::RemoveDir {
            path: config.join("strings").join(LANG),
        },
        Step::RemoveDir { path: config },
        Step::RemoveDir {
            path: gadget.join("strings").join(LANG),
        },
        Step::RemoveDir { path: gadget },
    ]
}

fn execute(root: &Path, steps: &[Step]) -> Result<(), GadgetError> {
    for step in steps {
        match step {
            Step::WriteFile { path, value } => {
                let full = root.join(path);
                if let Some(parent) = full.parent() {
                    std::fs::create_dir_all(parent).map_err(io_error)?;
                }
                std::fs::write(&full, value).map_err(io_error)?;
            }
            Step::Symlink { link, target } => {
                let full = root.join(link);
                if !full.exists() {
                    if let Some(parent) = full.parent() {
                        std::fs::create_dir_all(parent).map_err(io_error)?;
                    }
                    std::os::unix::fs::symlink(target, &full).map_err(io_error)?;
                }
            }
            Step::RemoveFile { path } => {
                let full = root.join(path);
                if full.exists() {
                    std::fs::remove_file(&full).map_err(io_error)?;
                }
            }
            Step::RemoveDir { path } => {
                let full = root.join(path);
                if full.exists() {
                    std::fs::remove_dir_all(&full).map_err(io_error)?;
                }
            }
        }
    }
    Ok(())
}

fn io_error(error: std::io::Error) -> GadgetError {
    GadgetError::Io(error.to_string())
}

/// Materialize the gadget, binding the UDC last; roll back on failure.
pub fn apply(root: &Path, spec: &GadgetSpec) -> Result<(), GadgetError> {
    validate_spec(spec)?;
    if let Err(error) = execute(root, &bind_steps(spec)) {
        let _ = execute(root, &remove_steps());
        return Err(error);
    }
    Ok(())
}

/// Unbind and remove the gadget. Idempotent.
pub fn remove(root: &Path) -> Result<(), GadgetError> {
    execute(root, &remove_steps())
}

#[cfg(test)]
mod tests {
    use super::*;

    const IMAGE_NAME: &str = "kubuntu.img";

    fn spec(image: &Path) -> GadgetSpec {
        GadgetSpec {
            id_vendor: 0x1d6b,
            id_product: 0x0104,
            bcd_usb: 0x0200,
            bcd_device: 0x0100,
            strings: GadgetStrings {
                manufacturer: "Hardened BLi-KVM".into(),
                product: "BLi-KVM Composite Device".into(),
                serial: "BLK-0123456789abcdef".into(),
                configuration: "BLi-KVM configuration".into(),
            },
            udc: "musb-hdrc.2.auto".into(),
            mass_storage: MassStorageConfig {
                file: image.to_path_buf(),
                read_only: true,
                cdrom: false,
                removable: true,
            },
        }
    }

    fn root_with_image() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join(IMAGE_NAME), b"image").unwrap();
        directory
    }

    fn read(root: &Path, path: &str) -> String {
        std::fs::read_to_string(root.join(path)).unwrap()
    }

    #[test]
    fn bind_steps_bind_the_udc_last_and_use_reviewed_descriptors() {
        let directory = root_with_image();
        let spec = spec(&directory.path().join(IMAGE_NAME));
        let steps = bind_steps(&spec);
        assert!(matches!(
            steps.last(),
            Some(Step::WriteFile { path, value }) if value == "musb-hdrc.2.auto" && path.ends_with("UDC")
        ));
        assert_eq!(execute(directory.path(), &steps), Ok(()));
        assert_eq!(read(directory.path(), "hardened/idVendor"), "1d6b");
        assert_eq!(read(directory.path(), "hardened/idProduct"), "0104");
        assert_eq!(
            read(directory.path(), "hardened/strings/0x409/manufacturer"),
            "Hardened BLi-KVM"
        );
        assert_eq!(
            read(
                directory.path(),
                "hardened/configs/c.1/strings/0x409/configuration"
            ),
            "BLi-KVM configuration"
        );
        assert_eq!(
            read(
                directory.path(),
                "hardened/functions/mass_storage.usb0/lun.0/ro"
            ),
            "1"
        );
        assert_eq!(
            read(
                directory.path(),
                "hardened/functions/mass_storage.usb0/lun.0/removable"
            ),
            "1"
        );
        assert!(
            directory
                .path()
                .join("hardened/configs/c.1/mass_storage.usb0")
                .is_symlink()
        );
    }

    #[test]
    fn apply_is_idempotent() {
        let directory = root_with_image();
        let spec = spec(&directory.path().join(IMAGE_NAME));
        assert_eq!(apply(directory.path(), &spec), Ok(()));
        let udc = read(directory.path(), "hardened/UDC");
        assert_eq!(apply(directory.path(), &spec), Ok(()));
        assert_eq!(read(directory.path(), "hardened/UDC"), udc);
    }

    #[test]
    fn remove_unbinds_and_is_idempotent() {
        let directory = root_with_image();
        let spec = spec(&directory.path().join(IMAGE_NAME));
        assert_eq!(apply(directory.path(), &spec), Ok(()));
        assert_eq!(remove(directory.path()), Ok(()));
        assert!(!directory.path().join(GADGET_NAME).exists());
        assert_eq!(remove(directory.path()), Ok(()));
    }

    #[test]
    fn remove_unbinds_before_removing() {
        let steps = remove_steps();
        assert!(
            matches!(steps.first(), Some(Step::WriteFile { value, .. }) if value.is_empty()),
            "removal must clear UDC first"
        );
    }

    #[test]
    fn invalid_specs_are_rejected_without_touching_the_root() {
        let directory = root_with_image();
        let image = directory.path().join(IMAGE_NAME);

        let mut invalid = spec(&image);
        invalid.strings.serial = LEGACY_SERIAL.into();
        assert_eq!(
            apply(directory.path(), &invalid),
            Err(GadgetError::InvalidString("serial"))
        );

        let mut invalid = spec(&image);
        invalid.strings.product = "bad\nproduct".into();
        assert_eq!(
            apply(directory.path(), &invalid),
            Err(GadgetError::InvalidString("product"))
        );

        let mut invalid = spec(&image);
        invalid.udc = "bad/udc".into();
        assert_eq!(
            apply(directory.path(), &invalid),
            Err(GadgetError::InvalidUdc)
        );

        let mut invalid = spec(&image);
        invalid.mass_storage.file = PathBuf::from("relative.img");
        assert_eq!(
            apply(directory.path(), &invalid),
            Err(GadgetError::InvalidImagePath)
        );

        let mut invalid = spec(&image);
        invalid.mass_storage.file = directory.path().join("../escape.img");
        assert_eq!(
            apply(directory.path(), &invalid),
            Err(GadgetError::InvalidImagePath)
        );

        let mut invalid = spec(&image);
        invalid.mass_storage.file = directory.path().join("missing.img");
        assert_eq!(
            apply(directory.path(), &invalid),
            Err(GadgetError::ImageMissing)
        );

        assert!(!directory.path().join(GADGET_NAME).exists());
    }
}
