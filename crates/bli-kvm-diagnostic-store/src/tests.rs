use std::{
    ffi::CString,
    fs::{self, Permissions},
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, PermissionsExt, symlink},
    },
    path::{Path, PathBuf},
};

use bli_kvm_diagnostic_export::{
    DiagnosticComponent, DiagnosticEvent, DiagnosticExporter, DiagnosticRecord,
};
use bli_kvm_diagnostic_journal::{
    CollectorObservation, DiagnosticBinding, DiagnosticJournalEntryState, DiagnosticJournalKey,
    DiagnosticJournalKeyRing, DiagnosticSnapshot, ReconciliationAction, plan_reconcile,
};
use nix::unistd::{getegid, geteuid};
use tempfile::TempDir;

use super::*;

static ACL_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn ids() -> (u32, u32) {
    (geteuid().as_raw(), getegid().as_raw())
}

fn root() -> (TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("diagnostics");
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, Permissions::from_mode(ROOT_MODE)).unwrap();
    (directory, path)
}

fn open_test(path: &Path) -> Result<DiagnosticJournalStore, DiagnosticStoreError> {
    let (owner, group) = ids();
    DiagnosticJournalStore::open_at(path.to_path_buf(), owner, group)
}

fn key(id: u32, value: u8) -> DiagnosticJournalKey {
    DiagnosticJournalKey::new(id, [value; 32]).unwrap()
}

fn bundle(offset: u32) -> bli_kvm_diagnostic_export::DiagnosticBundle {
    let mut exporter = DiagnosticExporter::new();
    exporter
        .push(
            DiagnosticRecord::new(
                offset,
                DiagnosticComponent::ControlBroker,
                DiagnosticEvent::Ready,
            )
            .unwrap(),
        )
        .unwrap();
    exporter.export().unwrap()
}

fn snapshot<'a>(
    instance: u64,
    window: u32,
    bundle: &'a bli_kvm_diagnostic_export::DiagnosticBundle,
) -> DiagnosticSnapshot<'a> {
    DiagnosticSnapshot::new(DiagnosticBinding::new(instance, window).unwrap(), 1, bundle).unwrap()
}

fn path_for(root: &Path, index: CopyIndex) -> PathBuf {
    root.join(match index {
        CopyIndex::A => COPY_A_FILE_NAME,
        CopyIndex::B => COPY_B_FILE_NAME,
    })
}

fn write_raw(path: &Path, bytes: &[u8], mode: u32) {
    fs::write(path, bytes).unwrap();
    fs::set_permissions(path, Permissions::from_mode(mode)).unwrap();
}

fn set_zero_named_user_acl(path: &Path, owner_permissions: u16) {
    let _guard = ACL_TEST_LOCK
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
        // POSIX ACL xattr representation for the call duration.
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
}

fn initialize(store: &DiagnosticJournalStore, keys: &DiagnosticJournalKeyRing<'_>) {
    store.inject_nonce([1; 24]);
    assert_eq!(
        store.initialize(keys).unwrap(),
        PersistedDiagnosticJournal { generation: 1 }
    );
}

#[test]
fn production_root_names_and_identity_are_compile_time_fixed() {
    assert_eq!(
        DIAGNOSTIC_STORE_ROOT,
        "/var/lib/hardened-blikvm-diagnostics"
    );
    assert_eq!(COPY_A_FILE_NAME, "journal-a.bin");
    assert_eq!(COPY_B_FILE_NAME, "journal-b.bin");
    assert_eq!(PLATFORM_BROKER_ACCOUNT, "hardened-blikvm-broker");
    assert_eq!(SHARED_GROUP, "hardened-blikvm");
}

