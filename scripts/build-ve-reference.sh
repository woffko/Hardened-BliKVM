#!/bin/sh
# Build the x264 reference of the hardware H.264 plan's lab for the board:
# the product's own x264 wrapper over pictures from a file
# (crates/bli-kvm-video-encoder/examples/h264_reference.rs), one static
# aarch64 binary. It is a lab instrument and goes into no package; a lab
# session copies it to the board's tmpfs.
#
# usage: build-ve-reference.sh TARGET_DIR
#
# The static libraries the encoder links come from where the product's
# build takes them: BLI_KVM_LIBVPX_DIR, BLI_KVM_X264_DIR and
# BLI_KVM_LIBJPEG_TURBO_DIR name their aarch64 builds
# (scripts/build-libvpx.sh, build-x264.sh, build-libjpeg-turbo.sh), and
# BLI_KVM_MUSL_SYSROOT the musl headers they were built against
# (scripts/musl-headers.sh), for the encoder's own C files, which clang
# compiles. The paths of the build machine are mapped away as for the
# product's binaries, so TARGET_DIR must be one that only this script builds
# into.
set -eu

project_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
target_dir=${1:-}
case "$target_dir" in /*) ;; *) echo "usage: $0 TARGET_DIR (absolute)" >&2; exit 2 ;; esac
for name in BLI_KVM_LIBVPX_DIR BLI_KVM_X264_DIR BLI_KVM_LIBJPEG_TURBO_DIR; do
  eval "value=\${$name:-}"
  [ -d "$value/lib" ] || { echo "$name must name an aarch64 build of the library" >&2; exit 2; }
done
[ -d "${BLI_KVM_MUSL_SYSROOT:-}/include" ] || { echo "BLI_KVM_MUSL_SYSROOT must name the musl headers" >&2; exit 2; }
cd "$project_root"
resource_dir=$(clang -print-resource-dir)
CC_aarch64_unknown_linux_musl=clang
CFLAGS_aarch64_unknown_linux_musl="--target=aarch64-linux-musl -nostdinc -isystem $BLI_KVM_MUSL_SYSROOT/include -isystem $resource_dir/include -ffile-prefix-map=$project_root=/src"
export CC_aarch64_unknown_linux_musl CFLAGS_aarch64_unknown_linux_musl
rust_sysroot=$(rustc --print sysroot)
cargo_home=$(
  cargo metadata --format-version 1 --locked --offline \
    | jq -r '[.packages[].manifest_path | capture("^(?<root>.+)/registry/src/").root][0] // empty'
)
BLI_KVM_REMAP_PROJECT_ROOT="$project_root" \
BLI_KVM_REMAP_RUST_SYSROOT="$rust_sysroot" \
BLI_KVM_REMAP_CARGO_HOME="$cargo_home" \
RUSTC_WRAPPER="$project_root/scripts/rustc-remap-wrapper.sh" \
CARGO_TARGET_DIR="$target_dir" cargo build \
  --release --locked --offline --target aarch64-unknown-linux-musl \
  -p bli-kvm-video-encoder --features h264,turbojpeg --example h264_reference
binary="$target_dir/aarch64-unknown-linux-musl/release/examples/h264_reference"
scripts/verify-aarch64-elf.sh "$binary"
echo "$(sha256sum "$binary" | cut -c1-64)  $binary"
