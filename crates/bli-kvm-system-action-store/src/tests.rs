use std::{
    ffi::CString,
    fs::{self, Permissions},
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, PermissionsExt, symlink},
    },
    path::{Path, PathBuf},
};

use bli_kvm_system_action_journal::{
    JournalError, MAX_OPERATION_RECORDS, SystemAction, SystemActionPhase,
    SystemActionRecordSnapshot, plan_initialize,
};
use tempfile::TempDir;

use super::*;

const BOOT_A: [u8; 16] = [0xA1; 16];

fn ids() -> (u32, u32) {
    let owner = unsafe {
        // SAFETY: geteuid has no preconditions.
        libc::geteuid()
    };
    let group = unsafe {
        // SAFETY: getegid has no preconditions.
        libc::getegid()
    };
    (owner, group)
}

fn root() -> (TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("system-actions");
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, Permissions::from_mode(ROOT_MODE)).unwrap();
    (directory, path)
}

fn open_test(path: &Path) -> Result<SystemActionStore, SystemActionStoreError> {
    let (owner, group) = ids();
    SystemActionStore::open_at(path.to_path_buf(), owner, group)
}

fn record_snapshot(phase: SystemActionPhase) -> SystemActionRecordSnapshot {
    let (dispatch_started_at_ms, terminal_at_ms, terminal_boot_binding) = match phase {
        SystemActionPhase::Prepared => (None, None, None),
        SystemActionPhase::Dispatching => (Some(60_002), None, None),
        SystemActionPhase::PostBootAccepted => (Some(60_002), Some(30_000), Some([0xB2; 16])),
        SystemActionPhase::Rejected => (Some(60_002), Some(60_003), Some(BOOT_A)),
    };
    SystemActionRecordSnapshot {
        operation_id: [1; 16],
        session_binding: 1,
        confirmation_binding: [2; 32],
        confirmation_issued_at_ms: 60_000,
        origin_boot_binding: BOOT_A,
        action: SystemAction::Reboot,
        phase,
        prepared_at_ms: 60_001,
        dispatch_started_at_ms,
        terminal_at_ms,
        terminal_boot_binding,
    }
}

fn state(generation: u64, phase: SystemActionPhase) -> SystemActionPolicyState {
    let mut records = [None; MAX_OPERATION_RECORDS];
    records[0] = Some(record_snapshot(phase));
    SystemActionPolicyState::from_record_snapshots(generation, records).unwrap()
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
    fn push_entry(bytes: &mut Vec<u8>, tag: u16, permissions: u16, id: u32) {
        bytes.extend_from_slice(&tag.to_le_bytes());
        bytes.extend_from_slice(&permissions.to_le_bytes());
        bytes.extend_from_slice(&id.to_le_bytes());
    }

    let (owner, _) = ids();
    let mut acl = Vec::with_capacity(44);
    acl.extend_from_slice(&2u32.to_le_bytes());
    push_entry(&mut acl, 0x01, owner_permissions, u32::MAX);
    push_entry(&mut acl, 0x02, 0, owner.saturating_add(1));
    push_entry(&mut acl, 0x04, 0, u32::MAX);
    push_entry(&mut acl, 0x10, 0, u32::MAX);
    push_entry(&mut acl, 0x20, 0, u32::MAX);
    let path = CString::new(path.as_os_str().as_bytes()).unwrap();
    let result = unsafe {
        // SAFETY: path and xattr name are NUL-terminated and acl points to its
        // complete Linux POSIX ACL xattr representation for the call duration.
        libc::setxattr(
            path.as_ptr(),
            c"system.posix_acl_access".as_ptr(),
            acl.as_ptr().cast(),
            acl.len(),
            0,
        )
    };
    assert_eq!(result, 0, "failed to install test POSIX ACL");
}

#[test]
fn production_root_and_names_are_compile_time_fixed() {
    assert_eq!(
        SYSTEM_ACTION_STORE_ROOT,
        "/var/lib/hardened-blikvm-system-actions"
    );
    assert_eq!(COPY_A_FILE_NAME, "journal-a.bin");
    assert_eq!(COPY_B_FILE_NAME, "journal-b.bin");
}

