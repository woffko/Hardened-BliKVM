use bli_kvm_system_layout::{
    ALIGNMENT_SECTORS, BootControlPolicy, FilesystemKind, LAYOUT_SCHEMA_VERSION, LAYOUT_TARGET,
    LayoutError, PartitionRegion, PartitionRole, PartitionTableKind, RecoveryStrategy,
    ReservedRegion, ReservedRole, SECTOR_BYTES, SystemLayoutProfile, validate_layout,
    validate_layout_bytes,
};

fn partition(
    number: u16,
    role: PartitionRole,
    filesystem: FilesystemKind,
    start_sector: u64,
    sector_count: u64,
    read_only_default: bool,
) -> PartitionRegion {
    PartitionRegion {
        number,
        role,
        filesystem,
        start_sector,
        sector_count,
        read_only_default,
    }
}

fn mbr_profile() -> SystemLayoutProfile {
    let root_sectors = 8_388_608;
    let state_sectors = 1_048_576;
    let msd_sectors = 20_971_520;
    let root_a_start = ALIGNMENT_SECTORS;
    let root_b_start = root_a_start + root_sectors;
    let state_start = root_b_start + root_sectors;
    let msd_start = state_start + state_sectors;
    SystemLayoutProfile {
        schema_version: LAYOUT_SCHEMA_VERSION,
        target: LAYOUT_TARGET.into(),
        layout_id: "layout-h616-recovery-mbr-v1".into(),
        partition_table: PartitionTableKind::Mbr,
        sector_bytes: SECTOR_BYTES,
        disk_sectors: 67_108_864,
        reserved_regions: vec![ReservedRegion {
            role: ReservedRole::BootloaderPrimary,
            start_sector: 16,
            sector_count: 2032,
        }],
        partitions: vec![
            partition(
                1,
                PartitionRole::RootA,
                FilesystemKind::Ext4,
                root_a_start,
                root_sectors,
                true,
            ),
            partition(
                2,
                PartitionRole::RootB,
                FilesystemKind::Ext4,
                root_b_start,
                root_sectors,
                true,
            ),
            partition(
                3,
                PartitionRole::State,
                FilesystemKind::Ext4,
                state_start,
                state_sectors,
                false,
            ),
            partition(
                4,
                PartitionRole::Msd,
                FilesystemKind::Ext4,
                msd_start,
                msd_sectors,
                true,
            ),
        ],
        boot_control: BootControlPolicy {
            journal_copies: 2,
            selector_copies: 2,
            maximum_attempts: 3,
        },
        recovery_strategy: RecoveryStrategy::ExternalMediaRequired,
    }
}

fn gpt_profile() -> SystemLayoutProfile {
    let boot_sectors = 32_768;
    let root_sectors = 8_388_608;
    let state_sectors = 1_048_576;
    let data_sectors = 10_485_760;
    let mut start = ALIGNMENT_SECTORS;
    let mut partitions = Vec::new();
    for (number, role, filesystem, sectors, read_only) in [
        (
            1,
            PartitionRole::BootA,
            FilesystemKind::Vfat,
            boot_sectors,
            true,
        ),
        (
            2,
            PartitionRole::BootB,
            FilesystemKind::Vfat,
            boot_sectors,
            true,
        ),
        (
            3,
            PartitionRole::RootA,
            FilesystemKind::Ext4,
            root_sectors,
            true,
        ),
        (
            4,
            PartitionRole::RootB,
            FilesystemKind::Ext4,
            root_sectors,
            true,
        ),
        (
            5,
            PartitionRole::State,
            FilesystemKind::Ext4,
            state_sectors,
            false,
        ),
        (
            6,
            PartitionRole::Data,
            FilesystemKind::Ext4,
            data_sectors,
            false,
        ),
    ] {
        partitions.push(partition(
            number, role, filesystem, start, sectors, read_only,
        ));
        start += sectors;
    }
    SystemLayoutProfile {
        schema_version: LAYOUT_SCHEMA_VERSION,
        target: LAYOUT_TARGET.into(),
        layout_id: "layout-h616-recovery-gpt-v1".into(),
        partition_table: PartitionTableKind::Gpt,
        sector_bytes: SECTOR_BYTES,
        disk_sectors: 67_108_864,
        reserved_regions: vec![ReservedRegion {
            role: ReservedRole::BootloaderPrimary,
            start_sector: 256,
            sector_count: 1792,
        }],
        partitions,
        boot_control: BootControlPolicy {
            journal_copies: 3,
            selector_copies: 2,
            maximum_attempts: 2,
        },
        recovery_strategy: RecoveryStrategy::ExternalMediaRequired,
    }
}

#[test]
fn validates_recovery_only_mbr_and_gpt_profiles() {
    let mbr = validate_layout(mbr_profile()).unwrap();
    assert_eq!(mbr.rootfs_bytes, 4 * 1024 * 1024 * 1024);
    assert_eq!(mbr.boot_bytes, None);
    assert_eq!(mbr.state_bytes, 512 * 1024 * 1024);
    assert_eq!(mbr.profile_sha256.len(), 64);

    let gpt_profile = gpt_profile();
    let bytes = serde_json::to_vec(&gpt_profile).unwrap();
    let gpt = validate_layout_bytes(&bytes).unwrap();
    assert_eq!(gpt.boot_bytes, Some(16 * 1024 * 1024));
    assert_eq!(gpt.profile, gpt_profile);
}

