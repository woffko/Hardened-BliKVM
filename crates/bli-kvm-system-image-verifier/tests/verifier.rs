use std::{
    fs::{self, Permissions},
    io::Cursor,
    os::unix::fs::{PermissionsExt, symlink},
    path::Path,
};

use bli_kvm_system_image_verifier::{
    MAX_ROOTFS_IMAGE_BYTES, MAX_SYSTEM_ARCHIVE_BYTES, SYSTEM_ARCHITECTURE, SYSTEM_FORMAT_V1,
    SYSTEM_PRODUCT, SYSTEM_TARGET, SystemComponent, SystemComponentRole, SystemContentType,
    SystemManifest, SystemTrustRoot, SystemVerificationPolicy, SystemVerifyError,
    verify_system_bytes, verify_system_file, verify_system_reader,
};
use ed25519_dalek::{Signer, SigningKey};
use sha2::{Digest, Sha256};
use tar::{Builder, EntryType, Header};

const KEY_ID: &str = "sys-test-key-1";
const LAYOUT_ID: &str = "layout-h616-test-v1";
const SOURCE_COMMIT: &str = "1234567890abcdef1234567890abcdef12345678";
const ROOTFS_BYTES: usize = 2048;
const BOOT_BYTES: usize = 512;

#[derive(Clone)]
struct EntrySpec {
    path: &'static str,
    entry_type: EntryType,
    mode: u32,
    bytes: Vec<u8>,
    link_name: Option<&'static str>,
}

fn signing() -> SigningKey {
    SigningKey::from_bytes(&[61; 32])
}

fn trust(signing: &SigningKey) -> SystemTrustRoot {
    SystemTrustRoot::new(KEY_ID, signing.verifying_key().to_bytes()).unwrap()
}

fn policy(with_boot: bool) -> SystemVerificationPolicy {
    SystemVerificationPolicy::new(
        LAYOUT_ID,
        3,
        4,
        ROOTFS_BYTES as u64,
        with_boot.then_some(BOOT_BYTES as u64),
    )
    .unwrap()
}

fn rootfs(fill: u8) -> Vec<u8> {
    let mut bytes = vec![fill; ROOTFS_BYTES];
    bytes[1080] = 0x53;
    bytes[1081] = 0xef;
    bytes
}

fn boot(fill: u8) -> Vec<u8> {
    let mut bytes = vec![fill; BOOT_BYTES];
    bytes[510] = 0x55;
    bytes[511] = 0xaa;
    bytes
}

fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").unwrap();
    }
    output
}

fn manifest(rootfs: &[u8], boot: Option<&[u8]>) -> SystemManifest {
    let mut components = Vec::new();
    if let Some(boot) = boot {
        components.push(SystemComponent {
            role: SystemComponentRole::Boot,
            path: "boot.img".into(),
            content_type: SystemContentType::Vfat,
            size: boot.len() as u64,
            sha256: sha256(boot),
            image_size: None,
            image_sha256: None,
        });
    }
    components.push(SystemComponent {
        role: SystemComponentRole::Rootfs,
        path: "rootfs.img".into(),
        content_type: SystemContentType::Ext4,
        size: rootfs.len() as u64,
        sha256: sha256(rootfs),
        image_size: None,
        image_sha256: None,
    });
    SystemManifest {
        format: SYSTEM_FORMAT_V1.into(),
        product: SYSTEM_PRODUCT.into(),
        target: SYSTEM_TARGET.into(),
        architecture: SYSTEM_ARCHITECTURE.into(),
        key_id: KEY_ID.into(),
        version: "1.0.0".into(),
        sequence: 7,
        minimum_recovery_sequence: 2,
        minimum_boot_control_sequence: 3,
        layout_id: LAYOUT_ID.into(),
        source_commit: SOURCE_COMMIT.into(),
        components,
    }
}

