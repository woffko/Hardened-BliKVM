#!/bin/sh
set -eu

BLI_KVM_PNPM_STORE=${BLI_KVM_PNPM_STORE:-/mnt/d/debug/blikvm/hardened-blikvm/pnpm-store}

pnpm_web() {
  PNPM_CONFIG_STORE_DIR="$BLI_KVM_PNPM_STORE" corepack pnpm --dir web "$@"
}

cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings

sh -n scripts/build-aarch64.sh
sh -n scripts/build-prototype-bundle.sh
sh -n scripts/check-source-boundary.sh
sh -n scripts/check.sh
sh -n scripts/rust-lld-linker.sh
sh -n scripts/verify-aarch64-elf.sh

scripts/check-source-boundary.sh

python3 -m unittest \
  tools/test_capture_pikvm_baseline.py \
  tools/test_capture_ssh_preflight.py \
  tools/test_stage_device_prototype.py \
  -v
python3 -m py_compile \
  tools/capture_pikvm_baseline.py \
  tools/capture_ssh_preflight.py \
  tools/stage_device_prototype.py \
  tools/test_capture_pikvm_baseline.py \
  tools/test_capture_ssh_preflight.py \
  tools/test_stage_device_prototype.py

if command -v basedpyright >/dev/null 2>&1; then
  basedpyright \
    tools/capture_pikvm_baseline.py \
    tools/capture_ssh_preflight.py \
    tools/stage_device_prototype.py \
    tools/test_capture_pikvm_baseline.py \
    tools/test_capture_ssh_preflight.py \
    tools/test_stage_device_prototype.py
else
  echo "basedpyright is required for the complete check" >&2
  exit 1
fi

pnpm_web build
pnpm_web lint
pnpm_web exec prettier --check 'src/**/*.{js,jsx,ts,tsx,json,css,scss,md}'

git diff --check