#[test]
fn rejects_current_single_root_shape_and_missing_state() {
    let mut profile = mbr_profile();
    profile.disk_sectors = 61_071_360;
    profile.partitions = vec![
        partition(
            2,
            PartitionRole::State,
            FilesystemKind::Ext4,
            2048,
            6144,
            false,
        ),
        partition(
            1,
            PartitionRole::RootA,
            FilesystemKind::Ext4,
            8192,
            10_641_408,
            true,
        ),
        partition(
            3,
            PartitionRole::Data,
            FilesystemKind::Ext4,
            10_649_600,
            24_907_776,
            false,
        ),
        partition(
            4,
            PartitionRole::Msd,
            FilesystemKind::Ext4,
            35_557_376,
            20_461_568,
            true,
        ),
    ];
    assert_eq!(validate_layout(profile), Err(LayoutError::InvalidRoleSet));
}

#[test]
fn rejects_unsorted_overlapping_unaligned_and_out_of_bounds_ranges() {
    let mut unsorted = mbr_profile();
    unsorted.partitions.swap(0, 1);
    assert_eq!(validate_layout(unsorted), Err(LayoutError::Overlap));

    let mut overlap = mbr_profile();
    overlap.partitions[1].start_sector = overlap.partitions[0].start_sector;
    assert_eq!(validate_layout(overlap), Err(LayoutError::Overlap));

    let mut unaligned = mbr_profile();
    unaligned.partitions[0].start_sector += 1;
    assert_eq!(
        validate_layout(unaligned),
        Err(LayoutError::InvalidGeometry)
    );

    let mut out_of_bounds = mbr_profile();
    out_of_bounds.partitions[3].sector_count = out_of_bounds.disk_sectors;
    assert_eq!(
        validate_layout(out_of_bounds),
        Err(LayoutError::InvalidGeometry)
    );
}

#[test]
fn rejects_invalid_root_state_and_boot_pair_policies() {
    let mut unequal_roots = mbr_profile();
    unequal_roots.partitions[1].sector_count -= ALIGNMENT_SECTORS;
    assert_eq!(
        validate_layout(unequal_roots),
        Err(LayoutError::IncompatiblePair)
    );

    let mut writable_root = mbr_profile();
    writable_root.partitions[0].read_only_default = false;
    assert_eq!(
        validate_layout(writable_root),
        Err(LayoutError::InvalidRoleSet)
    );

    let mut readonly_state = mbr_profile();
    readonly_state.partitions[2].read_only_default = true;
    assert_eq!(
        validate_layout(readonly_state),
        Err(LayoutError::InvalidRoleSet)
    );

    let mut one_boot = gpt_profile();
    one_boot
        .partitions
        .retain(|partition| partition.role != PartitionRole::BootB);
    assert_eq!(
        validate_layout(one_boot),
        Err(LayoutError::IncompatiblePair)
    );
}

#[test]
fn rejects_partition_limits_duplicates_and_unsafe_boot_control() {
    let mut too_many_mbr = mbr_profile();
    too_many_mbr.partitions.push(partition(
        1,
        PartitionRole::Data,
        FilesystemKind::Ext4,
        50_000_000,
        ALIGNMENT_SECTORS,
        false,
    ));
    assert_eq!(
        validate_layout(too_many_mbr),
        Err(LayoutError::PartitionLimit)
    );

    let mut duplicate_role = gpt_profile();
    duplicate_role.partitions[5].role = PartitionRole::State;
    assert_eq!(
        validate_layout(duplicate_role),
        Err(LayoutError::InvalidRoleSet)
    );

    let mut unsafe_control = mbr_profile();
    unsafe_control.boot_control.selector_copies = 1;
    assert_eq!(
        validate_layout(unsafe_control),
        Err(LayoutError::UnsafeBootControl)
    );
}

#[test]
fn rejects_unproven_or_overlapping_raw_bootloader_ranges() {
    let mut wrong_offset = mbr_profile();
    wrong_offset.reserved_regions[0].start_sector = 32;
    assert_eq!(
        validate_layout(wrong_offset),
        Err(LayoutError::InvalidGeometry)
    );

    let mut gpt_at_8k = gpt_profile();
    gpt_at_8k.reserved_regions[0].start_sector = 16;
    assert_eq!(
        validate_layout(gpt_at_8k),
        Err(LayoutError::InvalidGeometry)
    );

    let mut overlapping_copy = mbr_profile();
    overlapping_copy.reserved_regions.push(ReservedRegion {
        role: ReservedRole::BootloaderBackup,
        start_sector: 256,
        sector_count: 1792,
    });
    assert_eq!(validate_layout(overlapping_copy), Err(LayoutError::Overlap));
}

#[test]
fn rejects_noncanonical_unknown_or_oversized_documents() {
    let profile = mbr_profile();
    let canonical = serde_json::to_vec(&profile).unwrap();
    let mut spaced = vec![b' '];
    spaced.extend_from_slice(&canonical);
    assert_eq!(
        validate_layout_bytes(&spaced),
        Err(LayoutError::InvalidDocument)
    );

    let mut unknown = serde_json::to_value(&profile).unwrap();
    unknown
        .as_object_mut()
        .unwrap()
        .insert("device".into(), serde_json::json!("mmcblk0"));
    assert_eq!(
        validate_layout_bytes(&serde_json::to_vec(&unknown).unwrap()),
        Err(LayoutError::InvalidDocument)
    );

    assert_eq!(
        validate_layout_bytes(&vec![b' '; 64 * 1024 + 1]),
        Err(LayoutError::InvalidDocument)
    );
}