#[test]
fn root_must_be_absolute_canonical_owned_and_mode_0700() {
    let (owner, group) = ids();
    assert!(matches!(
        SystemActionStore::open_at(PathBuf::from("relative"), owner, group),
        Err(SystemActionStoreError::InvalidRoot)
    ));

    let (_directory, path) = root();
    fs::set_permissions(&path, Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        open_test(&path),
        Err(SystemActionStoreError::UnsafeRoot)
    ));

    let parent = tempfile::tempdir().unwrap();
    let target = parent.path().join("target");
    let link = parent.path().join("link");
    fs::create_dir(&target).unwrap();
    fs::set_permissions(&target, Permissions::from_mode(ROOT_MODE)).unwrap();
    symlink(&target, &link).unwrap();
    assert!(matches!(
        open_test(&link),
        Err(SystemActionStoreError::UnsafeRoot)
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
        Err(SystemActionStoreError::UnsafeRoot)
    ));
}

#[test]
fn directory_fd_lock_rejects_a_second_store_instance() {
    let (_directory, path) = root();
    let first = open_test(&path).unwrap();
    assert!(matches!(
        open_test(&path),
        Err(SystemActionStoreError::StoreBusy)
    ));
    drop(first);
    open_test(&path).unwrap();
}

#[test]
fn initialization_writes_two_exact_mode_0600_copies() {
    let (_directory, path) = root();
    let store = open_test(&path).unwrap();
    assert_eq!(
        store.initialize(SystemActionPolicyState::empty()).unwrap(),
        PersistedJournal { generation: 1 }
    );
    let selected = store.load(1).unwrap();
    assert!(!selected.degraded());
    assert_eq!(selected.generation(), 1);
    for index in [CopyIndex::A, CopyIndex::B] {
        let metadata = fs::symlink_metadata(path_for(&path, index)).unwrap();
        assert!(metadata.is_file());
        assert_eq!(metadata.permissions().mode() & 0o7777, COPY_MODE);
        assert_eq!(metadata.nlink(), 1);
        assert_eq!(metadata.len(), JOURNAL_COPY_BYTES as u64);
    }
    assert!(matches!(
        store.initialize(SystemActionPolicyState::empty()),
        Err(SystemActionStoreError::AlreadyInitialized)
    ));
}

#[test]
fn successful_update_checks_generation_and_converges_both_copies() {
    let (_directory, path) = root();
    let store = open_test(&path).unwrap();
    store.initialize(SystemActionPolicyState::empty()).unwrap();
    let prepared = state(1, SystemActionPhase::Prepared);
    assert_eq!(
        store.update(1, 1, prepared).unwrap(),
        PersistedJournal { generation: 2 }
    );
    let selected = store.load(2).unwrap();
    assert!(!selected.degraded());
    assert_eq!(selected.state(), prepared);
    assert!(matches!(
        store.update(1, 1, state(2, SystemActionPhase::Dispatching)),
        Err(SystemActionStoreError::Journal(
            JournalError::ConcurrentStateChange
        ))
    ));
}

#[test]
fn unsafe_copy_types_modes_and_hardlinks_are_rejected() {
    let initial = plan_initialize(SystemActionPolicyState::empty()).unwrap();

    let (_directory, symlink_root) = root();
    let external = symlink_root.parent().unwrap().join("external-copy");
    write_raw(&external, initial.bytes(), COPY_MODE);
    symlink(&external, path_for(&symlink_root, CopyIndex::A)).unwrap();
    assert!(matches!(
        open_test(&symlink_root),
        Err(SystemActionStoreError::UnsafeCopy(CopyIndex::A))
    ));

    let (_directory, mode_root) = root();
    write_raw(&path_for(&mode_root, CopyIndex::A), initial.bytes(), 0o644);
    assert!(matches!(
        open_test(&mode_root),
        Err(SystemActionStoreError::UnsafeCopy(CopyIndex::A))
    ));

    let (_directory, link_root) = root();
    let copy_a = path_for(&link_root, CopyIndex::A);
    write_raw(&copy_a, initial.bytes(), COPY_MODE);
    fs::hard_link(&copy_a, path_for(&link_root, CopyIndex::B)).unwrap();
    assert!(matches!(
        open_test(&link_root),
        Err(SystemActionStoreError::UnsafeCopy(CopyIndex::A))
    ));

    let (_directory, directory_root) = root();
    fs::create_dir(path_for(&directory_root, CopyIndex::A)).unwrap();
    assert!(matches!(
        open_test(&directory_root),
        Err(SystemActionStoreError::UnsafeCopy(CopyIndex::A))
    ));

    let (_directory, fifo_root) = root();
    let fifo = path_for(&fifo_root, CopyIndex::A);
    let fifo_name = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    let result = unsafe {
        // SAFETY: fifo_name is a valid NUL-terminated path and mode is valid.
        libc::mkfifo(fifo_name.as_ptr(), COPY_MODE as libc::mode_t)
    };
    assert_eq!(result, 0);
    assert!(matches!(
        open_test(&fifo_root),
        Err(SystemActionStoreError::UnsafeCopy(CopyIndex::A))
    ));

    let (_directory, oversized_root) = root();
    write_raw(
        &path_for(&oversized_root, CopyIndex::A),
        &vec![0; JOURNAL_COPY_BYTES + 1],
        COPY_MODE,
    );
    assert!(matches!(
        open_test(&oversized_root),
        Err(SystemActionStoreError::UnsafeCopy(CopyIndex::A))
    ));

    let (_directory, acl_copy_root) = root();
    let acl_copy = path_for(&acl_copy_root, CopyIndex::A);
    write_raw(&acl_copy, initial.bytes(), COPY_MODE);
    set_zero_named_user_acl(&acl_copy, 0x06);
    assert_eq!(
        fs::symlink_metadata(&acl_copy)
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        COPY_MODE
    );
    assert!(matches!(
        open_test(&acl_copy_root),
        Err(SystemActionStoreError::UnsafeCopy(CopyIndex::A))
    ));
}