#[test]
fn root_must_be_absolute_canonical_nonroot_owned_and_mode_0700() {
    let (owner, group) = ids();
    assert!(matches!(
        DiagnosticJournalStore::open_at(PathBuf::from("relative"), owner, group),
        Err(DiagnosticStoreError::InvalidRoot)
    ));
    let (_directory, path) = root();
    assert!(matches!(
        DiagnosticJournalStore::open_at(path.clone(), 0, group),
        Err(DiagnosticStoreError::WrongProcessIdentity)
    ));
    assert!(matches!(
        DiagnosticJournalStore::open_at(path.clone(), owner.saturating_add(1), group),
        Err(DiagnosticStoreError::UnsafeRoot)
    ));
    assert!(matches!(
        DiagnosticJournalStore::open_at(path.clone(), owner, group.saturating_add(1)),
        Err(DiagnosticStoreError::UnsafeRoot)
    ));
    fs::set_permissions(&path, Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        open_test(&path),
        Err(DiagnosticStoreError::UnsafeRoot)
    ));

    let parent = tempfile::tempdir().unwrap();
    let target = parent.path().join("target");
    let link = parent.path().join("link");
    fs::create_dir(&target).unwrap();
    fs::set_permissions(&target, Permissions::from_mode(ROOT_MODE)).unwrap();
    symlink(&target, &link).unwrap();
    assert!(matches!(
        open_test(&link),
        Err(DiagnosticStoreError::UnsafeRoot)
    ));

    let (_directory, acl_root) = root();
    set_zero_named_user_acl(&acl_root, 0x07);
    assert_eq!(
        fs::symlink_metadata(&acl_root)
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        ROOT_MODE
    );
    assert!(matches!(
        open_test(&acl_root),
        Err(DiagnosticStoreError::UnsafeRoot)
    ));
}

#[test]
fn directory_fd_lock_rejects_a_second_store_instance() {
    let (_directory, path) = root();
    let first = open_test(&path).unwrap();
    assert!(matches!(
        open_test(&path),
        Err(DiagnosticStoreError::StoreBusy)
    ));
    drop(first);
    open_test(&path).unwrap();
}

#[test]
fn initialization_writes_two_exact_encrypted_mode_0600_copies() {
    let (_directory, path) = root();
    let store = open_test(&path).unwrap();
    let primary_key = key(1, 7);
    let keys = DiagnosticJournalKeyRing::new(&primary_key, None).unwrap();
    initialize(&store, &keys);
    let selected = store.load(&keys, 1).unwrap();
    assert!(!selected.degraded());
    assert_eq!(selected.generation(), 1);
    assert_eq!(selected.journal().entry_count(), 0);
    for index in [CopyIndex::A, CopyIndex::B] {
        let metadata = fs::symlink_metadata(path_for(&path, index)).unwrap();
        assert!(metadata.is_file());
        assert_eq!(metadata.permissions().mode() & 0o7777, COPY_MODE);
        assert_eq!(metadata.nlink(), 1);
        assert_eq!(metadata.len(), DIAGNOSTIC_JOURNAL_COPY_BYTES as u64);
    }
    assert_eq!(
        fs::read(path_for(&path, CopyIndex::A)).unwrap(),
        fs::read(path_for(&path, CopyIndex::B)).unwrap()
    );
    assert!(matches!(
        store.initialize(&keys),
        Err(DiagnosticStoreError::AlreadyInitialized)
    ));
}

#[test]
fn successful_update_reselects_exact_mutation_and_rejects_stale_generation() {
    let (_directory, path) = root();
    let store = open_test(&path).unwrap();
    let key = key(1, 7);
    let keys = DiagnosticJournalKeyRing::new(&key, None).unwrap();
    initialize(&store, &keys);
    let selected = store.load(&keys, 1).unwrap();
    let bundle = bundle(1);
    let plan = plan_reconcile(
        &selected,
        CollectorObservation::Sealed(snapshot(9, 4, &bundle)),
    )
    .unwrap();
    assert_eq!(plan.action(), ReconciliationAction::Persist);
    store.inject_nonce([2; 24]);
    assert_eq!(
        store.update(&keys, 1, plan.mutation().unwrap()).unwrap(),
        PersistedDiagnosticJournal { generation: 2 }
    );
    let updated = store.load(&keys, 2).unwrap();
    assert!(!updated.degraded());
    assert_eq!(
        updated
            .journal()
            .entry(DiagnosticBinding::new(9, 4).unwrap())
            .unwrap()
            .state(),
        DiagnosticJournalEntryState::PendingAcknowledgement
    );
    for index in [CopyIndex::A, CopyIndex::B] {
        let encrypted = fs::read(path_for(&path, index)).unwrap();
        assert!(
            !encrypted
                .windows(bundle.as_bytes().len())
                .any(|window| window == bundle.as_bytes())
        );
    }
    assert_eq!(
        fs::read(path_for(&path, CopyIndex::A)).unwrap(),
        fs::read(path_for(&path, CopyIndex::B)).unwrap()
    );
    store.inject_nonce([3; 24]);
    assert_eq!(
        store.update(&keys, 1, plan.mutation().unwrap()),
        Err(DiagnosticStoreError::Journal(
            DiagnosticJournalError::ConcurrentStateChange
        ))
    );
}

