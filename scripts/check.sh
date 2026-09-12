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
python3 -B tools/verify_persistent_status_ordering.py
python3 -B tools/verify_persistent_write_ordering.py
python3 -B -m unittest \
  tools.test_application_write_contract tools.test_application_write_guard \
  tools.test_application_write_pipeline tools.test_application_write_observer \
  tools.test_application_write_overrides tools.test_application_write_state \
  tools.test_installed_application_write_gate tools.test_run_application_write_gate \
  tools.test_persistent_write_contract tools.test_persistent_write_files tools.test_persistent_write_decision \
  tools.test_persistent_write_lifecycle tools.test_persistent_write_fence tools.test_persistent_write_pipeline \
  tools.test_persistent_write_systemd tools.test_persistent_write_snapshot tools.test_installed_persistent_write \
  tools.test_run_persistent_write_rehearsal tools.test_finish_persistent_write_rehearsal \
  tools.test_upload_proxy_streaming tools.test_install_system_upgrade
node --test tools/test_application_write_acceptance.mjs tools/test_application_response_capture.mjs
node --test tools/test_upload_termination.mjs
node --check tools/playwright/device_application_write.mjs
node --check tools/playwright/device_application_upload_failures.mjs

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
  tools/test_retained_resource_baseline.py \
  tools/test_retained_load_ipc.py \
  tools/test_retained_backend_load.py \
  tools/test_secret_pair_stdin.py \
  tools/test_status_outage_contract.py \
  tools/test_status_socket_outage.py \
  tools/test_status_outage_channel.py \
  tools/test_capture_status_socket_outage.py \
  tools/test_capture_status_bus_preflight.py \
  tools/test_systemd_property.py \
  tools/test_status_restore_properties.py \
  tools/test_probe_status_restore_timer.py \
  tools/test_status_restore_event.py \
  tools/test_status_restore_monitor.py \
  tools/test_systemd_subscription.py \
  tools/test_subscription_process.py \
  tools/test_capture_subscription_preflight.py \
  tools/test_status_timer_firing.py \
  tools/test_capture_status_timer_firing.py \
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
  tools/test_stage_device_update_runtime_upgrade.py \
  tools/test_stage_device_update_config_credential.py \
  tools/test_application_bootstrap_roots.py \
  tools/test_signed_slot_serving.py \
  tools/test_upgrade_initialized_updater.py \
  tools/test_enable_update_services.py \
  tools/test_installed_application_update.py \
  tools/test_application_update_preflight.py \
  tools/test_application_update_flow.py \
  tools/test_run_application_update_flow.py \
  tools/test_status_api_wiring.py \
  tools/test_capture_retained_status.py \
  tools/test_persistent_status_contract.py \
  tools/test_persistent_status_wiring.py \
  tools/test_operational_reboot.py \
  tools/test_verify_update_status.py \
  tools/test_verify_candidate_health.py \
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
  tools/observe_service_resources.py \
  tools/retained_load_ipc.py \
  tools/observe_retained_backend_load.py \
  tools/capture_retained_backend_load.py \
  tools/secret_pair_stdin.py \
  tools/status_outage_contract.py \
  tools/capture_status_bus_preflight.py \
  tools/systemd_property.py \
  tools/status_restore_properties.py \
  tools/probe_status_restore_timer.py \
  tools/status_socket_outage.py \
  tools/status_outage_channel.py \
  tools/capture_status_socket_outage.py \
  tools/recover_status_socket_outage.py \
  tools/capture_status_restore_timer.py \
  tools/status_restore_event.py \
  tools/status_restore_monitor.py \
  tools/systemd_subscription.py \
  tools/subscription_process.py \
  tools/capture_subscription_preflight.py \
  tools/status_timer_firing.py \
  tools/capture_status_timer_firing.py \
  tools/capture_retained_resource_baseline.py \
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
  tools/verify_update_config_credential.py \
  tools/verify_installed_update_bundle.py \
  tools/application_bootstrap_roots.py \
  tools/initialize_application_state.py \
  tools/stage_device_application_bootstrap.py \
  tools/switch_signed_slot_server.py \
  tools/stage_device_signed_slot_server.py \
  tools/upgrade_initialized_updater.py \
  tools/enable_update_services.py \
  tools/installed_application_update.py \
  tools/application_update_preflight.py \
  tools/application_update_flow.py \
  tools/run_application_update_flow.py \
  tools/capture_application_update_preflight.py \
  tools/stage_installed_application_update.py \
  tools/capture_operational_boot.py \
  tools/verify_update_status.py \
  tools/stage_update_status.py \
  tools/prepare_interrupted_update.py \
  tools/stage_operational_reboot.py \
  tools/stage_initialized_updater_upgrade.py \
  tools/verify_candidate_health.py \
  tools/verify_activation_flow.py \
  tools/stage_candidate_health.py \
  tools/verify_apply_events.py \
  tools/stage_device_apply_events.py \
  tools/stage_device_update_config_credential.py \
  tools/stage_device_update_runtime_upgrade.py \
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
  tools/test_retained_resource_baseline.py \
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
node --check tools/playwright/local_application_upload.mjs
node --check tools/playwright/device_cutover_setup_smoke.mjs
node --check tools/playwright/device_existing_account_smoke.mjs
node --check tools/playwright/device_test_account_setup_smoke.mjs

