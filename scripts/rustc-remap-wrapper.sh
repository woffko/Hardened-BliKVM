#!/bin/sh
set -eu

if [ "$#" -lt 1 ]; then
  echo "rustc wrapper requires the real compiler" >&2
  exit 2
fi

rustc=$1
shift

case "${BLI_KVM_REMAP_PROJECT_ROOT:-}" in /*) ;; *) echo "invalid project remap root" >&2; exit 2 ;; esac
case "${BLI_KVM_REMAP_RUST_SYSROOT:-}" in /*) ;; *) echo "invalid Rust sysroot remap" >&2; exit 2 ;; esac

set -- "$@" \
  "--remap-path-prefix=$BLI_KVM_REMAP_PROJECT_ROOT=/usr/src/hardened-blikvm" \
  "--remap-path-prefix=$BLI_KVM_REMAP_RUST_SYSROOT=/usr/src/rust-toolchain"

if [ -n "${BLI_KVM_REMAP_CARGO_HOME:-}" ]; then
  case "$BLI_KVM_REMAP_CARGO_HOME" in /*) ;; *) echo "invalid Cargo remap root" >&2; exit 2 ;; esac
  set -- "$@" "--remap-path-prefix=$BLI_KVM_REMAP_CARGO_HOME=/usr/src/cargo-home"
fi

exec "$rustc" "$@"
