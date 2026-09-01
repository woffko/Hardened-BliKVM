use std::{
    ffi::{CString, OsStr},
    fs::{self, Permissions},
    os::unix::{
        ffi::OsStrExt,
        fs::{PermissionsExt, symlink},
    },
    path::{Path, PathBuf},
};

use bli_kvm_diagnostic_journal::{
    DiagnosticJournal, DiagnosticJournalKey, DiagnosticJournalKeyRing, DiagnosticJournalNonce,
    plan_initialize, select_copies,
};
use nix::unistd::{getegid, geteuid};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::*;

static ACL_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
static FILESYSTEM_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serialize_filesystem_test() -> std::sync::MutexGuard<'static, ()> {
    FILESYSTEM_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

fn ids() -> (u32, u32) {
    (geteuid().as_raw(), getegid().as_raw())
}

fn credential(
    active_id: u32,
    active_value: u8,
    previous: Option<(u32, u8)>,
) -> [u8; DIAGNOSTIC_KEY_CREDENTIAL_BYTES] {
    let mut bytes = [0_u8; DIAGNOSTIC_KEY_CREDENTIAL_BYTES];
    bytes[MAGIC_RANGE].copy_from_slice(CREDENTIAL_MAGIC);
    bytes[VERSION_OFFSET] = CREDENTIAL_VERSION;
    bytes[ALGORITHM_OFFSET] = CREDENTIAL_ALGORITHM_XCHACHA20_POLY1305;
    bytes[ACTIVE_ID_RANGE].copy_from_slice(&active_id.to_le_bytes());
    bytes[ACTIVE_KEY_RANGE].fill(active_value);
    if let Some((previous_id, previous_value)) = previous {
        bytes[FLAGS_OFFSET] = FLAG_PREVIOUS_KEY;
        bytes[PREVIOUS_ID_RANGE].copy_from_slice(&previous_id.to_le_bytes());
        bytes[PREVIOUS_KEY_RANGE].fill(previous_value);
    }
    bytes
}

fn root() -> (TempDir, PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("credentials");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, Permissions::from_mode(DIRECTORY_MODE_OWNER_WRITABLE)).unwrap();
    (temporary, root)
}

fn credential_path(root: &Path) -> PathBuf {
    root.join(DIAGNOSTIC_KEY_CREDENTIAL_NAME)
}

fn write_credential(root: &Path, bytes: &[u8]) {
    let path = credential_path(root);
    fs::write(&path, bytes).unwrap();
    fs::set_permissions(path, Permissions::from_mode(CREDENTIAL_FILE_MODE)).unwrap();
}

fn load_test(root: &Path) -> Result<LoadedDiagnosticJournalKeys, DiagnosticKeyCredentialError> {
    let (owner, group) = ids();
    load_from_directory_with_hook(root, owner, group, || {})
}

fn assert_invalid_format(mut bytes: [u8; DIAGNOSTIC_KEY_CREDENTIAL_BYTES]) {
    assert_eq!(
        decode_credential_in_place(&mut bytes).err(),
        Some(DiagnosticKeyCredentialError::InvalidCredentialFormat)
    );
    assert!(bytes.iter().all(|byte| *byte == 0));
}

fn set_zero_named_user_acl(
    path: &Path,
    owner_permissions: u16,
) -> std::sync::MutexGuard<'static, ()> {
    let guard = ACL_TEST_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());

    fn push_entry(bytes: &mut Vec<u8>, tag: u16, permissions: u16, id: u32) {
        bytes.extend_from_slice(&tag.to_le_bytes());
        bytes.extend_from_slice(&permissions.to_le_bytes());
        bytes.extend_from_slice(&id.to_le_bytes());
    }

    let (owner, _) = ids();
    let mut acl = Vec::with_capacity(44);
    acl.extend_from_slice(&2_u32.to_le_bytes());
    push_entry(&mut acl, 0x01, owner_permissions, u32::MAX);
    push_entry(&mut acl, 0x02, 0, owner.saturating_add(1));
    push_entry(&mut acl, 0x04, 0, u32::MAX);
    push_entry(&mut acl, 0x10, 0, u32::MAX);
    push_entry(&mut acl, 0x20, 0, u32::MAX);
    let path = CString::new(path.as_os_str().as_bytes()).unwrap();
    let result = unsafe {
        // SAFETY: path/name are NUL-terminated and acl is a complete Linux
        // POSIX ACL xattr representation for the duration of this call.
        libc::setxattr(
            path.as_ptr(),
            c"system.posix_acl_access".as_ptr(),
            acl.as_ptr().cast(),
            acl.len(),
            0,
        )
    };
    assert_eq!(
        result,
        0,
        "failed to install test POSIX ACL: {}",
        std::io::Error::last_os_error()
    );
    guard
}