#[test]
fn recheck_rejects_same_generation_reencrypted_ciphertext_replacement() {
    let (_directory, path) = root();
    let store = open_test(&path).unwrap();
    let key = key(1, 7);
    let keys = DiagnosticJournalKeyRing::new(&key, None).unwrap();
    initialize(&store, &keys);
    let selected = store.load(&keys, 1).unwrap();
    let alternate = bli_kvm_diagnostic_journal::plan_initialize(
        &DiagnosticJournal::empty(),
        &keys,
        DiagnosticJournalNonce::new([9; 24]).unwrap(),
    )
    .unwrap();
    for index in [CopyIndex::A, CopyIndex::B] {
        write_raw(&path_for(&path, index), alternate.bytes(), COPY_MODE);
    }
    assert_eq!(
        store.recheck_selected(&selected, &keys, 1),
        Err(DiagnosticStoreError::ConcurrentStateChange)
    );
}

#[test]
fn unsafe_copy_types_modes_links_sizes_and_acls_are_rejected() {
    let key = key(1, 7);
    let keys = DiagnosticJournalKeyRing::new(&key, None).unwrap();
    let plan = bli_kvm_diagnostic_journal::plan_initialize(
        &DiagnosticJournal::empty(),
        &keys,
        DiagnosticJournalNonce::new([1; 24]).unwrap(),
    )
    .unwrap();

    let (_directory, symlink_root) = root();
    let external = symlink_root.parent().unwrap().join("external-copy");
    write_raw(&external, plan.bytes(), COPY_MODE);
    symlink(&external, path_for(&symlink_root, CopyIndex::A)).unwrap();
    assert!(matches!(
        open_test(&symlink_root),
        Err(DiagnosticStoreError::UnsafeCopy(CopyIndex::A))
    ));

    let (_directory, mode_root) = root();
    write_raw(&path_for(&mode_root, CopyIndex::A), plan.bytes(), 0o644);
    assert!(matches!(
        open_test(&mode_root),
        Err(DiagnosticStoreError::UnsafeCopy(CopyIndex::A))
    ));

    let (_directory, link_root) = root();
    let copy_a = path_for(&link_root, CopyIndex::A);
    write_raw(&copy_a, plan.bytes(), COPY_MODE);
    fs::hard_link(&copy_a, path_for(&link_root, CopyIndex::B)).unwrap();
    assert!(matches!(
        open_test(&link_root),
        Err(DiagnosticStoreError::UnsafeCopy(CopyIndex::A))
    ));

    let (_directory, directory_root) = root();
    fs::create_dir(path_for(&directory_root, CopyIndex::A)).unwrap();
    assert!(matches!(
        open_test(&directory_root),
        Err(DiagnosticStoreError::UnsafeCopy(CopyIndex::A))
    ));

    let (_directory, fifo_root) = root();
    let fifo = path_for(&fifo_root, CopyIndex::A);
    let fifo_name = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    let result = unsafe {
        // SAFETY: fifo_name is NUL-terminated and mode is valid.
        libc::mkfifo(fifo_name.as_ptr(), COPY_MODE as libc::mode_t)
    };
    assert_eq!(result, 0);
    assert!(matches!(
        open_test(&fifo_root),
        Err(DiagnosticStoreError::UnsafeCopy(CopyIndex::A))
    ));

    let (_directory, oversized_root) = root();
    write_raw(
        &path_for(&oversized_root, CopyIndex::A),
        &vec![0; DIAGNOSTIC_JOURNAL_COPY_BYTES + 1],
        COPY_MODE,
    );
    assert!(matches!(
        open_test(&oversized_root),
        Err(DiagnosticStoreError::UnsafeCopy(CopyIndex::A))
    ));

    let (_directory, acl_copy_root) = root();
    let acl_copy = path_for(&acl_copy_root, CopyIndex::A);
    write_raw(&acl_copy, plan.bytes(), COPY_MODE);
    set_zero_named_user_acl(&acl_copy, 0x06);
    assert!(matches!(
        open_test(&acl_copy_root),
        Err(DiagnosticStoreError::UnsafeCopy(CopyIndex::A))
    ));
}

