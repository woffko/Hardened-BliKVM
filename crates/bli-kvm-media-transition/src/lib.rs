//! Persisted virtual-media transition state machine.
//!
//! The engine combines the private `bli-kvm-media-policy` journal with the
//! fixed-route `bli-kvm-kvmd-media-client` and the opaque-ID catalog. A permit
//! persists Pending, exactly one fixed KVMD route is called, and the completion
//! is persisted before the outcome is returned. Duplicates and conflicts return
//! the persisted result without another backend call.

use bli_kvm_core::VirtualMediaCommand;
#[cfg(feature = "kvmd-media-profile")]
use bli_kvm_kvmd_client::KvmdCredentials;
#[cfg(feature = "kvmd-media-profile")]
use bli_kvm_kvmd_media_client::KvmdMediaClient;
use bli_kvm_media_catalog::MediaCatalog;
use bli_kvm_media_policy::{
    MediaCompletion, MediaDecision, MediaEligibility, MediaObservedState, MediaOperation,
    MediaPolicyEngine, MediaPolicyError, MediaRejection,
};
use thiserror::Error;

#[cfg(feature = "gadget-media-profile")]
pub use bli_kvm_usb_gadget::GadgetStrings;
#[cfg(feature = "gadget-media-profile")]
use bli_kvm_usb_gadget::{Function, GadgetSpec, MassStorageConfig};
#[cfg(feature = "gadget-media-profile")]
use std::path::PathBuf;

/// Why the backend could not apply a media command, so the operator is told
/// more than "rejected".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaBackendFailure {
    /// The selected image's file is not in the image store.
    ImageMissing,
    /// The image is shorter than a CD-ROM needs.
    ImageTooSmall,
    Other,
}

#[cfg(feature = "gadget-media-profile")]
fn gadget_failure(error: &bli_kvm_usb_gadget::GadgetError) -> MediaBackendFailure {
    match error {
        bli_kvm_usb_gadget::GadgetError::ImageMissing => MediaBackendFailure::ImageMissing,
        bli_kvm_usb_gadget::GadgetError::ImageTooSmall => MediaBackendFailure::ImageTooSmall,
        _ => MediaBackendFailure::Other,
    }
}

#[derive(Debug, Error)]
pub enum MediaTransitionError {
    #[error("media policy journal failed")]
    Policy(#[from] MediaPolicyError),
    #[error("media backend state is unavailable")]
    Backend,
}

/// KVMD-backed media executor. Holds its own credential instance.
#[cfg(feature = "kvmd-media-profile")]
pub struct KvmdMediaExecutor {
    client: KvmdMediaClient,
    credentials: KvmdCredentials,
}

#[cfg(feature = "kvmd-media-profile")]
impl KvmdMediaExecutor {
    pub fn new(client: KvmdMediaClient, credentials: KvmdCredentials) -> Self {
        Self {
            client,
            credentials,
        }
    }

    async fn observe(&self) -> Result<MediaObservedState, MediaTransitionError> {
        let state = self
            .client
            .state(&self.credentials)
            .await
            .map_err(|_| MediaTransitionError::Backend)?;
        Ok(MediaObservedState {
            enabled: state.enabled,
            online: state.online,
            busy: state.busy,
            connected: state.connected,
        })
    }

    async fn execute(
        &self,
        command: &VirtualMediaCommand,
        catalog: &MediaCatalog,
    ) -> Result<(), MediaBackendFailure> {
        self.client
            .execute(command, catalog, &self.credentials)
            .await
            .map(|_| ())
            .map_err(|_| MediaBackendFailure::Other)
    }
}

/// Backend that reads the drive state and applies media commands.
///
/// The transition engine is backend-agnostic so the KVMD executor used during
/// migration can later be replaced by the Rust configfs MSD worker without
/// touching policy or broker wiring.
pub enum MediaExecutor {
    #[cfg(feature = "kvmd-media-profile")]
    Kvmd(KvmdMediaExecutor),
    #[cfg(feature = "gadget-media-profile")]
    Gadget(GadgetMediaExecutor),
    #[cfg(test)]
    Test {
        observed: MediaObservedState,
        applied: bool,
    },
}

impl MediaExecutor {
    /// Binds the pinned HID functions when the configfs root exists and no
    /// gadget is present, so keyboard and mouse work after a reboot before the
    /// first media operation. Returns whether a gadget was created and attached;
    /// backends without a gadget leave the call a no-op.
    pub fn ensure_hid_gadget(&self) -> Result<bool, MediaTransitionError> {
        #[cfg(feature = "gadget-media-profile")]
        match self {
            MediaExecutor::Gadget(executor) => executor.ensure_hid_gadget(),
            #[allow(unreachable_patterns)]
            _ => Ok(false),
        }
        #[cfg(not(feature = "gadget-media-profile"))]
        {
            let _ = self;
            Ok(false)
        }
    }

    /// Gives the absolute mouse's interface the target agent's collection,
    /// or takes it away (`GadgetMediaExecutor::set_agent`); a backend
    /// without a gadget of its own cannot.
    pub fn set_agent(&self, enabled: bool) -> Result<bool, MediaTransitionError> {
        #[cfg(feature = "gadget-media-profile")]
        match self {
            MediaExecutor::Gadget(executor) => executor.set_agent(enabled),
            #[allow(unreachable_patterns)]
            _ => Err(MediaTransitionError::Backend),
        }
        #[cfg(not(feature = "gadget-media-profile"))]
        {
            let _ = (self, enabled);
            Err(MediaTransitionError::Backend)
        }
    }

    /// Presents the target agent's drive, or stops
    /// (`GadgetMediaExecutor::set_agent_drive`); a backend without a gadget
    /// of its own cannot.
    pub fn set_agent_drive(
        &self,
        drive: Option<std::path::PathBuf>,
    ) -> Result<bool, MediaTransitionError> {
        #[cfg(feature = "gadget-media-profile")]
        match self {
            MediaExecutor::Gadget(executor) => executor.set_agent_drive(drive),
            #[allow(unreachable_patterns)]
            _ => Err(MediaTransitionError::Backend),
        }
        #[cfg(not(feature = "gadget-media-profile"))]
        {
            let _ = (self, drive);
            Err(MediaTransitionError::Backend)
        }
    }

    /// Sets the agent's drive of gadgets built from here on, without
    /// touching a present one.
    pub fn want_agent_drive(&self, drive: Option<std::path::PathBuf>) {
        // The gadget may be the only backend built.
        #[cfg(feature = "gadget-media-profile")]
        #[allow(irrefutable_let_patterns)]
        if let MediaExecutor::Gadget(executor) = self {
            executor.want_agent_drive(drive);
        }
        #[cfg(not(feature = "gadget-media-profile"))]
        let _ = (self, drive);
    }

    /// Sets whether gadgets built from here on carry the agent's collection,
    /// without touching a present one.
    pub fn want_agent(&self, enabled: bool) {
        // The gadget may be the only backend built.
        #[cfg(feature = "gadget-media-profile")]
        #[allow(irrefutable_let_patterns)]
        if let MediaExecutor::Gadget(executor) = self {
            executor.want_agent(enabled);
        }
        #[cfg(not(feature = "gadget-media-profile"))]
        let _ = (self, enabled);
    }
}

/// Configuration for the Rust configfs gadget media executor.
#[cfg(feature = "gadget-media-profile")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GadgetMediaConfig {
    pub configfs_root: PathBuf,
    pub udc: String,
    pub image_directory: PathBuf,
    pub strings: GadgetStrings,
    pub id_vendor: u16,
    pub id_product: u16,
    pub bcd_usb: u16,
    pub bcd_device: u16,
}

#[cfg(feature = "gadget-media-profile")]
#[derive(Clone)]
struct SelectedImage {
    path: PathBuf,
    read_only: bool,
    cdrom: bool,
}

/// Rust configfs MSD executor: select writes the LUN file, connect binds the
/// UDC, disconnect unbinds it.
#[cfg(feature = "gadget-media-profile")]
pub struct GadgetMediaExecutor {
    config: GadgetMediaConfig,
    selected: std::sync::Mutex<Option<SelectedImage>>,
    /// Whether `hid.usb1` carries the target agent's collection in every
    /// gadget built from scratch.
    agent: std::sync::atomic::AtomicBool,
    /// The target agent's drive, presented read-only beside the virtual
    /// media (LUN 1), or alone (LUN 0, not removable) without them.
    drive: std::sync::Mutex<Option<PathBuf>>,
}

#[cfg(feature = "gadget-media-profile")]
impl GadgetMediaExecutor {
    pub fn new(config: GadgetMediaConfig) -> Self {
        Self {
            config,
            selected: std::sync::Mutex::new(None),
            agent: std::sync::atomic::AtomicBool::new(false),
            drive: std::sync::Mutex::new(None),
        }
    }