#[test]
fn production_contract_is_fixed_and_accepts_only_systemd_unit_directories() {
    assert_eq!(DIAGNOSTIC_KEY_CREDENTIAL_NAME, "diagnostic-journal-keyring");
    assert_eq!(DIAGNOSTIC_KEY_CREDENTIAL_BYTES, 96);
    assert_eq!(CREDENTIAL_MAGIC, b"BLIDKEY\0");
    assert_eq!(CREDENTIAL_VERSION, 1);
    assert_eq!(CREDENTIAL_ALGORITHM_XCHACHA20_POLY1305, 1);
    assert!(allowed_systemd_directory(Path::new(
        PLATFORM_BROKER_CREDENTIAL_DIRECTORY
    )));
    assert!(allowed_systemd_directory(Path::new(
        MAINTENANCE_CREDENTIAL_DIRECTORY
    )));
    assert!(!allowed_systemd_directory(Path::new(
        "/run/credentials/unrelated.service"
    )));
    assert!(!allowed_systemd_directory(Path::new(
        "/etc/hardened-blikvm"
    )));
}

#[test]
fn credential_format_vector_is_stable() {
    let bytes = credential(0x0102_0304, 0x11, Some((0x0a0b_0c0d, 0x22)));
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    assert_eq!(
        digest,
        [
            0x10, 0x96, 0x94, 0x76, 0x1f, 0x8f, 0x66, 0x81, 0x00, 0x42, 0x99, 0x80, 0x06, 0x82,
            0xad, 0x08, 0x8f, 0x3e, 0xea, 0xcb, 0x14, 0x72, 0xd2, 0xa4, 0x93, 0xe1, 0xfc, 0xd0,
            0xbb, 0xd5, 0xba, 0x29,
        ]
    );
}

#[test]
fn active_only_credential_loads_and_raw_buffer_is_zeroized() {
    let _filesystem_guard = serialize_filesystem_test();
    let mut raw = credential(7, 0x31, None);
    let loaded = decode_credential_in_place(&mut raw).unwrap();
    assert!(raw.iter().all(|byte| *byte == 0));
    assert_eq!(loaded.active_key_id(), 7);
    assert_eq!(loaded.previous_key_id(), None);

    let ring = loaded.key_ring();
    let nonce = DiagnosticJournalNonce::new([1; 24]).unwrap();
    let plan = plan_initialize(&DiagnosticJournal::empty(), &ring, nonce).unwrap();
    let expected = DiagnosticJournalKey::new(7, [0x31; 32]).unwrap();
    let expected_ring = DiagnosticJournalKeyRing::new(&expected, None).unwrap();
    let selected =
        select_copies(Some(plan.bytes()), Some(plan.bytes()), &expected_ring, 1).unwrap();
    assert_eq!(selected.generation(), 1);

    let (_temporary, root) = root();
    write_credential(&root, &credential(7, 0x31, None));
    fs::set_permissions(&root, Permissions::from_mode(DIRECTORY_MODE_READ_ONLY)).unwrap();
    let loaded = load_test(&root).unwrap();
    assert_eq!(loaded.active_key_id(), 7);
}

#[test]
fn previous_key_rotation_is_typed_and_decrypts_the_old_generation() {
    let _filesystem_guard = serialize_filesystem_test();
    let (_temporary, root) = root();
    write_credential(&root, &credential(9, 0x39, Some((8, 0x28))));
    let loaded = load_test(&root).unwrap();
    assert_eq!(loaded.active_key_id(), 9);
    assert_eq!(loaded.previous_key_id(), Some(8));

    let old = DiagnosticJournalKey::new(8, [0x28; 32]).unwrap();
    let old_ring = DiagnosticJournalKeyRing::new(&old, None).unwrap();
    let old_copy = plan_initialize(
        &DiagnosticJournal::empty(),
        &old_ring,
        DiagnosticJournalNonce::new([2; 24]).unwrap(),
    )
    .unwrap();
    let selected = select_copies(
        Some(old_copy.bytes()),
        Some(old_copy.bytes()),
        &loaded.key_ring(),
        1,
    )
    .unwrap();
    assert_eq!(selected.generation(), 1);
}