#[test]
fn unexpected_entries_and_unsafe_temporary_files_fail_closed() {
    let (_directory, unexpected_root) = root();
    write_raw(&unexpected_root.join("unexpected"), b"x", COPY_MODE);
    assert!(matches!(
        open_test(&unexpected_root),
        Err(DiagnosticStoreError::UnexpectedEntry)
    ));

    let (_directory, safe_temp_root) = root();
    write_raw(
        &safe_temp_root.join(".journal-a.tmp"),
        b"partial",
        COPY_MODE,
    );
    let store = open_test(&safe_temp_root).unwrap();
    assert!(safe_temp_root.join(".journal-a.tmp").exists());
    let key = key(1, 7);
    let keys = DiagnosticJournalKeyRing::new(&key, None).unwrap();
    assert_eq!(
        store.load_read_only(&keys, 1).err(),
        Some(DiagnosticStoreError::RecoveryRequired)
    );
    assert!(safe_temp_root.join(".journal-a.tmp").exists());
    store.inject_nonce([1; 24]);
    assert_eq!(
        store.initialize(&keys).unwrap(),
        PersistedDiagnosticJournal { generation: 1 }
    );
    assert!(!safe_temp_root.join(".journal-a.tmp").exists());
    drop(store);

    let (_directory, unsafe_temp_root) = root();
    let external = unsafe_temp_root.parent().unwrap().join("external-temp");
    write_raw(&external, b"partial", COPY_MODE);
    symlink(&external, unsafe_temp_root.join(".journal-a.tmp")).unwrap();
    assert!(matches!(
        open_test(&unsafe_temp_root),
        Err(DiagnosticStoreError::UnsafeTemporary(CopyIndex::A))
    ));
}

#[test]
fn read_only_load_never_cleans_a_temporary_or_repairs_a_degraded_copy() {
    let (_directory, path) = root();
    let store = open_test(&path).unwrap();
    let key = key(1, 7);
    let keys = DiagnosticJournalKeyRing::new(&key, None).unwrap();
    initialize(&store, &keys);
    drop(store);

    let copy_a = path_for(&path, CopyIndex::A);
    let clean_a = fs::read(&copy_a).unwrap();
    let temporary = path.join(".journal-b.tmp");
    write_raw(&temporary, b"partial", COPY_MODE);
    let store = open_test(&path).unwrap();
    assert_eq!(
        store.load_read_only(&keys, 1).err(),
        Some(DiagnosticStoreError::RecoveryRequired)
    );
    assert_eq!(fs::read(&temporary).unwrap(), b"partial");
    assert_eq!(fs::read(&copy_a).unwrap(), clean_a);
    drop(store);
    fs::remove_file(&temporary).unwrap();

    let mut corrupt_a = clean_a.clone();
    corrupt_a[DIAGNOSTIC_JOURNAL_COPY_BYTES - 1] ^= 1;
    write_raw(&copy_a, &corrupt_a, COPY_MODE);
    let store = open_test(&path).unwrap();
    let selected = store.load_read_only(&keys, 1).unwrap();
    assert!(selected.degraded());
    assert_eq!(selected.repair_target(), Some(CopyIndex::A));
    assert_eq!(fs::read(&copy_a).unwrap(), corrupt_a);
}

