#!/bin/sh
# Build the restricted-control (HID+ATX) test bundle (test builds only).
#
# This bundle contains the feature-gated server and control broker plus the
# combined transition unit, its tmpfiles entry and the server drop-in that opts
# the retained application server into the restricted control surface. It is
# never a release artifact: the shipped prototype and application bundles keep
# the restricted control surface compiled out.
set -eu
umask 022

project_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
target_dir=${1:-/mnt/d/debug/blikvm/hardened-blikvm/target-aarch64-restricted-control}
output_dir=${2:-/mnt/d/debug/blikvm/hardened-blikvm/bundles}
pnpm_store=${BLI_KVM_PNPM_STORE:-/mnt/d/debug/blikvm/hardened-blikvm/pnpm-store}
bundle_name=hardened-blikvm-restricted-control-profile-aarch64
cargo_features=restricted-control-surface,hid-transition-profile,atx-transition-profile

for directory in "$target_dir" "$output_dir" "$pnpm_store"; do
  case "$directory" in
    /*) ;;
    *)
      echo "build paths must be absolute" >&2
      exit 2
      ;;
  esac
done

cd "$project_root"
BLI_KVM_CARGO_FEATURES="$cargo_features" \
  scripts/build-aarch64.sh "$target_dir"
PNPM_CONFIG_STORE_DIR="$pnpm_store" corepack pnpm --dir web build

release_dir="$target_dir/aarch64-unknown-linux-musl/release"
server_binary="$release_dir/bli-kvm-server"
broker_binary="$release_dir/bli-kvm-control-broker"

temp_root=$(mktemp -d /tmp/hardened-blikvm-restricted-control.XXXXXX)
cleanup() {
  case "$temp_root" in
    /tmp/hardened-blikvm-restricted-control.*) rm -rf -- "$temp_root" ;;
    *) echo "refusing unsafe temporary cleanup" >&2 ;;
  esac
}
trap cleanup EXIT HUP INT TERM

stage="$temp_root/$bundle_name"
install -d -m 0755 \
  "$stage/libexec" \
  "$stage/share/systemd" \
  "$stage/share/tmpfiles.d" \
  "$stage/share/server-dropin" \
  "$stage/share/web"
install -m 0755 "$server_binary" "$stage/libexec/bli-kvm-server"
install -m 0755 "$broker_binary" "$stage/libexec/bli-kvm-control-broker"
cp -a web/dist/. "$stage/share/web/"
install -m 0644 \
  packaging/systemd/hardened-blikvm-control-transition.service \
  "$stage/share/systemd/"
install -m 0644 \
  packaging/tmpfiles.d/hardened-blikvm-control-transition.conf \
  "$stage/share/tmpfiles.d/"
install -m 0644 \
  packaging/dropins/hardened-blikvm-server-current.service.d/90-restricted-control.conf \
  "$stage/share/server-dropin/"
install -m 0644 docs/api/control-v2.md "$stage/share/control-v2.md"
install -m 0644 docs/api/hid-transition.md "$stage/share/hid-transition.md"
install -m 0644 docs/api/atx-transition.md "$stage/share/atx-transition.md"

server_sha=$(sha256sum "$stage/libexec/bli-kvm-server" | cut -d ' ' -f 1)
server_size=$(wc -c < "$stage/libexec/bli-kvm-server" | tr -d ' ')
broker_sha=$(sha256sum "$stage/libexec/bli-kvm-control-broker" | cut -d ' ' -f 1)
broker_size=$(wc -c < "$stage/libexec/bli-kvm-control-broker" | tr -d ' ')

source_state=uncommitted-development-checkpoint
if git diff --quiet \
  && git diff --cached --quiet \
  && [ -z "$(git ls-files --others --exclude-standard)" ]; then
  source_state=committed-development-checkpoint
fi

web_index_sha=$(sha256sum "$stage/share/web/index.html" | cut -d ' ' -f 1)

jq -n \
  --arg server_sha "$server_sha" \
  --arg broker_sha "$broker_sha" \
  --arg source_state "$source_state" \
  --arg source_commit "$(git rev-parse HEAD)" \
  --arg cargo_features "$cargo_features" \
  --arg web_index_sha "$web_index_sha" \
  --argjson server_size "$server_size" \
  --argjson broker_size "$broker_size" \
  '{
    schemaVersion: 1,
    product: "Hardened BliKVM",
    profile: "restricted-control-test",
    sourceState: $source_state,
    sourceCommit: $source_commit,
    cargoFeatures: ($cargo_features | split(",")),
    releaseAccepted: false,
    deviceAccepted: false,
    artifacts: {
      server: {path: "libexec/bli-kvm-server", sha256: $server_sha, size: $server_size},
      controlBroker: {path: "libexec/bli-kvm-control-broker", sha256: $broker_sha, size: $broker_size},
      webIndex: {path: "share/web/index.html", sha256: $web_index_sha}
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
