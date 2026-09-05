#!/bin/sh
set -eu

BLI_KVM_PNPM_STORE=${BLI_KVM_PNPM_STORE:-/mnt/d/debug/blikvm/hardened-blikvm/pnpm-store}

pnpm_web() {
  PNPM_CONFIG_STORE_DIR="$BLI_KVM_PNPM_STORE" corepack pnpm --dir web "$@"
}

cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo check --workspace --all-targets --all-features --locked
cargo test --workspace --all-targets --locked
cargo test -p bli-kvm-broker-protocol --all-targets --locked --features diagnostic-export
cargo test -p bli-kvm-broker-client --all-targets --locked --features diagnostic-export
cargo test -p bli-kvm-platform-broker --all-targets --locked --features diagnostic-export
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy -p bli-kvm-broker-protocol --all-targets --locked --features diagnostic-export -- -D warnings
cargo clippy -p bli-kvm-broker-client --all-targets --locked --features diagnostic-export -- -D warnings
cargo clippy -p bli-kvm-platform-broker --all-targets --locked --features diagnostic-export -- -D warnings

sh -n scripts/build-aarch64.sh
sh -n scripts/build-prototype-bundle.sh
sh -n scripts/check-source-boundary.sh
sh -n scripts/check.sh
sh -n scripts/rust-lld-linker.sh
sh -n scripts/rustc-remap-wrapper.sh
sh -n scripts/verify-systemd-units.sh
sh -n scripts/verify-aarch64-elf.sh

scripts/check-source-boundary.sh
scripts/verify-systemd-units.sh

python3 -m unittest \
  tools/test_capture_pikvm_baseline.py \
  tools/test_packaging_boundaries.py \
  tools/test_rust_lld_static_pie.py \
  tools/test_rustc_remap_wrapper.py \
  tools/test_capture_ssh_preflight.py \
  tools/test_capture_rtc_baseline.py \
  tools/test_capture_gpio_line_baseline.py \
  tools/test_capture_serial_topology.py \
  tools/test_capture_oled_spi_topology.py \
  tools/test_capture_network_topology.py \
  tools/test_capture_packaging_preflight.py \
  tools/test_runtime_provisioning_policy.py \
  tools/test_capture_runtime_provision.py \
  tools/test_stage_device_runtime_provision.py \
  tools/test_stage_device_runtime_activation.py \
  tools/test_stage_device_installed_tls_browser.py \
  tools/test_capture_device_web_cutover_preflight.py \
  tools/test_capture_device_web_cutover_failure.py \
  tools/test_stage_device_web_cutover.py \
  tools/test_device_existing_account_browser.py \
  tools/test_device_test_account_setup_browser.py \
  tools/test_capture_device_persistent_soak.py \
  tools/test_stage_device_persistent_reboot.py \
  tools/test_capture_device_reboot_persistence_state.py \
  tools/test_recover_device_web_cutover_fallback.py \
  tools/test_capture_debian_runtime_retain_preflight.py \
  tools/test_capture_debian_runtime_retained.py \
  tools/test_build_debian_runtime_package.py \
  tools/test_build_debian_update_runtime_package.py \
  tools/test_stage_device_update_runtime_package.py \
  tools/test_stage_device_update_trust_config.py \
  tools/test_generate_production_update_signing_key.py \
  tools/test_build_production_update_bundle.py \
  tools/test_stage_device_unused_update_trust_removal.py \
  tools/test_stage_device_debian_runtime_package.py \
  tools/test_stage_device_debian_runtime_retain.py \
  tools/test_stage_device_runtime_package_upgrade.py \
  tools/test_capture_trusted_floor_topology.py \
  tools/test_stage_device_broker_inventory.py \
  tools/test_stage_device_prototype.py \
  tools/test_stage_device_systemd_gate.py \
  tools/test_stage_device_system_action_store.py \
  tools/test_stage_device_system_action_maintenance.py \
  tools/test_stage_device_diagnostic_maintenance.py \
  tools/test_stage_device_diagnostic_persistence.py \
  tools/test_stage_device_diagnostic_collector.py \
  tools/test_stage_device_tls_browser.py \
  tools/test_build_update_test_fixture.py \
  tools/test_build_system_image_test_fixture.py \
  -v
