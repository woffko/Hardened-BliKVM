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
# ring, in the QUIC gateway's TLS, compiles C and assembly for the board:
# clang against the pinned musl headers, the way the encoder libraries are
# built. Its objects carry no path of this machine.
musl_headers="$target_dir/musl-1.2.5-headers"
if [ ! -f "$musl_headers/include/stdint.h" ]; then
  scripts/musl-headers.sh "$musl_headers"
fi
clang_resources=$(clang -print-resource-dir)
CC_aarch64_unknown_linux_musl=clang
AR_aarch64_unknown_linux_musl=llvm-ar-18
CFLAGS_aarch64_unknown_linux_musl="--target=aarch64-linux-musl --sysroot=$musl_headers -nostdinc -isystem $musl_headers/include -isystem $clang_resources/include -ffile-prefix-map=$target_dir=/target -ffile-prefix-map=${cargo_home:-/nonexistent}=/cargo -ffile-prefix-map=$project_root=/src"
export CC_aarch64_unknown_linux_musl AR_aarch64_unknown_linux_musl CFLAGS_aarch64_unknown_linux_musl
# The board's owner names the commit it was built from in its hello, as the
# target agent built from the same tree does (scripts/build-target-agent.sh),
# so that the agent can say when it is not the board's
# (docs/design/target-agent-v1.md).
source_commit=$(git -C "$project_root" rev-parse HEAD 2>/dev/null || echo unknown)
if [ "$source_commit" != unknown ] \
  && [ -n "$(git -C "$project_root" status --porcelain --untracked-files=no)" ]; then
  source_commit="$source_commit-dirty"
fi
BLI_KVM_SOURCE_COMMIT="$source_commit" \
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
  "$release_dir/bli-kvm-system-update" \
  "$release_dir/bli-kvm-quic-gateway" \
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
  "$release_dir/bli-kvm-system-update" \
  "$release_dir/bli-kvm-quic-gateway" \
  "$release_dir/bli-kvm-update-trust-config"