    fn drive(&self) -> Option<PathBuf> {
        self.drive.lock().ok().and_then(|drive| drive.clone())
    }

    /// Sets the target agent's drive of every gadget built from here on,
    /// without touching a present one.
    pub fn want_agent_drive(&self, drive: Option<PathBuf>) {
        if let Ok(mut wanted) = self.drive.lock() {
            *wanted = drive;
        }
    }

    /// The HID functions, and the agent's drive alone when it is wanted:
    /// the gadget without virtual media.
    fn rest_spec(&self) -> GadgetSpec {
        let mut spec = self.hid_spec();
        if let Some(drive) = self.drive() {
            spec.functions
                .push(Function::MassStorage(MassStorageConfig {
                    file: drive,
                    read_only: true,
                    cdrom: false,
                    removable: false,
                    second_lun: None,
                }));
        }
        spec
    }

    /// The agent's drive the present gadget presents, whether beside an
    /// image or alone.
    fn presented_drive(&self) -> Option<PathBuf> {
        let state = bli_kvm_usb_gadget::observe(&self.config.configfs_root).ok()?;
        let storage = state.mass_storage?;
        if storage.removable {
            storage.second_lun
        } else {
            Some(storage.file)
        }
    }

    /// Presents the target agent's drive, or stops presenting it, and keeps
    /// gadgets built later so. An attached image stays attached; the target
    /// sees the device leave and come back. Returns whether anything
    /// changed.
    pub fn set_agent_drive(&self, drive: Option<PathBuf>) -> Result<bool, MediaTransitionError> {
        self.want_agent_drive(drive.clone());
        if !self.config.configfs_root.is_dir() {
            return Err(MediaTransitionError::Backend);
        }
        if !self.state_present() {
            return self.ensure_hid_gadget();
        }
        if self.presented_drive() == drive {
            return Ok(false);
        }
        let root = &self.config.configfs_root;
        let result = if self.has_mass_storage() {
            let Some(selected) = self.current_selection() else {
                return Err(MediaTransitionError::Backend);
            };
            bli_kvm_usb_gadget::replace_mass_storage(root, &self.spec(&selected))
        } else if drive.is_some() {
            bli_kvm_usb_gadget::replace_mass_storage(root, &self.rest_spec())
        } else {
            bli_kvm_usb_gadget::remove_mass_storage(root, &self.hid_spec())
        };
        result.map_err(|error| {
            eprintln!("media: the agent's drive could not be changed: {error:?}");
            MediaTransitionError::Backend
        })?;
        Ok(true)
    }

    fn hid_functions(&self) -> Vec<Function> {
        bli_kvm_usb_gadget::hid_functions(self.agent.load(std::sync::atomic::Ordering::SeqCst))
    }

    /// Sets whether gadgets built from here on carry the target agent's
    /// collection, without touching a present one: the HID bind before the
    /// owner starts builds the gadget as the switch says.
    pub fn want_agent(&self, enabled: bool) {
        self.agent
            .store(enabled, std::sync::atomic::Ordering::SeqCst);
    }

    /// Gives `hid.usb1` the target agent's collection, or takes it away, and
    /// keeps gadgets built later so. A present gadget is changed in place
    /// (`bli_kvm_usb_gadget::replace_hid_function`): the target sees the
    /// whole device leave and come back, and an attached image stays
    /// attached. Returns whether anything changed. A change that fails
    /// leaves gadgets built later as they were: the owner puts its switch
    /// back too, and the collection is never offered against it.
    pub fn set_agent(&self, enabled: bool) -> Result<bool, MediaTransitionError> {
        let was = self
            .agent
            .swap(enabled, std::sync::atomic::Ordering::SeqCst);
        let result = self.change_agent(enabled);
        if result.is_err() {
            self.want_agent(was);
        }
        result
    }

    fn change_agent(&self, enabled: bool) -> Result<bool, MediaTransitionError> {
        if !self.config.configfs_root.is_dir() {
            return Err(MediaTransitionError::Backend);
        }
        if !self.state_present() {
            return self.ensure_hid_gadget();
        }
        let root = &self.config.configfs_root;
        let present = bli_kvm_usb_gadget::absolute_mouse_report_id(root)
            .map_err(|_| MediaTransitionError::Backend)?
            .is_some();
        if present == enabled {
            return Ok(false);
        }
        bli_kvm_usb_gadget::replace_hid_function(
            root,
            &self.config.udc,
            &bli_kvm_usb_gadget::absolute_mouse_function(enabled),
        )
        .map_err(|error| {
            eprintln!("media: the agent's collection could not be changed: {error:?}");
            MediaTransitionError::Backend
        })?;
        let changed = bli_kvm_usb_gadget::absolute_mouse_report_id(root)
            .map_err(|_| MediaTransitionError::Backend)?
            .is_some();
        let state = bli_kvm_usb_gadget::observe(root).map_err(|_| MediaTransitionError::Backend)?;
        if changed != enabled || state.udc.is_none() {
            return Err(MediaTransitionError::Backend);
        }
        Ok(true)
    }

    fn spec(&self, selected: &SelectedImage) -> GadgetSpec {
        let mut functions = vec![Function::MassStorage(MassStorageConfig {
            file: selected.path.clone(),
            read_only: selected.read_only,
            cdrom: selected.cdrom,
            removable: true,
            second_lun: self.drive(),
        })];
        // The composite device also exposes the pinned HID functions captured
        // read-only from the board, the absolute mouse with the agent's
        // collection while the agent's switch is on.
        functions.extend(self.hid_functions());
        GadgetSpec {
            id_vendor: self.config.id_vendor,
            id_product: self.config.id_product,
            bcd_usb: self.config.bcd_usb,
            bcd_device: self.config.bcd_device,
            strings: self.config.strings.clone(),
            udc: self.config.udc.clone(),
            functions,
        }
    }

    fn state_present(&self) -> bool {
        bli_kvm_usb_gadget::observe(&self.config.configfs_root)
            .map(|state| state.present)
            .unwrap_or(false)
    }

    /// Whether the present gadget exposes an image of the operator's: the
    /// mass-storage function with its removable LUN 0. The owner keeps a
    /// HID-only gadget bound after a reboot, and the target agent's drive
    /// alone is a mass-storage function too (not removable), so "the gadget
    /// is up" does not imply "an image is attached".
    fn has_mass_storage(&self) -> bool {
        bli_kvm_usb_gadget::observe(&self.config.configfs_root)
            .map(|state| state.mass_storage.is_some_and(|storage| storage.removable))
            .unwrap_or(false)
    }

    fn hid_spec(&self) -> GadgetSpec {
        GadgetSpec {
            id_vendor: self.config.id_vendor,
            id_product: self.config.id_product,
            bcd_usb: self.config.bcd_usb,
            bcd_device: self.config.bcd_device,
            strings: self.config.strings.clone(),
            udc: self.config.udc.clone(),
            functions: self.hid_functions(),
        }
    }

    /// Brings the HID functions up before the first media operation. A
    /// reboot leaves no gadget behind, so without this the keyboard and mouse
    /// stay dead until an image is attached. A gadget that is already present is
    /// left untouched, so an attached image survives an owner restart.
    pub fn ensure_hid_gadget(&self) -> Result<bool, MediaTransitionError> {
        if !self.config.configfs_root.is_dir() || self.state_present() {
            return Ok(false);
        }
        bli_kvm_usb_gadget::apply(&self.config.configfs_root, &self.rest_spec())
            .map_err(|_| MediaTransitionError::Backend)?;
        // Read back what the kernel accepted: a spec that applied but did not
        // attach is still a broken HID path and must be visible to the caller.
        let state = bli_kvm_usb_gadget::observe(&self.config.configfs_root)
            .map_err(|_| MediaTransitionError::Backend)?;
        if state.udc.is_none() {
            return Err(MediaTransitionError::Backend);
        }
        Ok(true)
    }

    fn observe(&self) -> Result<MediaObservedState, MediaTransitionError> {
        let state = bli_kvm_usb_gadget::observe(&self.config.configfs_root)
            .map_err(|_| MediaTransitionError::Backend)?;
        Ok(MediaObservedState {
            enabled: true,
            online: self.config.configfs_root.is_dir(),
            busy: false,
            // The agent's drive alone (not removable) is no image.
            connected: state.udc.is_some()
                && state.mass_storage.is_some_and(|storage| storage.removable),
        })
    }

