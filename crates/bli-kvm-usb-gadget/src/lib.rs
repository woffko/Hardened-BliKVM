//! Idempotent Linux USB gadget (configfs) state machine.
//!
//! This is the Rust replacement for the vendor `enable-gadget.sh` /
//! `kvmd-msd.sh` shell scripts (see `docs/vendor-rust-migration.md`). It builds
//! the gadget from typed, validated values, binds the UDC last, and tears the
//! gadget down in reverse order. Applying an already-applied spec is a no-op.
//!
//! The steps are produced by a pure planner (`bind_steps`/`remove_steps`) so
//! ordering and idempotency can be asserted without touching real configfs;
//! `apply`/`remove` execute the same steps against a configfs root.
//!
//! HID report descriptors are caller-provided: they are protocol facts that
//! must be captured read-only from the device, never guessed here.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// Gadget directory name under the configfs root.
pub const GADGET_NAME: &str = "hardened";
/// Function name of the mass-storage function.
pub const MSD_FUNCTION: &str = "mass_storage.usb0";
/// Reviewed HID function names.
pub const HID_FUNCTIONS: [&str; 3] = ["hid.usb0", "hid.usb1", "hid.usb2"];
/// Device string language id.
pub const LANG: &str = "0x409";

const MAX_STRING_LEN: usize = 126;
const MAX_UDC_LEN: usize = 64;
const MAX_REPORT_LENGTH: u16 = 1024;
const MAX_REPORT_DESCRIPTOR: usize = 4096;
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
    #[error("gadget declares no functions")]
    NoFunctions,
    #[error("duplicate or unknown function name")]
    InvalidFunction,
    #[error("invalid HID report length")]
    InvalidReportLength,
    #[error("invalid HID report descriptor")]
    InvalidReportDescriptor,
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
pub struct HidConfig {
    /// One of [`HID_FUNCTIONS`].
    pub name: String,
    pub subclass: u8,
    pub protocol: u8,
    pub report_length: u16,
    pub report_descriptor: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Function {
    MassStorage(MassStorageConfig),
    Hid(HidConfig),
}

impl Function {
    fn name(&self) -> &str {
        match self {
            Function::MassStorage(_) => MSD_FUNCTION,
            Function::Hid(hid) => &hid.name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GadgetSpec {
    pub id_vendor: u16,
    pub id_product: u16,
    pub bcd_usb: u16,
    pub bcd_device: u16,
    pub strings: GadgetStrings,
    pub udc: String,
    pub functions: Vec<Function>,
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

fn config_path() -> PathBuf {
    gadget_path().join("configs").join("c.1")
}

fn function_path(name: &str) -> PathBuf {
    gadget_path().join("functions").join(name)
}

fn hex16(value: u16) -> String {
    format!("{value:04x}")
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn bool_value(value: bool) -> String {
    if value { "1".into() } else { "0".into() }
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

fn validate_udc(udc: &str) -> Result<(), GadgetError> {
    if udc.is_empty()
        || udc.len() > MAX_UDC_LEN
        || !udc
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | ':'))
    {
        return Err(GadgetError::InvalidUdc);
    }
    Ok(())
}

fn validate_image_path(file: &Path) -> Result<(), GadgetError> {
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

fn validate_function(function: &Function) -> Result<(), GadgetError> {
    match function {
        Function::MassStorage(config) => validate_image_path(&config.file),
        Function::Hid(config) => {
            if !HID_FUNCTIONS.contains(&config.name.as_str()) {
                return Err(GadgetError::InvalidFunction);
            }
            if config.report_length == 0 || config.report_length > MAX_REPORT_LENGTH {
                return Err(GadgetError::InvalidReportLength);
            }
            if config.report_descriptor.is_empty()
                || config.report_descriptor.len() > MAX_REPORT_DESCRIPTOR
            {
                return Err(GadgetError::InvalidReportDescriptor);
            }
            Ok(())
        }
    }
}

fn validate_spec(spec: &GadgetSpec) -> Result<(), GadgetError> {
    validate_string(&spec.strings.manufacturer, "manufacturer")?;
    validate_string(&spec.strings.product, "product")?;
    validate_string(&spec.strings.serial, "serial")?;
    validate_string(&spec.strings.configuration, "configuration")?;
    if spec.strings.serial.eq_ignore_ascii_case(LEGACY_SERIAL) {
        return Err(GadgetError::InvalidString("serial"));
    }
    validate_udc(&spec.udc)?;
    if spec.functions.is_empty() {
        return Err(GadgetError::NoFunctions);
    }
    let mut names = BTreeSet::new();
    for function in &spec.functions {
        validate_function(function)?;
        if !names.insert(function.name().to_string()) {
            return Err(GadgetError::InvalidFunction);
        }
    }
    Ok(())
}

fn function_steps(function: &Function) -> Vec<Step> {
    let config = config_path();
    let name = function.name().to_string();
    let base = function_path(&name);
    let mut steps = Vec::new();

    match function {
        Function::MassStorage(storage) => {
            let lun = base.join("lun.0");
            steps.push(Step::WriteFile {
                path: lun.join("file"),
                value: storage.file.to_string_lossy().into_owned(),
            });
            steps.push(Step::WriteFile {
                path: lun.join("ro"),
                value: bool_value(storage.read_only),
            });
            steps.push(Step::WriteFile {
                path: lun.join("cdrom"),
                value: bool_value(storage.cdrom),
            });
            steps.push(Step::WriteFile {
                path: lun.join("removable"),
                value: bool_value(storage.removable),
            });
        }
        Function::Hid(hid) => {
            steps.push(Step::WriteFile {
                path: base.join("protocol"),
                value: hid.protocol.to_string(),
            });
            steps.push(Step::WriteFile {
                path: base.join("subclass"),
                value: hid.subclass.to_string(),
            });
            steps.push(Step::WriteFile {
                path: base.join("report_length"),
                value: hid.report_length.to_string(),
            });
            steps.push(Step::WriteFile {
                path: base.join("report_desc"),
                value: hex_bytes(&hid.report_descriptor),
            });
        }
    }

    steps.push(Step::Symlink {
        link: config.join(&name),
        target: PathBuf::from("../../functions").join(&name),
    });
    steps
}

/// Ordered plan that materializes the gadget and binds the UDC last.
pub fn bind_steps(spec: &GadgetSpec) -> Vec<Step> {
    let gadget = gadget_path();
    let strings = gadget.join("strings").join(LANG);
    let config = config_path();

    let mut steps = vec![
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
    ];

    for function in &spec.functions {
        steps.extend(function_steps(function));
    }

    steps.push(Step::WriteFile {
        path: gadget.join("UDC"),
        value: spec.udc.clone(),
    });
    steps
}

/// Ordered plan that unbinds first and removes the tree last.
pub fn remove_steps(spec: &GadgetSpec) -> Vec<Step> {
    let gadget = gadget_path();
    let config = config_path();
    let mut steps = vec![Step::WriteFile {
        path: gadget.join("UDC"),
        value: String::new(),
    }];
    for function in &spec.functions {
        steps.push(Step::RemoveFile {
            path: config.join(function.name()),
        });
        steps.push(Step::RemoveDir {
            path: function_path(function.name()),
        });
    }
    steps.push(Step::RemoveDir {
        path: config.join("strings").join(LANG),
    });
    steps.push(Step::RemoveDir { path: config });
    steps.push(Step::RemoveDir {
        path: gadget.join("strings").join(LANG),
    });
    steps.push(Step::RemoveDir { path: gadget });
    steps
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
        let _ = execute(root, &remove_steps(spec));
        return Err(error);
    }
    Ok(())
}

/// Unbind and remove the gadget. Idempotent.
pub fn remove(root: &Path, spec: &GadgetSpec) -> Result<(), GadgetError> {
    execute(root, &remove_steps(spec))
}

/// Write the selected image to the mass-storage LUN. Requires the function to
/// exist; it does not bind the gadget.
pub fn set_lun_file(root: &Path, file: &Path) -> Result<(), GadgetError> {
    validate_image_path(file)?;
    execute(
        root,
        &[Step::WriteFile {
            path: function_path(MSD_FUNCTION).join("lun.0").join("file"),
            value: file.to_string_lossy().into_owned(),
        }],
    )
}

/// Bind the gadget to a UDC controller.
pub fn bind(root: &Path, udc: &str) -> Result<(), GadgetError> {
    validate_udc(udc)?;
    execute(
        root,
        &[Step::WriteFile {
            path: gadget_path().join("UDC"),
            value: udc.to_string(),
        }],
    )
}

/// Unbind the gadget from its UDC controller. Idempotent.
pub fn unbind(root: &Path) -> Result<(), GadgetError> {
    execute(
        root,
        &[Step::WriteFile {
            path: gadget_path().join("UDC"),
            value: String::new(),
        }],
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MassStorageState {
    pub file: PathBuf,
    pub read_only: bool,
    pub cdrom: bool,
    pub removable: bool,
}

/// Read-only snapshot of an existing gadget, used for media eligibility and
/// ownership-handoff verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GadgetState {
    pub present: bool,
    /// Controller name while bound, `None` when unbound (empty or missing).
    pub udc: Option<String>,
    pub functions: Vec<String>,
    pub mass_storage: Option<MassStorageState>,
}

/// Read the current gadget state from a configfs root without mutating it.
pub fn observe(root: &Path) -> Result<GadgetState, GadgetError> {
    let gadget = root.join(GADGET_NAME);
    if !gadget.is_dir() {
        return Ok(GadgetState {
            present: false,
            udc: None,
            functions: Vec::new(),
            mass_storage: None,
        });
    }

    let udc = read_optional_string(root, &gadget.join("UDC"))?;
    let mut functions = Vec::new();
    let functions_dir = gadget.join("functions");
    if functions_dir.is_dir() {
        let entries = std::fs::read_dir(&functions_dir).map_err(io_error)?;
        for entry in entries {
            let entry = entry.map_err(io_error)?;
            if entry.path().is_dir() {
                functions.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        functions.sort();
    }

    let mass_storage = if functions.iter().any(|name| name == MSD_FUNCTION) {
        let lun = function_path(MSD_FUNCTION).join("lun.0");
        Some(MassStorageState {
            file: PathBuf::from(read_required_string(root, &lun.join("file"))?),
            read_only: read_bool(root, &lun.join("ro"))?,
            cdrom: read_bool(root, &lun.join("cdrom"))?,
            removable: read_bool(root, &lun.join("removable"))?,
        })
    } else {
        None
    };

    Ok(GadgetState {
        present: true,
        udc,
        functions,
        mass_storage,
    })
}

fn read_optional_string(root: &Path, path: &Path) -> Result<Option<String>, GadgetError> {
    let full = root.join(path);
    match std::fs::read_to_string(full) {
        Ok(value) => {
            let value = value.trim().to_string();
            if value.is_empty() {
                Ok(None)
            } else {
                Ok(Some(value))
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error(error)),
    }
}

fn read_required_string(root: &Path, path: &Path) -> Result<String, GadgetError> {
    let full = root.join(path);
    std::fs::read_to_string(full)
        .map(|value| value.trim().to_string())
        .map_err(io_error)
}

fn read_bool(root: &Path, path: &Path) -> Result<bool, GadgetError> {
    let value = read_required_string(root, path)?;
    match value.as_str() {
        "1" | "y" | "Y" => Ok(true),
        "0" | "n" | "N" => Ok(false),
        _ => Err(GadgetError::Io("unexpected boolean gadget value".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IMAGE_NAME: &str = "kubuntu.img";

    fn msd(image: &Path) -> Function {
        Function::MassStorage(MassStorageConfig {
            file: image.to_path_buf(),
            read_only: true,
            cdrom: false,
            removable: true,
        })
    }

    fn hid(name: &str, report_length: u16) -> Function {
        Function::Hid(HidConfig {
            name: name.into(),
            subclass: 1,
            protocol: 1,
            report_length,
            report_descriptor: vec![0x05, 0x01, 0x09, 0x06],
        })
    }

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
            functions: vec![msd(image)],
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
    fn hid_functions_write_their_descriptors_and_link_into_the_config() {
        let directory = root_with_image();
        let mut spec = spec(&directory.path().join(IMAGE_NAME));
        spec.functions.push(hid("hid.usb0", 8));
        assert_eq!(apply(directory.path(), &spec), Ok(()));
        assert_eq!(
            read(directory.path(), "hardened/functions/hid.usb0/protocol"),
            "1"
        );
        assert_eq!(
            read(directory.path(), "hardened/functions/hid.usb0/subclass"),
            "1"
        );
        assert_eq!(
            read(
                directory.path(),
                "hardened/functions/hid.usb0/report_length"
            ),
            "8"
        );
        assert_eq!(
            read(directory.path(), "hardened/functions/hid.usb0/report_desc"),
            "05010906"
        );
        assert!(
            directory
                .path()
                .join("hardened/configs/c.1/hid.usb0")
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
        assert_eq!(remove(directory.path(), &spec), Ok(()));
        assert!(!directory.path().join(GADGET_NAME).exists());
        assert_eq!(remove(directory.path(), &spec), Ok(()));
    }

    #[test]
    fn remove_unbinds_before_removing() {
        let directory = root_with_image();
        let spec = spec(&directory.path().join(IMAGE_NAME));
        let steps = remove_steps(&spec);
        assert!(
            matches!(steps.first(), Some(Step::WriteFile { value, .. }) if value.is_empty()),
            "removal must clear UDC first"
        );
        assert!(
            matches!(steps.last(), Some(Step::RemoveDir { path }) if path.as_path() == Path::new(GADGET_NAME))
        );
    }

    #[test]
    fn observe_reports_presence_binding_and_functions() {
        let directory = root_with_image();
        let spec = spec(&directory.path().join(IMAGE_NAME));
        assert!(!observe(directory.path()).unwrap().present);

        assert_eq!(apply(directory.path(), &spec), Ok(()));
        let state = observe(directory.path()).unwrap();
        assert!(state.present);
        assert_eq!(state.udc.as_deref(), Some("musb-hdrc.2.auto"));
        assert_eq!(state.functions, vec![MSD_FUNCTION.to_string()]);
        let msd = state.mass_storage.expect("mass storage is present");
        assert_eq!(msd.file, directory.path().join(IMAGE_NAME));
        assert!(msd.read_only);
        assert!(msd.removable);
        assert!(!msd.cdrom);

        assert_eq!(remove(directory.path(), &spec), Ok(()));
        assert!(!observe(directory.path()).unwrap().present);
    }

    #[test]
    fn observe_lists_hid_functions_sorted() {
        let directory = root_with_image();
        let mut spec = spec(&directory.path().join(IMAGE_NAME));
        spec.functions.push(hid("hid.usb1", 7));
        spec.functions.push(hid("hid.usb0", 8));
        assert_eq!(apply(directory.path(), &spec), Ok(()));
        let state = observe(directory.path()).unwrap();
        assert_eq!(
            state.functions,
            vec![
                "hid.usb0".to_string(),
                "hid.usb1".to_string(),
                MSD_FUNCTION.to_string()
            ]
        );
    }

    #[test]
    fn bind_unbind_and_lun_selection_are_independent_and_validated() {
        let directory = root_with_image();
        let spec = spec(&directory.path().join(IMAGE_NAME));
        assert_eq!(apply(directory.path(), &spec), Ok(()));

        assert_eq!(unbind(directory.path()), Ok(()));
        assert!(observe(directory.path()).unwrap().udc.is_none());
        assert_eq!(bind(directory.path(), "musb-hdrc.2.auto"), Ok(()));
        assert_eq!(
            observe(directory.path()).unwrap().udc.as_deref(),
            Some("musb-hdrc.2.auto")
        );

        let other = directory.path().join("other.img");
        std::fs::write(&other, b"x").unwrap();
        assert_eq!(set_lun_file(directory.path(), &other), Ok(()));
        assert_eq!(
            observe(directory.path())
                .unwrap()
                .mass_storage
                .unwrap()
                .file,
            other
        );

        assert_eq!(
            bind(directory.path(), "bad udc"),
            Err(GadgetError::InvalidUdc)
        );
        assert_eq!(
            set_lun_file(directory.path(), Path::new("relative.img")),
            Err(GadgetError::InvalidImagePath)
        );
        assert_eq!(
            set_lun_file(directory.path(), &directory.path().join("missing.img")),
            Err(GadgetError::ImageMissing)
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
        invalid.functions = vec![msd(&directory.path().join("missing.img"))];
        assert_eq!(
            apply(directory.path(), &invalid),
            Err(GadgetError::ImageMissing)
        );

        let mut invalid = spec(&image);
        invalid.functions = vec![Function::MassStorage(MassStorageConfig {
            file: PathBuf::from("relative.img"),
            read_only: true,
            cdrom: false,
            removable: true,
        })];
        assert_eq!(
            apply(directory.path(), &invalid),
            Err(GadgetError::InvalidImagePath)
        );

        let mut invalid = spec(&image);
        invalid.functions.clear();
        assert_eq!(
            apply(directory.path(), &invalid),
            Err(GadgetError::NoFunctions)
        );

        let mut invalid = spec(&image);
        invalid.functions.push(hid("hid.usb9", 8));
        assert_eq!(
            apply(directory.path(), &invalid),
            Err(GadgetError::InvalidFunction)
        );

        let mut invalid = spec(&image);
        invalid.functions.push(hid("hid.usb0", 8));
        invalid.functions.push(hid("hid.usb0", 7));
        assert_eq!(
            apply(directory.path(), &invalid),
            Err(GadgetError::InvalidFunction)
        );

        let mut invalid = spec(&image);
        invalid.functions.push(hid("hid.usb0", 0));
        assert_eq!(
            apply(directory.path(), &invalid),
            Err(GadgetError::InvalidReportLength)
        );

        let mut invalid = spec(&image);
        let mut empty = hid("hid.usb0", 8);
        if let Function::Hid(config) = &mut empty {
            config.report_descriptor.clear();
        }
        invalid.functions.push(empty);
        assert_eq!(
            apply(directory.path(), &invalid),
            Err(GadgetError::InvalidReportDescriptor)
        );

        assert!(!directory.path().join(GADGET_NAME).exists());
    }
}