#[test]
fn format_rejects_every_noncanonical_header_and_key_relationship() {
    let mut invalid = credential(1, 1, None);
    invalid[0] ^= 1;
    assert_invalid_format(invalid);

    let mut invalid = credential(1, 1, None);
    invalid[VERSION_OFFSET] = 2;
    assert_invalid_format(invalid);

    let mut invalid = credential(1, 1, None);
    invalid[ALGORITHM_OFFSET] = 2;
    assert_invalid_format(invalid);

    let mut invalid = credential(1, 1, None);
    invalid[FLAGS_OFFSET] = 2;
    assert_invalid_format(invalid);

    let mut invalid = credential(1, 1, None);
    invalid[RESERVED_BYTE_OFFSET] = 1;
    assert_invalid_format(invalid);

    let mut invalid = credential(1, 1, None);
    invalid[RESERVED_RANGE.start] = 1;
    assert_invalid_format(invalid);

    assert_invalid_format(credential(0, 1, None));
    assert_invalid_format(credential(1, 0, None));

    let mut invalid = credential(1, 1, None);
    invalid[PREVIOUS_ID_RANGE].copy_from_slice(&2_u32.to_le_bytes());
    assert_invalid_format(invalid);

    let mut invalid = credential(1, 1, None);
    invalid[PREVIOUS_KEY_RANGE].fill(2);
    assert_invalid_format(invalid);

    assert_invalid_format(credential(1, 1, Some((0, 2))));
    assert_invalid_format(credential(1, 1, Some((1, 2))));
    assert_invalid_format(credential(1, 1, Some((2, 0))));
    assert_invalid_format(credential(1, 1, Some((2, 1))));
}

#[test]
fn directory_path_identity_mode_and_acl_fail_closed() {
    let _filesystem_guard = serialize_filesystem_test();
    let (owner, group) = ids();
    let (_temporary, root_path) = root();
    write_credential(&root_path, &credential(1, 1, None));
    assert!(load_from_directory_with_hook(Path::new("relative"), owner, group, || {}).is_err());
    assert!(load_from_directory_with_hook(&root_path, 0, group, || {}).is_err());
    assert!(
        load_from_directory_with_hook(&root_path, owner.wrapping_add(1), group, || {}).is_err()
    );

    fs::set_permissions(&root_path, Permissions::from_mode(0o750)).unwrap();
    assert_eq!(
        load_test(&root_path).err(),
        Some(DiagnosticKeyCredentialError::InvalidCredentialDirectory)
    );

    let (temporary, target) = root();
    write_credential(&target, &credential(1, 1, None));
    let link = temporary.path().join("linked-credentials");
    symlink(&target, &link).unwrap();
    assert_eq!(
        load_test(&link).err(),
        Some(DiagnosticKeyCredentialError::InvalidCredentialDirectory)
    );

    let (_temporary, acl_root) = root();
    write_credential(&acl_root, &credential(1, 1, None));
    let _acl_guard = set_zero_named_user_acl(&acl_root, 0x07);
    assert_eq!(
        load_test(&acl_root).err(),
        Some(DiagnosticKeyCredentialError::InvalidCredentialDirectory)
    );
}

