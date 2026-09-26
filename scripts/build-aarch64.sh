#!/bin/sh
set -eu

project_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
target_dir=${1:-/mnt/d/debug/blikvm/hardened-blikvm/target-aarch64}
case "$target_dir" in
  /*) ;;
  *)
    echo "target directory must be absolute" >&2
    exit 2
    ;;
esac

rust_sysroot=$(rustc --print sysroot)
cargo_home=$(
  cargo metadata --format-version 1 --locked \
    | jq -r '[.packages[].manifest_path | capture("^(?<root>.+)/registry/src/").root][0] // empty'
)
case "$rust_sysroot" in /*) ;; *) echo "Rust sysroot must be absolute" >&2; exit 1 ;; esac
case "$cargo_home" in ""|/*) ;; *) echo "Cargo source root must be absolute" >&2; exit 1 ;; esac

mkdir -p "$target_dir"
BLI_KVM_REMAP_PROJECT_ROOT="$project_root" \
BLI_KVM_REMAP_RUST_SYSROOT="$rust_sysroot" \
BLI_KVM_REMAP_CARGO_HOME="$cargo_home" \
RUSTC_WRAPPER="$project_root/scripts/rustc-remap-wrapper.sh" \
CARGO_TARGET_DIR="$target_dir" cargo build \
  --workspace \
  --release \
  --locked \
  ${BLI_KVM_CARGO_FEATURES:+--features "$BLI_KVM_CARGO_FEATURES"} \
  --target aarch64-unknown-linux-musl

release_dir="$target_dir/aarch64-unknown-linux-musl/release"
scripts/verify-aarch64-elf.sh \
  "$release_dir/bli-kvm-server" \
  "$release_dir/bli-kvm-platform-broker" \
  "$release_dir/bli-kvm-video-proxy" \
  "$release_dir/bli-kvm-control-broker" \
  "$release_dir/bli-kvm-update-supervisor" \
  "$release_dir/bli-kvm-update-health-reporter" \
  "$release_dir/bli-kvm-update-exec-helper" \
  "$release_dir/bli-kvm-admin-helper" \
  "$release_dir/bli-kvm-update-trust-config"
sha256sum \
  "$release_dir/bli-kvm-server" \
  "$release_dir/bli-kvm-platform-broker" \
  "$release_dir/bli-kvm-video-proxy" \
  "$release_dir/bli-kvm-control-broker" \
  "$release_dir/bli-kvm-update-supervisor" \
  "$release_dir/bli-kvm-update-health-reporter" \
  "$release_dir/bli-kvm-update-exec-helper" \
  "$release_dir/bli-kvm-admin-helper" \
  "$release_dir/bli-kvm-update-trust-config"