fn normal_entries(rootfs: &[u8], boot: Option<&[u8]>) -> Vec<EntrySpec> {
    let mut entries = vec![EntrySpec {
        path: "payload/",
        entry_type: EntryType::Directory,
        mode: 0o755,
        bytes: Vec::new(),
        link_name: None,
    }];
    if let Some(boot) = boot {
        entries.push(EntrySpec {
            path: "payload/boot.img",
            entry_type: EntryType::Regular,
            mode: 0o644,
            bytes: boot.to_vec(),
            link_name: None,
        });
    }
    entries.push(EntrySpec {
        path: "payload/rootfs.img",
        entry_type: EntryType::Regular,
        mode: 0o644,
        bytes: rootfs.to_vec(),
        link_name: None,
    });
    entries
}

fn archive(manifest_bytes: &[u8], signature: &[u8; 64], entries: &[EntrySpec]) -> Vec<u8> {
    let mut builder = Builder::new(Vec::new());
    append(
        &mut builder,
        EntrySpec {
            path: "manifest.json",
            entry_type: EntryType::Regular,
            mode: 0o644,
            bytes: manifest_bytes.to_vec(),
            link_name: None,
        },
    );
    append(
        &mut builder,
        EntrySpec {
            path: "manifest.sig",
            entry_type: EntryType::Regular,
            mode: 0o644,
            bytes: signature.to_vec(),
            link_name: None,
        },
    );
    for entry in entries {
        append(&mut builder, entry.clone());
    }
    builder.into_inner().unwrap()
}

fn append(builder: &mut Builder<Vec<u8>>, entry: EntrySpec) {
    let mut header = Header::new_ustar();
    header.set_entry_type(entry.entry_type);
    header.set_mode(entry.mode);
    header.set_uid(0);
    header.set_gid(0);
    header.set_mtime(0);
    header.set_size(entry.bytes.len() as u64);
    if let Some(link_name) = entry.link_name {
        header.set_link_name(link_name).unwrap();
    }
    header.set_cksum();
    builder
        .append_data(&mut header, entry.path, Cursor::new(entry.bytes))
        .unwrap();
}

fn signed_archive(
    signing: &SigningKey,
    manifest: &SystemManifest,
    rootfs: &[u8],
    boot: Option<&[u8]>,
) -> Vec<u8> {
    let manifest_bytes = serde_json::to_vec(manifest).unwrap();
    let signature = signing.sign(&manifest_bytes).to_bytes();
    archive(&manifest_bytes, &signature, &normal_entries(rootfs, boot))
}

#[test]
fn verifies_exact_rootfs_only_and_rootfs_boot_artifacts() {
    let signing = signing();
    let trust = trust(&signing);
    let root = rootfs(0x11);
    let root_manifest = manifest(&root, None);
    let verified = verify_system_bytes(
        &signed_archive(&signing, &root_manifest, &root, None),
        &trust,
        &policy(false),
    )
    .unwrap();
    assert_eq!(verified.manifest, root_manifest);
    assert_eq!(verified.payload_bytes, ROOTFS_BYTES as u64);
    assert_eq!(verified.manifest_sha256.len(), 64);

    let boot = boot(0x22);
    let combined_manifest = manifest(&root, Some(&boot));
    let verified = verify_system_bytes(
        &signed_archive(&signing, &combined_manifest, &root, Some(&boot)),
        &trust,
        &policy(true),
    )
    .unwrap();
    assert_eq!(verified.manifest, combined_manifest);
    assert_eq!(verified.payload_bytes, (ROOTFS_BYTES + BOOT_BYTES) as u64);
}

#[test]
fn system_trust_domain_rejects_application_keys() {
    let signing = signing();
    assert!(matches!(
        SystemTrustRoot::new("app-test-key", signing.verifying_key().to_bytes()),
        Err(SystemVerifyError::InvalidTrustRoot)
    ));
}

