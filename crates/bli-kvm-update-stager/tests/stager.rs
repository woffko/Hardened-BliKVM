use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, Permissions},
    os::unix::fs::{PermissionsExt, symlink},
    path::{Path, PathBuf},
};

use bli_kvm_update_stager::{
    ApplicationSlotStager, ApplicationStageError, SLOT_MANIFEST_PATH, SLOT_RECEIPT_PATH,
    SLOT_SIGNATURE_PATH, StageDisposition, StageQuota,
};
use bli_kvm_update_verifier::{
    APPLICATION_ARCHITECTURE, APPLICATION_FORMAT_V1, APPLICATION_MANIFEST_PATH,
    APPLICATION_PRODUCT, APPLICATION_SIGNATURE_PATH, APPLICATION_TARGET, ApplicationFile,
    ApplicationManifest, ApplicationTrustRoot, MAX_ARCHIVE_BYTES, UpdateVerifyError,
};
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};
use tar::{Builder, EntryType, Header};
use tempfile::TempDir;

const KEY_ID: &str = "app-stager-test-key-1";

fn payload() -> BTreeMap<&'static str, (u32, Vec<u8>)> {
    BTreeMap::from([
        ("bin/bli-kvm-server", (0o755, b"server".to_vec())),
        (
            "libexec/bli-kvm-control-broker",
            (0o755, b"control".to_vec()),
        ),
        (
            "libexec/bli-kvm-platform-broker",
            (0o755, b"broker".to_vec()),
        ),
        ("libexec/bli-kvm-video-proxy", (0o755, b"video".to_vec())),
        ("share/web/index.html", (0o644, b"<html></html>".to_vec())),
    ])
}

fn manifest(payload: &BTreeMap<&str, (u32, Vec<u8>)>) -> ApplicationManifest {
    ApplicationManifest {
        format: APPLICATION_FORMAT_V1.into(),
        product: APPLICATION_PRODUCT.into(),
        target: APPLICATION_TARGET.into(),
        architecture: APPLICATION_ARCHITECTURE.into(),
        key_id: KEY_ID.into(),
        version: "0.2.0-dev".into(),
        sequence: 2,
        minimum_system_sequence: 1,
        source_commit: "2222222222222222222222222222222222222222".into(),
        files: payload
            .iter()
            .map(|(path, (mode, bytes))| ApplicationFile {
                path: (*path).to_string(),
                mode: *mode,
                size: bytes.len() as u64,
                sha256: sha256_hex(bytes),
            })
            .collect(),
    }
}

fn header(path: &str, mode: u32, size: u64, entry_type: EntryType) -> Header {
    let mut header = Header::new_ustar();
    header.set_path(path).unwrap();
    header.set_mode(mode);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_size(size);
    header.set_entry_type(entry_type);
    header.set_cksum();
    header
}

fn payload_directories(payload: &BTreeMap<&str, (u32, Vec<u8>)>) -> BTreeSet<String> {
    let mut directories = BTreeSet::from(["payload".to_string()]);
    for path in payload.keys() {
        let mut current = PathBuf::from("payload");
        let components = Path::new(path).components().collect::<Vec<_>>();
        for component in components.iter().take(components.len() - 1) {
            current.push(component.as_os_str());
            directories.insert(current.to_str().unwrap().to_string());
        }
    }
    directories
}

fn bundle(
    signing_key: &SigningKey,
    manifest: &ApplicationManifest,
    payload: &BTreeMap<&str, (u32, Vec<u8>)>,
) -> Vec<u8> {
    let manifest_bytes = serde_json::to_vec(manifest).unwrap();
    let signature = signing_key.sign(&manifest_bytes).to_bytes();
    let mut builder = Builder::new(Vec::new());
    builder.mode(tar::HeaderMode::Deterministic);
    builder
        .append(
            &header(
                APPLICATION_MANIFEST_PATH,
                0o644,
                manifest_bytes.len() as u64,
                EntryType::Regular,
            ),
            manifest_bytes.as_slice(),
        )
        .unwrap();
    builder
        .append(
            &header(APPLICATION_SIGNATURE_PATH, 0o644, 64, EntryType::Regular),
            signature.as_slice(),
        )
        .unwrap();
    let directories = payload_directories(payload);
    let mut entries = directories.clone();
    entries.extend(payload.keys().map(|path| format!("payload/{path}")));
    for path in entries {
        if directories.contains(&path) {
            builder
                .append(&header(&path, 0o755, 0, EntryType::Directory), &[][..])
                .unwrap();
        } else {
            let relative = path.strip_prefix("payload/").unwrap();
            let (mode, bytes) = payload.get(relative).unwrap();
            builder
                .append(
                    &header(&path, *mode, bytes.len() as u64, EntryType::Regular),
                    bytes.as_slice(),
                )
                .unwrap();
        }
    }
    builder.into_inner().unwrap()
}