#[test]
fn unexpected_entries_and_unsafe_temporary_files_fail_closed() {
    let (_directory, unexpected_root) = root();
    write_raw(&unexpected_root.join("unexpected"), b"x", COPY_MODE);
    assert!(matches!(
        open_test(&unexpected_root),
        Err(SystemActionStoreError::UnexpectedEntry)
    ));

    let (_directory, safe_temp_root) = root();
    write_raw(
        &safe_temp_root.join(".journal-a.tmp"),
        b"partial",
        COPY_MODE,
    );
    let store = open_test(&safe_temp_root).unwrap();
    assert!(!safe_temp_root.join(".journal-a.tmp").exists());
    drop(store);

    let (_directory, unsafe_temp_root) = root();
    let external = unsafe_temp_root.parent().unwrap().join("external-temp");
    write_raw(&external, b"partial", COPY_MODE);
    symlink(&external, unsafe_temp_root.join(".journal-a.tmp")).unwrap();
    assert!(matches!(
        open_test(&unsafe_temp_root),
        Err(SystemActionStoreError::UnsafeTemporary(CopyIndex::A))
    ));
}

#[test]
fn corrupt_copy_loads_degraded_and_repair_converges_without_advancing() {
    let (_directory, path) = root();
    let store = open_test(&path).unwrap();
    store.initialize(SystemActionPolicyState::empty()).unwrap();
    let copy_a = path_for(&path, CopyIndex::A);
    let mut corrupt = fs::read(&copy_a).unwrap();
    corrupt[0] ^= 1;
    write_raw(&copy_a, &corrupt, COPY_MODE);

    let degraded = store.load(1).unwrap();
    assert!(degraded.degraded());
    assert_eq!(degraded.repair_target(), Some(CopyIndex::A));
    assert_eq!(
        store.repair(1).unwrap(),
        RepairDisposition::Repaired {
            generation: 1,
            copy: CopyIndex::A,
        }
    );
    assert!(!store.load(1).unwrap().degraded());
    assert_eq!(store.repair(1).unwrap(), RepairDisposition::NotNeeded);
}

#[test]
fn truncated_safe_copy_is_degraded_content_and_can_be_repaired() {
    let (_directory, path) = root();
    let store = open_test(&path).unwrap();
    store.initialize(SystemActionPolicyState::empty()).unwrap();
    let copy_a = path_for(&path, CopyIndex::A);
    let bytes = fs::read(&copy_a).unwrap();
    write_raw(&copy_a, &bytes[..bytes.len() / 2], COPY_MODE);

    let degraded = store.load(1).unwrap();
    assert!(degraded.degraded());
    assert_eq!(degraded.repair_target(), Some(CopyIndex::A));
    assert!(matches!(
        store.repair(1).unwrap(),
        RepairDisposition::Repaired {
            generation: 1,
            copy: CopyIndex::A,
        }
    ));
    assert!(!store.load(1).unwrap().degraded());
}

