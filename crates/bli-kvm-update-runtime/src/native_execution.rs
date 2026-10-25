//! Protected artifact preparation for the future local privileged coordinator.
//! This layer never opens manager state or authorizes service startup.

use std::{
    collections::BTreeSet,
    io::Read,
    path::{Path, PathBuf},
};

use bli_kvm_update_manager::PointerStore;
use bli_kvm_update_stager::{
    ApplicationSlotStager, ApplicationStageError, StageQuota, VerifiedSlotFiles,
};
use bli_kvm_update_verifier::{ApplicationManifest, ApplicationTrustRoot};

use crate::{
    composition::validate_native_payload, native_identity::FILES,
    native_selection::SelectionLedgerStore, slot_archive::open_slot_archive,
};

/// Values must be loaded from privileged local policy, never client IPC fields.
/// This is a policy snapshot, not a journal or automatic durable floor update.
#[derive(Debug, Clone, Copy)]
pub struct ExecutionPolicy {
    pub system_sequence: u64,
    pub minimum_application_sequence: u64,
    pub quota: StageQuota,
}

impl ExecutionPolicy {
    fn valid(self) -> bool {
        self.system_sequence != 0
            && self.minimum_application_sequence != 0
            && (1..=32).contains(&self.quota.max_slots)
            && self.quota.max_payload_bytes != 0
    }

    fn accepts(self, manifest: &ApplicationManifest) -> bool {
        self.valid()
            && manifest.sequence >= self.minimum_application_sequence
            && manifest.minimum_system_sequence <= self.system_sequence
            && validate_native_payload(
                manifest
                    .files
                    .iter()
                    .map(|file| (file.path.as_str(), file.mode, file.size)),
            )
            .is_ok()
    }
}

/// Owns a distinct protected-store lock, not the update-user manager locks.
pub struct NativeExecutionStore {
    stager: ApplicationSlotStager,
    trust: ApplicationTrustRoot,
    policy: ExecutionPolicy,
}

/// Held destination identities, deliberately different from staging-source
/// inodes. The borrowed store keeps its lock alive while these are in use.
pub struct PreparedNativeExecution<'store> {
    _store: &'store NativeExecutionStore,
    inventory: VerifiedSlotFiles,
}

impl PreparedNativeExecution<'_> {
    pub fn inventory(&self) -> &VerifiedSlotFiles {
        &self.inventory
    }

    /// For a caller that keeps the store borrowed for the inventory's use,
    /// such as one helper session, which holds the store lock throughout.
    pub fn into_inventory(self) -> VerifiedSlotFiles {
        self.inventory
    }
}

impl NativeExecutionStore {
    /// The path and trust root must come from protected local configuration.
    /// Rejects ordinary update-user directories; no ownership migration occurs.
    pub fn open(
        root: impl Into<PathBuf>,
        trust: ApplicationTrustRoot,
        policy: ExecutionPolicy,
    ) -> Result<Self, ApplicationStageError> {
        if !policy.valid() {
            return Err(ApplicationStageError::PolicyRejected);
        }
        Ok(Self {
            stager: ApplicationSlotStager::open_protected(root)?,
            trust,
            policy,
        })
    }