fn keys() -> (SigningKey, ApplicationTrustRoot) {
    let signing = SigningKey::from_bytes(&[17; 32]);
    let trust = ApplicationTrustRoot::new(KEY_ID, signing.verifying_key().to_bytes()).unwrap();
    (signing, trust)
}

fn write_source(directory: &TempDir, name: &str, bytes: &[u8]) -> PathBuf {
    let path = directory.path().join(name);
    fs::write(&path, bytes).unwrap();
    fs::set_permissions(&path, Permissions::from_mode(0o600)).unwrap();
    path
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
}

fn root_entries(root: &Path) -> Vec<String> {
    let mut entries = fs::read_dir(root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    entries.sort();
    entries
}

#[test]
fn publication_policy_rejects_before_publish_and_on_idempotent_retry() {
    let (signing, trust) = keys();
    let payload = payload();
    let manifest = manifest(&payload);
    let archive = bundle(&signing, &manifest, &payload);
    let slots = tempfile::tempdir().unwrap();
    let stager = ApplicationSlotStager::open(slots.path()).unwrap();
    let quota = StageQuota {
        max_slots: 1,
        max_payload_bytes: 1024,
    };
    let stage = |accept: &dyn Fn(&bli_kvm_update_verifier::VerifiedApplicationBundle) -> bool| {
        stager.stage_reader_with_policy(
            archive.as_slice(),
            archive.len() as u64,
            &trust,
            quota,
            accept,
        )
    };
    let reject = |verified: &bli_kvm_update_verifier::VerifiedApplicationBundle| {
        assert_eq!(verified.manifest, manifest);
        // A local floor above this valid signed bundle is a publication refusal.
        verified.manifest.sequence > manifest.sequence
    };
    assert!(matches!(
        stage(&reject),
        Err(ApplicationStageError::PolicyRejected)
    ));
    assert!(root_entries(slots.path()).is_empty());

    let published = stage(&|_| true).unwrap();
    let receipt = fs::read(published.slot_path.join(SLOT_RECEIPT_PATH)).unwrap();
    assert!(matches!(
        stage(&reject),
        Err(ApplicationStageError::PolicyRejected)
    ));
    assert_eq!(root_entries(slots.path()), vec![published.slot_id.clone()]);
    assert_eq!(
        fs::read(published.slot_path.join(SLOT_RECEIPT_PATH)).unwrap(),
        receipt
    );
    assert_eq!(
        stage(&|_| true).unwrap().disposition,
        StageDisposition::AlreadyPresent
    );
}

#[test]
fn publication_policy_never_receives_unverified_archive() {
    let (signing, trust) = keys();
    let original = payload();
    let manifest = manifest(&original);
    let mut corrupted = original.clone();
    corrupted.get_mut("bin/bli-kvm-server").unwrap().1[0] ^= 1;
    let foreign = SigningKey::from_bytes(&[93; 32]);
    let slots = tempfile::tempdir().unwrap();
    let stager = ApplicationSlotStager::open(slots.path()).unwrap();
    for archive in [
        bundle(&signing, &manifest, &corrupted),
        bundle(&foreign, &manifest, &original),
    ] {
        let result = stager.stage_reader_with_policy(
            archive.as_slice(),
            archive.len() as u64,
            &trust,
            StageQuota {
                max_slots: 1,
                max_payload_bytes: 1024,
            },
            |_| panic!("policy must not observe unauthenticated bundle contents"),
        );
        assert!(matches!(result, Err(ApplicationStageError::Verify(_))));
        assert!(root_entries(slots.path()).is_empty());
    }
}

#[test]
fn stages_one_readonly_exact_slot_and_is_idempotent() {
    let (signing, trust) = keys();
    let payload = payload();
    let manifest = manifest(&payload);
    let archive = bundle(&signing, &manifest, &payload);
    let source_directory = tempfile::tempdir().unwrap();
    let source = write_source(&source_directory, "application.tar", &archive);
    let slots = tempfile::tempdir().unwrap();
    let stager = ApplicationSlotStager::open(slots.path()).unwrap();

    let first = stager.stage(&source, &trust).unwrap();
    assert_eq!(first.disposition, StageDisposition::Published);
    assert_eq!(first.manifest, manifest);
    assert_eq!(first.payload_bytes, 6 + 7 + 6 + 5 + 13);
    assert_eq!(mode(&first.slot_path), 0o555);
    for (relative, (manifest_mode, bytes)) in &payload {
        let path = first.slot_path.join(relative);
        assert_eq!(fs::read(&path).unwrap(), *bytes);
        assert_eq!(mode(&path), manifest_mode & !0o222);
    }
    assert_eq!(mode(&first.slot_path.join(SLOT_RECEIPT_PATH)), 0o400);
    assert_eq!(mode(&first.slot_path.join(SLOT_MANIFEST_PATH)), 0o400);
    assert_eq!(mode(&first.slot_path.join(SLOT_SIGNATURE_PATH)), 0o400);
    assert_eq!(fs::read(&source).unwrap(), archive);

    let second = stager.stage(&source, &trust).unwrap();
    assert_eq!(second.disposition, StageDisposition::AlreadyPresent);
    assert_eq!(second.slot_id, first.slot_id);
    let inspected = stager.inspect_slot(&first.slot_id, &trust).unwrap();
    assert_eq!(inspected.manifest, first.manifest);
    assert_eq!(inspected.manifest_sha256, first.manifest_sha256);
    assert_eq!(root_entries(slots.path()), vec![first.slot_id]);
}

#[test]
fn streamed_archive_uses_the_same_immutable_idempotent_slot() {
    let (signing, trust) = keys();
    let payload = payload();
    let archive = bundle(&signing, &manifest(&payload), &payload);
    let slots = tempfile::tempdir().unwrap();
    let stager = ApplicationSlotStager::open(slots.path()).unwrap();
    let first = stager
        .stage_reader(archive.as_slice(), archive.len() as u64, &trust)
        .unwrap();
    assert_eq!(first.disposition, StageDisposition::Published);
    assert_eq!(mode(&first.slot_path), 0o555);
    for (relative, (manifest_mode, bytes)) in &payload {
        let path = first.slot_path.join(relative);
        assert_eq!(fs::read(&path).unwrap(), *bytes);
        assert_eq!(mode(&path), manifest_mode & !0o222);
    }
    let source_directory = tempfile::tempdir().unwrap();
    let source = write_source(&source_directory, "application.tar", &archive);
    let second = stager.stage(source, &trust).unwrap();
    assert_eq!(second.disposition, StageDisposition::AlreadyPresent);
    assert_eq!(second.slot_id, first.slot_id);
    assert_eq!(second.manifest_sha256, first.manifest_sha256);
    assert_eq!(root_entries(slots.path()), vec![first.slot_id]);
    assert!(!slots.path().join("current").exists());
}

#[test]
fn streamed_archive_rejects_length_mismatch_and_cleans_snapshot() {
    let (signing, trust) = keys();
    let payload = payload();
    let archive = bundle(&signing, &manifest(&payload), &payload);
    let slots = tempfile::tempdir().unwrap();
    let stager = ApplicationSlotStager::open(slots.path()).unwrap();
    for declared in [archive.len() as u64 - 1, archive.len() as u64 + 1] {
        assert!(matches!(
            stager.stage_reader(archive.as_slice(), declared, &trust),
            Err(ApplicationStageError::SourceChanged)
        ));
        assert!(root_entries(slots.path()).is_empty());
    }
    // A lying sender cannot force a full maximum-size copy for a tiny declaration.
    let mut reader = std::io::Cursor::new(&archive);
    assert!(matches!(
        stager.stage_reader(&mut reader, 1, &trust),
        Err(ApplicationStageError::SourceChanged)
    ));
    assert_eq!(reader.position(), 2);
    assert!(root_entries(slots.path()).is_empty());
}

#[test]
fn streamed_archive_rejects_invalid_bounds_without_reading() {
    let (_, trust) = keys();
    let slots = tempfile::tempdir().unwrap();
    let stager = ApplicationSlotStager::open(slots.path()).unwrap();
    struct MustNotRead;
    impl std::io::Read for MustNotRead {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            panic!("invalid declared size must be rejected before reading")
        }
    }
    for declared in [0, MAX_ARCHIVE_BYTES + 1, u64::MAX] {
        assert!(matches!(
            stager.stage_reader(MustNotRead, declared, &trust),
            Err(ApplicationStageError::ArchiveTooLarge)
        ));
        assert!(root_entries(slots.path()).is_empty());
    }
}