#[test]
fn first_copy_failure_preserves_the_old_clean_generation() {
    let (_directory, path) = root();
    let store = open_test(&path).unwrap();
    store.initialize(SystemActionPolicyState::empty()).unwrap();
    store.inject_fault(1, FaultPoint::TempSynced);
    assert!(matches!(
        store.update(1, 1, state(1, SystemActionPhase::Prepared)),
        Err(SystemActionStoreError::InjectedFault)
    ));
    drop(store);

    let reopened = open_test(&path).unwrap();
    let selected = reopened.load(1).unwrap();
    assert!(!selected.degraded());
    assert_eq!(selected.generation(), 1);
    assert_eq!(selected.state(), SystemActionPolicyState::empty());
}

#[test]
fn second_copy_failure_leaves_the_new_generation_repairable() {
    let (_directory, path) = root();
    let store = open_test(&path).unwrap();
    store.initialize(SystemActionPolicyState::empty()).unwrap();
    let prepared = state(1, SystemActionPhase::Prepared);
    store.inject_fault(2, FaultPoint::TempSynced);
    assert!(matches!(
        store.update(1, 1, prepared),
        Err(SystemActionStoreError::InjectedFault)
    ));
    drop(store);

    let reopened = open_test(&path).unwrap();
    let selected = reopened.load(1).unwrap();
    assert!(selected.degraded());
    assert_eq!(selected.generation(), 2);
    assert_eq!(selected.state(), prepared);
    assert!(matches!(
        reopened.repair(1).unwrap(),
        RepairDisposition::Repaired { generation: 2, .. }
    ));
    assert!(!reopened.load(2).unwrap().degraded());
}

#[test]
fn post_rename_fault_never_causes_automatic_action_or_repeat() {
    let (_directory, path) = root();
    let store = open_test(&path).unwrap();
    store.initialize(SystemActionPolicyState::empty()).unwrap();
    let prepared = state(1, SystemActionPhase::Prepared);
    store.inject_fault(1, FaultPoint::RenamedUnsyncedDirectory);
    assert!(matches!(
        store.update(1, 1, prepared),
        Err(SystemActionStoreError::InjectedFault)
    ));
    drop(store);

    let reopened = open_test(&path).unwrap();
    let selected = reopened.load(1).unwrap();
    assert!(selected.degraded());
    assert_eq!(selected.state(), prepared);
    assert_eq!(
        selected.state().records()[0].unwrap().phase(),
        SystemActionPhase::Prepared
    );
}

#[test]
fn failed_first_initialization_write_leaves_no_claimed_state() {
    let (_directory, path) = root();
    let store = open_test(&path).unwrap();
    store.inject_fault(1, FaultPoint::WriteUnsynced);
    assert!(matches!(
        store.initialize(SystemActionPolicyState::empty()),
        Err(SystemActionStoreError::InjectedFault)
    ));
    drop(store);

    let reopened = open_test(&path).unwrap();
    assert!(matches!(
        reopened.load(0),
        Err(SystemActionStoreError::Journal(JournalError::NoValidCopy))
    ));
}

#[test]
fn rollback_floor_and_live_root_replacement_are_rejected() {
    let (directory, path) = root();
    let store = open_test(&path).unwrap();
    store.initialize(SystemActionPolicyState::empty()).unwrap();
    assert!(matches!(
        store.load(2),
        Err(SystemActionStoreError::Journal(
            JournalError::RollbackDetected
        ))
    ));

    let moved = directory.path().join("moved-actions");
    fs::rename(&path, &moved).unwrap();
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, Permissions::from_mode(ROOT_MODE)).unwrap();
    assert!(matches!(
        store.load(1),
        Err(SystemActionStoreError::UnsafeRoot)
    ));
}

#[test]
fn fault_after_directory_sync_is_recoverable_as_new_degraded_state() {
    let (_directory, path) = root();
    let store = open_test(&path).unwrap();
    store.initialize(SystemActionPolicyState::empty()).unwrap();
    let prepared = state(1, SystemActionPhase::Prepared);
    store.inject_fault(1, FaultPoint::DirectorySyncedUnverified);
    assert!(matches!(
        store.update(1, 1, prepared),
        Err(SystemActionStoreError::InjectedFault)
    ));
    drop(store);
    let reopened = open_test(&path).unwrap();
    let selected = reopened.load(1).unwrap();
    assert!(selected.degraded());
    assert_eq!(selected.generation(), 2);
    assert_eq!(selected.state(), prepared);
}