    /// Independently verify a finite archive and copy its complete composition.
    /// The caller must authenticate and bound reception before entering here.
    /// The exclusive borrow serializes preparation and prevents another session
    /// from preparing while the returned inventory retains this store.
    /// Publication does not change selection, confirm health or advance a floor.
    pub fn prepare(
        &mut self,
        archive: impl Read,
        archive_size: u64,
    ) -> Result<PreparedNativeExecution<'_>, ApplicationStageError> {
        let staged = self.stager.stage_reader_with_policy(
            archive,
            archive_size,
            &self.trust,
            self.policy.quota,
            |verified| self.policy.accepts(&verified.manifest),
        )?;
        let inventory = self
            .stager
            .open_verified_files(&staged.slot_id, &FILES, &self.trust)?;
        Ok(PreparedNativeExecution {
            _store: self,
            inventory,
        })
    }

    /// Unprivileged fixture without the protected ancestry requirement.
    #[cfg(test)]
    pub(crate) fn fixture(
        root: &Path,
        trust: ApplicationTrustRoot,
        policy: ExecutionPolicy,
    ) -> Self {
        Self {
            stager: ApplicationSlotStager::open(root).unwrap(),
            trust,
            policy,
        }
    }

    /// Prepare from an updater-owned slot. `updater_root` must come from
    /// protected configuration; the slot tree itself is treated as untrusted.
    pub fn prepare_from_slot(
        &mut self,
        updater_root: &Path,
        slot_id: &str,
    ) -> Result<PreparedNativeExecution<'_>, ApplicationStageError> {
        let archive = open_slot_archive(updater_root, slot_id, &self.trust)?;
        let size = archive.size();
        let prepared = self.prepare(archive, size)?;
        if prepared.inventory.slot().slot_id != slot_id {
            return Err(ApplicationStageError::SlotConflict);
        }
        Ok(prepared)
    }

    /// Remove protected copies that are neither `keep`, the ledger's current
    /// or previous slot, nor the target of the `current` link (which the
    /// stager always protects), so a new preparation fits the slot quota.
    /// The one-step rollback slot is kept. Returns the removed slot IDs.
    pub fn collect_garbage(
        &self,
        ledger: &SelectionLedgerStore,
        keep: &str,
    ) -> Result<Vec<String>, ApplicationStageError> {
        // A preparation cut short (power, a timeout) may have left the
        // stager's own temporary or staging files: removed first, so that the
        // listing below, which refuses anything unknown, goes on.
        self.stager.cleanup_abandoned_staging()?;
        let selection = ledger.load()?;
        let protected = [Some(keep), selection.current(), selection.previous()]
            .into_iter()
            .flatten()
            .map(str::to_string)
            .collect::<BTreeSet<_>>();
        // So may a deletion of an earlier collection: it is finished before
        // the listing, which refuses its marker and tombstone (code audit of
        // 2026-10-20, AUD-04).
        self.stager
            .recover_slot_deletions(&protected, &self.trust)?;
        let mut removed = Vec::new();
        for slot in self.stager.list_slots(&self.trust)? {
            if !protected.contains(&slot.slot_id) {
                let deleted =
                    self.stager
                        .delete_unreferenced_slot(&slot.slot_id, &protected, &self.trust)?;
                removed.push(deleted.slot_id);
            }
        }
        Ok(removed)
    }

    /// Point the protected `current` link at an already prepared slot.
    /// The caller must have observed every managed unit stopped first.
    /// The ledger is written before the link: after a crash between the two,
    /// repeating the same selection is idempotent.
    pub fn select(
        &mut self,
        ledger: &SelectionLedgerStore,
        slot_id: &str,
    ) -> Result<PreparedNativeExecution<'_>, ApplicationStageError> {
        let inventory = self
            .stager
            .open_verified_files(slot_id, &FILES, &self.trust)?;
        if !self.policy.accepts(&inventory.slot().manifest) {
            return Err(ApplicationStageError::PolicyRejected);
        }
        let next = ledger
            .load()?
            .select(slot_id)
            .ok_or(ApplicationStageError::PolicyRejected)?;
        ledger.store(&next)?;
        let pointer = PointerStore::open(self.stager.root_path().to_path_buf())
            .map_err(|_| ApplicationStageError::UnsafeRoot)?;
        pointer
            .switch(slot_id)
            .map_err(|_| ApplicationStageError::UnsafeDeletionState)?;
        Ok(PreparedNativeExecution {
            _store: self,
            inventory,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bli_kvm_update_verifier::{ApplicationFile, ApplicationManifest};

    #[test]
    fn preparation_copies_signed_files_and_refuses_incompatible_publication() {
        use bli_kvm_update_verifier::{
            APPLICATION_ARCHITECTURE, APPLICATION_FORMAT_V1, APPLICATION_PRODUCT,
            APPLICATION_TARGET,
        };
        use ed25519_dalek::{Signer, SigningKey};
        use sha2::{Digest, Sha256};
        use std::{collections::BTreeMap, io::Read};

        let signing = SigningKey::from_bytes(&[37; 32]);
        let trust =
            ApplicationTrustRoot::new("app-test", signing.verifying_key().to_bytes()).unwrap();
        let mut manifest = bundle();
        manifest.format = APPLICATION_FORMAT_V1.into();
        manifest.product = APPLICATION_PRODUCT.into();
        manifest.target = APPLICATION_TARGET.into();
        manifest.architecture = APPLICATION_ARCHITECTURE.into();
        manifest.files.push(ApplicationFile {
            path: "libexec/bli-kvm-platform-broker".into(),
            mode: 0o755,
            size: 3,
            sha256: String::new(),
        });
        manifest.files.sort_by(|a, b| a.path.cmp(&b.path));
        for file in &mut manifest.files {
            file.size = 3;
            file.sha256 = Sha256::digest(b"bin")
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
        }
        let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
        let signature = signing.sign(&manifest_bytes).to_bytes();
        let mut builder = tar::Builder::new(Vec::new());
        let mut append = |path: &str, mode: u32, bytes: &[u8], directory: bool| {
            let mut header = tar::Header::new_ustar();
            header.set_path(path).unwrap();
            header.set_mode(mode);
            header.set_uid(0);
            header.set_gid(0);
            header.set_mtime(0);
            header.set_size(bytes.len() as u64);
            header.set_entry_type(if directory {
                tar::EntryType::Directory
            } else {
                tar::EntryType::Regular
            });
            header.set_cksum();
            builder.append(&header, bytes).unwrap();
        };
        append("manifest.json", 0o644, &manifest_bytes, false);
        append("manifest.sig", 0o644, &signature, false);
        let mut entries = BTreeMap::new();
        for path in [
            "payload",
            "payload/bin",
            "payload/libexec",
            "payload/share",
            "payload/share/web",
        ] {
            entries.insert(path.to_string(), (0o755, true));
        }
        for file in &manifest.files {
            entries.insert(format!("payload/{}", file.path), (file.mode, false));
        }
        for (path, (mode, directory)) in entries {
            append(&path, mode, if directory { b"" } else { b"bin" }, directory);
        }
        let mut archive = builder.into_inner().unwrap();
        let root = tempfile::tempdir().unwrap();
        // Test the preparation pipeline without claiming a privileged deployment.
        // Production construction only uses open_protected; that boundary has
        // its own caller/ancestor refusal tests in update-stager.
        let mut store = NativeExecutionStore {
            stager: ApplicationSlotStager::open(root.path()).unwrap(),
            trust,
            policy: ExecutionPolicy {
                system_sequence: 1,
                minimum_application_sequence: 14,
                quota: StageQuota {
                    max_slots: 1,
                    max_payload_bytes: 1024,
                },
            },
        };
        let error = store
            .prepare(archive.as_slice(), archive.len() as u64)
            .err()
            .expect("incompatible archive must be refused");
        assert!(
            matches!(error, ApplicationStageError::PolicyRejected),
            "{error:?}"
        );
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
        store.policy.system_sequence = 2;
        let prepared = store
            .prepare(archive.as_slice(), archive.len() as u64)
            .unwrap();
        assert_eq!(prepared.inventory().slot().manifest, manifest);
        archive.fill(0);
        for file in prepared.inventory().files().values() {
            let mut copy = file.try_clone().unwrap();
            let mut bytes = Vec::new();
            copy.read_to_end(&mut bytes).unwrap();
            assert_eq!(bytes, b"bin");
        }
    }

    // Policy fixtures alone are not cryptographic evidence. The stager tests
    // separately prove that this callback never receives an unverified archive.
    fn bundle() -> ApplicationManifest {
        ApplicationManifest {
            format: "hardened-blikvm-application-v1".into(),
            product: "hardened-blikvm".into(),
            target: "test".into(),
            architecture: "aarch64".into(),
            key_id: "app-test".into(),
            version: "1.0.0".into(),
            sequence: 14,
            minimum_system_sequence: 2,
            source_commit: "1".repeat(40),
            files: FILES
                .iter()
                .map(|path| ApplicationFile {
                    path: (*path).into(),
                    mode: if path.starts_with("share/") {
                        0o644
                    } else {
                        0o755
                    },
                    size: 1,
                    sha256: "1".repeat(64),
                })
                .collect(),
        }
    }

    #[test]
    fn execution_policy_requires_compatible_version_and_complete_native_payload() {
        let policy = ExecutionPolicy {
            system_sequence: 2,
            minimum_application_sequence: 14,
            quota: StageQuota {
                max_slots: 3,
                max_payload_bytes: 1024,
            },
        };
        assert!(policy.accepts(&bundle()));
        let mut older = bundle();
        older.sequence = 13;
        assert!(!policy.accepts(&older));
        let mut incompatible = bundle();
        incompatible.minimum_system_sequence = 3;
        assert!(!policy.accepts(&incompatible));
        for index in 0..FILES.len() {
            let mut incomplete = bundle();
            incomplete.files.remove(index);
            assert!(!policy.accepts(&incomplete));
            let mut empty = bundle();
            empty.files[index].size = 0;
            assert!(!policy.accepts(&empty));
            let mut wrong_mode = bundle();
            wrong_mode.files[index].mode = 0o777;
            assert!(!policy.accepts(&wrong_mode));
        }
        assert!(
            !ExecutionPolicy {
                system_sequence: 0,
                ..policy
            }
            .accepts(&bundle())
        );
        assert!(
            !ExecutionPolicy {
                minimum_application_sequence: 0,
                ..policy
            }
            .accepts(&bundle())
        );
        for quota in [
            StageQuota {
                max_slots: 0,
                max_payload_bytes: 1024,
            },
            StageQuota {
                max_slots: 33,
                max_payload_bytes: 1024,
            },
            StageQuota {
                max_slots: 1,
                max_payload_bytes: 0,
            },
        ] {
            assert!(!ExecutionPolicy { quota, ..policy }.accepts(&bundle()));
        }
    }

    fn updater_with(sequences: &[u64]) -> (tempfile::TempDir, ApplicationTrustRoot, Vec<String>) {
        let root = tempfile::tempdir().unwrap();
        let stager = ApplicationSlotStager::open(root.path()).unwrap();
        let mut trust = None;
        let mut slots = Vec::new();
        for &sequence in sequences {
            let (root_key, _, archive) = crate::slot_archive::fixture::signed_archive(sequence, 1);
            slots.push(
                stager
                    .stage_reader(archive.as_slice(), archive.len() as u64, &root_key)
                    .unwrap()
                    .slot_id,
            );
            trust = Some(root_key);
        }
        (root, trust.unwrap(), slots)
    }

    fn execution_store(root: &Path, trust: ApplicationTrustRoot) -> NativeExecutionStore {
        // Unprivileged fixture; production uses open_protected only.
        NativeExecutionStore {
            stager: ApplicationSlotStager::open(root).unwrap(),
            trust,
            policy: ExecutionPolicy {
                system_sequence: 1,
                minimum_application_sequence: 14,
                quota: StageQuota {
                    max_slots: 8,
                    max_payload_bytes: 1024 * 1024,
                },
            },
        }
    }

    #[test]
    fn prepare_from_slot_copies_verified_bytes_into_distinct_protected_inodes() {
        use std::os::unix::fs::MetadataExt;

        let (updater, trust, slots) = updater_with(&[15]);
        let protected = tempfile::tempdir().unwrap();
        let mut store = execution_store(protected.path(), trust);
        let prepared = store.prepare_from_slot(updater.path(), &slots[0]).unwrap();
        assert_eq!(prepared.inventory().slot().slot_id, slots[0]);
        for (path, file) in prepared.inventory().files() {
            let mut bytes = Vec::new();
            file.try_clone().unwrap().read_to_end(&mut bytes).unwrap();
            assert_eq!(bytes, crate::slot_archive::fixture::content(path));
            let source = std::fs::metadata(updater.path().join(&slots[0]).join(path)).unwrap();
            assert_ne!(
                (
                    file.metadata().unwrap().dev(),
                    file.metadata().unwrap().ino()
                ),
                (source.dev(), source.ino())
            );
        }
        drop(prepared);
        // Idempotent for the same verified slot.
        assert!(store.prepare_from_slot(updater.path(), &slots[0]).is_ok());
    }

    #[test]
    fn prepare_from_slot_refuses_modified_policy_rejected_or_mismatched_slots() {
        use std::os::unix::fs::PermissionsExt;

        let (updater, trust, slots) = updater_with(&[13, 15]);
        let protected = tempfile::tempdir().unwrap();
        let mut store = execution_store(protected.path(), trust);
        assert!(matches!(
            store.prepare_from_slot(updater.path(), &slots[0]),
            Err(ApplicationStageError::PolicyRejected)
        ));
        let slot = updater.path().join(&slots[1]);
        let target = slot.join("libexec/bli-kvm-boardd");
        for path in [slot.join("libexec"), target.clone()] {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let length = std::fs::metadata(&target).unwrap().len() as usize;
        std::fs::write(&target, vec![b'z'; length]).unwrap();
        assert!(matches!(
            store.prepare_from_slot(updater.path(), &slots[1]),
            Err(ApplicationStageError::Verify(_))
        ));
        assert_eq!(std::fs::read_dir(protected.path()).unwrap().count(), 0);
    }

    #[test]
    fn select_switches_the_protected_link_within_the_one_step_ledger() {
        let (updater, trust, slots) = updater_with(&[14, 15, 16]);
        let protected = tempfile::tempdir().unwrap();
        let ledger_root = tempfile::tempdir().unwrap();
        let ledger = SelectionLedgerStore::fixture(ledger_root.path());
        let mut store = execution_store(protected.path(), trust);
        let current = || {
            std::fs::read_link(protected.path().join("current"))
                .ok()
                .map(|target| target.to_str().unwrap().to_string())
        };
        assert!(matches!(
            store.select(&ledger, &slots[1]),
            Err(ApplicationStageError::UnsafeExistingSlot | ApplicationStageError::SlotNotFound)
        ));
        assert_eq!(current(), None);
        for slot in &slots {
            store.prepare_from_slot(updater.path(), slot).unwrap();
        }
        store.select(&ledger, &slots[1]).unwrap();
        assert_eq!(current().as_deref(), Some(slots[1].as_str()));
        store.select(&ledger, &slots[2]).unwrap();
        assert_eq!(current().as_deref(), Some(slots[2].as_str()));
        // Two steps back is refused without touching the link.
        assert!(matches!(
            store.select(&ledger, &slots[0]),
            Err(ApplicationStageError::PolicyRejected)
        ));
        assert_eq!(current().as_deref(), Some(slots[2].as_str()));
        store.select(&ledger, &slots[1]).unwrap();
        assert_eq!(current().as_deref(), Some(slots[1].as_str()));
        assert!(matches!(
            store.select(&ledger, &slots[0]),
            Err(ApplicationStageError::PolicyRejected)
        ));
        // Crash replay: ledger already records the selection, the link does not.
        store.select(&ledger, &slots[2]).unwrap();
        std::fs::remove_file(protected.path().join("current")).unwrap();
        store.select(&ledger, &slots[2]).unwrap();
        assert_eq!(current().as_deref(), Some(slots[2].as_str()));
        assert_eq!(ledger.load().unwrap().previous(), Some(slots[1].as_str()));
    }

    #[test]
    fn a_leftover_in_the_exec_store_does_not_stop_a_preparation() {
        use std::os::unix::fs::PermissionsExt;
        let (updater, trust, slots) = updater_with(&[14, 15]);
        let protected = tempfile::tempdir().unwrap();
        let ledger_root = tempfile::tempdir().unwrap();
        let ledger = SelectionLedgerStore::fixture(ledger_root.path());
        let mut store = execution_store(protected.path(), trust);
        store.prepare_from_slot(updater.path(), &slots[0]).unwrap();
        store.select(&ledger, &slots[0]).unwrap();
        // What an older stager left when a preparation was cut short: every
        // boot prepares the confirmed slot through this collection, so a
        // refusal here kept the board's application from starting.
        let leftover = protected.path().join(".tmpAbC123");
        std::fs::write(&leftover, b"half a snapshot").unwrap();
        std::fs::set_permissions(&leftover, std::fs::Permissions::from_mode(0o400)).unwrap();
        assert!(store.collect_garbage(&ledger, &slots[1]).is_ok());
        assert!(!leftover.exists());
        store.prepare_from_slot(updater.path(), &slots[1]).unwrap();
    }

    #[test]
    fn an_interrupted_deletion_in_the_exec_store_does_not_stop_a_preparation() {
        use std::os::unix::fs::PermissionsExt;
        let (updater, trust, slots) = updater_with(&[14, 15, 16]);
        let protected = tempfile::tempdir().unwrap();
        let ledger_root = tempfile::tempdir().unwrap();
        let ledger = SelectionLedgerStore::fixture(ledger_root.path());
        let mut store = execution_store(protected.path(), trust);
        store.prepare_from_slot(updater.path(), &slots[0]).unwrap();
        store.prepare_from_slot(updater.path(), &slots[1]).unwrap();
        store.select(&ledger, &slots[1]).unwrap();
        // A collection's deletion of the first slot cut short (code audit of
        // 2026-10-20, AUD-04): its marker written and the slot renamed to its
        // tombstone. Every boot prepares the confirmed slot through this
        // collection.
        let manifest_sha256 = store
            .stager
            .inspect_slot(&slots[0], &store.trust)
            .unwrap()
            .manifest_sha256;
        let marker = protected.path().join(format!(".delete-{}.json", slots[0]));
        std::fs::write(
            &marker,
            serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 1,
                "purpose": "hardened-blikvm-slot-deletion",
                "slotId": slots[0],
                "manifestSha256": manifest_sha256,
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o600)).unwrap();
        let tombstone = protected.path().join(format!(".deleting-{}", slots[0]));
        std::fs::rename(protected.path().join(&slots[0]), &tombstone).unwrap();
        assert_eq!(
            store.collect_garbage(&ledger, &slots[2]).unwrap(),
            Vec::<String>::new()
        );
        assert!(!marker.exists() && !tombstone.exists());
        store.prepare_from_slot(updater.path(), &slots[2]).unwrap();
    }

    #[test]
    fn garbage_collection_keeps_selection_rollback_and_target_within_quota() {
        let (updater, trust, slots) = updater_with(&[14, 15, 16, 17]);
        let protected = tempfile::tempdir().unwrap();
        let ledger_root = tempfile::tempdir().unwrap();
        let ledger = SelectionLedgerStore::fixture(ledger_root.path());
        let mut store = execution_store(protected.path(), trust);
        store.policy.quota.max_slots = 3;
        for slot in &slots[..3] {
            store.prepare_from_slot(updater.path(), slot).unwrap();
        }
        store.select(&ledger, &slots[1]).unwrap();
        // Observed on the board: a full store refused every new preparation.
        assert!(store.prepare_from_slot(updater.path(), &slots[3]).is_err());
        let mut removed = store.collect_garbage(&ledger, &slots[3]).unwrap();
        removed.sort();
        assert_eq!(removed, [slots[0].clone(), slots[2].clone()]);
        store.prepare_from_slot(updater.path(), &slots[3]).unwrap();
        store.select(&ledger, &slots[3]).unwrap();
        // The rollback slot and the selected slot survive the next collection.
        assert!(
            store
                .collect_garbage(&ledger, &slots[3])
                .unwrap()
                .is_empty()
        );
        store.select(&ledger, &slots[1]).unwrap();
        assert_eq!(
            std::fs::read_link(protected.path().join("current")).unwrap(),
            Path::new(&slots[1])
        );
    }
}
