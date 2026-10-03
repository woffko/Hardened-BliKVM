#!/bin/sh
set -eu

BLI_KVM_PNPM_STORE=${BLI_KVM_PNPM_STORE:-/mnt/d/debug/blikvm/hardened-blikvm/pnpm-store}

pnpm_web() {
  PNPM_CONFIG_STORE_DIR="$BLI_KVM_PNPM_STORE" corepack pnpm --dir web "$@"
}

cargo fmt --all -- --check
# The VP8 encoder links a pinned static libvpx; build the host copy once per
# target directory so all-feature checks and encoder tests can link it.
target_root=${CARGO_TARGET_DIR:-$PWD/target}
case "$target_root" in /*) ;; *) target_root=$PWD/$target_root ;; esac
BLI_KVM_LIBVPX_DIR="$target_root/libvpx-1.15.2-host"
if [ ! -f "$BLI_KVM_LIBVPX_DIR/lib/libvpx.a" ]; then
  scripts/build-libvpx.sh host "$BLI_KVM_LIBVPX_DIR"
fi
export BLI_KVM_LIBVPX_DIR
# The H.264 WebRTC encoder links a pinned static x264 the same way.
BLI_KVM_X264_DIR="$target_root/x264-165-host"
if [ ! -f "$BLI_KVM_X264_DIR/lib/libx264.a" ]; then
  scripts/build-x264.sh host "$BLI_KVM_X264_DIR"
fi
export BLI_KVM_X264_DIR
# The encoder decodes capture frames with a pinned static libjpeg-turbo.
BLI_KVM_LIBJPEG_TURBO_DIR="$target_root/libjpeg-turbo-3.2.0-host"
if [ ! -f "$BLI_KVM_LIBJPEG_TURBO_DIR/lib/libturbojpeg.a" ]; then
  scripts/build-libjpeg-turbo.sh host "$BLI_KVM_LIBJPEG_TURBO_DIR"
fi
export BLI_KVM_LIBJPEG_TURBO_DIR
cargo check --workspace --all-targets --locked
cargo check --workspace --all-targets --all-features --locked
cargo check -p bli-kvm-media-transition --lib --locked --no-default-features --features gadget-media-profile
cargo check -p bli-kvm-hid-transition --lib --locked --no-default-features --features gadget-hid-profile
cargo check -p bli-kvm-control-broker --lib --locked --features gadget-media-profile,gadget-hid-profile
cargo check -p bli-kvm-control-broker --lib --locked --features gpio-atx-profile
cargo check -p bli-kvm-atx-transition --lib --locked --no-default-features --features gpio-atx-profile
cargo test --workspace --all-targets --locked
cargo test -p bli-kvm-broker-protocol --all-targets --locked --features diagnostic-export
cargo test -p bli-kvm-broker-client --all-targets --locked --features diagnostic-export
cargo test -p bli-kvm-platform-broker --all-targets --locked --features diagnostic-export
cargo test -p bli-kvm-control-broker --all-targets --locked --features hid-transition-profile
cargo test -p bli-kvm-control-broker --all-targets --locked --features atx-transition-profile
cargo test -p bli-kvm-control-broker --all-targets --locked --all-features
cargo test -p bli-kvm-control-broker --all-targets --locked --features gadget-media-profile
cargo test -p bli-kvm-control-broker --all-targets --locked --features gadget-hid-profile
cargo test -p bli-kvm-control-broker --all-targets --locked --features system-action-profile
cargo test -p bli-kvm-control-broker --all-targets --locked --features gpio-atx-profile
cargo test -p bli-kvm-control-broker --all-targets --locked --features gadget-media-profile,gadget-hid-profile
cargo test -p bli-kvm-control-broker --all-targets --locked --features chardev-atx-profile
cargo test -p bli-kvm-control-broker --all-targets --locked --features gadget-media-profile,gadget-hid-profile,gpio-atx-profile
cargo test -p bli-kvm-media-transition --all-targets --locked --features gadget-media-profile
cargo test -p bli-kvm-hid-transition --all-targets --locked --features gadget-hid-profile
cargo test -p bli-kvm-atx-transition --all-targets --locked --features gpio-atx-profile
cargo test -p bli-kvm-atx-gpio-executor --all-targets --locked --features chardev-driver
cargo test -p bli-kvm-video-encoder --all-targets --locked --features h264
cargo test -p bli-kvm-video-encoder --all-targets --locked --features h264,turbojpeg
cargo test -p bli-kvm-video-encoder --all-targets --locked --no-default-features --features h264,turbojpeg
# Randomly damaged capture frames through libjpeg-turbo, as on the board.
BK_JPEG_DAMAGE_FRAMES=20000 cargo test -p bli-kvm-video-encoder --release --lib --locked \
  --features h264,turbojpeg damaged_frames_never_crash
sh -n scripts/build-libvpx.sh
sh -n scripts/build-x264.sh
sh -n tools/device/deploy_native_profile.sh
sh -n tools/device/deploy_vp8.sh
sh -n tools/image/build_sd_image.sh
sh -n tools/image/hardened-blikvm-firstboot.sh
sh -n tools/image/hardened-blikvm-grow-image-store.sh
sh -n packaging/system-update/initramfs-hook
# The scripts of the hardware H.264 plan's lab: the module build recipe, a
# lab session on the board, and the check of the recipe's modules.
sh -n scripts/build-ve-module.sh
sh -n scripts/build-ve-lab.sh
sh -n tools/device/ve_lab_session.sh
sh -n tools/device/ve_lab_run.sh
sh -n tools/device/ve_module_check.sh
sh -n scripts/build-ve-trace.sh
sh -n tools/device/ve_trace_run.sh
sh -n tools/device/ve_trace_session.sh
sh -n tools/device/ve_quality_run.sh
sh -n tools/device/ve_stream_run.sh
sh -n tools/device/ve_endurance_run.sh
sh -n tools/device/ve_driver_compare.sh
sh -n tools/device/ve_encoder_measure.sh
sh -n tools/device/ve_hold_run.sh
sh -n tools/device/ve_unit_cpu.sh
sh -n tools/device/ve_board_peers.sh
sh -n tools/device/ve_engine_switch_run.sh
sh -n kernel/hardened-blikvm-ve/test/run.sh
sh -n packaging/ve/bli-kvm-ve-load
sh -n tools/device/deploy_ve.sh
sh -n scripts/build-ve-reference.sh
node --check tools/playwright/ve_stream_check.mjs
python3 -m py_compile tools/device/ve_probe_check.py tools/device/ve_memory_scan.py \
  tools/device/ve_stream_report.py tools/device/ve_enc_hostile.py \
  tools/device/ve_capture_play.py tools/device/ve_demand_hold.py \
  tools/device/ve_engine_switch.py tools/device/ve_engine_watch.py \
  tools/device/ve_engine_choose.py tools/device/engine_switch_probe.py \
  tools/ve-trace/ve_trace_report.py
sh -n packaging/system-update/initramfs-local-premount
cargo test -p bli-kvm-server --all-targets --locked --features restricted-control-surface
cargo build -p bli-kvm-control-broker --locked --example firewall_health_gate
python3 tools/check_firewall_https.py --gate "${CARGO_TARGET_DIR:-target}/debug/examples/firewall_health_gate"
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy -p bli-kvm-video-encoder --all-targets --locked --features h264 -- -D warnings
cargo clippy -p bli-kvm-video-encoder --all-targets --locked --features h264,turbojpeg -- -D warnings
cargo clippy -p bli-kvm-broker-protocol --all-targets --locked --features diagnostic-export -- -D warnings
cargo clippy -p bli-kvm-broker-client --all-targets --locked --features diagnostic-export -- -D warnings
cargo clippy -p bli-kvm-platform-broker --all-targets --locked --features diagnostic-export -- -D warnings
cargo clippy -p bli-kvm-control-broker --all-targets --locked --features hid-transition-profile,atx-transition-profile -- -D warnings
cargo clippy -p bli-kvm-control-broker --all-targets --locked --features gadget-media-profile -- -D warnings
cargo clippy -p bli-kvm-control-broker --all-targets --locked --features gadget-hid-profile -- -D warnings
cargo clippy -p bli-kvm-control-broker --all-targets --locked --features system-action-profile -- -D warnings
cargo clippy -p bli-kvm-control-broker --all-targets --locked --features gpio-atx-profile -- -D warnings
cargo clippy -p bli-kvm-control-broker --all-targets --locked --features gadget-media-profile,gadget-hid-profile -- -D warnings
cargo clippy -p bli-kvm-control-broker --all-targets --locked --features chardev-atx-profile -- -D warnings
cargo clippy -p bli-kvm-control-broker --all-targets --locked --features gadget-media-profile,gadget-hid-profile,gpio-atx-profile -- -D warnings
cargo clippy -p bli-kvm-media-transition --all-targets --locked --features gadget-media-profile -- -D warnings
cargo clippy -p bli-kvm-hid-transition --all-targets --locked --features gadget-hid-profile -- -D warnings
cargo clippy -p bli-kvm-atx-transition --all-targets --locked --features gpio-atx-profile -- -D warnings
cargo clippy -p bli-kvm-atx-gpio-executor --all-targets --locked --features chardev-driver -- -D warnings
cargo clippy -p bli-kvm-server --all-targets --locked --features restricted-control-surface -- -D warnings

sh -n scripts/build-aarch64.sh
sh -n scripts/musl-headers.sh
bash -n scripts/build-board-profile.sh
sh -n scripts/build-hid-transition-profile.sh
bash -n scripts/build-prototype-bundle.sh
sh -n scripts/check-source-boundary.sh
sh -n scripts/check.sh
sh -n scripts/rust-lld-linker.sh
sh -n scripts/rustc-remap-wrapper.sh
sh -n scripts/verify-systemd-units.sh
python3 tools/render_native_profile.py --check
sh -n scripts/verify-aarch64-elf.sh

scripts/check-source-boundary.sh
python3 -B -m unittest tools.test_source_boundary
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
  tools.test_reclaim_application_slot tools.test_reclaim_retained_application_slot \
  tools.test_upload_proxy_streaming tools.test_install_system_upgrade tools.test_persistent_write_pending \
  tools.test_installed_pending_write_gate tools.test_persistent_write_boot_proof tools.test_observe_persistent_write_boot \
  tools.test_persistent_write_evidence_compatibility tools.test_persistent_write_activation_receipt \
  tools.test_persistent_write_recovery_receipt tools.test_persistent_write_retained_snapshot tools.test_persistent_write_commit \
  tools.test_persistent_write_evidence_store tools.test_stage_persistent_write_evidence \
  tools.test_persistent_write_evidence_pins tools.test_persistent_write_evidence_provider \
  tools.test_persistent_write_gui_failure_receipt tools.test_persistent_write_host_receipt \
  tools.test_capture_persistent_write_host_gate tools.test_persistent_write_upload_receipt \
  tools.test_capture_persistent_write_upload_gate tools.test_persistent_write_component_view \
  tools.test_run_persistent_write_retained_reboot tools.test_observe_retained_application_status \
  tools.test_persistent_write_gui_gate_receipt \
  tools.test_capture_persistent_write_gui_gate tools.test_run_persistent_write_commit_cycle
node --test tools/test_application_write_acceptance.mjs tools/test_application_response_capture.mjs
node --test tools/test_upload_termination.mjs
node --test tools/test_gui_parity_probe.mjs
node --test tools/test_gui_parity_app.mjs
node --test tools/test_restricted_power_probe.mjs
node --test tools/test_restricted_keyboard_probe.mjs
node --test tools/test_hid_report_queue.mjs
node --test tools/test_command_result.mjs
node --test tools/test_hid_text.mjs
node --test tools/test_sticky_modifiers.mjs
node --test tools/test_quic_messages.mjs
# The picture's receiver: reassembly, the chain, recovery, and 100000 random
# and damaged datagrams and streams; the board's clock; the codec string.
node --test tools/test_quic_video_receiver.mjs tools/test_quic_board_clock.mjs tools/test_quic_h264.mjs
node --test tools/test_impair_proxies.mjs
node --test tools/test_virtual_media_probe_app.mjs
node --test tools/test_virtual_media_acceptance.mjs
node --test tools/test_video_frame_timing_probe.mjs
node --check tools/playwright/device_application_write.mjs
node --check tools/playwright/device_application_upload_failures.mjs
node --check tools/playwright/gui_parity_probe.mjs
node --check tools/playwright/raw_system_update_probe.mjs
node --check tools/playwright/raw_system_update_device_test.mjs
node --check tools/playwright/firewall_modes_device_probe.mjs
node --check tools/playwright/firewall_page_device_test.mjs
node --check tools/playwright/gui_walk_device.mjs
node --check tools/playwright/media_gui_device_test.mjs
node --check tools/playwright/terminal_run.mjs
node --check tools/playwright/terminal_put.mjs
node --check tools/playwright/virtual_media_acceptance.mjs
node --check tools/playwright/restricted_power_probe.mjs
node --check tools/playwright/restricted_control_gui_probe.mjs
node --check tools/playwright/restricted_control_virtual_keyboard_probe.mjs
node --check tools/playwright/restricted_control_lifecycle.mjs
node --check tools/playwright/network_contract.mjs
node --check tools/playwright/firewall_modes_contract.mjs
node --check tools/playwright/webrtc_unavailable_contract.mjs
node --check tools/playwright/csp_contract.mjs
node --check tools/playwright/menu_autohide_contract.mjs
node --check tools/playwright/webrtc_quality_contract.mjs
node --check tools/playwright/theme_contract.mjs
node --check tools/playwright/mobile_keys_contract.mjs
node --check tools/playwright/touch_screen_reach_contract.mjs
node --check tools/playwright/quic_fixture.mjs
node --check tools/playwright/quic_transport_contract.mjs
node --check tools/playwright/quic_transport_device_probe.mjs
node --check tools/playwright/quic_input_contract.mjs
node --check tools/playwright/quic_stack_rig.mjs
node --check tools/playwright/quic_input_stack_contract.mjs
node --check tools/playwright/quic_input_latency.mjs
node --check tools/playwright/quic_input_device_probe.mjs
node --check tools/playwright/quic_video_contract.mjs
node --check tools/playwright/quic_video_device_probe.mjs
node --check tools/playwright/quic_video_measure.mjs
node --check tools/playwright/quic_firewall_device_probe.mjs
node --check tools/device/quic_firefox_probe.mjs
node --check tools/net/udp_impair_proxy.mjs
node --check tools/net/tcp_impair_proxy.mjs
node --check tools/playwright/touch_reach_device_probe.mjs
node --check tools/playwright/wifi_contract.mjs
node --check tools/playwright/vp8_webrtc_probe.mjs
node --check tools/playwright/video_settings_contract.mjs
node --check tools/playwright/quality_fixture.mjs
node --check tools/playwright/h264_engine_menu_contract.mjs
node --check tools/playwright/quality_monitor_contract.mjs
node --check tools/playwright/quality_monitor_gate.mjs
node --check tools/playwright/app_harness.mjs
node --check tools/playwright/video_frame_timing_probe.mjs

python3 -m unittest \
  tools/test_capture_pikvm_baseline.py \
  tools/test_packaging_boundaries.py \
  tools/test_build_sd_image.py \
  tools/test_build_scripts_guard.py \
  tools/test_web_api_routes.py \
  tools/test_hid_credential_service.py \
  tools/test_hid_transition_service.py \
  tools/test_build_hid_transition_profile.py \
  tools/test_build_board_profile.py \
  tools/test_verify_board_profile_bundle.py \
  tools/test_plan_board_profile_handoff.py \
  tools/test_stage_device_board_profile_gate.py \
  tools/test_stage_device_boardd_gate.py \
  tools/test_stage_device_system_action_gate.py \
  tools/test_stage_device_network_write_gate.py \
  tools/test_stage_device_storage_reboot_gate.py \
  tools/test_storage_acceptance_probe.py \
  tools/test_capture_vendor_replacement_state.py \
  tools/test_compare_gui_parity.py \
  tools/test_hid_keymap_alignment.py \
  tools/test_phase3_control_boundaries.py \
  tools/test_phase4_video_boundary.py \
  tools/test_video_transport_selection.py \
  tools/test_media_fixed_routes.py \
  tools/test_restricted_control_gui.py \
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
  tools/test_prototype_archive.py \
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
  tools/test_stage_device_hid_credential_gate.py \
  tools/test_stage_device_hid_isolated_gate.py \
  tools/test_stage_device_atx_live_gate.py \
  tools/test_stage_device_hid_profile_gate.py \
  tools/test_stage_device_media_isolated_gate.py \
  tools/test_stage_device_hid_reboot_gate.py \
  tools/test_stage_device_boot_gate.py \
  tools/test_stage_device_video_settings_gate.py \
  tools/test_stage_device_tailscale_gate.py \
  tools/test_stage_device_panel_gate.py \
  tools/test_stage_device_firewall_gate.py \
  tools/test_stage_device_hid_lease_gate.py \
  tools/test_stage_device_settings_gate.py \
  tools/test_stage_device_gadget_gate.py \
  tools/test_check_device_log_health.py \
  tools/test_gadget_profile_boundary.py \
  tools/test_license_inventory.py \
  tools/test_crate_licenses.py \
  tools/test_capture_device_kvm_topology.py \
  tools/test_capture_device_hid_descriptors.py \
  tools/test_capture_device_video_matrix.py \
  tools/test_capture_device_handoff_preflight.py \
  tools/test_capture_device_kvmd_control_source.py \
  tools/test_phase8_acceptance_report.py \
  tools/test_stage_device_kvmd_removal.py \
  tools/test_stage_device_board_bundle.py \
  tools/test_stage_device_owner_image.py \
  tools/test_upload_media_image.py \
  tools/test_phase8_acceptance_manifest.py \
  tools/test_doc_tool_paths.py \
  tools/test_plan_route_drift.py \
  tools/test_stage_device_system_action_store.py \
  tools/test_stage_device_system_action_maintenance.py \
  tools/test_stage_device_diagnostic_maintenance.py \
  tools/test_stage_device_diagnostic_persistence.py \
  tools/test_stage_device_diagnostic_collector.py \
  tools/test_stage_device_tls_browser.py \
  tools/test_build_update_test_fixture.py \
  tools/test_build_system_image_test_fixture.py \
  tools/test_build_system_raw_bundle.py \
  tools/test_ve_module.py \
  -v
python3 -m py_compile \
  tools/device/owner_lock_probe.py \
  tools/device/safe_writes_probe.py \
  tools/device/wifi_via_api.py \
  tools/device/stream_latency_probe.py \
  tools/device/hid_latency_probe.py \
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
  tools/stage_device_hid_credential_gate.py \
  tools/stage_device_hid_isolated_gate.py \
  tools/stage_device_atx_live_gate.py \
  tools/stage_device_hid_profile_gate.py \
  tools/stage_device_media_isolated_gate.py \
  tools/stage_device_hid_reboot_gate.py \
  tools/capture_device_hid_descriptors.py \
  tools/capture_device_video_matrix.py \
  tools/capture_device_handoff_preflight.py \
  tools/verify_board_profile_bundle.py \
  tools/plan_board_profile_handoff.py \
  tools/stage_device_board_profile_gate.py \
  tools/compare_gui_parity.py \
  tools/test_compare_gui_parity.py \
  tools/capture_device_kvm_topology.py \
  tools/check_device_log_health.py \
  tools/license_inventory.py \
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
  tools/test_hid_credential_service.py \
  tools/test_hid_transition_service.py \
  tools/test_build_hid_transition_profile.py \
  tools/test_hid_keymap_alignment.py \
  tools/test_phase3_control_boundaries.py \
  tools/test_phase4_video_boundary.py \
  tools/test_video_transport_selection.py \
  tools/test_media_fixed_routes.py \
  tools/test_restricted_control_gui.py \
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
  tools/test_prototype_archive.py \
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
  tools/test_stage_device_hid_credential_gate.py \
  tools/test_stage_device_hid_isolated_gate.py \
  tools/test_stage_device_atx_live_gate.py \
  tools/test_stage_device_hid_profile_gate.py \
  tools/test_stage_device_media_isolated_gate.py \
  tools/test_stage_device_hid_reboot_gate.py \
  tools/test_stage_device_boot_gate.py \
  tools/test_stage_device_video_settings_gate.py \
  tools/test_stage_device_tailscale_gate.py \
  tools/test_stage_device_panel_gate.py \
  tools/test_stage_device_firewall_gate.py \
  tools/test_stage_device_hid_lease_gate.py \
  tools/test_stage_device_settings_gate.py \
  tools/test_stage_device_gadget_gate.py \
  tools/test_check_device_log_health.py \
  tools/test_license_inventory.py \
  tools/test_crate_licenses.py \
  tools/test_stage_device_system_action_store.py \
  tools/test_stage_device_system_action_maintenance.py \
  tools/test_stage_device_diagnostic_maintenance.py \
  tools/test_stage_device_diagnostic_persistence.py \
  tools/test_stage_device_diagnostic_collector.py \
  tools/test_stage_device_tls_browser.py \
  tools/test_build_update_test_fixture.py \
  tools/test_build_system_image_test_fixture.py \
  tools/build_system_raw_bundle.py \
  tools/test_build_system_raw_bundle.py \
  tools/ve_module.py \
  tools/test_ve_module.py

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
    tools/persistent_write_pending.py tools/test_persistent_write_pending.py \
    tools/installed_pending_write_gate.py tools/test_installed_pending_write_gate.py \
    tools/cleanup_pending_staging.py \
    tools/audit_write_prerequisites.py \
    tools/reclaim_application_slot.py tools/reclaim_retained_application_slot.py tools/test_reclaim_retained_application_slot.py \
    tools/persistent_write_boot_proof.py tools/test_persistent_write_boot_proof.py tools/observe_persistent_write_boot.py \
    tools/test_observe_persistent_write_boot.py \
    tools/persistent_write_evidence_compatibility.py tools/test_persistent_write_evidence_compatibility.py \
    tools/persistent_write_activation_receipt.py tools/test_persistent_write_activation_receipt.py \
    tools/persistent_write_recovery_receipt.py tools/test_persistent_write_recovery_receipt.py \
    tools/persistent_write_retained_snapshot.py tools/test_persistent_write_retained_snapshot.py \
    tools/persistent_write_commit.py tools/test_persistent_write_commit.py \
    tools/persistent_write_evidence_store.py tools/test_persistent_write_evidence_store.py \
    tools/stage_persistent_write_evidence.py tools/test_stage_persistent_write_evidence.py \
    tools/persistent_write_evidence_pins.py tools/test_persistent_write_evidence_pins.py \
    tools/generate_persistent_write_evidence_pins.py \
    tools/persistent_write_evidence_provider.py tools/test_persistent_write_evidence_provider.py \
    tools/persistent_write_gui_failure_receipt.py tools/test_persistent_write_gui_failure_receipt.py \
    tools/persistent_write_gui_gate_receipt.py tools/test_persistent_write_gui_gate_receipt.py \
    tools/capture_persistent_write_gui_gate.py tools/test_capture_persistent_write_gui_gate.py \
    tools/persistent_write_host_receipt.py tools/test_persistent_write_host_receipt.py \
    tools/capture_persistent_write_host_gate.py tools/test_capture_persistent_write_host_gate.py \
    tools/persistent_write_upload_receipt.py tools/test_persistent_write_upload_receipt.py \
    tools/capture_persistent_write_upload_gate.py tools/test_capture_persistent_write_upload_gate.py \
    tools/persistent_write_component_view.py tools/test_persistent_write_component_view.py \
    tools/run_persistent_write_retained_reboot.py tools/test_run_persistent_write_retained_reboot.py \
    tools/observe_retained_application_status.py tools/test_observe_retained_application_status.py \
    tools/run_persistent_write_commit_cycle.py tools/test_run_persistent_write_commit_cycle.py \
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
    tools/stage_device_hid_credential_gate.py \
    tools/stage_device_hid_isolated_gate.py \
    tools/stage_device_atx_live_gate.py \
    tools/stage_device_hid_profile_gate.py \
    tools/stage_device_media_isolated_gate.py \
    tools/stage_device_hid_reboot_gate.py \
    tools/stage_device_boot_gate.py \
    tools/stage_device_video_settings_gate.py \
    tools/stage_device_tailscale_gate.py \
    tools/stage_device_panel_gate.py \
    tools/stage_device_firewall_gate.py \
    tools/stage_device_hid_lease_gate.py \
    tools/stage_device_settings_gate.py \
    tools/stage_device_gadget_gate.py \
    tools/check_device_log_health.py \
    tools/license_inventory.py \
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
    tools/test_hid_credential_service.py \
    tools/test_hid_transition_service.py \
    tools/test_build_hid_transition_profile.py \
    tools/test_hid_keymap_alignment.py \
    tools/test_phase3_control_boundaries.py \
    tools/test_phase4_video_boundary.py \
    tools/test_video_transport_selection.py \
    tools/test_media_fixed_routes.py \
    tools/test_restricted_control_gui.py \
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
    tools/test_prototype_archive.py \
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
    tools/test_stage_device_hid_credential_gate.py \
    tools/test_stage_device_hid_isolated_gate.py \
    tools/test_stage_device_atx_live_gate.py \
    tools/test_stage_device_hid_profile_gate.py \
    tools/test_stage_device_media_isolated_gate.py \
    tools/test_stage_device_hid_reboot_gate.py \
    tools/test_stage_device_boot_gate.py \
    tools/test_stage_device_video_settings_gate.py \
  tools/test_stage_device_tailscale_gate.py \
  tools/test_stage_device_panel_gate.py \
  tools/test_stage_device_firewall_gate.py \
  tools/test_stage_device_hid_lease_gate.py \
    tools/test_stage_device_settings_gate.py \
  tools/test_stage_device_settings_gate.py \
  tools/test_stage_device_video_settings_gate.py \
  tools/test_stage_device_tailscale_gate.py \
  tools/test_stage_device_panel_gate.py \
  tools/test_stage_device_firewall_gate.py \
  tools/test_stage_device_hid_lease_gate.py \
  tools/test_stage_device_settings_gate.py \
    tools/test_stage_device_gadget_gate.py \
    tools/test_check_device_log_health.py \
    tools/test_license_inventory.py \
    tools/test_crate_licenses.py \
    tools/test_stage_device_system_action_store.py \
    tools/test_stage_device_system_action_maintenance.py \
    tools/test_stage_device_diagnostic_maintenance.py \
    tools/test_stage_device_diagnostic_persistence.py \
    tools/test_stage_device_diagnostic_collector.py \
    tools/test_stage_device_tls_browser.py \
    tools/test_build_update_test_fixture.py \
    tools/test_build_system_image_test_fixture.py \
    tools/build_system_raw_bundle.py \
    tools/test_build_system_raw_bundle.py \
    tools/ve_module.py \
    tools/test_ve_module.py
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
node tools/playwright/restricted_control_lifecycle.mjs
node tools/playwright/network_contract.mjs
node tools/playwright/firewall_modes_contract.mjs
node tools/playwright/webrtc_unavailable_contract.mjs
node tools/playwright/csp_contract.mjs
node tools/playwright/menu_autohide_contract.mjs
node tools/playwright/webrtc_quality_contract.mjs
node tools/playwright/h264_engine_menu_contract.mjs
node tools/playwright/quality_monitor_contract.mjs
node tools/playwright/theme_contract.mjs
node tools/playwright/mobile_keys_contract.mjs
node tools/playwright/touch_screen_reach_contract.mjs
# The page against the host's build of the gateway, and input through the
# host's builds of the gateway and the server.
cargo build --locked -p bli-kvm-quic-gateway -p bli-kvm-server
BLI_KVM_QUIC_GATEWAY="$target_root/debug/bli-kvm-quic-gateway" node tools/playwright/quic_transport_contract.mjs
BLI_KVM_QUIC_GATEWAY="$target_root/debug/bli-kvm-quic-gateway" node tools/playwright/quic_input_contract.mjs
BLI_KVM_QUIC_GATEWAY="$target_root/debug/bli-kvm-quic-gateway" BLI_KVM_SERVER="$target_root/debug/bli-kvm-server" node tools/playwright/quic_input_stack_contract.mjs
# The picture: the encoder's own x264 engine behind a frame socket, the
# gateway, a relay that loses datagrams, and Chromium's decoder.
cargo build --locked -p bli-kvm-video-encoder --features h264 --example h264_frame_source
BLI_KVM_QUIC_GATEWAY="$target_root/debug/bli-kvm-quic-gateway" BLI_KVM_H264_FRAME_SOURCE="$target_root/debug/examples/h264_frame_source" node tools/playwright/quic_video_contract.mjs
# The lab tool of the hardware H.264 plan: Chromium decodes what its
# software stand-in makes through the register plans of both open maps.
cargo build --locked -p bli-kvm-ve-lab
BLI_KVM_VE_LAB="$target_root/debug/bli-kvm-ve-lab" node tools/playwright/ve_standin_contract.mjs
# The register trace of the same plan: behind the closed window this
# project's own client must come out of the trace complete and in order, and
# the report must read a trace made by hand.
scripts/build-ve-trace.sh host "$target_root/ve-trace"
python3 -B -m unittest tools/ve-trace/test_ve_trace_report.py
# The encoder driver's checks and register plan, as the kernel module has
# them: random requests, and the plan against the lab tool's.
kernel/hardened-blikvm-ve/test/run.sh "$target_root/ve-enc-plan" "$target_root/debug/bli-kvm-ve-lab"
node tools/playwright/wifi_contract.mjs
node tools/playwright/video_settings_contract.mjs
pnpm_web exec prettier --check 'src/**/*.{js,jsx,ts,tsx,json,css,scss,md}'
pnpm_web exec prettier --check '../tools/playwright/*.mjs'
pnpm_web exec prettier --check '../tools/net/*.mjs'
pnpm_web exec prettier --check '../tools/device/quic_firefox_probe.mjs'
pnpm_web exec prettier --check '../tools/test_impair_proxies.mjs'
pnpm_web exec prettier --check '../tools/test_quic_video_receiver.mjs' '../tools/test_quic_board_clock.mjs' '../tools/test_quic_h264.mjs'
pnpm_web exec prettier --check '../tools/test_application_update_ui.mjs'
pnpm_web exec prettier --check '../tools/test_status_outage_audit.mjs'
pnpm_web exec prettier --check '../tools/test_gui_parity_probe.mjs'
pnpm_web exec prettier --check '../tools/test_gui_parity_app.mjs'
pnpm_web exec prettier --check '../tools/test_virtual_media_acceptance.mjs'
pnpm_web exec prettier --check '../tools/test_video_frame_timing_probe.mjs'
pnpm_web exec prettier --check '../tools/test_restricted_power_probe.mjs'
pnpm_web exec prettier --check '../tools/test_restricted_keyboard_probe.mjs'
pnpm_web exec prettier --check '../tools/test_virtual_media_probe_app.mjs'
pnpm_web exec prettier --check '../tools/playwright/restricted_power_probe.mjs'
pnpm_web exec prettier --check '../tools/playwright/app_harness.mjs'

git diff --check