python3 -m py_compile \
  tools/capture_pikvm_baseline.py \
  tools/capture_ssh_preflight.py \
  tools/capture_rtc_baseline.py \
  tools/capture_gpio_line_baseline.py \
  tools/capture_serial_topology.py \
  tools/capture_oled_spi_topology.py \
  tools/capture_network_topology.py \
  tools/capture_packaging_preflight.py \
  tools/runtime_provisioning_policy.py \
  tools/capture_runtime_provision_preflight.py \
  tools/capture_runtime_provisioned.py \
  tools/stage_device_runtime_provision.py \
  tools/stage_device_runtime_provision_prepare.py \
  tools/recover_device_runtime_provision.py \
  tools/stage_device_runtime_activation.py \
  tools/stage_device_installed_tls_browser.py \
  tools/capture_device_web_cutover_preflight.py \
  tools/capture_device_web_cutover_failure.py \
  tools/capture_device_persistent_soak.py \
  tools/stage_device_persistent_reboot.py \
  tools/capture_device_reboot_persistence_state.py \
  tools/recover_device_web_cutover_fallback.py \
  tools/stage_device_web_cutover.py \
  tools/capture_debian_runtime_retain_preflight.py \
  tools/capture_debian_runtime_retained.py \
  tools/build_debian_runtime_package.py \
  tools/build_debian_update_runtime_package.py \
  tools/stage_device_update_runtime_package.py \
  tools/stage_device_update_trust_config.py \
  tools/generate_production_update_signing_key.py \
  tools/build_production_update_bundle.py \
  tools/verify_production_update_bundle.py \
  tools/stage_device_unused_update_trust_removal.py \
  tools/stage_device_debian_runtime_package.py \
  tools/stage_device_debian_runtime_retain.py \
  tools/stage_device_runtime_package_upgrade.py \
  tools/capture_trusted_floor_topology.py \
  tools/stage_device_broker_inventory.py \
  tools/stage_device_prototype.py \
  tools/stage_device_systemd_gate.py \
  tools/stage_device_system_action_store.py \
  tools/stage_device_system_action_maintenance.py \
  tools/stage_device_diagnostic_maintenance.py \
  tools/stage_device_diagnostic_persistence.py \
  tools/stage_device_diagnostic_collector.py \
  tools/stage_device_tls_browser.py \
  tools/build_update_test_fixture.py \
  tools/build_system_image_test_fixture.py \
  scripts/rust_lld_static_pie.py \
  tools/test_capture_pikvm_baseline.py \
  tools/test_packaging_boundaries.py \
  tools/test_rust_lld_static_pie.py \
  tools/test_rustc_remap_wrapper.py \
  tools/test_capture_ssh_preflight.py \
  tools/test_capture_rtc_baseline.py \
  tools/test_capture_gpio_line_baseline.py \
  tools/test_capture_serial_topology.py \
  tools/test_capture_oled_spi_topology.py \
  tools/test_capture_network_topology.py \
  tools/test_capture_packaging_preflight.py \
  tools/test_runtime_provisioning_policy.py \
  tools/test_capture_runtime_provision.py \
  tools/test_stage_device_runtime_provision.py \
  tools/test_stage_device_runtime_activation.py \
  tools/test_stage_device_installed_tls_browser.py \
  tools/test_capture_device_web_cutover_preflight.py \
  tools/test_capture_device_web_cutover_failure.py \
  tools/test_stage_device_web_cutover.py \
  tools/test_device_existing_account_browser.py \
  tools/test_device_test_account_setup_browser.py \
  tools/test_capture_device_persistent_soak.py \
  tools/test_stage_device_persistent_reboot.py \
  tools/test_capture_device_reboot_persistence_state.py \
  tools/test_recover_device_web_cutover_fallback.py \
  tools/test_capture_debian_runtime_retain_preflight.py \
  tools/test_capture_debian_runtime_retained.py \
  tools/test_build_debian_runtime_package.py \
  tools/test_build_debian_update_runtime_package.py \
  tools/test_stage_device_update_runtime_package.py \
  tools/test_stage_device_update_trust_config.py \
  tools/test_stage_device_debian_runtime_package.py \
  tools/test_stage_device_debian_runtime_retain.py \
  tools/test_stage_device_runtime_package_upgrade.py \
  tools/test_capture_trusted_floor_topology.py \
  tools/test_stage_device_broker_inventory.py \
  tools/test_stage_device_prototype.py \
  tools/test_stage_device_systemd_gate.py \
  tools/test_stage_device_system_action_store.py \
  tools/test_stage_device_system_action_maintenance.py \
  tools/test_stage_device_diagnostic_maintenance.py \
  tools/test_stage_device_diagnostic_persistence.py \
  tools/test_stage_device_diagnostic_collector.py \
  tools/test_stage_device_tls_browser.py \
  tools/test_build_update_test_fixture.py \
  tools/test_build_system_image_test_fixture.py

node --check tools/playwright/device_tls_auth_smoke.mjs
node --check tools/playwright/device_cutover_setup_smoke.mjs
node --check tools/playwright/device_existing_account_smoke.mjs
node --check tools/playwright/device_test_account_setup_smoke.mjs