    fn select(
        &self,
        catalog: &MediaCatalog,
        image_id: u64,
        mode: bli_kvm_core::VirtualMediaControlMode,
        writable: bool,
    ) -> Result<(), MediaBackendFailure> {
        let Ok(selection) = catalog.resolve(image_id, mode, writable) else {
            return Err(MediaBackendFailure::Other);
        };
        let selected = SelectedImage {
            path: self.config.image_directory.join(selection.image_name()),
            read_only: !selection.writable(),
            cdrom: selection.mode() == bli_kvm_core::VirtualMediaControlMode::CdRom,
        };
        if self.state_present() && self.has_mass_storage() {
            bli_kvm_usb_gadget::set_lun_file(&self.config.configfs_root, &selected.path)
                .map_err(|error| gadget_failure(&error))?;
        }
        match self.selected.lock() {
            Ok(mut guard) => *guard = Some(selected),
            Err(_) => return Err(MediaBackendFailure::Other),
        }
        Ok(())
    }

    fn set_connected(&self, connected: bool) -> Result<(), MediaBackendFailure> {
        if !connected {
            if !self.state_present() || !self.has_mass_storage() {
                return Ok(());
            }
            // The gadget also owns the pinned HID functions, so an eject drops
            // the image and leaves the keyboard and mouse bound, and the
            // agent's drive alone where it is wanted.
            return if self.drive().is_some() {
                bli_kvm_usb_gadget::replace_mass_storage(
                    &self.config.configfs_root,
                    &self.rest_spec(),
                )
            } else {
                bli_kvm_usb_gadget::remove_mass_storage(
                    &self.config.configfs_root,
                    &self.hid_spec(),
                )
            }
            .map_err(|error| gadget_failure(&error));
        }
        let Some(selected) = self.current_selection() else {
            return Err(MediaBackendFailure::Other);
        };
        let spec = self.spec(&selected);
        // A gadget kept up for HID is extended in place; a cold gadget is built
        // from scratch. Both attach the drive to the target.
        let result = if self.state_present() {
            bli_kvm_usb_gadget::replace_mass_storage(&self.config.configfs_root, &spec)
        } else {
            bli_kvm_usb_gadget::apply(&self.config.configfs_root, &spec)
        };
        result.map_err(|error| {
            // The operator log keeps the detail; the answer names the reason
            // when it is one the operator can act on.
            eprintln!("media: gadget apply failed: {error:?}");
            gadget_failure(&error)
        })
    }

    fn state(&self) -> Result<MediaState, MediaTransitionError> {
        let state = bli_kvm_usb_gadget::observe(&self.config.configfs_root)
            .map_err(|_| MediaTransitionError::Backend)?;
        // The agent's drive alone (not removable) is no image.
        let storage = state
            .mass_storage
            .as_ref()
            .filter(|storage| storage.removable);
        let connected = state.udc.is_some() && storage.is_some();
        Ok(MediaState {
            // The reference surface treats a disconnected image as unmounted:
            // the backing file may stay configured for a later remount, but it
            // is only reported while the gadget is attached.
            file: connected
                .then(|| {
                    storage
                        .and_then(|storage| storage.file.file_name())
                        .map(|name| name.to_string_lossy().into_owned())
                })
                .flatten(),
            connected,
            cdrom: storage.is_some_and(|storage| storage.cdrom),
            writable: storage.is_some_and(|storage| !storage.read_only),
        })
    }

    /// The recorded selection, or the one the gadget currently exposes after
    /// a restart of the owner process. The kernel names the backing file as
    /// the reading process reaches it: to an owner started again, an image
    /// store mounted in the former owner's namespace is named from the
    /// store's own root (`/disk.iso`). Every selection is an image of the
    /// store by its name, so it is that name in the store.
    fn current_selection(&self) -> Option<SelectedImage> {
        if let Ok(guard) = self.selected.lock()
            && let Some(selected) = guard.clone()
        {
            return Some(selected);
        }
        let state = bli_kvm_usb_gadget::observe(&self.config.configfs_root).ok()?;
        let storage = state.mass_storage.filter(|storage| storage.removable)?;
        Some(SelectedImage {
            path: self.config.image_directory.join(storage.file.file_name()?),
            read_only: storage.read_only,
            cdrom: storage.cdrom,
        })
    }

    fn reconnect(&self) -> Result<(), MediaBackendFailure> {
        // Rebuild the function when the selection is known: that covers a
        // gadget left torn by a failed swap and a stale controller after an
        // eject, and it re-enumerates cleanly for the target.
        if self.state_present()
            && let Some(selected) = self.current_selection()
        {
            match bli_kvm_usb_gadget::replace_mass_storage(
                &self.config.configfs_root,
                &self.spec(&selected),
            ) {
                Ok(()) => return Ok(()),
                Err(error) => eprintln!("media: gadget reconnect failed: {error:?}"),
            }
        }
        bli_kvm_usb_gadget::reconnect(&self.config.configfs_root, &self.config.udc).map_err(
            |error| {
                eprintln!("media: gadget rebind failed: {error:?}");
                gadget_failure(&error)
            },
        )
    }

    fn mount(
        &self,
        catalog: &MediaCatalog,
        image_id: u64,
        mode: bli_kvm_core::VirtualMediaControlMode,
        writable: bool,
    ) -> Result<(), MediaBackendFailure> {
        // A LUN that still has a backing file open refuses attribute writes
        // (the kernel returns EBUSY for `ro`, `cdrom` and `file`), so drop the
        // gadget and rebuild it for the new selection instead of patching it.
        let Ok(selection) = catalog.resolve(image_id, mode, writable) else {
            return Err(MediaBackendFailure::Other);
        };
        let selected = SelectedImage {
            path: self.config.image_directory.join(selection.image_name()),
            read_only: !selection.writable(),
            cdrom: selection.mode() == bli_kvm_core::VirtualMediaControlMode::CdRom,
        };
        // The kernel refuses `ro`, `cdrom` and `file` writes while a LUN still
        // holds a backing file, so rebuild the mass-storage function for the
        // new selection instead of patching the existing one. The HID
        // functions are left alone: the owner keeps their character devices.
        let spec = self.spec(&selected);
        let result = if self.state_present() {
            bli_kvm_usb_gadget::replace_mass_storage(&self.config.configfs_root, &spec)
        } else {
            bli_kvm_usb_gadget::apply(&self.config.configfs_root, &spec)
        };
        if let Err(error) = result {
            eprintln!("media: gadget mount failed: {error:?}");
            return Err(gadget_failure(&error));
        }
        match self.selected.lock() {
            Ok(mut guard) => *guard = Some(selected),
            Err(_) => return Err(MediaBackendFailure::Other),
        }
        Ok(())
    }

    fn execute(
        &self,
        command: &VirtualMediaCommand,
        catalog: &MediaCatalog,
    ) -> Result<(), MediaBackendFailure> {
        match command {
            VirtualMediaCommand::Select {
                image_id,
                mode,
                writable,
            } => self.select(catalog, *image_id, *mode, *writable),
            VirtualMediaCommand::Mount {
                image_id,
                mode,
                writable,
            } => self.mount(catalog, *image_id, *mode, *writable),
            VirtualMediaCommand::SetConnected { connected } => self.set_connected(*connected),
            VirtualMediaCommand::Reconnect => self.reconnect(),
        }
    }
}

/// Read-only current media state, independent of the policy journal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaState {
    pub file: Option<String>,
    pub connected: bool,
    pub cdrom: bool,
    pub writable: bool,
}

impl MediaExecutor {
    async fn state(&self) -> Option<MediaState> {
        match self {
            #[cfg(feature = "kvmd-media-profile")]
            MediaExecutor::Kvmd(_) => None,
            #[cfg(feature = "gadget-media-profile")]
            MediaExecutor::Gadget(executor) => executor.state().ok(),
            #[cfg(test)]
            MediaExecutor::Test { .. } => None,
        }
    }

    async fn observe(&self) -> Result<MediaObservedState, MediaTransitionError> {
        match self {
            #[cfg(feature = "kvmd-media-profile")]
            MediaExecutor::Kvmd(executor) => executor.observe().await,
            #[cfg(feature = "gadget-media-profile")]
            MediaExecutor::Gadget(executor) => executor.observe(),
            #[cfg(test)]
            MediaExecutor::Test { observed, .. } => Ok(*observed),
        }
    }