#[test]
fn rejects_invalid_or_mismatched_layout_policy() {
    assert!(matches!(
        SystemVerificationPolicy::new("h616", 1, 1, ROOTFS_BYTES as u64, None),
        Err(SystemVerifyError::InvalidPolicy)
    ));
    assert!(matches!(
        SystemVerificationPolicy::new(LAYOUT_ID, 0, 1, ROOTFS_BYTES as u64, None),
        Err(SystemVerifyError::InvalidPolicy)
    ));
    assert!(matches!(
        SystemVerificationPolicy::new(LAYOUT_ID, 1, 1, MAX_ROOTFS_IMAGE_BYTES + 1, None),
        Err(SystemVerifyError::InvalidPolicy)
    ));

    let signing = signing();
    let trust = trust(&signing);
    let root = rootfs(0x33);
    let mut wrong_layout = manifest(&root, None);
    wrong_layout.layout_id = "layout-other-v1".into();
    assert_eq!(
        verify_system_bytes(
            &signed_archive(&signing, &wrong_layout, &root, None),
            &trust,
            &policy(false),
        ),
        Err(SystemVerifyError::PolicyMismatch)
    );

    let mut new_recovery = manifest(&root, None);
    new_recovery.minimum_recovery_sequence = 4;
    assert_eq!(
        verify_system_bytes(
            &signed_archive(&signing, &new_recovery, &root, None),
            &trust,
            &policy(false),
        ),
        Err(SystemVerifyError::PolicyMismatch)
    );

    let root_only = manifest(&root, None);
    assert_eq!(
        verify_system_bytes(
            &signed_archive(&signing, &root_only, &root, None),
            &trust,
            &policy(true),
        ),
        Err(SystemVerifyError::PolicyMismatch)
    );

    let boot = boot(0x31);
    let with_boot = manifest(&root, Some(&boot));
    assert_eq!(
        verify_system_bytes(
            &signed_archive(&signing, &with_boot, &root, Some(&boot)),
            &trust,
            &policy(false),
        ),
        Err(SystemVerifyError::PolicyMismatch)
    );
}

#[test]
fn rejects_invalid_manifest_shape_before_payload_processing() {
    let signing = signing();
    let trust = trust(&signing);
    let root = rootfs(0x39);

    let mut zero_sequence = manifest(&root, None);
    zero_sequence.sequence = 0;
    assert_eq!(
        verify_system_bytes(
            &signed_archive(&signing, &zero_sequence, &root, None),
            &trust,
            &policy(false),
        ),
        Err(SystemVerifyError::InvalidManifestShape)
    );

    let mut device_path = manifest(&root, None);
    device_path.components[0].path = "/dev/mmcblk0".into();
    assert_eq!(
        verify_system_bytes(
            &signed_archive(&signing, &device_path, &root, None),
            &trust,
            &policy(false),
        ),
        Err(SystemVerifyError::InvalidManifestShape)
    );
}

#[test]
fn rejects_wrong_signature_noncanonical_manifest_and_unknown_fields() {
    let signing = signing();
    let trust = trust(&signing);
    let root = rootfs(0x44);
    let manifest = manifest(&root, None);
    let canonical = serde_json::to_vec(&manifest).unwrap();

    let wrong = SigningKey::from_bytes(&[62; 32]);
    let wrong_signature = wrong.sign(&canonical).to_bytes();
    assert_eq!(
        verify_system_bytes(
            &archive(&canonical, &wrong_signature, &normal_entries(&root, None)),
            &trust,
            &policy(false),
        ),
        Err(SystemVerifyError::InvalidSignature)
    );

    let mut noncanonical = vec![b' '];
    noncanonical.extend_from_slice(&canonical);
    let signature = signing.sign(&noncanonical).to_bytes();
    assert_eq!(
        verify_system_bytes(
            &archive(&noncanonical, &signature, &normal_entries(&root, None)),
            &trust,
            &policy(false),
        ),
        Err(SystemVerifyError::InvalidManifest)
    );

    let mut unknown = serde_json::to_value(&manifest).unwrap();
    unknown
        .as_object_mut()
        .unwrap()
        .insert("device".into(), serde_json::json!("mmcblk0"));
    let unknown = serde_json::to_vec(&unknown).unwrap();
    let signature = signing.sign(&unknown).to_bytes();
    assert_eq!(
        verify_system_bytes(
            &archive(&unknown, &signature, &normal_entries(&root, None)),
            &trust,
            &policy(false),
        ),
        Err(SystemVerifyError::InvalidManifest)
    );
}

