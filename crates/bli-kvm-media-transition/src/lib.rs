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

    async fn execute(&self, command: &VirtualMediaCommand, catalog: &MediaCatalog) -> bool {
        self.client
            .execute(command, catalog, &self.credentials)
            .await
            .is_ok()
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
}

#[cfg(feature = "gadget-media-profile")]
impl GadgetMediaExecutor {
    pub fn new(config: GadgetMediaConfig) -> Self {
        Self {
            config,
            selected: std::sync::Mutex::new(None),
        }
    }

    fn spec(&self, selected: &SelectedImage) -> GadgetSpec {
        let mut functions = vec![Function::MassStorage(MassStorageConfig {
            file: selected.path.clone(),
            read_only: selected.read_only,
            cdrom: selected.cdrom,
            removable: true,
        })];
        // The composite device also exposes the pinned HID functions captured
        // read-only from the board.
        functions.extend(bli_kvm_usb_gadget::pinned_hid_functions());
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

    fn observe(&self) -> Result<MediaObservedState, MediaTransitionError> {
        let state = bli_kvm_usb_gadget::observe(&self.config.configfs_root)
            .map_err(|_| MediaTransitionError::Backend)?;
        Ok(MediaObservedState {
            enabled: true,
            online: self.config.configfs_root.is_dir(),
            busy: false,
            connected: state.udc.is_some(),
        })
    }

    fn select(
        &self,
        catalog: &MediaCatalog,
        image_id: u64,
        mode: bli_kvm_core::VirtualMediaControlMode,
        writable: bool,
    ) -> bool {
        let Ok(selection) = catalog.resolve(image_id, mode, writable) else {
            return false;
        };
        let selected = SelectedImage {
            path: self.config.image_directory.join(selection.image_name()),
            read_only: !selection.writable(),
            cdrom: selection.mode() == bli_kvm_core::VirtualMediaControlMode::CdRom,
        };
        if self.state_present()
            && bli_kvm_usb_gadget::set_lun_file(&self.config.configfs_root, &selected.path).is_err()
        {
            return false;
        }
        match self.selected.lock() {
            Ok(mut guard) => *guard = Some(selected),
            Err(_) => return false,
        }
        true
    }

    fn set_connected(&self, connected: bool) -> bool {
        if !connected {
            if !self.state_present() {
                return true;
            }
            return bli_kvm_usb_gadget::unbind(&self.config.configfs_root).is_ok();
        }
        let selected = match self.selected.lock() {
            Ok(guard) => guard,
            Err(_) => return false,
        };
        let Some(selected) = selected.as_ref() else {
            return false;
        };
        let spec = self.spec(selected);
        match bli_kvm_usb_gadget::apply(&self.config.configfs_root, &spec) {
            Ok(()) => true,
            Err(error) => {
                // The board owner has no other channel to explain a failed
                // attach; the operator log is the diagnostic surface.
                eprintln!("media: gadget apply failed: {error:?}");
                false
            }
        }
    }

    fn state(&self) -> Result<MediaState, MediaTransitionError> {
        let state = bli_kvm_usb_gadget::observe(&self.config.configfs_root)
            .map_err(|_| MediaTransitionError::Backend)?;
        let storage = state.mass_storage.as_ref();
        Ok(MediaState {
            file: storage
                .and_then(|storage| storage.file.file_name())
                .map(|name| name.to_string_lossy().into_owned()),
            connected: state.udc.is_some(),
            cdrom: storage.is_some_and(|storage| storage.cdrom),
            writable: storage.is_some_and(|storage| !storage.read_only),
        })
    }

    fn reconnect(&self) -> bool {
        bli_kvm_usb_gadget::reconnect(&self.config.configfs_root, &self.config.udc).is_ok()
    }

    fn mount(
        &self,
        catalog: &MediaCatalog,
        image_id: u64,
        mode: bli_kvm_core::VirtualMediaControlMode,
        writable: bool,
    ) -> bool {
        if self.state_present() && bli_kvm_usb_gadget::unbind(&self.config.configfs_root).is_err() {
            return false;
        }
        if !self.select(catalog, image_id, mode, writable) {
            return false;
        }
        self.set_connected(true)
    }

    fn execute(&self, command: &VirtualMediaCommand, catalog: &MediaCatalog) -> bool {
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

    async fn execute(&self, command: &VirtualMediaCommand, catalog: &MediaCatalog) -> bool {
        match self {
            #[cfg(feature = "kvmd-media-profile")]
            MediaExecutor::Kvmd(executor) => executor.execute(command, catalog).await,
            #[cfg(feature = "gadget-media-profile")]
            MediaExecutor::Gadget(executor) => executor.execute(command, catalog),
            #[cfg(test)]
            MediaExecutor::Test { applied, .. } => *applied,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaTransitionOutcome {
    Applied,
    Noop,
    ReplayedApplied,
    ReplayedRejected,
    BackendFailed,
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
                let applied = self.executor.execute(&command, &self.catalog).await;
                let completion = if applied {
                    MediaCompletion::Applied
                } else {
                    MediaCompletion::Rejected
                };
                self.policy.complete(operation_id, completion, now_ms)?;
                if applied {
                    Ok(MediaTransitionOutcome::Applied)
                } else {
                    Ok(MediaTransitionOutcome::BackendFailed)
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
        assert!(!executor.execute(
            &VirtualMediaCommand::SetConnected { connected: true },
            &catalog
        ));
        assert!(executor.execute(
            &VirtualMediaCommand::Select {
                image_id: 7,
                mode: VirtualMediaControlMode::RemovableDisk,
                writable: false,
            },
            &catalog
        ));
        assert!(executor.execute(
            &VirtualMediaCommand::SetConnected { connected: true },
            &catalog
        ));
        let state = executor.observe().unwrap();
        assert!(state.online);
        assert!(state.connected);
        assert!(executor.execute(
            &VirtualMediaCommand::SetConnected { connected: false },
            &catalog
        ));
        assert!(!executor.observe().unwrap().connected);
        assert!(!executor.execute(
            &VirtualMediaCommand::Select {
                image_id: 99,
                mode: VirtualMediaControlMode::RemovableDisk,
                writable: false,
            },
            &catalog
        ));
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
        assert!(executor.execute(
            &VirtualMediaCommand::Select {
                image_id: 7,
                mode: VirtualMediaControlMode::RemovableDisk,
                writable: false,
            },
            &catalog
        ));
        assert!(!executor.execute(
            &VirtualMediaCommand::SetConnected { connected: true },
            &catalog
        ));
        assert!(!executor.observe().unwrap().connected);
        assert!(!configfs.join("hardened").exists());
    }

    #[cfg(feature = "gadget-media-profile")]
    #[tokio::test]
    async fn gadget_executor_mounts_and_reconnects() {
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
                serial: "BLK-mount-test".into(),
                configuration: "BLi-KVM configuration".into(),
            },
            id_vendor: 0x1d6b,
            id_product: 0x0104,
            bcd_usb: 0x0200,
            bcd_device: 0x0100,
        });

        assert!(executor.execute(
            &VirtualMediaCommand::Mount {
                image_id: 7,
                mode: VirtualMediaControlMode::RemovableDisk,
                writable: false,
            },
            &catalog
        ));
        let state = executor.state().unwrap();
        assert_eq!(state.file.as_deref(), Some("kubuntu.img"));
        assert!(state.connected);
        assert!(!state.cdrom);
        assert!(!state.writable);

        assert!(executor.execute(&VirtualMediaCommand::Reconnect, &catalog));
        let after = executor.state().unwrap();
        assert!(after.connected);
        assert_eq!(after.file.as_deref(), Some("kubuntu.img"));

        assert!(executor.execute(
            &VirtualMediaCommand::Mount {
                image_id: 7,
                mode: VirtualMediaControlMode::CdRom,
                writable: false,
            },
            &catalog
        ));
        let cdrom = executor.state().unwrap();
        assert!(cdrom.cdrom);
        assert!(cdrom.connected);
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

        assert!(executor.execute(
            &VirtualMediaCommand::Select {
                image_id: 7,
                mode: VirtualMediaControlMode::RemovableDisk,
                writable: false,
            },
            &catalog
        ));
        assert!(executor.execute(
            &VirtualMediaCommand::SetConnected { connected: true },
            &catalog
        ));
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
            MediaTransitionOutcome::BackendFailed
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
            MediaTransitionOutcome::BackendFailed
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