if command -v basedpyright >/dev/null 2>&1; then
  basedpyright tools/application_write_*.py tools/test_application_write_*.py \
    tools/installed_application_write_gate.py tools/test_installed_application_write_gate.py \
    tools/run_application_write_gate.py tools/test_run_application_write_gate.py \
    tools/cleanup_application_write_guard.py tools/capture_application_write_failure.py \
    tools/capture_system_upgrade_audit.py tools/relocate_apt_cache.py tools/prepare_system_upgrade_plan.py \
    tools/download_system_upgrade.py \
    tools/refresh_nginx_runtime.py \
    tools/install_system_upgrade.py tools/test_install_system_upgrade.py \
    tools/persistent_write_contract.py tools/test_persistent_write_contract.py \
    tools/persistent_write_files.py tools/test_persistent_write_files.py tools/verify_persistent_write_ordering.py \
    tools/persistent_write_decision.py tools/test_persistent_write_decision.py \
    tools/persistent_write_lifecycle.py tools/test_persistent_write_lifecycle.py \
    tools/persistent_write_fence.py tools/test_persistent_write_fence.py \
    tools/persistent_write_pipeline.py tools/test_persistent_write_pipeline.py \
    tools/persistent_write_systemd.py tools/test_persistent_write_systemd.py \
    tools/persistent_write_snapshot.py tools/test_persistent_write_snapshot.py \
    tools/installed_persistent_write.py tools/test_installed_persistent_write.py \
    tools/run_persistent_write_rehearsal.py tools/test_run_persistent_write_rehearsal.py \
    tools/finish_persistent_write_rehearsal.py tools/test_finish_persistent_write_rehearsal.py \
    tools/upload_proxy_streaming.py tools/test_upload_proxy_streaming.py tools/run_upload_proxy_streaming.py
  basedpyright \
    tools/upgrade_initialized_updater.py \
    tools/enable_update_services.py \
    tools/installed_application_update.py \
    tools/application_update_preflight.py \
    tools/application_update_flow.py \
    tools/run_application_update_flow.py \
    tools/test_application_update_flow.py \
    tools/test_run_application_update_flow.py \
    tools/capture_application_update_preflight.py \
    tools/test_application_update_preflight.py \
    tools/stage_installed_application_update.py \
    tools/capture_operational_boot.py \
    tools/verify_update_status.py \
    tools/test_verify_update_status.py \
    tools/stage_update_status.py \
    tools/prepare_interrupted_update.py \
    tools/stage_operational_reboot.py \
    tools/test_operational_reboot.py \
    tools/test_installed_application_update.py \
    tools/status_api_wiring.py \
    tools/capture_retained_status.py \
    tools/test_capture_retained_status.py \
    tools/stage_status_api_wiring.py \
    tools/test_status_api_wiring.py \
    tools/persistent_status_contract.py \
    tools/persistent_status_wiring.py \
    tools/capture_status_boot_failure.py \
    tools/test_persistent_status_wiring.py \
    tools/verify_persistent_status_ordering.py \
    tools/test_persistent_status_contract.py \
    tools/test_enable_update_services.py \
    tools/stage_initialized_updater_upgrade.py \
    tools/verify_candidate_health.py \
    tools/verify_activation_flow.py \
    tools/stage_candidate_health.py \
    tools/test_verify_candidate_health.py \
    tools/test_upgrade_initialized_updater.py \
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
    tools/observe_service_resources.py \
    tools/retained_load_ipc.py \
    tools/test_retained_load_ipc.py \
    tools/observe_retained_backend_load.py \
    tools/capture_retained_backend_load.py \
    tools/test_retained_backend_load.py \
    tools/secret_pair_stdin.py \
    tools/test_secret_pair_stdin.py \
    tools/status_outage_contract.py \
    tools/test_status_outage_contract.py \
    tools/capture_status_bus_preflight.py \
    tools/test_capture_status_bus_preflight.py \
    tools/systemd_property.py \
    tools/test_systemd_property.py \
    tools/status_restore_properties.py \
    tools/test_status_restore_properties.py \
    tools/probe_status_restore_timer.py \
    tools/status_socket_outage.py \
    tools/test_status_socket_outage.py \
    tools/status_outage_channel.py \
    tools/test_status_outage_channel.py \
    tools/capture_status_socket_outage.py \
    tools/recover_status_socket_outage.py \
    tools/test_capture_status_socket_outage.py \
    tools/capture_status_restore_timer.py \
    tools/test_probe_status_restore_timer.py \
    tools/status_restore_event.py \
    tools/test_status_restore_event.py \
    tools/status_restore_monitor.py \
    tools/test_status_restore_monitor.py \
    tools/systemd_subscription.py \
    tools/test_systemd_subscription.py \
    tools/subscription_process.py \
    tools/test_subscription_process.py \
    tools/capture_subscription_preflight.py \
    tools/test_capture_subscription_preflight.py \
    tools/status_timer_firing.py \
    tools/test_status_timer_firing.py \
    tools/capture_status_timer_firing.py \
    tools/test_capture_status_timer_firing.py \
    tools/capture_retained_resource_baseline.py \
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
    tools/test_retained_resource_baseline.py \
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

node tools/test_application_update_ui.mjs
node tools/test_notification_lifecycle.mjs
node tools/test_status_soak.mjs
node tools/playwright/local_application_upload.mjs
node tools/playwright/local_upload_logout.mjs
pnpm_web build
pnpm_web lint
pnpm_web exec prettier --check 'src/**/*.{js,jsx,ts,tsx,json,css,scss,md}'
pnpm_web exec prettier --check '../tools/playwright/*.mjs'
pnpm_web exec prettier --check '../tools/test_application_update_ui.mjs'
pnpm_web exec prettier --check '../tools/test_status_outage_audit.mjs'

git diff --check