#[test]
fn streamed_archive_rejects_read_failure_and_untrusted_signature_without_publication() {
    let (signing, _) = keys();
    let untrusted = SigningKey::from_bytes(&[18; 32]);
    let trust = ApplicationTrustRoot::new(KEY_ID, untrusted.verifying_key().to_bytes()).unwrap();
    let payload = payload();
    let archive = bundle(&signing, &manifest(&payload), &payload);
    let slots = tempfile::tempdir().unwrap();
    let stager = ApplicationSlotStager::open(slots.path()).unwrap();
    assert!(matches!(
        stager.stage_reader(archive.as_slice(), archive.len() as u64, &trust),
        Err(ApplicationStageError::Verify(_))
    ));
    struct Broken;
    impl std::io::Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("test interruption"))
        }
    }
    assert!(matches!(
        stager.stage_reader(Broken, 1, &trust),
        Err(ApplicationStageError::Io(_))
    ));
    assert!(root_entries(slots.path()).is_empty());
}

#[test]
fn upload_quota_preserves_existing_slot_and_allows_idempotent_retry_at_capacity() {
    let (signing, trust) = keys();
    let payload = payload();
    let mut manifest = manifest(&payload);
    let first_archive = bundle(&signing, &manifest, &payload);
    manifest.sequence += 1;
    let next_archive = bundle(&signing, &manifest, &payload);
    let slots = tempfile::tempdir().unwrap();
    let stager = ApplicationSlotStager::open(slots.path()).unwrap();
    let quota = StageQuota {
        max_slots: 1,
        max_payload_bytes: 37,
    };
    let first = stager
        .stage_reader_with_quota(
            first_archive.as_slice(),
            first_archive.len() as u64,
            &trust,
            quota,
        )
        .unwrap();
    assert_eq!(first.disposition, StageDisposition::Published);
    assert_eq!(first.payload_bytes, quota.max_payload_bytes);
    symlink(&first.slot_id, slots.path().join("current")).unwrap();
    let before = root_entries(slots.path());
    assert!(matches!(
        stager.stage_reader_with_quota(
            next_archive.as_slice(),
            next_archive.len() as u64,
            &trust,
            quota,
        ),
        Err(ApplicationStageError::QuotaExceeded)
    ));
    let retry = stager
        .stage_reader_with_quota(
            first_archive.as_slice(),
            first_archive.len() as u64,
            &trust,
            quota,
        )
        .unwrap();
    assert_eq!(retry.disposition, StageDisposition::AlreadyPresent);
    assert_eq!(retry.slot_id, first.slot_id);
    let lowered_quota_retry = stager
        .stage_reader_with_quota(
            first_archive.as_slice(),
            first_archive.len() as u64,
            &trust,
            StageQuota {
                max_slots: 1,
                max_payload_bytes: 1,
            },
        )
        .unwrap();
    assert_eq!(
        lowered_quota_retry.disposition,
        StageDisposition::AlreadyPresent
    );
    assert_eq!(root_entries(slots.path()), before);
    assert_eq!(
        fs::read_link(slots.path().join("current")).unwrap(),
        PathBuf::from(&first.slot_id)
    );
    assert_eq!(
        stager
            .inspect_slot(&first.slot_id, &trust)
            .unwrap()
            .manifest,
        first.manifest
    );
}