#[test]
fn corrupt_or_truncated_copy_is_degraded_and_repair_converges_without_nonce() {
    for truncate in [false, true] {
        let (_directory, path) = root();
        let store = open_test(&path).unwrap();
        let key = key(1, 7);
        let keys = DiagnosticJournalKeyRing::new(&key, None).unwrap();
        initialize(&store, &keys);
        let copy_a = path_for(&path, CopyIndex::A);
        let mut bytes = fs::read(&copy_a).unwrap();
        if truncate {
            bytes.truncate(bytes.len() / 2);
        } else {
            bytes[DIAGNOSTIC_JOURNAL_COPY_BYTES - 1] ^= 1;
        }
        write_raw(&copy_a, &bytes, COPY_MODE);
        let degraded = store.load(&keys, 1).unwrap();
        assert!(degraded.degraded());
        assert_eq!(degraded.repair_target(), Some(CopyIndex::A));
        assert_eq!(
            store.repair(&keys, 1).unwrap(),
            RepairDisposition::Repaired {
                generation: 1,
                copy: CopyIndex::A,
            }
        );
        assert!(!store.load(&keys, 1).unwrap().degraded());
        assert_eq!(
            store.repair(&keys, 1).unwrap(),
            RepairDisposition::NotNeeded
        );
    }
}

fn prepared_update<'a>(
    store: &DiagnosticJournalStore,
    keys: &DiagnosticJournalKeyRing<'a>,
    bundle: &'a bli_kvm_diagnostic_export::DiagnosticBundle,
) -> bli_kvm_diagnostic_journal::ReconciliationPlan {
    let selected = store.load(keys, 1).unwrap();
    plan_reconcile(
        &selected,
        CollectorObservation::Sealed(snapshot(9, 4, bundle)),
    )
    .unwrap()
}

#[test]
fn first_copy_failure_preserves_old_clean_generation() {
    let (_directory, path) = root();
    let store = open_test(&path).unwrap();
    let key = key(1, 7);
    let keys = DiagnosticJournalKeyRing::new(&key, None).unwrap();
    initialize(&store, &keys);
    let bundle = bundle(1);
    let plan = prepared_update(&store, &keys, &bundle);
    store.inject_nonce([2; 24]);
    store.inject_fault(1, FaultPoint::TempSynced);
    assert_eq!(
        store.update(&keys, 1, plan.mutation().unwrap()),
        Err(DiagnosticStoreError::InjectedFault)
    );
    drop(store);
    let reopened = open_test(&path).unwrap();
    let selected = reopened.load(&keys, 1).unwrap();
    assert!(!selected.degraded());
    assert_eq!(selected.generation(), 1);
    assert_eq!(selected.journal().entry_count(), 0);
}

#[test]
fn second_copy_failure_leaves_new_generation_repairable() {
    let (_directory, path) = root();
    let store = open_test(&path).unwrap();
    let key = key(1, 7);
    let keys = DiagnosticJournalKeyRing::new(&key, None).unwrap();
    initialize(&store, &keys);
    let bundle = bundle(1);
    let plan = prepared_update(&store, &keys, &bundle);
    store.inject_nonce([2; 24]);
    store.inject_fault(2, FaultPoint::TempSynced);
    assert_eq!(
        store.update(&keys, 1, plan.mutation().unwrap()),
        Err(DiagnosticStoreError::InjectedFault)
    );
    drop(store);
    let reopened = open_test(&path).unwrap();
    let selected = reopened.load(&keys, 1).unwrap();
    assert!(selected.degraded());
    assert_eq!(selected.generation(), 2);
    assert_eq!(selected.journal(), plan.mutation().unwrap().journal());
    assert!(matches!(
        reopened.repair(&keys, 1).unwrap(),
        RepairDisposition::Repaired { generation: 2, .. }
    ));
}

