use bli_kvm_recovery_inputs::{
    InputProvenance, RECOVERY_INPUTS_FORMAT_V1, RECOVERY_PRODUCT, RECOVERY_TARGET,
    RecoveryBuildInputs, RecoveryInput, RecoveryInputError, RecoveryInputKind,
    RedistributionStatus, SPDX_CONTEXT_URI, SPDX_MODEL_URI, SPDX_SCHEMA_URI, SPDX_VERSION,
    SpdxBinding, validate_recovery_inputs, validate_recovery_inputs_bytes,
};

const PROJECT_COMMIT: &str = "1111111111111111111111111111111111111111";

fn hash(character: char) -> String {
    std::iter::repeat_n(character, 64).collect()
}

fn component(name: &str, kind: RecoveryInputKind, marker: char) -> RecoveryInput {
    RecoveryInput {
        name: name.into(),
        kind,
        version: "1.0.0".into(),
        source_revision: if kind == RecoveryInputKind::HardenedBliKvm {
            PROJECT_COMMIT.into()
        } else {
            format!("rev-{marker}")
        },
        source_sha256: hash(marker),
        artifact_sha256: hash('d'),
        license_expression: "GPL-3.0-only".into(),
        license_text_sha256: hash('e'),
        corresponding_source_available: true,
        provenance: InputProvenance::BuiltFromSource,
        redistribution_status: RedistributionStatus::Cleared,
    }
}

fn manifest() -> RecoveryBuildInputs {
    let mut components = vec![
        component("u-boot", RecoveryInputKind::UBoot, '1'),
        component(
            "trusted-firmware-a",
            RecoveryInputKind::TrustedFirmwareA,
            '4',
        ),
        component("linux-kernel", RecoveryInputKind::LinuxKernel, '5'),
        component("initramfs", RecoveryInputKind::Initramfs, '6'),
        component("rootfs", RecoveryInputKind::Rootfs, '7'),
        component("hardened-blikvm", RecoveryInputKind::HardenedBliKvm, '8'),
        component("toolchain", RecoveryInputKind::Toolchain, '9'),
        component("build-recipe", RecoveryInputKind::BuildRecipe, 'a'),
        component("layout-profile", RecoveryInputKind::LayoutProfile, 'b'),
        component("spdx-validator", RecoveryInputKind::SpdxValidator, 'c'),
    ];
    components.sort_by(|left, right| left.name.cmp(&right.name));
    let layout_sha256 = components
        .iter()
        .find(|component| component.kind == RecoveryInputKind::LayoutProfile)
        .unwrap()
        .artifact_sha256
        .clone();
    let build_recipe_sha256 = components
        .iter()
        .find(|component| component.kind == RecoveryInputKind::BuildRecipe)
        .unwrap()
        .artifact_sha256
        .clone();
    let source_archive_sha256 = components
        .iter()
        .find(|component| component.kind == RecoveryInputKind::HardenedBliKvm)
        .unwrap()
        .source_sha256
        .clone();
    RecoveryBuildInputs {
        format: RECOVERY_INPUTS_FORMAT_V1.into(),
        product: RECOVERY_PRODUCT.into(),
        target: RECOVERY_TARGET.into(),
        recovery_sequence: 1,
        source_commit: PROJECT_COMMIT.into(),
        layout_id: "layout-h616-recovery-test-v1".into(),
        layout_sha256,
        build_recipe_sha256,
        source_archive_sha256,
        notices_sha256: hash('e'),
        spdx: SpdxBinding {
            spec_version: SPDX_VERSION.into(),
            context_uri: SPDX_CONTEXT_URI.into(),
            context_sha256: hash('f'),
            schema_uri: SPDX_SCHEMA_URI.into(),
            schema_sha256: hash('1'),
            model_uri: SPDX_MODEL_URI.into(),
            model_sha256: hash('2'),
            sbom_sha256: hash('3'),
            structural_validation_passed: true,
            semantic_validation_passed: true,
        },
        components,
    }
}

#[test]
fn validates_complete_canonical_recovery_inputs() {
    let manifest = manifest();
    let validated = validate_recovery_inputs(manifest.clone()).unwrap();
    assert_eq!(validated.manifest, manifest);
    assert_eq!(validated.manifest_sha256.len(), 64);

    let bytes = serde_json::to_vec(&manifest).unwrap();
    assert_eq!(
        validate_recovery_inputs_bytes(&bytes).unwrap().manifest,
        manifest
    );
}

#[test]
fn rejects_missing_required_component() {
    let mut manifest = manifest();
    manifest
        .components
        .retain(|component| component.kind != RecoveryInputKind::TrustedFirmwareA);
    assert_eq!(
        validate_recovery_inputs(manifest),
        Err(RecoveryInputError::MissingRequiredComponent)
    );
}

#[test]
fn rejects_blocked_noassertion_and_missing_source() {
    let mut blocked = manifest();
    blocked.components[0].redistribution_status = RedistributionStatus::Blocked;
    assert_eq!(
        validate_recovery_inputs(blocked),
        Err(RecoveryInputError::RedistributionBlocked)
    );

    let mut no_assertion = manifest();
    no_assertion.components[0].redistribution_status = RedistributionStatus::NoAssertion;
    assert_eq!(
        validate_recovery_inputs(no_assertion),
        Err(RecoveryInputError::RedistributionBlocked)
    );

    let mut no_source = manifest();
    no_source.components[0].corresponding_source_available = false;
    assert_eq!(
        validate_recovery_inputs(no_source),
        Err(RecoveryInputError::MissingSource)
    );

    let mut binary_kernel = manifest();
    let kernel = binary_kernel
        .components
        .iter_mut()
        .find(|component| component.kind == RecoveryInputKind::LinuxKernel)
        .unwrap();
    kernel.provenance = InputProvenance::ReviewedBinaryFirmware;
    assert_eq!(
        validate_recovery_inputs(binary_kernel),
        Err(RecoveryInputError::MissingSource)
    );
}