#[test]
fn upload_payload_quota_counts_existing_and_new_slots() {
    let (signing, trust) = keys();
    let payload = payload();
    let mut manifest = manifest(&payload);
    let first_archive = bundle(&signing, &manifest, &payload);
    manifest.sequence += 1;
    let next_archive = bundle(&signing, &manifest, &payload);
    let slots = tempfile::tempdir().unwrap();
    let stager = ApplicationSlotStager::open(slots.path()).unwrap();
    let first = stager
        .stage_reader(first_archive.as_slice(), first_archive.len() as u64, &trust)
        .unwrap();
    assert!(matches!(
        stager.stage_reader_with_quota(
            next_archive.as_slice(),
            next_archive.len() as u64,
            &trust,
            StageQuota {
                max_slots: 2,
                max_payload_bytes: first.payload_bytes * 2 - 1
            },
        ),
        Err(ApplicationStageError::QuotaExceeded)
    ));
    assert_eq!(root_entries(slots.path()), vec![first.slot_id]);
    let next = stager
        .stage_reader_with_quota(
            next_archive.as_slice(),
            next_archive.len() as u64,
            &trust,
            StageQuota {
                max_slots: 2,
                max_payload_bytes: first.payload_bytes * 2,
            },
        )
        .unwrap();
    assert_eq!(next.disposition, StageDisposition::Published);
    assert_eq!(stager.list_slots(&trust).unwrap().len(), 2);
}

