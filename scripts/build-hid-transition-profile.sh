#!/bin/sh
# Build the HID transition-profile test bundle (test builds only).
#
# This bundle contains the feature-gated control broker plus the dedicated
# transition unit and its tmpfiles/sysusers entries. It is never a release
# artifact: the default prototype bundle keeps the transition profile
# compiled out.
set -eu
umask 022

project_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
target_dir=${1:-/mnt/d/debug/blikvm/hardened-blikvm/target-aarch64-hid-profile}
output_dir=${2:-/mnt/d/debug/blikvm/hardened-blikvm/bundles}
bundle_name=hardened-blikvm-hid-transition-profile-aarch64

for directory in "$target_dir" "$output_dir"; do
  case "$directory" in
    /*) ;;
    *)
      echo "build paths must be absolute" >&2
      exit 2
      ;;
  esac
done

cd "$project_root"
BLI_KVM_CARGO_FEATURES=hid-transition-profile \
  scripts/build-aarch64.sh "$target_dir"

release_dir="$target_dir/aarch64-unknown-linux-musl/release"
broker_binary="$release_dir/bli-kvm-control-broker"

temp_root=$(mktemp -d /tmp/hardened-blikvm-hid-profile.XXXXXX)
cleanup() {
  case "$temp_root" in
    /tmp/hardened-blikvm-hid-profile.*) rm -rf -- "$temp_root" ;;
    *) echo "refusing unsafe temporary cleanup" >&2 ;;
  esac
}
trap cleanup EXIT HUP INT TERM

stage="$temp_root/$bundle_name"
install -d -m 0755 \
  "$stage/libexec" \
  "$stage/share/systemd" \
  "$stage/share/tmpfiles.d" \
  "$stage/share/sysusers.d"
install -m 0755 "$broker_binary" "$stage/libexec/bli-kvm-control-broker"
install -m 0644 \
  packaging/systemd/hardened-blikvm-hid-transition.service \
  "$stage/share/systemd/"
install -m 0644 \
  packaging/tmpfiles.d/hardened-blikvm-hid.conf \
  "$stage/share/tmpfiles.d/"
install -m 0644 \
  packaging/sysusers.d/hardened-blikvm.conf \
  "$stage/share/sysusers.d/"
install -m 0644 docs/api/control-v2.md "$stage/share/control-v2.md"

broker_sha=$(sha256sum "$stage/libexec/bli-kvm-control-broker" | cut -d ' ' -f 1)
broker_size=$(wc -c < "$stage/libexec/bli-kvm-control-broker" | tr -d ' ')

source_state=uncommitted-development-checkpoint
if git diff --quiet \
  && git diff --cached --quiet \
  && [ -z "$(git ls-files --others --exclude-standard)" ]; then
  source_state=committed-development-checkpoint
fi

jq -n \
  --arg broker_sha "$broker_sha" \
  --arg source_state "$source_state" \
  --arg source_commit "$(git rev-parse HEAD)" \
  --argjson broker_size "$broker_size" \
  '{
    schemaVersion: 1,
    product: "Hardened BliKVM",
    profile: "hid-transition-test",
    sourceState: $source_state,
    sourceCommit: $source_commit,
    releaseAccepted: false,
    deviceAccepted: false,
    artifacts: {
      controlBroker: {path: "libexec/bli-kvm-control-broker", sha256: $broker_sha, size: $broker_size}
    }
  }' > "$stage/MANIFEST.json"
chmod 0644 "$stage/MANIFEST.json"

(
  cd "$stage"
  find . -type f ! -name SHA256SUMS -print0 \
    | sort -z \
    | xargs -0 sha256sum > SHA256SUMS
  chmod 0644 SHA256SUMS
)

mkdir -p "$output_dir"
archive="$output_dir/$bundle_name.tar.gz"
temporary_archive=$(mktemp "$output_dir/.${bundle_name}.XXXXXX.tar.gz")
tar \
  --sort=name \
  --mtime=@0 \
  --owner=0 \
  --group=0 \
  --numeric-owner \
  --format=posix \
  --pax-option=delete=atime,delete=ctime \
  -C "$temp_root" \
  -cf - \
  "$bundle_name" \
  | gzip -n -9 > "$temporary_archive"
mv -f -- "$temporary_archive" "$archive"
(
  cd "$output_dir"
  sha256sum "$bundle_name.tar.gz" > "$bundle_name.tar.gz.sha256"
)

sha256sum "$archive"
