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

mod hid_descriptors;

pub use hid_descriptors::{
    ABSOLUTE_MOUSE_REPORT_ID, AGENT_REPORT_ID, HID_ABSOLUTE_MOUSE_NAME, HID_AGENT_REPORT_LENGTH,
    HID_KEYBOARD_NAME, HID_RELATIVE_MOUSE_NAME, absolute_mouse_function, hid_functions,
    pinned_hid_functions,
};

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// Gadget directory name under the configfs root.
pub const GADGET_NAME: &str = "hardened";
/// Function name of the mass-storage function.
pub const MSD_FUNCTION: &str = "mass_storage.usb0";
/// The mass-storage function's second LUN: the target agent's drive.
const SECOND_LUN: &str = "lun.1";
/// Reviewed HID function names.
pub const HID_FUNCTIONS: [&str; 3] = ["hid.usb0", "hid.usb1", "hid.usb2"];
/// The order in which [`replace_hid_function`] links the functions back into
/// the configuration, which numbers their interfaces: the HID functions as a
/// boot binds them, then the virtual media as an attach adds it.
const LINK_ORDER: [&str; 4] = ["hid.usb0", "hid.usb1", "hid.usb2", MSD_FUNCTION];
/// Device string language id.
pub const LANG: &str = "0x409";

const MAX_STRING_LEN: usize = 126;
const MAX_UDC_LEN: usize = 64;
const MAX_REPORT_LENGTH: u16 = 1024;
const MAX_REPORT_DESCRIPTOR: usize = 4096;
const MAX_HID_REPORT_BYTES: usize = 64;
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
    #[error("mass-storage image is too small for a CD-ROM")]
    ImageTooSmall,
    #[error("gadget declares no functions")]
    NoFunctions,
    #[error("duplicate or unknown function name")]
    InvalidFunction,
    #[error("invalid HID report length")]
    InvalidReportLength,
    #[error("invalid HID report descriptor")]
    InvalidReportDescriptor,
    #[error("invalid HID report")]
    InvalidReport,
    #[error("gadget spec does not match the requested operation")]
    InvalidSpec,
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
    /// A second LUN, read-only and not removable: the target agent's drive
    /// beside the virtual media, which keep LUN 0.
    pub second_lun: Option<PathBuf>,
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
    WriteFile {
        path: PathBuf,
        value: String,
    },
    /// Binary configfs attributes (the HID report descriptor is raw bytes).
    WriteBytes {
        path: PathBuf,
        value: Vec<u8>,
    },
    /// Release a configfs attribute by writing a single newline.
    ///
    /// A zero-byte write never reaches the kernel's store handler, so an empty
    /// value silently does nothing; the newline is the shortest write that
    /// still clears the attribute. This is how the UDC is unbound and how the
    /// mass-storage backing file is released.
    ClearAttribute {
        path: PathBuf,
    },
    /// Remove a configfs item directory.
    ///
    /// On configfs a plain `rmdir` drops the item and its attributes. The
    /// fallback covers the host tests, whose fake configfs is a real directory
    /// tree where the attribute files must be deleted first.
    RemoveEmptyDir {
        path: PathBuf,
    },
    Symlink {
        link: PathBuf,
        target: PathBuf,
    },
    RemoveFile {
        path: PathBuf,
    },
    RemoveDir {
        path: PathBuf,
    },
    /// Present an image on a LUN: its file is opened without following a
    /// link and checked, and the LUN's `file` gets what `presented_value`
    /// says.
    PresentImage {
        path: PathBuf,
        image: PathBuf,
    },
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

/// configfs parses these attributes with base 0, so the `0x` prefix is
/// required (`1d6b` alone is rejected with EINVAL).
fn hex16(value: u16) -> String {
    format!("0x{value:04x}")
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

/// What is written into a LUN's `file` for an image whose checked file is
/// open as `descriptor`: on configfs that descriptor's own path, so the
/// kernel, which opens the name it is given as root, opens the very file
/// that was checked and no link swapped in meanwhile; elsewhere (the host
/// tests' configfs of plain files) the image's path.
fn presented_value(descriptor: i32, image: &Path, configfs: bool) -> String {
    if configfs {
        format!("/proc/self/fd/{descriptor}")
    } else {
        image.to_string_lossy().into_owned()
    }
}

/// An image the owner may present: an absolute path whose file is a regular
/// file with one link, not itself a link. Another account that can write the
/// image store must not give the target root's files through a link or a
/// second name (security review of 2026-10-18, CRIT-02).
fn validate_image_path(file: &Path) -> Result<(), GadgetError> {
    if !file.is_absolute()
        || file
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(GadgetError::InvalidImagePath);
    }
    let metadata = std::fs::symlink_metadata(file).map_err(|_| GadgetError::ImageMissing)?;
    checked_image(&metadata)
}

fn checked_image(metadata: &std::fs::Metadata) -> Result<(), GadgetError> {
    use std::os::unix::fs::MetadataExt;
    if metadata.file_type().is_symlink() || metadata.nlink() > 1 {
        return Err(GadgetError::InvalidImagePath);
    }
    if !metadata.is_file() {
        return Err(GadgetError::ImageMissing);
    }
    Ok(())
}

/// Opens `image` without following a link at its name and checks the file
/// it got, for `Step::PresentImage`.
fn open_image(image: &Path) -> Result<std::fs::File, GadgetError> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(image)
        .map_err(|error| match error.raw_os_error() {
            Some(libc::ELOOP) => GadgetError::InvalidImagePath,
            _ => GadgetError::ImageMissing,
        })?;
    checked_image(&file.metadata().map_err(io_error)?)?;
    Ok(file)
}

