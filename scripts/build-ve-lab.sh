#!/bin/sh
# Build the lab tool of the hardware H.264 plan for the board: one static
# aarch64 binary. It is a lab instrument and goes into no package; a lab
# session copies it to the board's tmpfs.
#
# usage: build-ve-lab.sh TARGET_DIR
#
# The paths of the build machine are mapped away as for the product's
# binaries. Cargo does not notice the wrapper that does it, so TARGET_DIR
# must be one that only this script builds into.
set -eu

project_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
target_dir=${1:-}
case "$target_dir" in /*) ;; *) echo "usage: $0 TARGET_DIR (absolute)" >&2; exit 2 ;; esac
cd "$project_root"
commit=$(git rev-parse --short HEAD)
[ -z "$(git status --porcelain -- crates/bli-kvm-ve-lab)" ] || commit="$commit+"
rust_sysroot=$(rustc --print sysroot)
cargo_home=$(
  cargo metadata --format-version 1 --locked --offline \
    | jq -r '[.packages[].manifest_path | capture("^(?<root>.+)/registry/src/").root][0] // empty'
)
BLI_KVM_REMAP_PROJECT_ROOT="$project_root" \
BLI_KVM_REMAP_RUST_SYSROOT="$rust_sysroot" \
BLI_KVM_REMAP_CARGO_HOME="$cargo_home" \
RUSTC_WRAPPER="$project_root/scripts/rustc-remap-wrapper.sh" \
BLI_KVM_VE_LAB_COMMIT="$commit" CARGO_TARGET_DIR="$target_dir" cargo build \
  --release --locked --offline --target aarch64-unknown-linux-musl -p bli-kvm-ve-lab
binary="$target_dir/aarch64-unknown-linux-musl/release/bli-kvm-ve-lab"
scripts/verify-aarch64-elf.sh "$binary"
echo "$(sha256sum "$binary" | cut -c1-64)  $binary ($commit)"