#[test]
fn post_rename_and_post_directory_sync_faults_are_recoverable_new_generation() {
    for point in [
        FaultPoint::RenamedUnsyncedDirectory,
        FaultPoint::DirectorySyncedUnverified,
    ] {
        let (_directory, path) = root();
        let store = open_test(&path).unwrap();
        let key = key(1, 7);
        let keys = DiagnosticJournalKeyRing::new(&key, None).unwrap();
        initialize(&store, &keys);
        let bundle = bundle(1);
        let plan = prepared_update(&store, &keys, &bundle);
        store.inject_nonce([2; 24]);
        store.inject_fault(1, point);
        assert_eq!(
            store.update(&keys, 1, plan.mutation().unwrap()),
            Err(DiagnosticStoreError::InjectedFault)
        );
        drop(store);
        let reopened = open_test(&path).unwrap();
        let selected = reopened.load(&keys, 1).unwrap();
        assert!(selected.degraded());
        assert_eq!(selected.generation(), 2);
        assert_eq!(selected.journal(), plan.mutation().unwrap().journal());
    }
}

#[test]
fn every_durability_fault_point_on_both_copy_writes_is_recoverable() {
    for write_number in [1, 2] {
        for point in [
            FaultPoint::WriteUnsynced,
            FaultPoint::TempSynced,
            FaultPoint::RenamedUnsyncedDirectory,
            FaultPoint::DirectorySyncedUnverified,
        ] {
            let (_directory, path) = root();
            let store = open_test(&path).unwrap();
            let key = key(1, 7);
            let keys = DiagnosticJournalKeyRing::new(&key, None).unwrap();
            initialize(&store, &keys);
            let bundle = bundle(1);
            let plan = prepared_update(&store, &keys, &bundle);
            store.inject_nonce([2; 24]);
            store.inject_fault(write_number, point);
            assert_eq!(
                store.update(&keys, 1, plan.mutation().unwrap()),
                Err(DiagnosticStoreError::InjectedFault)
            );
            drop(store);

            let reopened = open_test(&path).unwrap();
            let selected = reopened.load(&keys, 1).unwrap();
            let new_visible = write_number == 2
                || matches!(
                    point,
                    FaultPoint::RenamedUnsyncedDirectory | FaultPoint::DirectorySyncedUnverified
                );
            if new_visible {
                assert_eq!(selected.generation(), 2);
                assert_eq!(selected.journal(), plan.mutation().unwrap().journal());
            } else {
                assert_eq!(selected.generation(), 1);
                assert_eq!(selected.journal().entry_count(), 0);
            }
        }
    }
}

#[test]
fn failed_first_initialization_write_leaves_no_claimed_state() {
    let (_directory, path) = root();
    let store = open_test(&path).unwrap();
    let key = key(1, 7);
    let keys = DiagnosticJournalKeyRing::new(&key, None).unwrap();
    store.inject_nonce([1; 24]);
    store.inject_fault(1, FaultPoint::WriteUnsynced);
    assert_eq!(
        store.initialize(&keys),
        Err(DiagnosticStoreError::InjectedFault)
    );
    drop(store);
    let reopened = open_test(&path).unwrap();
    assert_eq!(
        reopened.load(&keys, 0).err(),
        Some(DiagnosticStoreError::Journal(
            DiagnosticJournalError::NoValidCopy
        ))
    );
}

#[test]
fn failed_second_initialization_write_leaves_repairable_authenticated_state() {
    let (_directory, path) = root();
    let store = open_test(&path).unwrap();
    let key = key(1, 7);
    let keys = DiagnosticJournalKeyRing::new(&key, None).unwrap();
    store.inject_nonce([1; 24]);
    store.inject_fault(2, FaultPoint::TempSynced);
    assert_eq!(
        store.initialize(&keys),
        Err(DiagnosticStoreError::InjectedFault)
    );
    drop(store);
    let reopened = open_test(&path).unwrap();
    let selected = reopened.load(&keys, 1).unwrap();
    assert!(selected.degraded());
    assert_eq!(selected.generation(), 1);
    assert_eq!(selected.journal().entry_count(), 0);
    assert!(matches!(
        reopened.repair(&keys, 1).unwrap(),
        RepairDisposition::Repaired { generation: 1, .. }
    ));
}

