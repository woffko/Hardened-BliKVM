use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const LAYOUT_SCHEMA_VERSION: u32 = 1;
pub const LAYOUT_TARGET: &str = "bli-h616-pikvm";
pub const SECTOR_BYTES: u64 = 512;
pub const ALIGNMENT_SECTORS: u64 = 2048;
pub const MAX_LAYOUT_BYTES: usize = 64 * 1024;

const MIN_DISK_SECTORS: u64 = 2 * 1024 * 1024 * 1024 / SECTOR_BYTES;
const MIN_ROOT_SECTORS: u64 = 512 * 1024 * 1024 / SECTOR_BYTES;
const MIN_STATE_SECTORS: u64 = 64 * 1024 * 1024 / SECTOR_BYTES;
const MIN_BOOT_SECTORS: u64 = 16 * 1024 * 1024 / SECTOR_BYTES;
const TABLE_GUARD_SECTORS: u64 = ALIGNMENT_SECTORS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartitionTableKind {
    Mbr,
    Gpt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReservedRole {
    BootloaderPrimary,
    BootloaderBackup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartitionRole {
    RootA,
    RootB,
    State,
    BootA,
    BootB,
    Data,
    Msd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilesystemKind {
    Ext4,
    Vfat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryStrategy {
    ExternalMediaRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReservedRegion {
    pub role: ReservedRole,
    pub start_sector: u64,
    pub sector_count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PartitionRegion {
    pub number: u16,
    pub role: PartitionRole,
    pub filesystem: FilesystemKind,
    pub start_sector: u64,
    pub sector_count: u64,
    pub read_only_default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BootControlPolicy {
    pub journal_copies: u8,
    pub selector_copies: u8,
    pub maximum_attempts: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SystemLayoutProfile {
    pub schema_version: u32,
    pub target: String,
    pub layout_id: String,
    pub partition_table: PartitionTableKind,
    pub sector_bytes: u64,
    pub disk_sectors: u64,
    pub reserved_regions: Vec<ReservedRegion>,
    pub partitions: Vec<PartitionRegion>,
    pub boot_control: BootControlPolicy,
    pub recovery_strategy: RecoveryStrategy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedSystemLayout {
    pub profile: SystemLayoutProfile,
    pub profile_sha256: String,
    pub rootfs_bytes: u64,
    pub boot_bytes: Option<u64>,
    pub state_bytes: u64,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum LayoutError {
    #[error("system layout document is invalid or non-canonical")]
    InvalidDocument,
    #[error("system layout identity or fixed policy is invalid")]
    InvalidProfile,
    #[error("system layout contains an invalid or out-of-bounds range")]
    InvalidGeometry,
    #[error("system layout regions overlap or are not physically sorted")]
    Overlap,
    #[error("system layout contains an invalid partition number or count")]
    PartitionLimit,
    #[error("system layout contains a missing or duplicate semantic role")]
    InvalidRoleSet,
    #[error("system layout A/B roles are incompatible")]
    IncompatiblePair,
    #[error("system layout boot-control redundancy or attempt policy is unsafe")]
    UnsafeBootControl,
}

pub fn validate_layout_bytes(bytes: &[u8]) -> Result<ValidatedSystemLayout, LayoutError> {
    if bytes.is_empty() || bytes.len() > MAX_LAYOUT_BYTES {
        return Err(LayoutError::InvalidDocument);
    }
    let profile: SystemLayoutProfile =
        serde_json::from_slice(bytes).map_err(|_| LayoutError::InvalidDocument)?;
    let canonical = serde_json::to_vec(&profile).map_err(|_| LayoutError::InvalidDocument)?;
    if canonical != bytes {
        return Err(LayoutError::InvalidDocument);
    }
    validate_layout_with_bytes(profile, &canonical)
}

pub fn validate_layout(profile: SystemLayoutProfile) -> Result<ValidatedSystemLayout, LayoutError> {
    let canonical = serde_json::to_vec(&profile).map_err(|_| LayoutError::InvalidDocument)?;
    validate_layout_with_bytes(profile, &canonical)
}

fn validate_layout_with_bytes(
    profile: SystemLayoutProfile,
    canonical: &[u8],
) -> Result<ValidatedSystemLayout, LayoutError> {
    if profile.schema_version != LAYOUT_SCHEMA_VERSION
        || profile.target != LAYOUT_TARGET
        || !valid_layout_id(&profile.layout_id)
        || profile.sector_bytes != SECTOR_BYTES
        || profile.disk_sectors < MIN_DISK_SECTORS
        || profile.reserved_regions.is_empty()
        || profile.partitions.is_empty()
        || !matches!(
            profile.recovery_strategy,
            RecoveryStrategy::ExternalMediaRequired
        )
    {
        return Err(LayoutError::InvalidProfile);
    }
    validate_boot_control(&profile.boot_control)?;
    validate_partition_limits(&profile)?;
    validate_reserved_regions(&profile)?;
    let roles = validate_partitions(&profile)?;
    validate_all_overlaps(&profile)?;
    let (rootfs_bytes, boot_bytes, state_bytes) = validate_role_invariants(&roles)?;

    Ok(ValidatedSystemLayout {
        profile,
        profile_sha256: sha256_hex(canonical),
        rootfs_bytes,
        boot_bytes,
        state_bytes,
    })
}

fn validate_boot_control(policy: &BootControlPolicy) -> Result<(), LayoutError> {
    if !(2..=3).contains(&policy.journal_copies)
        || !(2..=3).contains(&policy.selector_copies)
        || !(1..=3).contains(&policy.maximum_attempts)
    {
        return Err(LayoutError::UnsafeBootControl);
    }
    Ok(())
}

fn validate_partition_limits(profile: &SystemLayoutProfile) -> Result<(), LayoutError> {
    let maximum_number = match profile.partition_table {
        PartitionTableKind::Mbr => 4,
        PartitionTableKind::Gpt => 128,
    };
    if profile.partitions.len() > maximum_number
        || profile.partitions.iter().any(|partition| {
            partition.number == 0 || usize::from(partition.number) > maximum_number
        })
    {
        return Err(LayoutError::PartitionLimit);
    }
    let numbers = profile
        .partitions
        .iter()
        .map(|partition| partition.number)
        .collect::<BTreeSet<_>>();
    if numbers.len() != profile.partitions.len() {
        return Err(LayoutError::PartitionLimit);
    }
    Ok(())
}

fn validate_reserved_regions(profile: &SystemLayoutProfile) -> Result<(), LayoutError> {
    let mut previous_end = 0_u64;
    let mut roles = BTreeSet::new();
    for region in &profile.reserved_regions {
        let end = range_end(
            region.start_sector,
            region.sector_count,
            profile.disk_sectors,
        )?;
        if region.start_sector < previous_end {
            return Err(LayoutError::Overlap);
        }
        if !roles.insert(region.role) {
            return Err(LayoutError::InvalidRoleSet);
        }
        if !matches!(region.start_sector, 16 | 256)
            || (profile.partition_table == PartitionTableKind::Gpt && region.start_sector != 256)
            || end > ALIGNMENT_SECTORS
        {
            return Err(LayoutError::InvalidGeometry);
        }
        previous_end = end;
    }
    if !roles.contains(&ReservedRole::BootloaderPrimary) {
        return Err(LayoutError::InvalidRoleSet);
    }
    Ok(())
}

fn validate_partitions(
    profile: &SystemLayoutProfile,
) -> Result<BTreeMap<PartitionRole, &PartitionRegion>, LayoutError> {
    let mut previous_end = match profile.partition_table {
        PartitionTableKind::Mbr | PartitionTableKind::Gpt => TABLE_GUARD_SECTORS,
    };
    let maximum_end = match profile.partition_table {
        PartitionTableKind::Mbr => profile.disk_sectors,
        PartitionTableKind::Gpt => profile
            .disk_sectors
            .checked_sub(TABLE_GUARD_SECTORS)
            .ok_or(LayoutError::InvalidGeometry)?,
    };
    let mut roles = BTreeMap::new();
    for partition in &profile.partitions {
        if partition.start_sector % ALIGNMENT_SECTORS != 0
            || partition.sector_count % ALIGNMENT_SECTORS != 0
        {
            return Err(LayoutError::InvalidGeometry);
        }
        let end = range_end(partition.start_sector, partition.sector_count, maximum_end)?;
        if partition.start_sector < previous_end {
            return Err(LayoutError::Overlap);
        }
        if roles.insert(partition.role, partition).is_some() {
            return Err(LayoutError::InvalidRoleSet);
        }
        previous_end = end;
    }
    Ok(roles)
}

fn validate_all_overlaps(profile: &SystemLayoutProfile) -> Result<(), LayoutError> {
    for reserved in &profile.reserved_regions {
        let reserved_end = range_end(
            reserved.start_sector,
            reserved.sector_count,
            profile.disk_sectors,
        )?;
        for partition in &profile.partitions {
            let partition_end = range_end(
                partition.start_sector,
                partition.sector_count,
                profile.disk_sectors,
            )?;
            if reserved.start_sector < partition_end && partition.start_sector < reserved_end {
                return Err(LayoutError::Overlap);
            }
        }
    }
    Ok(())
}

fn validate_role_invariants(
    roles: &BTreeMap<PartitionRole, &PartitionRegion>,
) -> Result<(u64, Option<u64>, u64), LayoutError> {
    let root_a = required_role(roles, PartitionRole::RootA)?;
    let root_b = required_role(roles, PartitionRole::RootB)?;
    let state = required_role(roles, PartitionRole::State)?;
    validate_partition_policy(root_a, FilesystemKind::Ext4, true, MIN_ROOT_SECTORS)?;
    validate_partition_policy(root_b, FilesystemKind::Ext4, true, MIN_ROOT_SECTORS)?;
    if root_a.sector_count != root_b.sector_count {
        return Err(LayoutError::IncompatiblePair);
    }
    validate_partition_policy(state, FilesystemKind::Ext4, false, MIN_STATE_SECTORS)?;

    let boot_bytes = match (
        roles.get(&PartitionRole::BootA),
        roles.get(&PartitionRole::BootB),
    ) {
        (None, None) => None,
        (Some(boot_a), Some(boot_b)) => {
            validate_partition_policy(boot_a, FilesystemKind::Vfat, true, MIN_BOOT_SECTORS)?;
            validate_partition_policy(boot_b, FilesystemKind::Vfat, true, MIN_BOOT_SECTORS)?;
            if boot_a.sector_count != boot_b.sector_count {
                return Err(LayoutError::IncompatiblePair);
            }
            Some(sectors_to_bytes(boot_a.sector_count)?)
        }
        _ => return Err(LayoutError::IncompatiblePair),
    };

    if let Some(data) = roles.get(&PartitionRole::Data) {
        validate_partition_policy(data, FilesystemKind::Ext4, false, ALIGNMENT_SECTORS)?;
    }
    if let Some(msd) = roles.get(&PartitionRole::Msd) {
        validate_partition_policy(msd, FilesystemKind::Ext4, true, MIN_STATE_SECTORS)?;
    }

    Ok((
        sectors_to_bytes(root_a.sector_count)?,
        boot_bytes,
        sectors_to_bytes(state.sector_count)?,
    ))
}

fn required_role<'a>(
    roles: &BTreeMap<PartitionRole, &'a PartitionRegion>,
    role: PartitionRole,
) -> Result<&'a PartitionRegion, LayoutError> {
    roles.get(&role).copied().ok_or(LayoutError::InvalidRoleSet)
}

fn validate_partition_policy(
    partition: &PartitionRegion,
    filesystem: FilesystemKind,
    read_only_default: bool,
    minimum_sectors: u64,
) -> Result<(), LayoutError> {
    if partition.filesystem != filesystem
        || partition.read_only_default != read_only_default
        || partition.sector_count < minimum_sectors
    {
        return Err(LayoutError::InvalidRoleSet);
    }
    Ok(())
}

fn range_end(start: u64, count: u64, maximum_end: u64) -> Result<u64, LayoutError> {
    if count == 0 {
        return Err(LayoutError::InvalidGeometry);
    }
    let end = start
        .checked_add(count)
        .ok_or(LayoutError::InvalidGeometry)?;
    if end > maximum_end {
        return Err(LayoutError::InvalidGeometry);
    }
    Ok(end)
}

fn sectors_to_bytes(sectors: u64) -> Result<u64, LayoutError> {
    sectors
        .checked_mul(SECTOR_BYTES)
        .ok_or(LayoutError::InvalidGeometry)
}

fn valid_layout_id(value: &str) -> bool {
    (8..=64).contains(&value.len())
        && value.starts_with("layout-")
        && !value.ends_with('.')
        && !value.contains("..")
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}