    async fn execute(
        &self,
        command: &VirtualMediaCommand,
        catalog: &MediaCatalog,
    ) -> Result<(), MediaBackendFailure> {
        match self {
            #[cfg(feature = "kvmd-media-profile")]
            MediaExecutor::Kvmd(executor) => executor.execute(command, catalog).await,
            #[cfg(feature = "gadget-media-profile")]
            MediaExecutor::Gadget(executor) => executor.execute(command, catalog),
            #[cfg(test)]
            MediaExecutor::Test { applied, .. } => {
                if *applied {
                    Ok(())
                } else {
                    Err(MediaBackendFailure::Other)
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaTransitionOutcome {
    Applied,
    Noop,
    ReplayedApplied,
    ReplayedRejected,
    BackendFailed(MediaBackendFailure),
    Rejected(MediaRejection),
}

pub struct MediaTransitionEngine {
    policy: MediaPolicyEngine,
    executor: MediaExecutor,
    catalog: MediaCatalog,
    /// When set, the catalog is re-read from this file before every permit so
    /// an operator upload becomes selectable without restarting the owner. An
    /// unreadable file keeps the last good catalog instead of failing closed
    /// on the whole engine.
    catalog_path: Option<std::path::PathBuf>,
}

impl MediaTransitionEngine {
    #[cfg(feature = "kvmd-media-profile")]
    pub fn open(
        journal_path: impl Into<std::path::PathBuf>,
        client: KvmdMediaClient,
        catalog: MediaCatalog,
        credentials: KvmdCredentials,
    ) -> Result<Self, MediaTransitionError> {
        Self::open_with_executor(
            journal_path,
            catalog,
            MediaExecutor::Kvmd(KvmdMediaExecutor::new(client, credentials)),
        )
    }

    /// Build an engine over any reviewed executor backend.
    pub fn open_with_executor(
        journal_path: impl Into<std::path::PathBuf>,
        catalog: MediaCatalog,
        executor: MediaExecutor,
    ) -> Result<Self, MediaTransitionError> {
        Ok(Self {
            policy: MediaPolicyEngine::open(journal_path)?,
            executor,
            catalog,
            catalog_path: None,
        })
    }

    /// Reload the catalog from this path before each applied operation.
    pub fn with_catalog_path(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.catalog_path = Some(path.into());
        self
    }

    /// Binds the pinned HID functions at owner startup so a reboot does not
    /// leave the keyboard and mouse without a gadget. Best-effort by design:
    /// the caller keeps serving ATX and the control API when the USB path is
    /// unavailable, and the device HID gate is the acceptance proof.
    pub fn ensure_hid_gadget(&self) -> Result<bool, MediaTransitionError> {
        self.executor.ensure_hid_gadget()
    }

    /// See [`MediaExecutor::set_agent`].
    pub fn set_agent(&self, enabled: bool) -> Result<bool, MediaTransitionError> {
        self.executor.set_agent(enabled)
    }

    /// See [`MediaExecutor::want_agent`].
    pub fn want_agent(&self, enabled: bool) {
        self.executor.want_agent(enabled);
    }

    /// See [`MediaExecutor::set_agent_drive`].
    pub fn set_agent_drive(
        &self,
        drive: Option<std::path::PathBuf>,
    ) -> Result<bool, MediaTransitionError> {
        self.executor.set_agent_drive(drive)
    }

    /// See [`MediaExecutor::want_agent_drive`].
    pub fn want_agent_drive(&self, drive: Option<std::path::PathBuf>) {
        self.executor.want_agent_drive(drive);
    }

    fn refresh_catalog(&mut self) {
        let Some(path) = self.catalog_path.as_deref() else {
            return;
        };
        if let Ok(catalog) = MediaCatalog::open(path) {
            self.catalog = catalog;
        }
    }

    pub fn pending_count(&self) -> usize {
        self.policy.pending_count()
    }

    /// Reads the fixed read-only `/msd` state used for media eligibility.
    pub async fn observe(&self) -> Result<MediaObservedState, MediaTransitionError> {
        self.executor.observe().await
    }

    /// Current backend state (image name, attachment, mode), when the backend
    /// can report it. KVMD profiles cannot and return `None`.
    pub async fn state(&self) -> Option<MediaState> {
        self.executor.state().await
    }

    pub async fn execute(
        &mut self,
        operation: MediaOperation,
        eligibility: MediaEligibility,
        observed: MediaObservedState,
        now_ms: u64,
    ) -> Result<MediaTransitionOutcome, MediaTransitionError> {
        let decision = self
            .policy
            .prepare(operation, eligibility, observed, now_ms)?;
        match decision {
            MediaDecision::Reject(rejection) => Ok(MediaTransitionOutcome::Rejected(rejection)),
            MediaDecision::Noop => Ok(MediaTransitionOutcome::Noop),
            MediaDecision::ReplayApplied => Ok(MediaTransitionOutcome::ReplayedApplied),
            MediaDecision::ReplayRejected => Ok(MediaTransitionOutcome::ReplayedRejected),
            MediaDecision::Permit => {
                self.refresh_catalog();
                let operation_id = operation.operation_id();
                let command = operation.command();
                let result = self.executor.execute(&command, &self.catalog).await;
                let completion = if result.is_ok() {
                    MediaCompletion::Applied
                } else {
                    MediaCompletion::Rejected
                };
                self.policy.complete(operation_id, completion, now_ms)?;
                match result {
                    Ok(()) => Ok(MediaTransitionOutcome::Applied),
                    Err(failure) => Ok(MediaTransitionOutcome::BackendFailed(failure)),
                }
            }
        }
    }

    pub fn resolve_pending(
        &mut self,
        operation_id: [u8; 16],
        completion: MediaCompletion,
        reconciled_at_ms: u64,
    ) -> Result<(), MediaTransitionError> {
        self.policy
            .resolve_pending(operation_id, completion, reconciled_at_ms)
            .map_err(MediaTransitionError::Policy)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use bli_kvm_core::{VirtualMediaCommand, VirtualMediaControlMode};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::UnixListener,
    };

    use super::*;

    const CATALOG: &str = r#"{
        "schemaVersion": 1,
        "entries": [{
            "imageId": 7,
            "imageName": "rescue.iso",
            "size": 1,
            "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "cdromAllowed": true,
            "removableAllowed": false,
            "writableAllowed": false
        }]
    }"#;

    async fn serve(socket: &Path, statuses: &[&str]) -> tokio::task::JoinHandle<Vec<String>> {
        let listener = UnixListener::bind(socket).unwrap();
        let statuses = statuses
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>();
        tokio::spawn(async move {
            let mut requests = Vec::new();
            for status in statuses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut chunk = [0_u8; 1024];
                    let read = stream.read(&mut chunk).await.unwrap();
                    request.extend_from_slice(&chunk[..read]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let body = if status.starts_with("200") {
                    r#"{"ok":true}"#
                } else {
                    r#"{"ok":false}"#
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
                requests.push(String::from_utf8_lossy(&request).into_owned());
            }
            requests
        })
    }

    fn catalog(directory: &Path) -> MediaCatalog {
        let path = directory.join("catalog.json");
        std::fs::write(&path, CATALOG).unwrap();
        MediaCatalog::open(&path).unwrap()
    }

    fn credentials() -> KvmdCredentials {
        KvmdCredentials::new("test-user", "not-a-real-password").unwrap()
    }

    fn engine(directory: &Path, socket: &Path) -> MediaTransitionEngine {
        MediaTransitionEngine::open(
            directory.join("journal.json"),
            KvmdMediaClient::new(socket).unwrap(),
            catalog(directory),
            credentials(),
        )
        .unwrap()
    }

    fn observed(busy: bool, connected: bool) -> MediaObservedState {
        MediaObservedState {
            enabled: true,
            online: true,
            busy,
            connected,
        }
    }

    fn operation(seed: u8, command: VirtualMediaCommand) -> MediaOperation {
        MediaOperation::new([seed; 16], 9, [0x44; 32], 1_000, command).unwrap()
    }

    #[tokio::test]
    async fn a_catalog_change_is_picked_up_without_rebuilding_the_engine() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("media.sock");
        let catalog_path = directory.path().join("catalog.json");
        std::fs::write(&catalog_path, CATALOG).unwrap();
        let server = serve(&socket, &["200", "200"]).await;

        let catalog = MediaCatalog::open(&catalog_path).unwrap();
        let mut engine = MediaTransitionEngine::open(
            directory.path().join("journal.json"),
            KvmdMediaClient::new(&socket).unwrap(),
            catalog,
            credentials(),
        )
        .unwrap()
        .with_catalog_path(&catalog_path);

        // An operator upload appends a new image to the catalog file.
        let catalog_image = bli_kvm_media_catalog::CatalogImage {
            image_name: "uploaded.img".to_string(),
            size: 4096,
            sha256: "b".repeat(64),
            cdrom_allowed: false,
            removable_allowed: true,
            writable_allowed: false,
        };
        let summary = MediaCatalog::upsert_image(&catalog_path, &catalog_image).unwrap();
        assert_eq!(summary.image_id, 8);

        let outcome = engine
            .execute(
                operation(
                    9,
                    VirtualMediaCommand::Select {
                        image_id: summary.image_id,
                        mode: VirtualMediaControlMode::RemovableDisk,
                        writable: false,
                    },
                ),
                MediaEligibility::verified_for_test_profile(),
                observed(false, false),
                2_000,
            )
            .await
            .unwrap();
        assert_eq!(outcome, MediaTransitionOutcome::Applied);
        drop(server);
    }

    #[cfg(feature = "gadget-media-profile")]
    fn gadget_config_for_test(directory: &std::path::Path, serial: &str) -> GadgetMediaConfig {
        let image_dir = directory.join("images");
        std::fs::create_dir_all(&image_dir).unwrap();
        GadgetMediaConfig {
            configfs_root: directory.join("configfs"),
            udc: "musb-hdrc.2.auto".into(),
            image_directory: image_dir,
            strings: GadgetStrings {
                manufacturer: "Hardened BLi-KVM".into(),
                product: "BLi-KVM Composite Device".into(),
                serial: serial.into(),
                configuration: "BLi-KVM configuration".into(),
            },
            id_vendor: 0x1d6b,
            id_product: 0x0104,
            bcd_usb: 0x0200,
            bcd_device: 0x0100,
        }
    }

    #[cfg(feature = "gadget-media-profile")]
    #[test]
    fn startup_binds_the_pinned_hid_functions_before_the_first_media_request() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join("configfs")).unwrap();
        let config = gadget_config_for_test(directory.path(), "BLK-ensure-hid");
        let configfs = config.configfs_root.clone();
        let executor = GadgetMediaExecutor::new(config);

        assert!(executor.ensure_hid_gadget().unwrap());
        let state = bli_kvm_usb_gadget::observe(&configfs).unwrap();
        assert!(state.present);
        assert!(
            state.udc.is_some(),
            "the gadget must attach, not only exist"
        );
        assert!(state.mass_storage.is_none());
        // The pinned keyboard and mouse functions are the point of the bind.
        assert!(configfs.join("hardened/functions/hid.usb0").is_dir());
        assert!(configfs.join("hardened/functions/hid.usb1").is_dir());
        assert!(configfs.join("hardened/functions/hid.usb2").is_dir());
        // A gadget that is already up is left untouched.
        assert!(!executor.ensure_hid_gadget().unwrap());
    }

    #[cfg(feature = "gadget-media-profile")]
    fn absolute_mouse_report_id(configfs: &std::path::Path) -> Option<u8> {
        bli_kvm_usb_gadget::absolute_mouse_report_id(configfs).unwrap()
    }

    #[cfg(feature = "gadget-media-profile")]
    #[test]
    fn the_agents_switch_changes_the_absolute_mouse_and_keeps_it_so() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join("configfs")).unwrap();
        let config = gadget_config_for_test(directory.path(), "BLK-agent");
        let configfs = config.configfs_root.clone();
        let executor = GadgetMediaExecutor::new(config);
        assert!(executor.ensure_hid_gadget().unwrap());
        assert_eq!(absolute_mouse_report_id(&configfs), None);

        assert!(executor.set_agent(true).unwrap());
        assert_eq!(absolute_mouse_report_id(&configfs), Some(1));
        assert!(!executor.set_agent(true).unwrap());
        assert!(
            bli_kvm_usb_gadget::observe(&configfs)
                .unwrap()
                .udc
                .is_some()
        );

        assert!(executor.set_agent(false).unwrap());
        assert_eq!(absolute_mouse_report_id(&configfs), None);
        assert!(!executor.set_agent(false).unwrap());
    }