if command -v basedpyright >/dev/null 2>&1; then
  basedpyright \
    tools/capture_pikvm_baseline.py \
    tools/capture_ssh_preflight.py \
    tools/capture_rtc_baseline.py \
    tools/capture_gpio_line_baseline.py \
    tools/capture_serial_topology.py \
    tools/capture_oled_spi_topology.py \
    tools/capture_network_topology.py \
    tools/capture_packaging_preflight.py \
    tools/runtime_provisioning_policy.py \
    tools/capture_runtime_provision_preflight.py \
    tools/capture_runtime_provisioned.py \
    tools/stage_device_runtime_provision.py \
    tools/stage_device_runtime_provision_prepare.py \
    tools/recover_device_runtime_provision.py \
    tools/stage_device_runtime_activation.py \
    tools/stage_device_installed_tls_browser.py \
    tools/capture_device_web_cutover_preflight.py \
    tools/capture_device_web_cutover_failure.py \
    tools/capture_device_persistent_soak.py \
    tools/stage_device_persistent_reboot.py \
    tools/capture_device_reboot_persistence_state.py \
    tools/recover_device_web_cutover_fallback.py \
    tools/stage_device_web_cutover.py \
    tools/capture_debian_runtime_retain_preflight.py \
    tools/capture_debian_runtime_retained.py \
    tools/build_debian_runtime_package.py \
    tools/build_debian_update_runtime_package.py \
    tools/stage_device_update_runtime_package.py \
    tools/stage_device_update_trust_config.py \
    tools/stage_device_debian_runtime_package.py \
    tools/stage_device_debian_runtime_retain.py \
    tools/stage_device_runtime_package_upgrade.py \
    tools/capture_trusted_floor_topology.py \
    tools/stage_device_broker_inventory.py \
    tools/stage_device_prototype.py \
    tools/stage_device_systemd_gate.py \
    tools/stage_device_system_action_store.py \
    tools/stage_device_system_action_maintenance.py \
    tools/stage_device_diagnostic_maintenance.py \
    tools/stage_device_diagnostic_persistence.py \
    tools/stage_device_tls_browser.py \
    tools/build_update_test_fixture.py \
    tools/build_system_image_test_fixture.py \
    scripts/rust_lld_static_pie.py \
    tools/test_capture_pikvm_baseline.py \
    tools/test_packaging_boundaries.py \
    tools/test_rust_lld_static_pie.py \
    tools/test_rustc_remap_wrapper.py \
    tools/test_capture_ssh_preflight.py \
    tools/test_capture_rtc_baseline.py \
    tools/test_capture_gpio_line_baseline.py \
    tools/test_capture_serial_topology.py \
    tools/test_capture_oled_spi_topology.py \
    tools/test_capture_network_topology.py \
    tools/test_capture_packaging_preflight.py \
    tools/test_runtime_provisioning_policy.py \
    tools/test_capture_runtime_provision.py \
    tools/test_stage_device_runtime_provision.py \
    tools/test_stage_device_runtime_activation.py \
    tools/test_stage_device_installed_tls_browser.py \
    tools/test_capture_device_web_cutover_preflight.py \
    tools/test_capture_device_web_cutover_failure.py \
    tools/test_stage_device_web_cutover.py \
    tools/test_device_existing_account_browser.py \
    tools/test_device_test_account_setup_browser.py \
    tools/test_capture_device_persistent_soak.py \
    tools/test_stage_device_persistent_reboot.py \
    tools/test_capture_device_reboot_persistence_state.py \
    tools/test_recover_device_web_cutover_fallback.py \
    tools/test_capture_debian_runtime_retain_preflight.py \
    tools/test_capture_debian_runtime_retained.py \
    tools/test_build_debian_runtime_package.py \
    tools/test_build_debian_update_runtime_package.py \
    tools/test_stage_device_update_runtime_package.py \
    tools/test_stage_device_update_trust_config.py \
    tools/test_stage_device_debian_runtime_package.py \
    tools/test_stage_device_debian_runtime_retain.py \
    tools/test_stage_device_runtime_package_upgrade.py \
    tools/test_capture_trusted_floor_topology.py \
    tools/test_stage_device_broker_inventory.py \
    tools/test_stage_device_prototype.py \
    tools/test_stage_device_systemd_gate.py \
    tools/test_stage_device_system_action_store.py \
    tools/test_stage_device_system_action_maintenance.py \
    tools/test_stage_device_diagnostic_maintenance.py \
    tools/test_stage_device_diagnostic_persistence.py \
    tools/test_stage_device_diagnostic_collector.py \
    tools/test_stage_device_tls_browser.py \
    tools/test_build_update_test_fixture.py \
    tools/test_build_system_image_test_fixture.py
else
  echo "basedpyright is required for the complete check" >&2
  exit 1
fi

pnpm_web build
pnpm_web lint
pnpm_web exec prettier --check 'src/**/*.{js,jsx,ts,tsx,json,css,scss,md}'
pnpm_web exec prettier --check '../tools/playwright/*.mjs'

git diff --check