/// Whether `path` lies on configfs.
fn on_configfs(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    const CONFIGFS_MAGIC: i64 = 0x6265_6570;
    let Ok(name) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: statfs only writes into the zeroed structure.
    let mut stats: libc::statfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statfs(name.as_ptr(), &mut stats) } != 0 {
        return false;
    }
    #[allow(clippy::unnecessary_cast)]
    let kind = stats.f_type as i64;
    kind == CONFIGFS_MAGIC
}

/// The kernel's mass-storage function refuses a CD-ROM image shorter than
/// 300 blocks of 2048 bytes (`ETOOSMALL`); refusing it here names the reason.
pub const MIN_CDROM_IMAGE_BYTES: u64 = 300 * 2048;

fn validate_mass_storage(config: &MassStorageConfig) -> Result<(), GadgetError> {
    // A removable LUN 0 may be empty: the target ejected its medium, and a
    // rebuild for the agent's drive keeps it so.
    let empty = config.file.as_os_str().is_empty();
    if empty && !config.removable {
        return Err(GadgetError::ImageMissing);
    }
    if !empty {
        validate_image_path(&config.file)?;
    }
    if let Some(second) = &config.second_lun {
        validate_image_path(second)?;
    }
    if config.cdrom && !empty {
        let size = std::fs::metadata(&config.file)
            .map(|metadata| metadata.len())
            .map_err(|_| GadgetError::ImageMissing)?;
        if size < MIN_CDROM_IMAGE_BYTES {
            return Err(GadgetError::ImageTooSmall);
        }
    }
    Ok(())
}