#[test]
fn permits_only_separately_named_cleared_firmware_records() {
    let mut with_firmware = manifest();
    let mut firmware = component("firmware-wifi", RecoveryInputKind::Firmware, '6');
    firmware.provenance = InputProvenance::ReviewedBinaryFirmware;
    with_firmware.components.push(firmware);
    with_firmware
        .components
        .sort_by(|left, right| left.name.cmp(&right.name));
    validate_recovery_inputs(with_firmware).unwrap();

    let mut wrong_name = manifest();
    wrong_name
        .components
        .push(component("wifi", RecoveryInputKind::Firmware, '7'));
    wrong_name
        .components
        .sort_by(|left, right| left.name.cmp(&right.name));
    assert_eq!(
        validate_recovery_inputs(wrong_name),
        Err(RecoveryInputError::InvalidComponent)
    );
}

#[test]
fn rejects_unsorted_duplicate_or_wrong_fixed_component_identity() {
    let mut unsorted = manifest();
    unsorted.components.swap(0, 1);
    assert_eq!(
        validate_recovery_inputs(unsorted),
        Err(RecoveryInputError::InvalidComponent)
    );

    let mut wrong_name = manifest();
    let kernel = wrong_name
        .components
        .iter_mut()
        .find(|component| component.kind == RecoveryInputKind::LinuxKernel)
        .unwrap();
    kernel.name = "kernel".into();
    wrong_name
        .components
        .sort_by(|left, right| left.name.cmp(&right.name));
    assert_eq!(
        validate_recovery_inputs(wrong_name),
        Err(RecoveryInputError::InvalidComponent)
    );
}

#[test]
fn rejects_unpinned_revision_invalid_license_and_zero_hash() {
    let mut latest = manifest();
    latest.components[0].source_revision = "latest".into();
    assert_eq!(
        validate_recovery_inputs(latest),
        Err(RecoveryInputError::InvalidComponent)
    );

    for expression in ["NOASSERTION", "NONE"] {
        let mut invalid = manifest();
        invalid.components[0].license_expression = expression.into();
        assert_eq!(
            validate_recovery_inputs(invalid),
            Err(RecoveryInputError::InvalidLicense)
        );
    }

    let mut zero_hash = manifest();
    zero_hash.components[0].artifact_sha256 = hash('0');
    assert_eq!(
        validate_recovery_inputs(zero_hash),
        Err(RecoveryInputError::InvalidComponent)
    );
}

#[test]
fn rejects_spdx_version_resource_or_validation_drift() {
    let mut version = manifest();
    version.spdx.spec_version = "3.1-dev".into();
    assert_eq!(
        validate_recovery_inputs(version),
        Err(RecoveryInputError::InvalidSpdxBinding)
    );

    let mut schema = manifest();
    schema.spdx.schema_uri = "https://example.invalid/schema.json".into();
    assert_eq!(
        validate_recovery_inputs(schema),
        Err(RecoveryInputError::InvalidSpdxBinding)
    );

    let mut semantic = manifest();
    semantic.spdx.semantic_validation_passed = false;
    assert_eq!(
        validate_recovery_inputs(semantic),
        Err(RecoveryInputError::InvalidSpdxBinding)
    );
}

#[test]
fn rejects_cross_reference_mismatch() {
    for mutate in ["layout", "recipe", "source", "revision"] {
        let mut invalid = manifest();
        match mutate {
            "layout" => invalid.layout_sha256 = hash('9'),
            "recipe" => invalid.build_recipe_sha256 = hash('9'),
            "source" => invalid.source_archive_sha256 = hash('9'),
            "revision" => invalid.source_commit = "2222222222222222222222222222222222222222".into(),
            _ => unreachable!(),
        }
        assert_eq!(
            validate_recovery_inputs(invalid),
            Err(RecoveryInputError::CrossReferenceMismatch)
        );
    }
}

#[test]
fn rejects_noncanonical_unknown_and_oversized_documents() {
    let canonical = serde_json::to_vec(&manifest()).unwrap();
    let mut spaced = vec![b' '];
    spaced.extend_from_slice(&canonical);
    assert_eq!(
        validate_recovery_inputs_bytes(&spaced),
        Err(RecoveryInputError::InvalidDocument)
    );

    let mut unknown = serde_json::to_value(manifest()).unwrap();
    unknown.as_object_mut().unwrap().insert(
        "downloadUrl".into(),
        serde_json::json!("https://example.invalid"),
    );
    assert_eq!(
        validate_recovery_inputs_bytes(&serde_json::to_vec(&unknown).unwrap()),
        Err(RecoveryInputError::InvalidDocument)
    );

    assert_eq!(
        validate_recovery_inputs_bytes(&vec![b' '; 256 * 1024 + 1]),
        Err(RecoveryInputError::InvalidDocument)
    );
}