#[test]
fn upload_quota_refuses_corrupt_existing_slot_without_overwriting_it() {
    let (signing, trust) = keys();
    let payload = payload();
    let mut manifest = manifest(&payload);
    let archive = bundle(&signing, &manifest, &payload);
    let slots = tempfile::tempdir().unwrap();
    let stager = ApplicationSlotStager::open(slots.path()).unwrap();
    let first = stager
        .stage_reader(archive.as_slice(), archive.len() as u64, &trust)
        .unwrap();
    let damaged = first.slot_path.join("share/web/index.html");
    fs::set_permissions(&damaged, Permissions::from_mode(0o600)).unwrap();
    fs::write(&damaged, b"tampered").unwrap();
    fs::set_permissions(&damaged, Permissions::from_mode(0o444)).unwrap();
    manifest.sequence += 1;
    let next = bundle(&signing, &manifest, &payload);
    assert!(
        stager
            .stage_reader_with_quota(
                next.as_slice(),
                next.len() as u64,
                &trust,
                StageQuota {
                    max_slots: 2,
                    max_payload_bytes: 1024
                },
            )
            .is_err()
    );
    assert_eq!(root_entries(slots.path()), vec![first.slot_id]);
    assert_eq!(fs::read(damaged).unwrap(), b"tampered");
}

#[test]
fn upload_quota_refuses_unknown_root_content_without_removing_it() {
    let (signing, trust) = keys();
    let payload = payload();
    let archive = bundle(&signing, &manifest(&payload), &payload);
    let slots = tempfile::tempdir().unwrap();
    let stager = ApplicationSlotStager::open(slots.path()).unwrap();
    let unrelated = slots.path().join("unrelated");
    fs::write(&unrelated, b"preserve").unwrap();
    assert!(matches!(
        stager.stage_reader_with_quota(
            archive.as_slice(),
            archive.len() as u64,
            &trust,
            StageQuota {
                max_slots: 2,
                max_payload_bytes: 1024
            },
        ),
        Err(ApplicationStageError::UnexpectedRootEntry)
    ));
    assert_eq!(root_entries(slots.path()), vec!["unrelated"]);
    assert_eq!(fs::read(unrelated).unwrap(), b"preserve");
}

#[test]
fn upload_quota_rejects_invalid_policy_before_consuming_source() {
    let (_, trust) = keys();
    let slots = tempfile::tempdir().unwrap();
    let stager = ApplicationSlotStager::open(slots.path()).unwrap();
    for quota in [
        StageQuota {
            max_slots: 0,
            max_payload_bytes: 1,
        },
        StageQuota {
            max_slots: 33,
            max_payload_bytes: 1,
        },
        StageQuota {
            max_slots: 1,
            max_payload_bytes: 0,
        },
    ] {
        let mut source = std::io::Cursor::new(b"test");
        assert!(matches!(
            stager.stage_reader_with_quota(&mut source, 4, &trust, quota),
            Err(ApplicationStageError::InvalidQuota)
        ));
        assert_eq!(source.position(), 0);
        assert!(root_entries(slots.path()).is_empty());
    }
}

#[test]
fn standalone_inspection_rejects_untrusted_slot_identifiers() {
    let (_, trust) = keys();
    let slots = tempfile::tempdir().unwrap();
    let stager = ApplicationSlotStager::open(slots.path()).unwrap();
    for slot_id in [
        "../escape",
        "app-00000000000000000000-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "app-00000000000000000001-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        "app-00000000000000000001-0000000000000000000000000000000000000000000000000000000000000000",
        "app-1-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    ] {
        assert!(matches!(
            stager.inspect_slot(slot_id, &trust),
            Err(ApplicationStageError::InvalidSlotId)
        ));
    }
}

