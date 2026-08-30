#!/bin/sh
set -eu

target_dir=${1:-/mnt/d/debug/blikvm/hardened-blikvm/target-aarch64}
case "$target_dir" in
  /*) ;;
  *)
    echo "target directory must be absolute" >&2
    exit 2
    ;;
esac

mkdir -p "$target_dir"
CARGO_TARGET_DIR="$target_dir" cargo build \
  --workspace \
  --bins \
  --release \
  --locked \
  --target aarch64-unknown-linux-musl

release_dir="$target_dir/aarch64-unknown-linux-musl/release"
scripts/verify-aarch64-elf.sh \
  "$release_dir/bli-kvm-server" \
  "$release_dir/bli-kvm-platform-broker" \
  "$release_dir/bli-kvm-video-proxy"
sha256sum \
  "$release_dir/bli-kvm-server" \
  "$release_dir/bli-kvm-platform-broker" \
  "$release_dir/bli-kvm-video-proxy"