#[test]
fn rejects_payload_tamper_wrong_filesystem_and_unexpected_entries() {
    let signing = signing();
    let trust = trust(&signing);
    let root = rootfs(0x55);
    let expected_manifest = manifest(&root, None);
    let manifest_bytes = serde_json::to_vec(&expected_manifest).unwrap();
    let signature = signing.sign(&manifest_bytes).to_bytes();

    let mut tampered = root.clone();
    tampered[1500] ^= 1;
    assert_eq!(
        verify_system_bytes(
            &archive(
                &manifest_bytes,
                &signature,
                &normal_entries(&tampered, None),
            ),
            &trust,
            &policy(false),
        ),
        Err(SystemVerifyError::PayloadMismatch)
    );

    let invalid_rootfs = vec![0x66; ROOTFS_BYTES];
    let invalid_manifest = manifest(&invalid_rootfs, None);
    assert_eq!(
        verify_system_bytes(
            &signed_archive(&signing, &invalid_manifest, &invalid_rootfs, None,),
            &trust,
            &policy(false),
        ),
        Err(SystemVerifyError::InvalidFilesystemImage)
    );

    let mut extra = normal_entries(&root, None);
    extra.push(EntrySpec {
        path: "payload/bootloader.bin",
        entry_type: EntryType::Regular,
        mode: 0o644,
        bytes: vec![0; 512],
        link_name: None,
    });
    assert_eq!(
        verify_system_bytes(
            &archive(&manifest_bytes, &signature, &extra),
            &trust,
            &policy(false),
        ),
        Err(SystemVerifyError::InvalidEntryPath)
    );
}

#[test]
fn rejects_wrong_entry_order_metadata_and_type() {
    let signing = signing();
    let trust = trust(&signing);
    let root = rootfs(0x77);
    let manifest = manifest(&root, None);
    let manifest_bytes = serde_json::to_vec(&manifest).unwrap();
    let signature = signing.sign(&manifest_bytes).to_bytes();

    let mut wrong_order = normal_entries(&root, None);
    wrong_order.swap(0, 1);
    assert_eq!(
        verify_system_bytes(
            &archive(&manifest_bytes, &signature, &wrong_order),
            &trust,
            &policy(false),
        ),
        Err(SystemVerifyError::InvalidEntryPath)
    );

    let mut wrong_mode = normal_entries(&root, None);
    wrong_mode[1].mode = 0o600;
    assert_eq!(
        verify_system_bytes(
            &archive(&manifest_bytes, &signature, &wrong_mode),
            &trust,
            &policy(false),
        ),
        Err(SystemVerifyError::InvalidEntryMetadata)
    );

    let mut link = normal_entries(&root, None);
    link[1].entry_type = EntryType::Symlink;
    link[1].bytes.clear();
    link[1].link_name = Some("/dev/mmcblk0");
    assert_eq!(
        verify_system_bytes(
            &archive(&manifest_bytes, &signature, &link),
            &trust,
            &policy(false),
        ),
        Err(SystemVerifyError::ForbiddenEntryType)
    );
}

#[test]
fn standalone_file_requires_exact_owned_safe_inode() {
    let signing = signing();
    let trust = trust(&signing);
    let root = rootfs(0x88);
    let manifest = manifest(&root, None);
    let bundle = signed_archive(&signing, &manifest, &root, None);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("system.tar");
    fs::write(&path, &bundle).unwrap();
    fs::set_permissions(&path, Permissions::from_mode(0o600)).unwrap();
    verify_system_file(&path, &trust, &policy(false)).unwrap();

    fs::set_permissions(&path, Permissions::from_mode(0o622)).unwrap();
    assert!(matches!(
        verify_system_file(&path, &trust, &policy(false)),
        Err(SystemVerifyError::UnsafeArchiveFile)
    ));
    fs::set_permissions(&path, Permissions::from_mode(0o600)).unwrap();

    let hardlink = directory.path().join("system-hardlink.tar");
    fs::hard_link(&path, &hardlink).unwrap();
    assert!(matches!(
        verify_system_file(&path, &trust, &policy(false)),
        Err(SystemVerifyError::UnsafeArchiveFile)
    ));
    fs::remove_file(hardlink).unwrap();

    let linked = directory.path().join("system-link.tar");
    symlink(&path, &linked).unwrap();
    assert!(matches!(
        verify_system_file(&linked, &trust, &policy(false)),
        Err(SystemVerifyError::UnsafeArchiveFile)
    ));
    assert!(matches!(
        verify_system_file(Path::new("relative.tar"), &trust, &policy(false)),
        Err(SystemVerifyError::InvalidArchivePath)
    ));
}