#[test]
fn rejects_wrong_signature_and_payload_tamper_without_residue() {
    let (signing, trust) = keys();
    let payload = payload();
    let manifest = manifest(&payload);
    let source_directory = tempfile::tempdir().unwrap();
    let slots = tempfile::tempdir().unwrap();
    let stager = ApplicationSlotStager::open(slots.path()).unwrap();

    let wrong_signer = SigningKey::from_bytes(&[18; 32]);
    let wrong = write_source(
        &source_directory,
        "wrong-key.tar",
        &bundle(&wrong_signer, &manifest, &payload),
    );
    assert!(matches!(
        stager.stage(&wrong, &trust),
        Err(ApplicationStageError::Verify(
            UpdateVerifyError::InvalidSignature
        ))
    ));
    assert!(root_entries(slots.path()).is_empty());

    let oversized = source_directory.path().join("oversized.tar");
    fs::File::create(&oversized)
        .unwrap()
        .set_len(MAX_ARCHIVE_BYTES + 1)
        .unwrap();
    fs::set_permissions(&oversized, Permissions::from_mode(0o600)).unwrap();
    assert!(matches!(
        stager.stage(&oversized, &trust),
        Err(ApplicationStageError::ArchiveTooLarge)
    ));
    assert!(root_entries(slots.path()).is_empty());

    let mut tampered = payload.clone();
    tampered.insert("bin/bli-kvm-server", (0o755, b"tampered".to_vec()));
    let tampered = write_source(
        &source_directory,
        "tampered.tar",
        &bundle(&signing, &manifest, &tampered),
    );
    assert!(matches!(
        stager.stage(&tampered, &trust),
        Err(ApplicationStageError::Verify(
            UpdateVerifyError::PayloadMismatch
        ))
    ));
    assert!(root_entries(slots.path()).is_empty());
}

#[test]
fn rejects_symlink_or_writable_source_and_unsafe_root() {
    let (signing, trust) = keys();
    let payload = payload();
    let archive = bundle(&signing, &manifest(&payload), &payload);
    let source_directory = tempfile::tempdir().unwrap();
    let source = write_source(&source_directory, "application.tar", &archive);
    let source_link = source_directory.path().join("application-link.tar");
    symlink(&source, &source_link).unwrap();
    let slots = tempfile::tempdir().unwrap();
    let stager = ApplicationSlotStager::open(slots.path()).unwrap();
    assert!(matches!(
        stager.stage(&source_link, &trust),
        Err(ApplicationStageError::UnsafeSource)
    ));

    fs::set_permissions(&source, Permissions::from_mode(0o620)).unwrap();
    assert!(matches!(
        stager.stage(&source, &trust),
        Err(ApplicationStageError::UnsafeSource)
    ));
    assert!(root_entries(slots.path()).is_empty());

    let unsafe_root = tempfile::tempdir().unwrap();
    fs::set_permissions(unsafe_root.path(), Permissions::from_mode(0o770)).unwrap();
    assert!(matches!(
        ApplicationSlotStager::open(unsafe_root.path()),
        Err(ApplicationStageError::UnsafeRoot)
    ));
    let root_parent = tempfile::tempdir().unwrap();
    let real_root = root_parent.path().join("real");
    fs::create_dir(&real_root).unwrap();
    fs::set_permissions(&real_root, Permissions::from_mode(0o700)).unwrap();
    let linked_root = root_parent.path().join("linked");
    symlink(&real_root, &linked_root).unwrap();
    assert!(matches!(
        ApplicationSlotStager::open(linked_root),
        Err(ApplicationStageError::UnsafeRoot)
    ));
}

#[test]
fn refuses_a_corrupted_existing_slot_instead_of_overwriting_it() {
    let (signing, trust) = keys();
    let payload = payload();
    let archive = bundle(&signing, &manifest(&payload), &payload);
    let source_directory = tempfile::tempdir().unwrap();
    let source = write_source(&source_directory, "application.tar", &archive);
    let slots = tempfile::tempdir().unwrap();
    let stager = ApplicationSlotStager::open(slots.path()).unwrap();
    let staged = stager.stage(&source, &trust).unwrap();

    let server = staged.slot_path.join("bin/bli-kvm-server");
    fs::set_permissions(&server, Permissions::from_mode(0o4555)).unwrap();
    assert!(matches!(
        stager.stage(&source, &trust),
        Err(ApplicationStageError::UnsafeExistingSlot)
    ));
    fs::set_permissions(&server, Permissions::from_mode(0o600)).unwrap();
    fs::write(&server, b"corrupt").unwrap();
    assert!(matches!(
        stager.stage(&source, &trust),
        Err(ApplicationStageError::UnsafeExistingSlot)
    ));
    assert_eq!(fs::read(&server).unwrap(), b"corrupt");
    assert_eq!(root_entries(slots.path()), vec![staged.slot_id]);
}

