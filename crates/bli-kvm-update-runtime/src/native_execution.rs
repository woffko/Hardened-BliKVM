//! Protected artifact preparation for the future local privileged coordinator.
//! This layer never opens manager state or authorizes service startup.

use std::{io::Read, path::PathBuf};

use bli_kvm_update_stager::{
    ApplicationSlotStager, ApplicationStageError, StageQuota, VerifiedSlotFiles,
};
use bli_kvm_update_verifier::{ApplicationTrustRoot, VerifiedApplicationBundle};

use crate::{composition::validate_native_payload, native_identity::FILES};

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

    fn accepts(self, bundle: &VerifiedApplicationBundle) -> bool {
        self.valid()
            && bundle.manifest.sequence >= self.minimum_application_sequence
            && bundle.manifest.minimum_system_sequence <= self.system_sequence
            && validate_native_payload(
                bundle
                    .manifest
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
            |verified| self.policy.accepts(verified),
        )?;
        let inventory = self
            .stager
            .open_verified_files(&staged.slot_id, &FILES, &self.trust)?;
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
        let mut manifest = bundle().manifest;
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
    fn bundle() -> VerifiedApplicationBundle {
        VerifiedApplicationBundle {
            manifest: ApplicationManifest {
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
            },
            manifest_sha256: "2".repeat(64),
            payload_bytes: 5,
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
        older.manifest.sequence = 13;
        assert!(!policy.accepts(&older));
        let mut incompatible = bundle();
        incompatible.manifest.minimum_system_sequence = 3;
        assert!(!policy.accepts(&incompatible));
        for index in 0..FILES.len() {
            let mut incomplete = bundle();
            incomplete.manifest.files.remove(index);
            assert!(!policy.accepts(&incomplete));
            let mut empty = bundle();
            empty.manifest.files[index].size = 0;
            assert!(!policy.accepts(&empty));
            let mut wrong_mode = bundle();
            wrong_mode.manifest.files[index].mode = 0o777;
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
}