#[test]
fn nonce_failure_reuse_wrong_key_floor_and_root_replacement_fail_closed() {
    let (directory, path) = root();
    let store = open_test(&path).unwrap();
    let primary_key = key(1, 7);
    let keys = DiagnosticJournalKeyRing::new(&primary_key, None).unwrap();
    assert_eq!(
        store.initialize(&keys),
        Err(DiagnosticStoreError::NonceUnavailable)
    );
    initialize(&store, &keys);
    assert_eq!(
        store.load(&keys, 2).err(),
        Some(DiagnosticStoreError::Journal(
            DiagnosticJournalError::RollbackDetected
        ))
    );

    let selected = store.load(&keys, 1).unwrap();
    let bundle = bundle(1);
    let plan = plan_reconcile(
        &selected,
        CollectorObservation::Sealed(snapshot(9, 4, &bundle)),
    )
    .unwrap();
    store.inject_nonce([1; 24]);
    assert_eq!(
        store.update(&keys, 1, plan.mutation().unwrap()),
        Err(DiagnosticStoreError::Journal(
            DiagnosticJournalError::NonceReuse
        ))
    );

    let wrong_key = key(2, 8);
    let wrong_keys = DiagnosticJournalKeyRing::new(&wrong_key, None).unwrap();
    assert_eq!(
        store.load(&wrong_keys, 1).err(),
        Some(DiagnosticStoreError::Journal(
            DiagnosticJournalError::KeyUnavailable
        ))
    );
    let wrong_same_id = key(1, 8);
    let wrong_same_id_keys = DiagnosticJournalKeyRing::new(&wrong_same_id, None).unwrap();
    assert_eq!(
        store.load(&wrong_same_id_keys, 1).err(),
        Some(DiagnosticStoreError::Journal(
            DiagnosticJournalError::NoValidCopy
        ))
    );

    let moved = directory.path().join("moved-diagnostics");
    fs::rename(&path, &moved).unwrap();
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, Permissions::from_mode(ROOT_MODE)).unwrap();
    assert_eq!(
        store.load(&keys, 1).err(),
        Some(DiagnosticStoreError::UnsafeRoot)
    );
}

#[test]
fn active_previous_key_rotation_rewrites_both_copies_under_active_key() {
    let (_directory, path) = root();
    let store = open_test(&path).unwrap();
    let old = key(1, 7);
    let old_keys = DiagnosticJournalKeyRing::new(&old, None).unwrap();
    initialize(&store, &old_keys);

    let active = key(2, 8);
    let rotating = DiagnosticJournalKeyRing::new(&active, Some(&old)).unwrap();
    let selected = store.load(&rotating, 1).unwrap();
    let bundle = bundle(1);
    let plan = plan_reconcile(
        &selected,
        CollectorObservation::Sealed(snapshot(9, 4, &bundle)),
    )
    .unwrap();
    store.inject_nonce([1; 24]);
    assert_eq!(
        store
            .update(&rotating, 1, plan.mutation().unwrap())
            .unwrap(),
        PersistedDiagnosticJournal { generation: 2 }
    );

    let active_only = DiagnosticJournalKeyRing::new(&active, None).unwrap();
    let selected = store.load(&active_only, 2).unwrap();
    assert!(!selected.degraded());
    assert_eq!(selected.generation(), 2);
    assert_eq!(selected.journal(), plan.mutation().unwrap().journal());
}

#[test]
fn errors_and_store_debug_surface_expose_no_paths_keys_or_io_details() {
    let (_directory, path) = root();
    let store = open_test(&path).unwrap();
    let error = DiagnosticStoreError::Io;
    assert_eq!(std::format!("{error:?}"), "Io");
    assert!(!std::format!("{error}").contains("/var/"));
    assert!(!std::format!("{error}").contains("/tmp/"));
    let type_name = std::any::type_name_of_val(&store);
    assert_eq!(
        type_name,
        "bli_kvm_diagnostic_store::DiagnosticJournalStore"
    );
}