#[test]
fn lists_and_transactionally_deletes_only_unreferenced_signed_slots() {
    let (signing, trust) = keys();
    let first_payload = payload();
    let first_manifest = manifest(&first_payload);
    let source_directory = tempfile::tempdir().unwrap();
    let first_source = write_source(
        &source_directory,
        "first.tar",
        &bundle(&signing, &first_manifest, &first_payload),
    );
    let mut second_payload = first_payload.clone();
    second_payload.insert(
        "bin/bli-kvm-server",
        (0o755, b"different-signed-server".to_vec()),
    );
    let second_manifest = manifest(&second_payload);
    let second_source = write_source(
        &source_directory,
        "second.tar",
        &bundle(&signing, &second_manifest, &second_payload),
    );
    let slots = tempfile::tempdir().unwrap();
    let stager = ApplicationSlotStager::open(slots.path()).unwrap();
    let first = stager.stage(first_source, &trust).unwrap();
    let second = stager.stage(second_source, &trust).unwrap();
    assert_eq!(stager.list_slots(&trust).unwrap().len(), 2);

    let protected = BTreeSet::from([first.slot_id.clone()]);
    assert!(matches!(
        stager.delete_unreferenced_slot(&first.slot_id, &protected, &trust),
        Err(ApplicationStageError::ProtectedSlot)
    ));
    assert_eq!(
        stager
            .delete_unreferenced_slot(&second.slot_id, &protected, &trust)
            .unwrap()
            .slot_id,
        second.slot_id
    );
    assert!(!second.slot_path.exists());
    assert_eq!(stager.list_slots(&trust).unwrap().len(), 1);
    assert!(!fs::read_dir(slots.path()).unwrap().any(|entry| {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        name.starts_with(".delete-") || name.starts_with(".deleting-")
    }));
}

#[test]
fn recovers_marker_before_rename_and_rejects_unmarked_tombstones() {
    let (signing, trust) = keys();
    let payload = payload();
    let manifest = manifest(&payload);
    let source_directory = tempfile::tempdir().unwrap();
    let source = write_source(
        &source_directory,
        "application.tar",
        &bundle(&signing, &manifest, &payload),
    );
    let slots = tempfile::tempdir().unwrap();
    let stager = ApplicationSlotStager::open(slots.path()).unwrap();
    let staged = stager.stage(source, &trust).unwrap();
    let marker = slots
        .path()
        .join(format!(".delete-{}.json", staged.slot_id));
    fs::write(
        &marker,
        serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 1,
            "purpose": "hardened-blikvm-slot-deletion",
            "slotId": staged.slot_id,
            "manifestSha256": staged.manifest_sha256,
        }))
        .unwrap(),
    )
    .unwrap();
    fs::set_permissions(&marker, Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        stager
            .recover_slot_deletions(&BTreeSet::new(), &trust)
            .unwrap()
            .completed,
        1
    );
    assert!(!staged.slot_path.exists());
    assert!(!marker.exists());

    let orphan = slots.path().join(format!(".deleting-{}", staged.slot_id));
    fs::write(
        &marker,
        serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 1,
            "purpose": "hardened-blikvm-slot-deletion",
            "slotId": staged.slot_id,
            "manifestSha256": staged.manifest_sha256,
        }))
        .unwrap(),
    )
    .unwrap();
    fs::set_permissions(&marker, Permissions::from_mode(0o600)).unwrap();
    fs::create_dir(&orphan).unwrap();
    fs::create_dir(orphan.join("share")).unwrap();
    assert_eq!(
        stager
            .recover_slot_deletions(&BTreeSet::new(), &trust)
            .unwrap()
            .completed,
        1
    );
    assert!(!marker.exists());
    assert!(!orphan.exists());

    fs::create_dir(&orphan).unwrap();
    assert!(matches!(
        stager.recover_slot_deletions(&BTreeSet::new(), &trust),
        Err(ApplicationStageError::UnsafeDeletionState)
    ));
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut value = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut value, "{byte:02x}").unwrap();
    }
    value
}