    #[cfg(feature = "gadget-media-profile")]
    #[test]
    fn a_refused_agent_change_leaves_later_gadgets_as_before() {
        let directory = tempfile::tempdir().unwrap();
        let configfs = directory.path().join("configfs");
        std::fs::create_dir_all(&configfs).unwrap();
        let config = gadget_config_for_test(directory.path(), "BLK-agent-refused");
        let executor = GadgetMediaExecutor::new(config);

        // No gadget yet, and one cannot be built: turning the agent on
        // fails, and the gadget built afterwards (the lazy recovery of a
        // HID request) has no agent's collection, as the switch says.
        std::fs::write(configfs.join("hardened"), b"not a directory").unwrap();
        assert!(executor.set_agent(true).is_err());
        std::fs::remove_file(configfs.join("hardened")).unwrap();
        assert!(executor.ensure_hid_gadget().unwrap());
        assert_eq!(absolute_mouse_report_id(&configfs), None);

        // The agent on, then a turn-off the gadget refuses: a gadget built
        // from scratch afterwards still carries the collection.
        assert!(executor.set_agent(true).unwrap());
        assert_eq!(absolute_mouse_report_id(&configfs), Some(1));
        let descriptor = configfs.join("hardened/functions/hid.usb1/report_desc");
        std::fs::remove_file(&descriptor).unwrap();
        std::fs::create_dir(&descriptor).unwrap();
        assert!(executor.set_agent(false).is_err());
        std::fs::remove_dir_all(configfs.join("hardened")).unwrap();
        assert!(executor.ensure_hid_gadget().unwrap());
        assert_eq!(absolute_mouse_report_id(&configfs), Some(1));
    }

