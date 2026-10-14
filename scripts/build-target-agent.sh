#!/bin/sh
# Builds the target agent for Windows (docs/baselines/work-plan-20261014-target-agent.md): the
# x86_64-pc-windows-msvc target with cargo-xwin and its cached Windows SDK, the C runtime linked in,
# no debug information, the build's paths mapped away and the linker's timestamp made from the
# content (/Brepro), so that the same source gives the same bytes. The agent is copied to
# OUTPUT_DIR as hardened-blikvm-agent.exe and the channel's test tool as
# hardened-blikvm-agent-test.exe, and their SHA-256 printed. A tree with changes says so in the
# commit the programs name.
#
# usage: scripts/build-target-agent.sh OUTPUT_DIR
set -eu
out=${1:?usage: build-target-agent.sh OUTPUT_DIR}
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
commit=$(git rev-parse HEAD)
[ -z "$(git status --porcelain --untracked-files=no)" ] || commit="$commit-dirty"
target_dir=${CARGO_TARGET_DIR:-$root/target}/target-agent-windows
sysroot=$(rustc --print sysroot)
BLI_KVM_SOURCE_COMMIT=$commit CARGO_TARGET_DIR=$target_dir \
  RUSTFLAGS="-C target-feature=+crt-static -C link-arg=/Brepro -C link-arg=/DEBUG:NONE --remap-path-prefix=$root=/src --remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo --remap-path-prefix=$sysroot=/rustc" \
  cargo xwin build --release --locked --offline --target x86_64-pc-windows-msvc -p bli-kvm-target-agent
mkdir -p "$out"
cp "$target_dir/x86_64-pc-windows-msvc/release/bli-kvm-target-agent.exe" "$out/hardened-blikvm-agent.exe"
cp "$target_dir/x86_64-pc-windows-msvc/release/bli-kvm-target-agent-test.exe" "$out/hardened-blikvm-agent-test.exe"
(cd "$out" && sha256sum hardened-blikvm-agent.exe hardened-blikvm-agent-test.exe)