#[test]
fn declared_archive_bound_is_enforced_before_reading() {
    let signing = signing();
    let trust = trust(&signing);
    assert_eq!(
        verify_system_reader(
            Cursor::new(Vec::<u8>::new()),
            MAX_SYSTEM_ARCHIVE_BYTES + 1,
            &trust,
            &policy(false),
        ),
        Err(SystemVerifyError::ArchiveTooLarge)
    );
}

#[test]
fn rejects_nonzero_or_unbounded_trailing_archive_data() {
    let signing = signing();
    let trust = trust(&signing);
    let root = rootfs(0x99);
    let manifest = manifest(&root, None);
    let mut nonzero = signed_archive(&signing, &manifest, &root, None);
    nonzero.push(1);
    assert_eq!(
        verify_system_bytes(&nonzero, &trust, &policy(false)),
        Err(SystemVerifyError::InvalidArchive)
    );

    let mut excessive = signed_archive(&signing, &manifest, &root, None);
    excessive.extend(vec![0; 64 * 1024 + 1]);
    assert_eq!(
        verify_system_bytes(&excessive, &trust, &policy(false)),
        Err(SystemVerifyError::InvalidArchive)
    );
}

#[test]
fn verifies_a_compressed_rootfs_that_fits_the_partition() {
    let signing = signing();
    let trust = trust(&signing);
    let policy = SystemVerificationPolicy::new(LAYOUT_ID, 3, 4, 8192, None).unwrap();
    let mut compressed = vec![0x28, 0xb5, 0x2f, 0xfd];
    compressed.extend([0x42; 60]);
    let component = |image_size: u64, bytes: &[u8]| SystemComponent {
        role: SystemComponentRole::Rootfs,
        path: "rootfs.img.zst".into(),
        content_type: SystemContentType::Ext4Zstd,
        size: bytes.len() as u64,
        sha256: sha256(bytes),
        image_size: Some(image_size),
        image_sha256: Some(sha256(b"uncompressed")),
    };
    let bundle = |manifest: &SystemManifest, bytes: &[u8]| {
        let manifest_bytes = serde_json::to_vec(manifest).unwrap();
        let signature = signing.sign(&manifest_bytes).to_bytes();
        let entries = vec![
            EntrySpec {
                path: "payload/",
                entry_type: EntryType::Directory,
                mode: 0o755,
                bytes: Vec::new(),
                link_name: None,
            },
            EntrySpec {
                path: "payload/rootfs.img.zst",
                entry_type: EntryType::Regular,
                mode: 0o644,
                bytes: bytes.to_vec(),
                link_name: None,
            },
        ];
        archive(&manifest_bytes, &signature, &entries)
    };
    let mut good = manifest(&rootfs(0), None);
    good.components = vec![component(4096, &compressed)];
    let verified = verify_system_bytes(&bundle(&good, &compressed), &trust, &policy).unwrap();
    assert_eq!(verified.manifest.components[0].image_size, Some(4096));

    // Larger than the partition, or not a multiple of the block size.
    for image_size in [12288, 4097] {
        let mut wrong = good.clone();
        wrong.components = vec![component(image_size, &compressed)];
        assert_eq!(
            verify_system_bytes(&bundle(&wrong, &compressed), &trust, &policy),
            Err(SystemVerifyError::PolicyMismatch)
        );
    }
    // Not zstd.
    let plain = vec![0x42; 64];
    let mut not_zstd = good.clone();
    not_zstd.components = vec![component(4096, &plain)];
    assert_eq!(
        verify_system_bytes(&bundle(&not_zstd, &plain), &trust, &policy),
        Err(SystemVerifyError::InvalidFilesystemImage)
    );
    // The uncompressed digest is required.
    let mut missing = good.clone();
    missing.components[0].image_sha256 = None;
    assert_eq!(
        verify_system_bytes(&bundle(&missing, &compressed), &trust, &policy),
        Err(SystemVerifyError::InvalidManifestShape)
    );
}