    #[cfg(feature = "gadget-media-profile")]
    #[test]
    fn a_gadget_built_from_scratch_follows_the_agents_switch() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join("configfs")).unwrap();
        let config = gadget_config_for_test(directory.path(), "BLK-agent-cold");
        let configfs = config.configfs_root.clone();
        let executor = GadgetMediaExecutor::new(config);
        // The HID bind before the owner starts, with the switch on.
        executor.want_agent(true);
        assert!(executor.ensure_hid_gadget().unwrap());
        assert_eq!(absolute_mouse_report_id(&configfs), Some(1));

        // Without a gadget, switching builds one.
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join("configfs")).unwrap();
        let config = gadget_config_for_test(directory.path(), "BLK-agent-none");
        let configfs = config.configfs_root.clone();
        let executor = GadgetMediaExecutor::new(config);
        assert!(executor.set_agent(true).unwrap());
        assert_eq!(absolute_mouse_report_id(&configfs), Some(1));

        // Without configfs, the switch cannot be applied.
        let directory = tempfile::tempdir().unwrap();
        let executor =
            GadgetMediaExecutor::new(gadget_config_for_test(directory.path(), "BLK-agent-off"));
        assert!(matches!(
            executor.set_agent(true),
            Err(MediaTransitionError::Backend)
        ));
    }

    #[cfg(feature = "gadget-media-profile")]
    #[test]
    fn startup_bind_is_a_noop_without_a_configfs_root() {
        let directory = tempfile::tempdir().unwrap();
        let config = gadget_config_for_test(directory.path(), "BLK-ensure-absent");
        let executor = GadgetMediaExecutor::new(config);
        assert!(!executor.ensure_hid_gadget().unwrap());
    }

    #[cfg(feature = "gadget-media-profile")]
    #[tokio::test]
    async fn gadget_executor_selects_attaches_and_detaches() {
        let directory = tempfile::tempdir().unwrap();
        let image_dir = directory.path().join("images");
        std::fs::create_dir_all(&image_dir).unwrap();
        std::fs::write(image_dir.join("kubuntu.img"), b"image").unwrap();
        let catalog_path = image_dir.join("catalog.json");
        std::fs::write(
            &catalog_path,
            format!(
                r#"{{"schemaVersion":1,"entries":[{{"imageId":7,"imageName":"kubuntu.img","size":1,"sha256":"{}","cdromAllowed":true,"removableAllowed":true,"writableAllowed":false}}]}}"#,
                "a".repeat(64)
            ),
        )
        .unwrap();
        let catalog = MediaCatalog::open(&catalog_path).unwrap();

        let configfs = directory.path().join("configfs");
        std::fs::create_dir_all(&configfs).unwrap();
        let executor = GadgetMediaExecutor::new(GadgetMediaConfig {
            configfs_root: configfs.clone(),
            udc: "musb-hdrc.2.auto".into(),
            image_directory: image_dir,
            strings: GadgetStrings {
                manufacturer: "Hardened BLi-KVM".into(),
                product: "BLi-KVM Composite Device".into(),
                serial: "BLK-test".into(),
                configuration: "BLi-KVM configuration".into(),
            },
            id_vendor: 0x1d6b,
            id_product: 0x0104,
            bcd_usb: 0x0200,
            bcd_device: 0x0100,
        });

        assert_eq!(
            executor.observe().unwrap(),
            MediaObservedState {
                enabled: true,
                online: true,
                busy: false,
                connected: false,
            }
        );
        assert!(
            executor
                .execute(
                    &VirtualMediaCommand::SetConnected { connected: true },
                    &catalog
                )
                .is_err()
        );
        assert!(
            executor
                .execute(
                    &VirtualMediaCommand::Select {
                        image_id: 7,
                        mode: VirtualMediaControlMode::RemovableDisk,
                        writable: false,
                    },
                    &catalog
                )
                .is_ok()
        );
        assert!(
            executor
                .execute(
                    &VirtualMediaCommand::SetConnected { connected: true },
                    &catalog
                )
                .is_ok()
        );
        let state = executor.observe().unwrap();
        assert!(state.online);
        assert!(state.connected);
        assert!(
            executor
                .execute(
                    &VirtualMediaCommand::SetConnected { connected: false },
                    &catalog
                )
                .is_ok()
        );
        assert!(!executor.observe().unwrap().connected);
        assert!(
            executor
                .execute(
                    &VirtualMediaCommand::Select {
                        image_id: 99,
                        mode: VirtualMediaControlMode::RemovableDisk,
                        writable: false,
                    },
                    &catalog
                )
                .is_err()
        );
    }

    #[cfg(feature = "gadget-media-profile")]
    #[tokio::test]
    async fn a_catalog_entry_without_its_file_fails_closed() {
        let directory = tempfile::tempdir().unwrap();
        let image_dir = directory.path().join("images");
        std::fs::create_dir_all(&image_dir).unwrap();
        let image = image_dir.join("kubuntu.img");
        std::fs::write(&image, b"image").unwrap();
        let catalog_path = image_dir.join("catalog.json");
        std::fs::write(
            &catalog_path,
            format!(
                r#"{{"schemaVersion":1,"entries":[{{"imageId":7,"imageName":"kubuntu.img","size":1,"sha256":"{}","cdromAllowed":true,"removableAllowed":true,"writableAllowed":false}}]}}"#,
                "a".repeat(64)
            ),
        )
        .unwrap();
        let catalog = MediaCatalog::open(&catalog_path).unwrap();
        // The image disappears after the catalog was reviewed.
        std::fs::remove_file(&image).unwrap();

        let configfs = directory.path().join("configfs");
        std::fs::create_dir_all(&configfs).unwrap();
        let executor = GadgetMediaExecutor::new(GadgetMediaConfig {
            configfs_root: configfs.clone(),
            udc: "musb-hdrc.2.auto".into(),
            image_directory: image_dir,
            strings: GadgetStrings {
                manufacturer: "Hardened BLi-KVM".into(),
                product: "BLi-KVM Composite Device".into(),
                serial: "BLK-test".into(),
                configuration: "BLi-KVM configuration".into(),
            },
            id_vendor: 0x1d6b,
            id_product: 0x0104,
            bcd_usb: 0x0200,
            bcd_device: 0x0100,
        });

        // Selection is catalog-only; the missing file must fail the attach and
        // must not leave a half-created gadget behind.
        assert!(
            executor
                .execute(
                    &VirtualMediaCommand::Select {
                        image_id: 7,
                        mode: VirtualMediaControlMode::RemovableDisk,
                        writable: false,
                    },
                    &catalog
                )
                .is_ok()
        );
        assert_eq!(
            executor.execute(
                &VirtualMediaCommand::SetConnected { connected: true },
                &catalog
            ),
            Err(MediaBackendFailure::ImageMissing)
        );
        assert!(!executor.observe().unwrap().connected);
        assert!(!configfs.join("hardened").exists());

        // Back, but shorter than a CD-ROM needs: the reason is named too.
        std::fs::write(&image, b"image").unwrap();
        assert!(
            executor
                .execute(
                    &VirtualMediaCommand::Select {
                        image_id: 7,
                        mode: VirtualMediaControlMode::CdRom,
                        writable: false,
                    },
                    &catalog
                )
                .is_ok()
        );
        assert_eq!(
            executor.execute(
                &VirtualMediaCommand::SetConnected { connected: true },
                &catalog
            ),
            Err(MediaBackendFailure::ImageTooSmall)
        );
        assert!(!configfs.join("hardened").exists());
    }

    #[cfg(feature = "gadget-media-profile")]
    #[tokio::test]
    async fn gadget_executor_mounts_and_reconnects() {
        let directory = tempfile::tempdir().unwrap();
        let image_dir = directory.path().join("images");
        std::fs::create_dir_all(&image_dir).unwrap();
        // Large enough for a CD-ROM as well.
        std::fs::write(
            image_dir.join("kubuntu.img"),
            vec![0_u8; bli_kvm_usb_gadget::MIN_CDROM_IMAGE_BYTES as usize],
        )
        .unwrap();
        let catalog_path = image_dir.join("catalog.json");
        std::fs::write(
            &catalog_path,
            format!(
                r#"{{"schemaVersion":1,"entries":[{{"imageId":7,"imageName":"kubuntu.img","size":1,"sha256":"{}","cdromAllowed":true,"removableAllowed":true,"writableAllowed":false}}]}}"#,
                "a".repeat(64)
            ),
        )
        .unwrap();
        let catalog = MediaCatalog::open(&catalog_path).unwrap();
        let configfs = directory.path().join("configfs");
        std::fs::create_dir_all(&configfs).unwrap();
        let executor = GadgetMediaExecutor::new(GadgetMediaConfig {
            configfs_root: configfs.clone(),
            udc: "musb-hdrc.2.auto".into(),
            image_directory: image_dir,
            strings: GadgetStrings {
                manufacturer: "Hardened BLi-KVM".into(),
                product: "BLi-KVM Composite Device".into(),
                serial: "BLK-mount-test".into(),
                configuration: "BLi-KVM configuration".into(),
            },
            id_vendor: 0x1d6b,
            id_product: 0x0104,
            bcd_usb: 0x0200,
            bcd_device: 0x0100,
        });

        assert!(
            executor
                .execute(
                    &VirtualMediaCommand::Mount {
                        image_id: 7,
                        mode: VirtualMediaControlMode::RemovableDisk,
                        writable: false,
                    },
                    &catalog
                )
                .is_ok()
        );
        let state = executor.state().unwrap();
        assert_eq!(state.file.as_deref(), Some("kubuntu.img"));
        assert!(state.connected);
        assert!(!state.cdrom);
        assert!(!state.writable);

        assert!(
            executor
                .execute(&VirtualMediaCommand::Reconnect, &catalog)
                .is_ok()
        );
        let after = executor.state().unwrap();
        assert!(after.connected);
        assert_eq!(after.file.as_deref(), Some("kubuntu.img"));

        assert!(
            executor
                .execute(
                    &VirtualMediaCommand::Mount {
                        image_id: 7,
                        mode: VirtualMediaControlMode::CdRom,
                        writable: false,
                    },
                    &catalog
                )
                .is_ok()
        );
        let cdrom = executor.state().unwrap();
        assert!(cdrom.cdrom);
        assert!(cdrom.connected);

        // Eject, then mount a different mode: the executor must drop and
        // rebuild the mass-storage function instead of patching a busy LUN.
        assert!(
            executor
                .execute(
                    &VirtualMediaCommand::SetConnected { connected: false },
                    &catalog
                )
                .is_ok()
        );
        assert_eq!(executor.state().unwrap().file, None);
        assert!(
            executor
                .execute(
                    &VirtualMediaCommand::Mount {
                        image_id: 7,
                        mode: VirtualMediaControlMode::RemovableDisk,
                        writable: false,
                    },
                    &catalog
                )
                .is_ok()
        );
        let after = executor.state().unwrap();
        assert!(after.connected);
        assert!(!after.cdrom);
        assert_eq!(after.file.as_deref(), Some("kubuntu.img"));

        // Reconnect keeps the selection and re-enumerates.
        assert!(
            executor
                .execute(&VirtualMediaCommand::Reconnect, &catalog)
                .is_ok()
        );
        let reconnected = executor.state().unwrap();
        assert!(reconnected.connected);
        assert_eq!(reconnected.file.as_deref(), Some("kubuntu.img"));
    }

    #[cfg(feature = "gadget-media-profile")]
    #[tokio::test]
    async fn gadget_executor_creates_a_composite_device_with_pinned_hid() {
        let directory = tempfile::tempdir().unwrap();
        let image_dir = directory.path().join("images");
        std::fs::create_dir_all(&image_dir).unwrap();
        std::fs::write(image_dir.join("kubuntu.img"), b"image").unwrap();
        let catalog_path = image_dir.join("catalog.json");
        std::fs::write(
            &catalog_path,
            format!(
                r#"{{"schemaVersion":1,"entries":[{{"imageId":7,"imageName":"kubuntu.img","size":1,"sha256":"{}","cdromAllowed":true,"removableAllowed":true,"writableAllowed":false}}]}}"#,
                "a".repeat(64)
            ),
        )
        .unwrap();
        let catalog = MediaCatalog::open(&catalog_path).unwrap();
        let configfs = directory.path().join("configfs");
        std::fs::create_dir_all(&configfs).unwrap();
        let executor = GadgetMediaExecutor::new(GadgetMediaConfig {
            configfs_root: configfs.clone(),
            udc: "musb-hdrc.2.auto".into(),
            image_directory: image_dir,
            strings: GadgetStrings {
                manufacturer: "Hardened BLi-KVM".into(),
                product: "BLi-KVM Composite Device".into(),
                serial: "BLK-test".into(),
                configuration: "BLi-KVM configuration".into(),
            },
            id_vendor: 0x1d6b,
            id_product: 0x0104,
            bcd_usb: 0x0200,
            bcd_device: 0x0100,
        });

        assert!(
            executor
                .execute(
                    &VirtualMediaCommand::Select {
                        image_id: 7,
                        mode: VirtualMediaControlMode::RemovableDisk,
                        writable: false,
                    },
                    &catalog
                )
                .is_ok()
        );
        assert!(
            executor
                .execute(
                    &VirtualMediaCommand::SetConnected { connected: true },
                    &catalog
                )
                .is_ok()
        );
        let state = bli_kvm_usb_gadget::observe(&configfs).unwrap();
        assert!(state.present);
        assert_eq!(
            state.functions,
            vec![
                "hid.usb0".to_string(),
                "hid.usb1".to_string(),
                "hid.usb2".to_string(),
                "mass_storage.usb0".to_string(),
            ]
        );
        let storage = state.mass_storage.expect("mass storage is present");
        assert!(storage.file.ends_with("kubuntu.img"));
    }

    #[cfg(feature = "gadget-media-profile")]
    #[tokio::test]
    async fn virtual_media_and_the_agents_collection_keep_each_other() {
        let directory = tempfile::tempdir().unwrap();
        let config = gadget_config_for_test(directory.path(), "BLK-agent-media");
        let image_dir = config.image_directory.clone();
        let configfs = config.configfs_root.clone();
        std::fs::create_dir_all(&configfs).unwrap();
        std::fs::write(image_dir.join("kubuntu.img"), b"image").unwrap();
        let catalog_path = image_dir.join("catalog.json");
        std::fs::write(
            &catalog_path,
            format!(
                r#"{{"schemaVersion":1,"entries":[{{"imageId":7,"imageName":"kubuntu.img","size":1,"sha256":"{}","cdromAllowed":true,"removableAllowed":true,"writableAllowed":false}}]}}"#,
                "a".repeat(64)
            ),
        )
        .unwrap();
        let catalog = MediaCatalog::open(&catalog_path).unwrap();
        let executor = GadgetMediaExecutor::new(config);
        assert!(executor.ensure_hid_gadget().unwrap());
        let mount = VirtualMediaCommand::Mount {
            image_id: 7,
            mode: VirtualMediaControlMode::RemovableDisk,
            writable: false,
        };
        assert!(executor.execute(&mount, &catalog).is_ok());

        // The switch keeps the attached image, and the interfaces' order.
        assert!(executor.set_agent(true).unwrap());
        let state = bli_kvm_usb_gadget::observe(&configfs).unwrap();
        assert!(state.udc.is_some());
        assert!(state.mass_storage.unwrap().file.ends_with("kubuntu.img"));
        assert_eq!(absolute_mouse_report_id(&configfs), Some(1));

        // A swap and an eject keep the agent's collection.
        assert!(executor.execute(&mount, &catalog).is_ok());
        assert_eq!(absolute_mouse_report_id(&configfs), Some(1));
        assert!(
            executor
                .execute(
                    &VirtualMediaCommand::SetConnected { connected: false },
                    &catalog
                )
                .is_ok()
        );
        assert_eq!(absolute_mouse_report_id(&configfs), Some(1));
        assert!(
            bli_kvm_usb_gadget::observe(&configfs)
                .unwrap()
                .mass_storage
                .is_none()
        );
        assert!(executor.set_agent(false).unwrap());
        assert_eq!(absolute_mouse_report_id(&configfs), None);
    }

    #[cfg(feature = "gadget-media-profile")]
    #[tokio::test]
    async fn the_agents_drive_stays_beside_the_media_and_is_no_image() {
        let directory = tempfile::tempdir().unwrap();
        let config = gadget_config_for_test(directory.path(), "BLK-agent-drive");
        let image_dir = config.image_directory.clone();
        let configfs = config.configfs_root.clone();
        std::fs::create_dir_all(&configfs).unwrap();
        std::fs::write(image_dir.join("kubuntu.img"), b"image").unwrap();
        let drive = directory.path().join("agent-drive.img");
        std::fs::write(&drive, b"drive").unwrap();
        let catalog_path = image_dir.join("catalog.json");
        std::fs::write(
            &catalog_path,
            format!(
                r#"{{"schemaVersion":1,"entries":[{{"imageId":7,"imageName":"kubuntu.img","size":1,"sha256":"{}","cdromAllowed":true,"removableAllowed":true,"writableAllowed":false}}]}}"#,
                "a".repeat(64)
            ),
        )
        .unwrap();
        let catalog = MediaCatalog::open(&catalog_path).unwrap();
        let executor = GadgetMediaExecutor::new(config);
        let storage = || bli_kvm_usb_gadget::observe(&configfs).unwrap().mass_storage;

        // A boot with the drive wanted: the drive alone, not removable, and
        // no image as far as the media are concerned.
        executor.want_agent_drive(Some(drive.clone()));
        assert!(executor.ensure_hid_gadget().unwrap());
        let alone = storage().unwrap();
        assert_eq!(
            (alone.file.clone(), alone.removable),
            (drive.clone(), false)
        );
        assert!(!executor.observe().unwrap().connected);
        assert!(!executor.state().unwrap().connected);

        // An image goes to LUN 0, the drive to LUN 1.
        let mount = VirtualMediaCommand::Mount {
            image_id: 7,
            mode: VirtualMediaControlMode::RemovableDisk,
            writable: false,
        };
        assert!(executor.execute(&mount, &catalog).is_ok());
        let both = storage().unwrap();
        assert!(both.file.ends_with("kubuntu.img") && both.removable);
        assert_eq!(both.second_lun, Some(drive.clone()));
        let state = executor.state().unwrap();
        assert!(state.connected);
        assert_eq!(state.file.as_deref(), Some("kubuntu.img"));

        // An eject leaves the drive alone.
        assert!(
            executor
                .execute(
                    &VirtualMediaCommand::SetConnected { connected: false },
                    &catalog
                )
                .is_ok()
        );
        assert_eq!(storage().unwrap().file, drive);
        assert!(!executor.state().unwrap().connected);

        // No drive wanted: the function goes; wanted again beside an image:
        // the image stays.
        assert!(executor.set_agent_drive(None).unwrap());
        assert!(storage().is_none());
        assert!(!executor.set_agent_drive(None).unwrap());
        assert!(executor.execute(&mount, &catalog).is_ok());
        assert!(executor.set_agent_drive(Some(drive.clone())).unwrap());
        let both = storage().unwrap();
        assert!(both.file.ends_with("kubuntu.img"));
        assert_eq!(both.second_lun, Some(drive));
        assert!(
            bli_kvm_usb_gadget::observe(&configfs)
                .unwrap()
                .udc
                .is_some()
        );
    }

    #[cfg(feature = "gadget-media-profile")]
    #[tokio::test]
    async fn an_owner_started_again_finds_the_attached_image_in_the_store() {
        let directory = tempfile::tempdir().unwrap();
        let config = gadget_config_for_test(directory.path(), "BLK-agent-restart");
        let image_dir = config.image_directory.clone();
        let configfs = config.configfs_root.clone();
        std::fs::create_dir_all(&configfs).unwrap();
        std::fs::write(image_dir.join("kubuntu.img"), b"image").unwrap();
        let drive = directory.path().join("agent-drive.img");
        std::fs::write(&drive, b"drive").unwrap();
        let catalog_path = image_dir.join("catalog.json");
        std::fs::write(
            &catalog_path,
            format!(
                r#"{{"schemaVersion":1,"entries":[{{"imageId":7,"imageName":"kubuntu.img","size":1,"sha256":"{}","cdromAllowed":true,"removableAllowed":true,"writableAllowed":false}}]}}"#,
                "a".repeat(64)
            ),
        )
        .unwrap();
        let catalog = MediaCatalog::open(&catalog_path).unwrap();
        let storage = || bli_kvm_usb_gadget::observe(&configfs).unwrap().mass_storage;
        let mount = VirtualMediaCommand::Mount {
            image_id: 7,
            mode: VirtualMediaControlMode::RemovableDisk,
            writable: false,
        };
        assert!(
            GadgetMediaExecutor::new(config.clone())
                .execute(&mount, &catalog)
                .is_ok()
        );
        // The image as the kernel names it to the next owner: from the root
        // of the store the former owner's namespace mounted.
        let lun = configfs.join("hardened/functions/mass_storage.usb0/lun.0/file");
        std::fs::write(&lun, "/kubuntu.img\n").unwrap();

        // The next owner, with the agent's drive wanted: the drive goes
        // beside the image, which stays the store's.
        let executor = GadgetMediaExecutor::new(config.clone());
        executor.want_agent_drive(Some(drive.clone()));
        assert!(executor.set_agent_drive(Some(drive.clone())).unwrap());
        let both = storage().unwrap();
        assert_eq!(both.file, image_dir.join("kubuntu.img"));
        assert!(both.removable);
        assert_eq!(both.second_lun, Some(drive.clone()));

        // And an image reconnected by an owner started again is the store's.
        std::fs::write(&lun, "/kubuntu.img\n").unwrap();
        let executor = GadgetMediaExecutor::new(config);
        assert!(
            executor
                .execute(
                    &VirtualMediaCommand::SetConnected { connected: true },
                    &catalog
                )
                .is_ok()
        );
        assert_eq!(storage().unwrap().file, image_dir.join("kubuntu.img"));
    }

    #[tokio::test]
    async fn engine_executes_through_any_reviewed_backend() {
        let directory = tempfile::tempdir().unwrap();
        let mut engine = MediaTransitionEngine::open_with_executor(
            directory.path().join("journal.json"),
            catalog(directory.path()),
            MediaExecutor::Test {
                observed: observed(false, false),
                applied: true,
            },
        )
        .unwrap();
        let eligibility = MediaEligibility::verified_for_test_profile();
        assert_eq!(engine.observe().await.unwrap(), observed(false, false));
        assert_eq!(
            engine
                .execute(
                    operation(11, VirtualMediaCommand::SetConnected { connected: true }),
                    eligibility,
                    observed(false, false),
                    1_500
                )
                .await
                .unwrap(),
            MediaTransitionOutcome::Applied
        );

        let mut failing = MediaTransitionEngine::open_with_executor(
            directory.path().join("failing.json"),
            catalog(directory.path()),
            MediaExecutor::Test {
                observed: observed(false, false),
                applied: false,
            },
        )
        .unwrap();
        assert_eq!(
            failing
                .execute(
                    operation(12, VirtualMediaCommand::SetConnected { connected: true }),
                    eligibility,
                    observed(false, false),
                    1_500
                )
                .await
                .unwrap(),
            MediaTransitionOutcome::BackendFailed(MediaBackendFailure::Other)
        );
    }

    #[tokio::test]
    async fn set_connected_is_applied_once_and_replays() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("media.sock");
        let server = serve(&socket, &["200 OK"]).await;
        let mut engine = engine(directory.path(), &socket);
        let request = operation(1, VirtualMediaCommand::SetConnected { connected: true });
        let eligibility = MediaEligibility::verified_for_test_profile();

        assert_eq!(
            engine
                .execute(request, eligibility, observed(false, false), 1_500)
                .await
                .unwrap(),
            MediaTransitionOutcome::Applied
        );
        assert_eq!(
            engine
                .execute(request, eligibility, observed(false, true), 1_600)
                .await
                .unwrap(),
            MediaTransitionOutcome::ReplayedApplied
        );
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with("POST /msd/set_connected?connected=1 HTTP/1.1\r\n"));
    }

    #[tokio::test]
    async fn select_uses_only_the_catalog_route() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("media.sock");
        let server = serve(&socket, &["200 OK"]).await;
        let mut engine = engine(directory.path(), &socket);
        let request = operation(
            2,
            VirtualMediaCommand::Select {
                image_id: 7,
                mode: VirtualMediaControlMode::CdRom,
                writable: false,
            },
        );

        assert_eq!(
            engine
                .execute(
                    request,
                    MediaEligibility::verified_for_test_profile(),
                    observed(false, false),
                    1_500
                )
                .await
                .unwrap(),
            MediaTransitionOutcome::Applied
        );
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0]
                .starts_with("POST /msd/set_params?image=rescue.iso&cdrom=1&rw=0 HTTP/1.1\r\n")
        );
        assert!(!requests[0].contains("/dev/"));
    }

    #[tokio::test]
    async fn busy_and_must_disconnect_fail_without_backend_io() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("absent.sock");
        let mut engine = engine(directory.path(), &socket);
        let eligibility = MediaEligibility::verified_for_test_profile();

        assert_eq!(
            engine
                .execute(
                    operation(3, VirtualMediaCommand::SetConnected { connected: true }),
                    eligibility,
                    observed(true, false),
                    1_500
                )
                .await
                .unwrap(),
            MediaTransitionOutcome::Rejected(MediaRejection::Busy)
        );
        assert_eq!(
            engine
                .execute(
                    operation(
                        4,
                        VirtualMediaCommand::Select {
                            image_id: 7,
                            mode: VirtualMediaControlMode::CdRom,
                            writable: false
                        }
                    ),
                    eligibility,
                    observed(false, true),
                    1_500
                )
                .await
                .unwrap(),
            MediaTransitionOutcome::Rejected(MediaRejection::MustDisconnect)
        );
        assert_eq!(
            engine
                .execute(
                    operation(5, VirtualMediaCommand::SetConnected { connected: true }),
                    eligibility,
                    observed(false, true),
                    1_500
                )
                .await
                .unwrap(),
            MediaTransitionOutcome::Noop
        );
        assert_eq!(engine.pending_count(), 0);
        assert!(!socket.exists());
    }

    #[tokio::test]
    async fn backend_failure_is_persisted_without_retry() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("media.sock");
        let server = serve(&socket, &["500 Error"]).await;
        let mut engine = engine(directory.path(), &socket);
        let request = operation(6, VirtualMediaCommand::SetConnected { connected: true });

        assert_eq!(
            engine
                .execute(
                    request,
                    MediaEligibility::verified_for_test_profile(),
                    observed(false, false),
                    1_500
                )
                .await
                .unwrap(),
            MediaTransitionOutcome::BackendFailed(MediaBackendFailure::Other)
        );
        assert_eq!(
            engine
                .execute(
                    request,
                    MediaEligibility::verified_for_test_profile(),
                    observed(false, false),
                    1_600
                )
                .await
                .unwrap(),
            MediaTransitionOutcome::ReplayedRejected
        );
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 1);
    }

    #[tokio::test]
    async fn pending_survives_restart_and_requires_reconciliation() {
        use bli_kvm_media_policy::{MediaDecision, MediaPolicyEngine};

        let directory = tempfile::tempdir().unwrap();
        let journal = directory.path().join("journal.json");
        {
            let mut policy = MediaPolicyEngine::open(&journal).unwrap();
            assert_eq!(
                policy
                    .prepare(
                        operation(7, VirtualMediaCommand::SetConnected { connected: true }),
                        MediaEligibility::verified_for_test_profile(),
                        observed(false, false),
                        1_500
                    )
                    .unwrap(),
                MediaDecision::Permit
            );
        }
        let socket = directory.path().join("absent.sock");
        let mut engine = MediaTransitionEngine::open(
            &journal,
            KvmdMediaClient::new(&socket).unwrap(),
            catalog(directory.path()),
            credentials(),
        )
        .unwrap();
        assert_eq!(engine.pending_count(), 1);
        assert_eq!(
            engine
                .execute(
                    operation(8, VirtualMediaCommand::SetConnected { connected: false }),
                    MediaEligibility::verified_for_test_profile(),
                    observed(false, true),
                    1_600
                )
                .await
                .unwrap(),
            MediaTransitionOutcome::Rejected(MediaRejection::IndeterminatePending)
        );
        engine
            .resolve_pending([7; 16], MediaCompletion::Applied, 1_700)
            .unwrap();
        assert_eq!(engine.pending_count(), 0);
        assert_eq!(
            engine
                .execute(
                    operation(7, VirtualMediaCommand::SetConnected { connected: true }),
                    MediaEligibility::verified_for_test_profile(),
                    observed(false, true),
                    1_800
                )
                .await
                .unwrap(),
            MediaTransitionOutcome::ReplayedApplied
        );
        assert!(!socket.exists());
    }

    #[tokio::test]
    async fn conflict_and_cooldown_fail_closed() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("media.sock");
        let server = serve(&socket, &["200 OK"]).await;
        let mut engine = engine(directory.path(), &socket);
        let eligibility = MediaEligibility::verified_for_test_profile();

        assert_eq!(
            engine
                .execute(
                    operation(9, VirtualMediaCommand::SetConnected { connected: true }),
                    eligibility,
                    observed(false, false),
                    1_500
                )
                .await
                .unwrap(),
            MediaTransitionOutcome::Applied
        );
        assert_eq!(
            engine
                .execute(
                    operation(9, VirtualMediaCommand::SetConnected { connected: false }),
                    eligibility,
                    observed(false, true),
                    1_600
                )
                .await
                .unwrap(),
            MediaTransitionOutcome::Rejected(MediaRejection::OperationIdConflict)
        );
        assert_eq!(
            engine
                .execute(
                    operation(10, VirtualMediaCommand::SetConnected { connected: false }),
                    eligibility,
                    observed(false, true),
                    1_600
                )
                .await
                .unwrap(),
            MediaTransitionOutcome::Rejected(MediaRejection::CooldownActive)
        );
        assert_eq!(server.await.unwrap().len(), 1);
    }
}