#[test]
fn unsafe_file_type_mode_link_length_and_acl_are_rejected() {
    let _filesystem_guard = serialize_filesystem_test();
    let bytes = credential(1, 1, None);

    let (_temporary, mode_root) = root();
    write_credential(&mode_root, &bytes);
    fs::set_permissions(credential_path(&mode_root), Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        load_test(&mode_root).err(),
        Some(DiagnosticKeyCredentialError::InvalidCredentialFile)
    );

    let (temporary, symlink_root) = root();
    let external = temporary.path().join("external");
    fs::write(&external, bytes).unwrap();
    fs::set_permissions(&external, Permissions::from_mode(CREDENTIAL_FILE_MODE)).unwrap();
    symlink(&external, credential_path(&symlink_root)).unwrap();
    assert_eq!(
        load_test(&symlink_root).err(),
        Some(DiagnosticKeyCredentialError::InvalidCredentialFile)
    );

    let (temporary, link_root) = root();
    write_credential(&link_root, &bytes);
    fs::hard_link(
        credential_path(&link_root),
        temporary.path().join("second-link"),
    )
    .unwrap();
    assert_eq!(
        load_test(&link_root).err(),
        Some(DiagnosticKeyCredentialError::InvalidCredentialFile)
    );

    let (_temporary, directory_root) = root();
    fs::create_dir(credential_path(&directory_root)).unwrap();
    assert_eq!(
        load_test(&directory_root).err(),
        Some(DiagnosticKeyCredentialError::InvalidCredentialFile)
    );

    let (_temporary, fifo_root) = root();
    let fifo = credential_path(&fifo_root);
    let fifo_name = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    let result = unsafe {
        // SAFETY: fifo_name is NUL-terminated and the mode is valid.
        libc::mkfifo(fifo_name.as_ptr(), CREDENTIAL_FILE_MODE as libc::mode_t)
    };
    assert_eq!(result, 0);
    assert_eq!(
        load_test(&fifo_root).err(),
        Some(DiagnosticKeyCredentialError::InvalidCredentialFile)
    );

    for length in [
        DIAGNOSTIC_KEY_CREDENTIAL_BYTES - 1,
        DIAGNOSTIC_KEY_CREDENTIAL_BYTES + 1,
    ] {
        let (_temporary, length_root) = root();
        write_credential(&length_root, &vec![1; length]);
        assert_eq!(
            load_test(&length_root).err(),
            Some(DiagnosticKeyCredentialError::InvalidCredentialFile)
        );
    }

    let (_temporary, acl_root) = root();
    write_credential(&acl_root, &bytes);
    let _acl_guard = set_zero_named_user_acl(&credential_path(&acl_root), 0x04);
    assert_eq!(
        load_test(&acl_root).err(),
        Some(DiagnosticKeyCredentialError::InvalidCredentialFile)
    );
}

#[test]
fn inode_replacement_and_same_inode_rewrite_are_concurrent_changes() {
    let _filesystem_guard = serialize_filesystem_test();
    let (owner, group) = ids();
    let (temporary, replacement_root) = root();
    write_credential(&replacement_root, &credential(1, 1, None));
    let replacement = temporary.path().join("replacement");
    fs::write(&replacement, credential(2, 2, None)).unwrap();
    fs::set_permissions(&replacement, Permissions::from_mode(CREDENTIAL_FILE_MODE)).unwrap();
    assert_eq!(
        load_from_directory_with_hook(&replacement_root, owner, group, || {
            fs::rename(&replacement, credential_path(&replacement_root)).unwrap();
        })
        .err(),
        Some(DiagnosticKeyCredentialError::ConcurrentCredentialChange)
    );

    let (_temporary, rewrite_root) = root();
    write_credential(&rewrite_root, &credential(1, 1, None));
    assert_eq!(
        load_from_directory_with_hook(&rewrite_root, owner, group, || {
            let path = credential_path(&rewrite_root);
            fs::set_permissions(&path, Permissions::from_mode(0o600)).unwrap();
            fs::write(&path, credential(2, 2, None)).unwrap();
            fs::set_permissions(path, Permissions::from_mode(CREDENTIAL_FILE_MODE)).unwrap();
        })
        .err(),
        Some(DiagnosticKeyCredentialError::ConcurrentCredentialChange)
    );
}

#[test]
fn errors_are_fixed_and_never_include_paths_or_key_material() {
    let errors = [
        DiagnosticKeyCredentialError::MissingCredentialDirectory,
        DiagnosticKeyCredentialError::InvalidCredentialDirectory,
        DiagnosticKeyCredentialError::IdentityUnavailable,
        DiagnosticKeyCredentialError::WrongProcessIdentity,
        DiagnosticKeyCredentialError::InvalidCredentialFile,
        DiagnosticKeyCredentialError::ConcurrentCredentialChange,
        DiagnosticKeyCredentialError::InvalidCredentialFormat,
    ];
    for error in errors {
        let display = error.to_string();
        let debug = format!("{error:?}");
        for forbidden in [
            "/tmp/",
            "/run/credentials/",
            DIAGNOSTIC_KEY_CREDENTIAL_NAME,
            "31313131",
            "key_id=",
        ] {
            assert!(!display.contains(forbidden));
            assert!(!debug.contains(forbidden));
        }
    }
}

#[test]
fn credential_name_is_one_normal_component() {
    assert_eq!(
        Path::new(DIAGNOSTIC_KEY_CREDENTIAL_NAME)
            .components()
            .count(),
        1
    );
    assert_eq!(
        Path::new(DIAGNOSTIC_KEY_CREDENTIAL_NAME).file_name(),
        Some(OsStr::new(DIAGNOSTIC_KEY_CREDENTIAL_NAME))
    );
}