fn validate_function(function: &Function) -> Result<(), GadgetError> {
    match function {
        Function::MassStorage(config) => validate_mass_storage(config),
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

/// A LUN's steps: read-only, CD-ROM and removable as `flags` say, and its
/// file last, as the kernel makes the LUN busy as soon as `file` is
/// written; an empty file leaves the LUN empty, as an eject does.
fn lun_steps(steps: &mut Vec<Step>, lun: &Path, flags: [bool; 3], file: &Path) {
    for (attribute, on) in ["ro", "cdrom", "removable"].into_iter().zip(flags) {
        steps.push(Step::WriteFile {
            path: lun.join(attribute),
            value: bool_value(on),
        });
    }
    if file.as_os_str().is_empty() {
        steps.push(Step::ClearAttribute {
            path: lun.join("file"),
        });
    } else {
        steps.push(Step::PresentImage {
            path: lun.join("file"),
            image: file.to_path_buf(),
        });
    }
}

fn function_steps(function: &Function) -> Vec<Step> {
    let config = config_path();
    let name = function.name().to_string();
    let base = function_path(&name);
    let mut steps = Vec::new();

    match function {
        Function::MassStorage(storage) => {
            lun_steps(
                &mut steps,
                &base.join("lun.0"),
                [storage.read_only, storage.cdrom, storage.removable],
                &storage.file,
            );
            // The kernel makes a LUN only while the function is not linked:
            // the second is made here, before the link, read-only and not
            // removable.
            if let Some(second) = &storage.second_lun {
                lun_steps(
                    &mut steps,
                    &base.join(SECOND_LUN),
                    [true, false, false],
                    second,
                );
            }
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
            steps.push(Step::WriteBytes {
                path: base.join("report_desc"),
                value: hid.report_descriptor.clone(),
            });
        }
    }

    steps.push(Step::Symlink {
        link: config.join(&name),
        // The target is gadget-relative here; execute() passes the absolute
        // configfs path, which is the only form this kernel accepts (every
        // relative form is rejected with ENOENT).
        target: PathBuf::from("functions").join(&name),
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

fn step_error(path: &Path, error: std::io::Error) -> GadgetError {
    GadgetError::Io(format!("{}: {error}", path.display()))
}

/// How many changes this process has made to the gadget under each configfs
/// root, counted as each change ends.
static CHANGES: std::sync::Mutex<BTreeMap<PathBuf, u64>> = std::sync::Mutex::new(BTreeMap::new());

/// The changes this process has made to the gadget under `root`: what was
/// read from the gadget with this count taken before stays current while
/// the count is the same. A change made by another process is not counted.
pub fn changes(root: &Path) -> u64 {
    CHANGES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(root)
        .copied()
        .unwrap_or(0)
}

fn execute(root: &Path, steps: &[Step]) -> Result<(), GadgetError> {
    let result = run_steps(root, steps);
    // Counted after the steps, whatever came of them: a reading made while
    // they ran is taken again.
    *CHANGES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .entry(root.to_path_buf())
        .or_insert(0) += 1;
    result
}

fn run_steps(root: &Path, steps: &[Step]) -> Result<(), GadgetError> {
    for step in steps {
        match step {
            Step::WriteFile { path, value } => {
                let full = root.join(path);
                if let Some(parent) = full.parent() {
                    std::fs::create_dir_all(parent).map_err(|error| step_error(path, error))?;
                }
                std::fs::write(&full, value).map_err(|error| step_error(path, error))?;
            }
            Step::WriteBytes { path, value } => {
                let full = root.join(path);
                if let Some(parent) = full.parent() {
                    std::fs::create_dir_all(parent).map_err(|error| step_error(path, error))?;
                }
                std::fs::write(&full, value).map_err(|error| step_error(path, error))?;
            }
            Step::Symlink { link, target } => {
                let full = root.join(link);
                // Test the link itself: configfs stores its own relative form,
                // so a plain exists() can misread it.
                if std::fs::symlink_metadata(&full).is_err() {
                    if let Some(parent) = full.parent() {
                        std::fs::create_dir_all(parent).map_err(|error| step_error(link, error))?;
                    }
                    // Only the absolute configfs path is accepted as a target;
                    // every relative form is rejected with ENOENT.
                    let absolute = root.join(GADGET_NAME).join(target);
                    std::os::unix::fs::symlink(&absolute, &full)
                        .map_err(|error| step_error(link, error))?;
                }
            }
            Step::RemoveFile { path } => {
                let full = root.join(path);
                if full.exists() {
                    std::fs::remove_file(&full).map_err(|error| step_error(path, error))?;
                }
            }
            Step::RemoveDir { path } => {
                let full = root.join(path);
                if full.exists() {
                    std::fs::remove_dir_all(&full).map_err(|error| step_error(path, error))?;
                }
            }
            Step::ClearAttribute { path } => {
                let full = root.join(path);
                std::fs::write(&full, "\n").map_err(|error| step_error(path, error))?;
            }
            Step::PresentImage { path, image } => {
                use std::os::fd::AsRawFd;
                let full = root.join(path);
                if let Some(parent) = full.parent() {
                    std::fs::create_dir_all(parent).map_err(|error| step_error(path, error))?;
                }
                let checked = open_image(image)?;
                let configfs = full.parent().is_some_and(on_configfs);
                let value = presented_value(checked.as_raw_fd(), image, configfs);
                // The kernel opens the file during this write; the descriptor
                // is closed after it.
                std::fs::write(&full, value).map_err(|error| step_error(path, error))?;
                drop(checked);
            }
            Step::RemoveEmptyDir { path } => {
                let full = root.join(path);
                if !full.exists() {
                    continue;
                }
                if std::fs::remove_dir(&full).is_err() {
                    // Real configfs removed the item; the fallback handles an
                    // ordinary directory tree with attribute files inside.
                    std::fs::remove_dir_all(&full).map_err(|error| step_error(path, error))?;
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

/// Replace the mass-storage function in place without touching the other
/// functions.
///
/// This is the image-swap path. The kernel refuses `ro`, `cdrom` and
/// `removable` writes while a LUN still holds a backing file, and it refuses
/// to drop a function that is linked into a bound configuration, so the only
/// reliable order is: unbind, drop the old function, write the new attributes
/// (read-only first, file last) and bind again. The HID functions and their
/// open character devices are left alone, which is what makes an image change
/// possible while the board owner keeps running.
pub fn replace_mass_storage(root: &Path, spec: &GadgetSpec) -> Result<(), GadgetError> {
    validate_spec(spec)?;
    let gadget = gadget_path();
    let config = config_path();
    let Some(function) = spec
        .functions
        .iter()
        .find(|function| matches!(function, Function::MassStorage(_)))
    else {
        return Err(GadgetError::NoFunctions);
    };
    // Order matters: the config must stop referencing the function, the LUN
    // must release its backing file, and only then can the function item be
    // dropped.
    unbind(root)?;
    let mut steps = vec![Step::RemoveFile {
        path: config.join(MSD_FUNCTION),
    }];
    let lun_file = function_path(MSD_FUNCTION).join("lun.0").join("file");
    if root.join(&lun_file).exists() {
        steps.push(Step::ClearAttribute { path: lun_file });
    }
    steps.extend(second_lun_removal(root));
    steps.push(Step::RemoveEmptyDir {
        path: function_path(MSD_FUNCTION),
    });
    steps.extend(function_steps(function));
    steps.push(Step::WriteFile {
        path: gadget.join("UDC"),
        value: spec.udc.clone(),
    });
    execute_then_bind(root, &spec.udc, &steps)
}

/// Remove the mass-storage function from a live gadget without touching the
/// other functions.
///
/// This is the media-detach path. The order mirrors [`replace_mass_storage`]:
/// the configuration must stop referencing the function, the LUN must release
/// its backing file, and only then can the function item be dropped. The pinned
/// HID functions and their character devices stay in place and are rebound, so
/// an eject does not take the keyboard and mouse down with it.
pub fn remove_mass_storage(root: &Path, spec: &GadgetSpec) -> Result<(), GadgetError> {
    validate_spec(spec)?;
    if spec
        .functions
        .iter()
        .any(|function| matches!(function, Function::MassStorage(_)))
    {
        return Err(GadgetError::InvalidSpec);
    }
    let gadget = gadget_path();
    let config = config_path();
    unbind(root)?;
    let lun_file = function_path(MSD_FUNCTION).join("lun.0").join("file");
    let mut steps = vec![Step::RemoveFile {
        path: config.join(MSD_FUNCTION),
    }];
    if root.join(&lun_file).exists() {
        steps.push(Step::ClearAttribute { path: lun_file });
    }
    steps.extend(second_lun_removal(root));
    steps.push(Step::RemoveEmptyDir {
        path: function_path(MSD_FUNCTION),
    });
    steps.push(Step::WriteFile {
        path: gadget.join("UDC"),
        value: spec.udc.clone(),
    });
    execute_then_bind(root, &spec.udc, &steps)
}

/// Runs the steps of a change made with the controller unbound, and binds
/// it again when a step fails: the functions left, the keyboard and mice
/// among them, keep working rather than wait for a rebuild that may not
/// come. The step's error is the answer.
fn execute_then_bind(root: &Path, udc: &str, steps: &[Step]) -> Result<(), GadgetError> {
    let result = execute(root, steps);
    if result.is_err() && !udc_bound(root) {
        let rebound = execute(
            root,
            &[Step::WriteFile {
                path: gadget_path().join("UDC"),
                value: udc.to_string(),
            }],
        );
        if let Err(error) = rebound {
            eprintln!("usb gadget: the controller could not be bound again: {error:?}");
        }
    }
    result
}

/// Rewrite one HID function of a present gadget (its subclass, protocol,
/// report length and descriptor), keeping every other function as it is.
///
/// This is the target agent's switch: `hid.usb1` with or without the agent's
/// collection. The kernel refuses a function's attributes while the function
/// is linked into the configuration, and a link added again goes to the
/// end, which renumbers the interfaces. So the gadget is unbound, every link
/// is taken out, the attributes are written, and the links are put back in
/// [`LINK_ORDER`] before the controller is bound again; if the attributes
/// cannot be written, the links and the binding are restored all the same.
/// A link this code does not know stops the change before anything is
/// touched.
///
/// The function's character device goes away with the unbinding and comes
/// back with the binding; whoever keeps it open must close it first.
pub fn replace_hid_function(root: &Path, udc: &str, hid: &HidConfig) -> Result<(), GadgetError> {
    validate_udc(udc)?;
    validate_function(&Function::Hid(hid.clone()))?;
    let config = config_path();
    if !root.join(function_path(&hid.name)).is_dir() {
        return Err(GadgetError::InvalidSpec);
    }
    let mut linked = Vec::new();
    let entries = std::fs::read_dir(root.join(&config)).map_err(io_error)?;
    for entry in entries {
        let entry = entry.map_err(io_error)?;
        if !entry.file_type().map_err(io_error)?.is_symlink() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !LINK_ORDER.contains(&name.as_str()) {
            return Err(GadgetError::InvalidSpec);
        }
        linked.push(name);
    }
    if !linked.contains(&hid.name) {
        return Err(GadgetError::InvalidSpec);
    }
    unbind(root)?;
    let unlink: Vec<Step> = linked
        .iter()
        .map(|name| Step::RemoveFile {
            path: config.join(name),
        })
        .collect();
    let base = function_path(&hid.name);
    let attributes = [
        Step::WriteFile {
            path: base.join("protocol"),
            value: hid.protocol.to_string(),
        },
        Step::WriteFile {
            path: base.join("subclass"),
            value: hid.subclass.to_string(),
        },
        Step::WriteFile {
            path: base.join("report_length"),
            value: hid.report_length.to_string(),
        },
        Step::WriteBytes {
            path: base.join("report_desc"),
            value: hid.report_descriptor.clone(),
        },
    ];
    let mut relink: Vec<Step> = LINK_ORDER
        .iter()
        .filter(|name| linked.iter().any(|linked| linked == *name))
        .map(|name| Step::Symlink {
            link: config.join(name),
            target: PathBuf::from("functions").join(name),
        })
        .collect();
    relink.push(Step::WriteFile {
        path: gadget_path().join("UDC"),
        value: udc.to_string(),
    });
    let written = execute(root, &unlink).and_then(|()| execute(root, &attributes));
    let restored = execute(root, &relink);
    written.and(restored)
}

/// The report ID the absolute mouse's reports need on the present gadget,
/// read from `hid.usb1`'s descriptor; an error when the gadget or the
/// function is missing or the descriptor is not one of the two known ones.
pub fn absolute_mouse_report_id(root: &Path) -> Result<Option<u8>, GadgetError> {
    let descriptor = std::fs::read(
        root.join(function_path(HID_ABSOLUTE_MOUSE_NAME))
            .join("report_desc"),
    )
    .map_err(io_error)?;
    hid_descriptors::absolute_mouse_report_id(&descriptor)
        .ok_or(GadgetError::InvalidReportDescriptor)
}

/// Takes the second LUN (the target agent's drive) out of a present gadget
/// and binds it again, leaving LUN 0 and every other function as they are:
/// the way out when the mass storage cannot be built again, so that the
/// drive never outlives its switch and an owner from before the drive,
/// which knows one LUN, can still take the function apart. The kernel lets
/// a LUN go only from a function that is not in use, hence the unbinding.
pub fn remove_second_lun(root: &Path, udc: &str) -> Result<(), GadgetError> {
    validate_udc(udc)?;
    let steps = second_lun_removal(root);
    if steps.is_empty() {
        return Ok(());
    }
    unbind(root)?;
    let removed = execute(root, &steps);
    let bound = execute(
        root,
        &[Step::WriteFile {
            path: gadget_path().join("UDC"),
            value: udc.to_string(),
        }],
    );
    removed.and(bound)
}

/// Releases and removes the second LUN, if there is one: configfs drops a
/// function only once the LUNs made in it are gone.
fn second_lun_removal(root: &Path) -> Vec<Step> {
    let lun = function_path(MSD_FUNCTION).join(SECOND_LUN);
    if !root.join(&lun).is_dir() {
        return Vec::new();
    }
    vec![
        Step::ClearAttribute {
            path: lun.join("file"),
        },
        Step::RemoveEmptyDir { path: lun },
    ]
}

/// Write the selected image to the mass-storage LUN. Requires the function to
/// exist; it does not bind the gadget.
pub fn set_lun_file(root: &Path, file: &Path) -> Result<(), GadgetError> {
    validate_image_path(file)?;
    execute(
        root,
        &[Step::PresentImage {
            path: function_path(MSD_FUNCTION).join("lun.0").join("file"),
            image: file.to_path_buf(),
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
    // The kernel answers an empty write to an already empty UDC with ENODEV,
    // so idempotence needs the state read, not just a tolerant write.
    if !udc_bound(root) {
        return Ok(());
    }
    execute(
        root,
        &[Step::ClearAttribute {
            path: gadget_path().join("UDC"),
        }],
    )
}

/// Whether the gadget currently has a controller bound.
fn udc_bound(root: &Path) -> bool {
    std::fs::read_to_string(root.join(gadget_path()).join("UDC"))
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
}

/// Release the backing image from LUN 0 without touching the gadget.
pub fn clear_lun_file(root: &Path) -> Result<(), GadgetError> {
    execute(
        root,
        &[Step::ClearAttribute {
            path: function_path(MSD_FUNCTION).join("lun.0").join("file"),
        }],
    )
}

/// Reconnect the gadget to its controller: unbind, then bind the same UDC.
///
/// The brief USB interruption is intentional and is expected to be driven only
/// from a reviewed gate or the board owner; this primitive performs no policy
/// decision of its own and is idempotent when the gadget is already unbound.
pub fn reconnect(root: &Path, udc: &str) -> Result<(), GadgetError> {
    validate_udc(udc)?;
    unbind(root)?;
    bind(root, udc)
}

/// Map a reviewed HID function name to its character device.
pub fn hid_device_name(function: &str) -> Option<&'static str> {
    match function {
        "hid.usb0" => Some("hidg0"),
        "hid.usb1" => Some("hidg1"),
        "hid.usb2" => Some("hidg2"),
        _ => None,
    }
}

/// Write one bounded HID report to the function's character device.
pub fn write_report(hidg_dir: &Path, function: &str, report: &[u8]) -> Result<(), GadgetError> {
    let name = hid_device_name(function).ok_or(GadgetError::InvalidFunction)?;
    if report.is_empty() || report.len() > MAX_HID_REPORT_BYTES {
        return Err(GadgetError::InvalidReport);
    }
    let path = hidg_dir.join(name);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .map_err(io_error)?;
    file.write_all(report).map_err(io_error)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MassStorageState {
    pub file: PathBuf,
    pub read_only: bool,
    pub cdrom: bool,
    pub removable: bool,
    /// The second LUN's file, when there is a second LUN.
    pub second_lun: Option<PathBuf>,
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
            second_lun: {
                let second = function_path(MSD_FUNCTION).join(SECOND_LUN);
                if root.join(&second).is_dir() {
                    Some(PathBuf::from(read_required_string(
                        root,
                        &second.join("file"),
                    )?))
                } else {
                    None
                }
            },
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
            second_lun: None,
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
    fn a_link_in_the_image_store_is_not_presented() {
        let directory = root_with_image();
        let outside = tempfile::tempdir().unwrap();
        let secret = outside.path().join("shadow");
        std::fs::write(&secret, b"root's file").unwrap();
        let link = directory.path().join("linked.img");
        std::os::unix::fs::symlink(&secret, &link).unwrap();
        let root = directory.path().join("configfs");
        assert_eq!(
            apply(&root, &spec(&link)),
            Err(GadgetError::InvalidImagePath)
        );
        assert!(
            !root
                .join("hardened/functions/mass_storage.usb0/lun.0/file")
                .exists(),
            "the link reached the LUN"
        );
    }

    #[test]
    fn a_hard_linked_image_is_not_presented() {
        let directory = root_with_image();
        let image = directory.path().join(IMAGE_NAME);
        std::fs::hard_link(&image, directory.path().join("again.img")).unwrap();
        let root = directory.path().join("configfs");
        assert_eq!(
            apply(&root, &spec(&image)),
            Err(GadgetError::InvalidImagePath)
        );
    }

    #[test]
    fn a_link_is_not_set_on_a_lun_either() {
        let directory = root_with_image();
        let image = directory.path().join(IMAGE_NAME);
        let root = directory.path().join("configfs");
        assert_eq!(apply(&root, &spec(&image)), Ok(()));
        let link = directory.path().join("linked.img");
        std::os::unix::fs::symlink(&image, &link).unwrap();
        assert_eq!(
            set_lun_file(&root, &link),
            Err(GadgetError::InvalidImagePath)
        );
        assert_eq!(
            read(&root, "hardened/functions/mass_storage.usb0/lun.0/file"),
            image.to_string_lossy()
        );
    }

    #[test]
    fn configfs_gets_the_descriptor_of_the_file_checked() {
        let image = Path::new("/var/lib/hardened-blikvm-gadget/images/kubuntu.img");
        assert_eq!(presented_value(7, image, true), "/proc/self/fd/7");
        assert_eq!(
            presented_value(7, image, false),
            "/var/lib/hardened-blikvm-gadget/images/kubuntu.img"
        );
    }

    #[test]
    fn a_storage_change_that_fails_binds_the_gadget_again() {
        let directory = root_with_image();
        let image = directory.path().join(IMAGE_NAME);
        let mut with_hid = spec(&image);
        with_hid.functions.push(hid(HID_KEYBOARD_NAME, 8));
        assert_eq!(apply(directory.path(), &with_hid), Ok(()));
        // A step after the unbinding fails (the LUN's file refuses the
        // release); the keyboard must not be left without its controller.
        let file = directory
            .path()
            .join("hardened/functions/mass_storage.usb0/lun.0/file");
        std::fs::remove_file(&file).unwrap();
        std::fs::create_dir(&file).unwrap();
        assert!(replace_mass_storage(directory.path(), &with_hid).is_err());
        assert!(udc_bound(directory.path()));
        let mut without_storage = with_hid.clone();
        without_storage
            .functions
            .retain(|function| !matches!(function, Function::MassStorage(_)));
        assert!(remove_mass_storage(directory.path(), &without_storage).is_err());
        assert!(udc_bound(directory.path()));
    }

    #[test]
    fn an_empty_lun_zero_is_a_removable_one_and_the_drive_can_go_alone() {
        let directory = root_with_image();
        let drive = directory.path().join("agent-drive.img");
        std::fs::write(&drive, b"drive").unwrap();
        let empty = |removable| {
            let mut spec = spec(Path::new(""));
            spec.functions = vec![Function::MassStorage(MassStorageConfig {
                file: PathBuf::new(),
                read_only: true,
                cdrom: true,
                removable,
                second_lun: Some(drive.clone()),
            })];
            spec
        };
        // A fixed LUN 0 needs its file; a removable one may be left empty,
        // even as a CD-ROM, as an eject leaves it.
        assert_eq!(validate_spec(&empty(false)), Err(GadgetError::ImageMissing));
        assert_eq!(apply(directory.path(), &empty(true)), Ok(()));
        let storage = observe(directory.path()).unwrap().mass_storage.unwrap();
        assert!(storage.file.as_os_str().is_empty() && storage.removable);
        assert_eq!(storage.second_lun.as_deref(), Some(drive.as_path()));

        // The drive goes alone, and the controller is bound again.
        assert_eq!(
            remove_second_lun(directory.path(), "musb-hdrc.2.auto"),
            Ok(())
        );
        let state = observe(directory.path()).unwrap();
        assert_eq!(state.mass_storage.unwrap().second_lun, None);
        assert_eq!(state.udc.as_deref(), Some("musb-hdrc.2.auto"));
        // Nothing to take out: nothing is unbound.
        assert_eq!(
            remove_second_lun(directory.path(), "musb-hdrc.2.auto"),
            Ok(())
        );
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
        assert_eq!(read(directory.path(), "hardened/idVendor"), "0x1d6b");
        assert_eq!(read(directory.path(), "hardened/idProduct"), "0x0104");
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
        // configfs expects the raw descriptor bytes, never a hex rendering.
        assert_eq!(
            std::fs::read(
                directory
                    .path()
                    .join("hardened/functions/hid.usb0/report_desc")
            )
            .unwrap(),
            vec![0x05, 0x01, 0x09, 0x06]
        );
        let link = directory.path().join("hardened/configs/c.1/hid.usb0");
        assert!(link.is_symlink());
        // configfs only accepts the absolute configfs path as the target.
        assert_eq!(
            std::fs::read_link(&link).unwrap(),
            directory.path().join("hardened/functions/hid.usb0")
        );
    }

    #[test]
    fn mass_storage_attributes_are_written_before_the_file() {
        let directory = root_with_image();
        let spec = spec(&directory.path().join(IMAGE_NAME));
        let steps = bind_steps(&spec);
        let order: Vec<&Path> = steps
            .iter()
            .filter_map(|step| match step {
                Step::WriteFile { path, .. } | Step::PresentImage { path, .. } => {
                    Some(path.as_path())
                }
                _ => None,
            })
            .collect();
        let ro = order
            .iter()
            .position(|path| path.ends_with("lun.0/ro"))
            .expect("ro is written");
        let file = order
            .iter()
            .position(|path| path.ends_with("lun.0/file"))
            .expect("file is written");
        assert!(
            ro < file,
            "the LUN attributes must be set before the backing file: {order:?}"
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
    fn write_report_targets_the_function_device_and_bounds_the_report() {
        let directory = tempfile::tempdir().unwrap();
        for name in ["hidg0", "hidg1", "hidg2"] {
            std::fs::write(directory.path().join(name), b"").unwrap();
        }
        let report = [0_u8, 0, 4, 0, 0, 0, 0, 0];
        assert_eq!(write_report(directory.path(), "hid.usb0", &report), Ok(()));
        assert_eq!(
            std::fs::read(directory.path().join("hidg0")).unwrap(),
            report
        );
        assert_eq!(
            write_report(directory.path(), "hid.usb2", &[1, 2, 3]),
            Ok(())
        );
        assert_eq!(
            std::fs::read(directory.path().join("hidg2")).unwrap(),
            vec![1, 2, 3]
        );

        assert_eq!(
            write_report(directory.path(), "hid.usb9", &report),
            Err(GadgetError::InvalidFunction)
        );
        assert_eq!(
            write_report(directory.path(), "hid.usb0", &[]),
            Err(GadgetError::InvalidReport)
        );
        assert_eq!(
            write_report(directory.path(), "hid.usb0", &[0_u8; 65]),
            Err(GadgetError::InvalidReport)
        );
        assert_eq!(hid_device_name("hid.usb1"), Some("hidg1"));
        assert_eq!(hid_device_name("hid.usb3"), None);
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

        // A CD-ROM needs 300 blocks of 2048 bytes; the kernel would answer
        // ETOOSMALL, so the gadget refuses it first and names the reason.
        let small = directory.path().join("small.iso");
        std::fs::write(&small, vec![0_u8; (MIN_CDROM_IMAGE_BYTES - 2048) as usize]).unwrap();
        let cdrom = |file: &Path| {
            Function::MassStorage(MassStorageConfig {
                file: file.to_path_buf(),
                read_only: true,
                cdrom: true,
                removable: true,
                second_lun: None,
            })
        };
        let mut invalid = spec(&image);
        invalid.functions = vec![cdrom(&small)];
        assert_eq!(
            apply(directory.path(), &invalid),
            Err(GadgetError::ImageTooSmall)
        );
        let large = directory.path().join("large.iso");
        std::fs::write(&large, vec![0_u8; MIN_CDROM_IMAGE_BYTES as usize]).unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let mut valid = spec(&image);
        valid.functions = vec![cdrom(&large)];
        assert_ne!(
            apply(elsewhere.path(), &valid),
            Err(GadgetError::ImageTooSmall)
        );

        let mut invalid = spec(&image);
        invalid.functions = vec![Function::MassStorage(MassStorageConfig {
            file: PathBuf::from("relative.img"),
            read_only: true,
            cdrom: false,
            removable: true,
            second_lun: None,
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

    #[test]
    fn reconnect_rebinds_the_same_controller() {
        let directory = root_with_image();
        let spec = spec(&directory.path().join(IMAGE_NAME));
        assert_eq!(apply(directory.path(), &spec), Ok(()));
        assert_eq!(reconnect(directory.path(), &spec.udc), Ok(()));
        assert_eq!(read(directory.path(), "hardened/UDC"), spec.udc);
        assert_eq!(reconnect(directory.path(), &spec.udc), Ok(()));
        assert_eq!(read(directory.path(), "hardened/UDC"), spec.udc);
        assert_eq!(unbind(directory.path()), Ok(()));
        // A real configfs strips the trailing newline of the one-byte write.
        assert_eq!(read(directory.path(), "hardened/UDC").trim(), "");
    }

    #[test]
    fn replace_mass_storage_swaps_the_image_and_rebinds() {
        let directory = root_with_image();
        std::fs::write(directory.path().join("other.img"), b"image").unwrap();
        let base = spec(&directory.path().join(IMAGE_NAME));
        assert_eq!(apply(directory.path(), &base), Ok(()));
        assert_eq!(read(directory.path(), "hardened/UDC"), base.udc);

        assert_eq!(unbind(directory.path()), Ok(()));
        assert_eq!(unbind(directory.path()), Ok(()));
        // Rebind so the swap starts from a bound gadget, the common path.
        assert_eq!(apply(directory.path(), &base), Ok(()));

        let swapped = spec(&directory.path().join("other.img"));
        assert_eq!(replace_mass_storage(directory.path(), &swapped), Ok(()));
        assert_eq!(read(directory.path(), "hardened/UDC"), swapped.udc);
        let state = observe(directory.path()).unwrap();
        let storage = state.mass_storage.unwrap();
        assert!(storage.file.ends_with("other.img"));
        // The function is recreated, not duplicated.
        assert!(
            directory
                .path()
                .join("hardened/functions/mass_storage.usb0/lun.0/file")
                .is_file()
        );
    }

    #[test]
    fn remove_mass_storage_keeps_the_other_functions_bound() {
        let directory = root_with_image();
        let image = directory.path().join(IMAGE_NAME);
        let mut functions = vec![msd(&image)];
        functions.extend(pinned_hid_functions());
        let composite = GadgetSpec {
            functions,
            ..spec(&image)
        };
        assert_eq!(apply(directory.path(), &composite), Ok(()));

        let hid_only = GadgetSpec {
            functions: pinned_hid_functions(),
            ..spec(&image)
        };
        assert_eq!(remove_mass_storage(directory.path(), &hid_only), Ok(()));
        let state = observe(directory.path()).unwrap();
        // The drive is gone while the gadget stays bound for HID.
        assert!(state.mass_storage.is_none());
        assert_eq!(state.udc.as_deref(), Some("musb-hdrc.2.auto"));
        assert!(
            directory
                .path()
                .join("hardened/functions/hid.usb0")
                .is_dir()
        );
        // Removing it again is a tolerated no-op, and a mass-storage spec is
        // rejected: this path only ever drops the drive.
        assert_eq!(remove_mass_storage(directory.path(), &hid_only), Ok(()));
        assert_eq!(
            remove_mass_storage(directory.path(), &composite),
            Err(GadgetError::InvalidSpec)
        );
    }

    fn linked(root: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(root.join("hardened/configs/c.1"))
            .unwrap()
            .map(|entry| entry.unwrap())
            .filter(|entry| entry.file_type().unwrap().is_symlink())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn hid_gadget(root: &Path, image: Option<&Path>) -> GadgetSpec {
        let mut spec = spec(root.join(IMAGE_NAME).as_path());
        spec.functions = pinned_hid_functions();
        if let Some(image) = image {
            spec.functions.insert(0, msd(image));
        }
        spec
    }

    #[test]
    fn replace_hid_function_switches_the_agents_collection_and_keeps_the_rest() {
        let directory = root_with_image();
        let root = directory.path();
        let image = root.join(IMAGE_NAME);
        assert_eq!(apply(root, &hid_gadget(root, Some(&image))), Ok(()));
        assert_eq!(absolute_mouse_report_id(root), Ok(None));

        let agent = absolute_mouse_function(true);
        assert_eq!(
            replace_hid_function(root, "musb-hdrc.2.auto", &agent),
            Ok(())
        );
        assert_eq!(
            absolute_mouse_report_id(root),
            Ok(Some(ABSOLUTE_MOUSE_REPORT_ID))
        );
        assert_eq!(
            read(root, "hardened/functions/hid.usb1/report_length"),
            "64"
        );
        assert_eq!(read(root, "hardened/UDC"), "musb-hdrc.2.auto");
        assert_eq!(
            linked(root),
            ["hid.usb0", "hid.usb1", "hid.usb2", "mass_storage.usb0"]
        );
        // The virtual media and the other functions are untouched.
        assert_eq!(
            read(root, "hardened/functions/mass_storage.usb0/lun.0/file"),
            image.to_string_lossy()
        );
        assert_eq!(read(root, "hardened/functions/hid.usb0/report_length"), "8");

        let plain = absolute_mouse_function(false);
        assert_eq!(
            replace_hid_function(root, "musb-hdrc.2.auto", &plain),
            Ok(())
        );
        assert_eq!(absolute_mouse_report_id(root), Ok(None));
        assert_eq!(read(root, "hardened/functions/hid.usb1/report_length"), "7");
    }

    #[test]
    fn replace_hid_function_refuses_what_it_does_not_know_before_touching_it() {
        let directory = root_with_image();
        let root = directory.path();
        // No gadget at all.
        assert_eq!(
            replace_hid_function(root, "musb-hdrc.2.auto", &absolute_mouse_function(true)),
            Err(GadgetError::InvalidSpec)
        );
        assert_eq!(apply(root, &hid_gadget(root, None)), Ok(()));
        // A function outside the reviewed names, and a bad controller.
        let mut other = absolute_mouse_function(true);
        other.name = "hid.usb7".into();
        assert_eq!(
            replace_hid_function(root, "musb-hdrc.2.auto", &other),
            Err(GadgetError::InvalidFunction)
        );
        assert_eq!(
            replace_hid_function(root, "../udc", &absolute_mouse_function(true)),
            Err(GadgetError::InvalidUdc)
        );
        // A link nobody reviewed.
        std::os::unix::fs::symlink(
            root.join("hardened/functions/hid.usb0"),
            root.join("hardened/configs/c.1/acm.usb0"),
        )
        .unwrap();
        assert_eq!(
            replace_hid_function(root, "musb-hdrc.2.auto", &absolute_mouse_function(true)),
            Err(GadgetError::InvalidSpec)
        );
        assert_eq!(read(root, "hardened/UDC"), "musb-hdrc.2.auto");
        assert_eq!(absolute_mouse_report_id(root), Ok(None));
    }

    #[test]
    fn a_failed_attribute_write_leaves_the_gadget_linked_and_bound() {
        let directory = root_with_image();
        let root = directory.path();
        assert_eq!(apply(root, &hid_gadget(root, None)), Ok(()));
        // The descriptor cannot be written: the links and the binding come
        // back all the same.
        let descriptor = root.join("hardened/functions/hid.usb1/report_desc");
        std::fs::remove_file(&descriptor).unwrap();
        std::fs::create_dir(&descriptor).unwrap();
        assert!(matches!(
            replace_hid_function(root, "musb-hdrc.2.auto", &absolute_mouse_function(true)),
            Err(GadgetError::Io(_))
        ));
        assert_eq!(linked(root), ["hid.usb0", "hid.usb1", "hid.usb2"]);
        assert_eq!(read(root, "hardened/UDC"), "musb-hdrc.2.auto");
    }

    #[test]
    fn an_unknown_absolute_mouse_descriptor_has_no_report_id() {
        let directory = root_with_image();
        let root = directory.path();
        assert!(absolute_mouse_report_id(root).is_err());
        let mut spec = hid_gadget(root, None);
        spec.functions[1] = hid("hid.usb1", 7);
        assert_eq!(apply(root, &spec), Ok(()));
        assert_eq!(
            absolute_mouse_report_id(root),
            Err(GadgetError::InvalidReportDescriptor)
        );
    }

    #[test]
    fn a_second_lun_is_made_before_the_link_and_goes_before_the_function() {
        let directory = root_with_image();
        let root = directory.path();
        let image = root.join(IMAGE_NAME);
        let drive = root.join("agent-drive.img");
        std::fs::write(&drive, b"drive").unwrap();
        let with_drive = |file: &Path, removable: bool| {
            let mut spec = hid_gadget(root, None);
            spec.functions
                .push(Function::MassStorage(MassStorageConfig {
                    file: file.to_path_buf(),
                    read_only: true,
                    cdrom: false,
                    removable,
                    second_lun: (file != drive).then(|| drive.clone()),
                }));
            spec
        };
        // The image on LUN 0, the drive on LUN 1: read-only, not removable,
        // its attributes before its file and all before the link.
        let spec = with_drive(&image, true);
        let steps = bind_steps(&spec);
        let position = |wanted: &str| {
            steps
                .iter()
                .position(|step| match step {
                    Step::WriteFile { path, .. }
                    | Step::PresentImage { path, .. }
                    | Step::Symlink { link: path, .. } => path.ends_with(wanted),
                    _ => false,
                })
                .unwrap()
        };
        assert!(position("lun.1/ro") < position("lun.1/file"));
        assert!(position("lun.1/file") < position("configs/c.1/mass_storage.usb0"));
        assert_eq!(apply(root, &spec), Ok(()));
        let lun = "hardened/functions/mass_storage.usb0/lun.1";
        assert_eq!(read(root, &format!("{lun}/ro")), "1");
        assert_eq!(read(root, &format!("{lun}/removable")), "0");
        let state = observe(root).unwrap().mass_storage.unwrap();
        assert!(state.removable);
        assert_eq!(state.second_lun, Some(drive.clone()));

        // A swap keeps the drive when asked to, and drops it when not.
        assert_eq!(replace_mass_storage(root, &spec), Ok(()));
        assert_eq!(
            observe(root).unwrap().mass_storage.unwrap().second_lun,
            Some(drive.clone())
        );
        let mut without = spec.clone();
        if let Function::MassStorage(storage) = without.functions.last_mut().unwrap() {
            storage.second_lun = None;
        }
        assert_eq!(replace_mass_storage(root, &without), Ok(()));
        assert!(!root.join(lun).exists());
        assert_eq!(
            observe(root).unwrap().mass_storage.unwrap().second_lun,
            None
        );

        // The drive alone on LUN 0, not removable; an eject removes it all.
        assert_eq!(
            replace_mass_storage(root, &with_drive(&drive, false)),
            Ok(())
        );
        let state = observe(root).unwrap().mass_storage.unwrap();
        assert!(!state.removable && state.file == drive && state.second_lun.is_none());
        assert_eq!(replace_mass_storage(root, &spec), Ok(()));
        assert_eq!(remove_mass_storage(root, &hid_gadget(root, None)), Ok(()));
        assert!(observe(root).unwrap().mass_storage.is_none());
        assert_eq!(read(root, "hardened/UDC"), "musb-hdrc.2.auto");
    }

    #[test]
    fn reconnect_rejects_an_invalid_controller() {
        let directory = root_with_image();
        assert!(reconnect(directory.path(), "bad controller").is_err());
        assert!(reconnect(directory.path(), "").is_err());
    }
}
