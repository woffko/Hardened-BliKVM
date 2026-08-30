use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const RECOVERY_INPUTS_FORMAT_V1: &str = "hardened-blikvm-recovery-inputs-v1";
pub const RECOVERY_PRODUCT: &str = "Hardened BliKVM";
pub const RECOVERY_TARGET: &str = "bli-h616-pikvm";
pub const SPDX_VERSION: &str = "3.0.1";
pub const SPDX_CONTEXT_URI: &str = "https://spdx.org/rdf/3.0.1/spdx-context.jsonld";
pub const SPDX_SCHEMA_URI: &str = "https://spdx.org/schema/3.0.1/spdx-json-schema.json";
pub const SPDX_MODEL_URI: &str = "https://spdx.org/rdf/3.0.1/spdx-model.ttl";
pub const MAX_RECOVERY_INPUT_BYTES: usize = 256 * 1024;
pub const MAX_RECOVERY_COMPONENTS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryInputKind {
    UBoot,
    TrustedFirmwareA,
    LinuxKernel,
    Initramfs,
    Rootfs,
    HardenedBliKvm,
    Toolchain,
    BuildRecipe,
    LayoutProfile,
    SpdxValidator,
    Firmware,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputProvenance {
    BuiltFromSource,
    ReviewedBinaryFirmware,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RedistributionStatus {
    Cleared,
    Blocked,
    NoAssertion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryInput {
    pub name: String,
    pub kind: RecoveryInputKind,
    pub version: String,
    pub source_revision: String,
    pub source_sha256: String,
    pub artifact_sha256: String,
    pub license_expression: String,
    pub license_text_sha256: String,
    pub corresponding_source_available: bool,
    pub provenance: InputProvenance,
    pub redistribution_status: RedistributionStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SpdxBinding {
    pub spec_version: String,
    pub context_uri: String,
    pub context_sha256: String,
    pub schema_uri: String,
    pub schema_sha256: String,
    pub model_uri: String,
    pub model_sha256: String,
    pub sbom_sha256: String,
    pub structural_validation_passed: bool,
    pub semantic_validation_passed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecoveryBuildInputs {
    pub format: String,
    pub product: String,
    pub target: String,
    pub recovery_sequence: u64,
    pub source_commit: String,
    pub layout_id: String,
    pub layout_sha256: String,
    pub build_recipe_sha256: String,
    pub source_archive_sha256: String,
    pub notices_sha256: String,
    pub spdx: SpdxBinding,
    pub components: Vec<RecoveryInput>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedRecoveryInputs {
    pub manifest: RecoveryBuildInputs,
    pub manifest_sha256: String,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryInputError {
    #[error("recovery input document is invalid or non-canonical")]
    InvalidDocument,
    #[error("recovery input manifest shape is invalid")]
    InvalidManifest,
    #[error("recovery input SPDX binding is invalid or incomplete")]
    InvalidSpdxBinding,
    #[error("recovery input components are duplicate, unsorted, or malformed")]
    InvalidComponent,
    #[error("recovery input manifest is missing a required component")]
    MissingRequiredComponent,
    #[error("recovery input lacks corresponding source or pinned provenance")]
    MissingSource,
    #[error("recovery input license expression or retained text is invalid")]
    InvalidLicense,
    #[error("recovery input redistribution is not explicitly cleared")]
    RedistributionBlocked,
    #[error("recovery input cross-reference hashes do not match")]
    CrossReferenceMismatch,
}

pub fn validate_recovery_inputs_bytes(
    bytes: &[u8],
) -> Result<ValidatedRecoveryInputs, RecoveryInputError> {
    if bytes.is_empty() || bytes.len() > MAX_RECOVERY_INPUT_BYTES {
        return Err(RecoveryInputError::InvalidDocument);
    }
    let manifest: RecoveryBuildInputs =
        serde_json::from_slice(bytes).map_err(|_| RecoveryInputError::InvalidDocument)?;
    let canonical =
        serde_json::to_vec(&manifest).map_err(|_| RecoveryInputError::InvalidDocument)?;
    if canonical != bytes {
        return Err(RecoveryInputError::InvalidDocument);
    }
    validate_manifest(&manifest)?;
    Ok(ValidatedRecoveryInputs {
        manifest,
        manifest_sha256: sha256_hex(&canonical),
    })
}

pub fn validate_recovery_inputs(
    manifest: RecoveryBuildInputs,
) -> Result<ValidatedRecoveryInputs, RecoveryInputError> {
    let canonical =
        serde_json::to_vec(&manifest).map_err(|_| RecoveryInputError::InvalidDocument)?;
    validate_manifest(&manifest)?;
    Ok(ValidatedRecoveryInputs {
        manifest,
        manifest_sha256: sha256_hex(&canonical),
    })
}

fn validate_manifest(manifest: &RecoveryBuildInputs) -> Result<(), RecoveryInputError> {
    if manifest.format != RECOVERY_INPUTS_FORMAT_V1
        || manifest.product != RECOVERY_PRODUCT
        || manifest.target != RECOVERY_TARGET
        || manifest.recovery_sequence == 0
        || !valid_commit(&manifest.source_commit)
        || !valid_layout_id(&manifest.layout_id)
        || !valid_sha256(&manifest.layout_sha256)
        || !valid_sha256(&manifest.build_recipe_sha256)
        || !valid_sha256(&manifest.source_archive_sha256)
        || !valid_sha256(&manifest.notices_sha256)
        || manifest.components.is_empty()
        || manifest.components.len() > MAX_RECOVERY_COMPONENTS
    {
        return Err(RecoveryInputError::InvalidManifest);
    }
    validate_spdx(&manifest.spdx)?;
    validate_components(manifest)
}

fn validate_spdx(spdx: &SpdxBinding) -> Result<(), RecoveryInputError> {
    if spdx.spec_version != SPDX_VERSION
        || spdx.context_uri != SPDX_CONTEXT_URI
        || spdx.schema_uri != SPDX_SCHEMA_URI
        || spdx.model_uri != SPDX_MODEL_URI
        || !valid_sha256(&spdx.context_sha256)
        || !valid_sha256(&spdx.schema_sha256)
        || !valid_sha256(&spdx.model_sha256)
        || !valid_sha256(&spdx.sbom_sha256)
        || !spdx.structural_validation_passed
        || !spdx.semantic_validation_passed
    {
        return Err(RecoveryInputError::InvalidSpdxBinding);
    }
    Ok(())
}

fn validate_components(manifest: &RecoveryBuildInputs) -> Result<(), RecoveryInputError> {
    let required = required_kinds();
    let mut found = BTreeSet::new();
    let mut previous_name: Option<&str> = None;
    let mut firmware_count = 0_usize;
    let mut layout_hash: Option<&str> = None;
    let mut recipe_hash: Option<&str> = None;
    let mut project_source_hash: Option<&str> = None;
    let mut project_revision: Option<&str> = None;

    for component in &manifest.components {
        if previous_name.is_some_and(|name| component.name.as_str() <= name)
            || !valid_component_name(&component.name)
            || !valid_version(&component.version)
            || !valid_revision(&component.source_revision)
            || !valid_sha256(&component.source_sha256)
            || !valid_sha256(&component.artifact_sha256)
        {
            return Err(RecoveryInputError::InvalidComponent);
        }
        previous_name = Some(component.name.as_str());
        validate_component_identity(component)?;
        if !valid_license_expression(&component.license_expression)
            || !valid_sha256(&component.license_text_sha256)
        {
            return Err(RecoveryInputError::InvalidLicense);
        }
        if !component.corresponding_source_available {
            return Err(RecoveryInputError::MissingSource);
        }
        if component.redistribution_status != RedistributionStatus::Cleared {
            return Err(RecoveryInputError::RedistributionBlocked);
        }
        if component.kind != RecoveryInputKind::Firmware
            && component.provenance != InputProvenance::BuiltFromSource
        {
            return Err(RecoveryInputError::MissingSource);
        }

        if component.kind == RecoveryInputKind::Firmware {
            firmware_count += 1;
            if firmware_count > 16 {
                return Err(RecoveryInputError::InvalidComponent);
            }
        } else if !found.insert(component.kind) {
            return Err(RecoveryInputError::InvalidComponent);
        }

        match component.kind {
            RecoveryInputKind::LayoutProfile => layout_hash = Some(&component.artifact_sha256),
            RecoveryInputKind::BuildRecipe => recipe_hash = Some(&component.artifact_sha256),
            RecoveryInputKind::HardenedBliKvm => {
                project_source_hash = Some(&component.source_sha256);
                project_revision = Some(&component.source_revision);
            }
            _ => {}
        }
    }

    if !required.is_subset(&found) {
        return Err(RecoveryInputError::MissingRequiredComponent);
    }
    if layout_hash != Some(manifest.layout_sha256.as_str())
        || recipe_hash != Some(manifest.build_recipe_sha256.as_str())
        || project_source_hash != Some(manifest.source_archive_sha256.as_str())
        || project_revision != Some(manifest.source_commit.as_str())
    {
        return Err(RecoveryInputError::CrossReferenceMismatch);
    }
    Ok(())
}

fn validate_component_identity(component: &RecoveryInput) -> Result<(), RecoveryInputError> {
    let expected_name = match component.kind {
        RecoveryInputKind::UBoot => "u-boot",
        RecoveryInputKind::TrustedFirmwareA => "trusted-firmware-a",
        RecoveryInputKind::LinuxKernel => "linux-kernel",
        RecoveryInputKind::Initramfs => "initramfs",
        RecoveryInputKind::Rootfs => "rootfs",
        RecoveryInputKind::HardenedBliKvm => "hardened-blikvm",
        RecoveryInputKind::Toolchain => "toolchain",
        RecoveryInputKind::BuildRecipe => "build-recipe",
        RecoveryInputKind::LayoutProfile => "layout-profile",
        RecoveryInputKind::SpdxValidator => "spdx-validator",
        RecoveryInputKind::Firmware => {
            if component.name.starts_with("firmware-") {
                return Ok(());
            }
            return Err(RecoveryInputError::InvalidComponent);
        }
    };
    if component.name != expected_name {
        return Err(RecoveryInputError::InvalidComponent);
    }
    Ok(())
}

fn required_kinds() -> BTreeSet<RecoveryInputKind> {
    [
        RecoveryInputKind::UBoot,
        RecoveryInputKind::TrustedFirmwareA,
        RecoveryInputKind::LinuxKernel,
        RecoveryInputKind::Initramfs,
        RecoveryInputKind::Rootfs,
        RecoveryInputKind::HardenedBliKvm,
        RecoveryInputKind::Toolchain,
        RecoveryInputKind::BuildRecipe,
        RecoveryInputKind::LayoutProfile,
        RecoveryInputKind::SpdxValidator,
    ]
    .into_iter()
    .collect()
}

fn valid_component_name(value: &str) -> bool {
    (3..=64).contains(&value.len())
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase())
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

fn valid_revision(value: &str) -> bool {
    if !(1..=128).contains(&value.len())
        || value.eq_ignore_ascii_case("latest")
        || value.eq_ignore_ascii_case("main")
        || value.eq_ignore_ascii_case("master")
        || value.eq_ignore_ascii_case("unknown")
        || value.bytes().all(|byte| byte == b'0')
    {
        return false;
    }
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
}

fn valid_license_expression(value: &str) -> bool {
    let upper = value.to_ascii_uppercase();
    (1..=256).contains(&value.len())
        && !upper.contains("NOASSERTION")
        && upper != "NONE"
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || byte == b' '
                || matches!(byte, b'.' | b'+' | b'-' | b'(' | b')' | b':')
        })
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

fn valid_version(value: &str) -> bool {
    (1..=64).contains(&value.len())
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && value
            .bytes()
            .last()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'+' | b'-'))
}

fn valid_commit(value: &str) -> bool {
    value.len() == 40
        && !value.bytes().all(|byte| byte == b'0')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && !value.bytes().all(|byte| byte == b'0')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
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
