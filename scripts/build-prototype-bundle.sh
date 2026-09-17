#!/bin/sh
set -eu
umask 022

project_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
target_dir=${1:-/mnt/d/debug/blikvm/hardened-blikvm/target-aarch64}
output_dir=${2:-/mnt/d/debug/blikvm/hardened-blikvm/bundles}
pnpm_store=${BLI_KVM_PNPM_STORE:-/mnt/d/debug/blikvm/hardened-blikvm/pnpm-store}
bundle_name=hardened-blikvm-prototype-0.1.0-dev-aarch64
# The shipped build carries the board profile: the Rust gadget (media + HID),
# the reviewed native ATX GPIO executor and the privileged system-action
# profile, so the signed slot serves our board arrangement.
cargo_features=${BLI_KVM_CARGO_FEATURES:-restricted-control-surface,gadget-media-profile,gadget-hid-profile,chardev-atx-profile,system-action-profile}

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
BLI_KVM_CARGO_FEATURES="$cargo_features" scripts/build-aarch64.sh "$target_dir"
PNPM_CONFIG_STORE_DIR="$pnpm_store" corepack pnpm --dir web build

release_dir="$target_dir/aarch64-unknown-linux-musl/release"
server_binary="$release_dir/bli-kvm-server"
broker_binary="$release_dir/bli-kvm-platform-broker"
video_proxy_binary="$release_dir/bli-kvm-video-proxy"
control_broker_binary="$release_dir/bli-kvm-control-broker"
boardd_binary="$release_dir/bli-kvm-boardd"
hid_credential_gate_binary="$release_dir/bli-kvm-hid-credential-gate"
hid_isolated_gate_binary="$release_dir/bli-kvm-hid-isolated-gate"
atx_live_gate_binary="$release_dir/bli-kvm-atx-live-gate"
media_isolated_gate_binary="$release_dir/bli-kvm-media-isolated-gate"
update_supervisor_binary="$release_dir/bli-kvm-update-supervisor"
update_health_reporter_binary="$release_dir/bli-kvm-update-health-reporter"
scripts/verify-aarch64-elf.sh "$server_binary" "$broker_binary" "$video_proxy_binary" "$control_broker_binary" "$boardd_binary" "$hid_credential_gate_binary" "$hid_isolated_gate_binary" "$atx_live_gate_binary" "$media_isolated_gate_binary" "$update_supervisor_binary" "$update_health_reporter_binary"

temp_root=$(mktemp -d /tmp/hardened-blikvm-bundle.XXXXXX)
cleanup() {
  case "$temp_root" in
    /tmp/hardened-blikvm-bundle.*) rm -rf -- "$temp_root" ;;
    *) echo "refusing unsafe temporary cleanup" >&2 ;;
  esac
}
trap cleanup EXIT HUP INT TERM

stage="$temp_root/$bundle_name"
install -d -m 0755 \
  "$stage/bin" \
  "$stage/libexec" \
  "$stage/share/web" \
  "$stage/share/nginx" \
  "$stage/share/systemd" \
  "$stage/share/sysusers.d" \
  "$stage/share/tmpfiles.d" \
  "$stage/share/udev" \
  "$stage/share/docs"
install -m 0755 "$server_binary" "$stage/bin/bli-kvm-server"
install -m 0755 "$broker_binary" "$stage/libexec/bli-kvm-platform-broker"
install -m 0755 "$video_proxy_binary" "$stage/libexec/bli-kvm-video-proxy"
install -m 0755 "$control_broker_binary" "$stage/libexec/bli-kvm-control-broker"
install -m 0755 "$boardd_binary" "$stage/libexec/bli-kvm-boardd"
install -m 0755 "$hid_credential_gate_binary" "$stage/libexec/bli-kvm-hid-credential-gate"
install -m 0755 "$hid_isolated_gate_binary" "$stage/libexec/bli-kvm-hid-isolated-gate"
install -m 0755 "$atx_live_gate_binary" "$stage/libexec/bli-kvm-atx-live-gate"
install -m 0755 "$media_isolated_gate_binary" "$stage/libexec/bli-kvm-media-isolated-gate"
install -m 0755 "$update_supervisor_binary" "$stage/libexec/bli-kvm-update-supervisor"
install -m 0755 "$update_health_reporter_binary" "$stage/libexec/bli-kvm-update-health-reporter"
cp -a web/dist/. "$stage/share/web/"
rm -f -- "$stage/share/web/mockServiceWorker.js"
find "$stage/share/web" -type d -exec chmod 0755 {} +
find "$stage/share/web" -type f -exec chmod 0644 {} +
install -m 0644 \
  packaging/nginx/hardened-blikvm.conf \
  "$stage/share/nginx/"
install -m 0644 \
  packaging/systemd/hardened-blikvm-platform-broker.service \
  packaging/systemd/hardened-blikvm-control-broker.service \
  packaging/systemd/hardened-blikvm-hid-credential.service \
  packaging/systemd/hardened-blikvm-video-proxy.service \
  packaging/systemd/hardened-blikvm-server.service \
  packaging/systemd/hardened-blikvm-nginx.service \
  packaging/systemd/hardened-blikvm-board-owner.service \
  packaging/systemd/hardened-blikvm-board-server.service \
  packaging/systemd/hardened-blikvm-board-video-proxy.service \
  packaging/systemd/hardened-blikvm-boardd.service \
  packaging/systemd/hardened-blikvm-ustreamer.service \
  packaging/systemd/hardened-blikvm-update-watchdog.service \
  packaging/systemd/hardened-blikvm-update-recover.service \
  packaging/systemd/hardened-blikvm-update-status.socket \
  packaging/systemd/hardened-blikvm-update-status@.service \
  packaging/systemd/hardened-blikvm-update-upload.socket \
  packaging/systemd/hardened-blikvm-update-upload@.service \
  packaging/systemd/hardened-blikvm-update-activation.socket \
  packaging/systemd/hardened-blikvm-update-activation@.service \
  packaging/systemd/hardened-blikvm-update-confirm@.service \
  packaging/systemd/hardened-blikvm-update-activate@.service \
  packaging/systemd/hardened-blikvm-update-apply-candidate.service \
  packaging/systemd/hardened-blikvm-update-apply-confirmed.service \
  packaging/systemd/hardened-blikvm-server-current.service \
  packaging/systemd/hardened-blikvm-server-candidate.service \
  "$stage/share/systemd/"
install -m 0644 \
  packaging/systemd/hardened-blikvm-update-watchdog.timer \
  packaging/systemd/hardened-blikvm-update-confirm.socket \
  "$stage/share/systemd/"
install -m 0644 \
  packaging/sysusers.d/hardened-blikvm.conf \
  "$stage/share/sysusers.d/"
install -m 0644 \
  packaging/tmpfiles.d/hardened-blikvm-update.conf \
  packaging/tmpfiles.d/hardened-blikvm-gadget.conf \
  "$stage/share/tmpfiles.d/"
install -m 0644 \
  packaging/udev/99-hardened-blikvm-video.rules \
  "$stage/share/udev/"
install -m 0644 LICENSE "$stage/LICENSE"
install -m 0644 packaging/PROTOTYPE-README.md "$stage/README.md"
install -m 0644 docs/architecture.md "$stage/share/docs/architecture.md"
install -m 0644 docs/security-boundaries.md "$stage/share/docs/security-boundaries.md"
install -m 0644 docs/api/broker-v1.md "$stage/share/docs/broker-v1.md"
install -m 0644 docs/api/inventory-v1.md "$stage/share/docs/inventory-v1.md"
install -m 0644 docs/api/inventory-v2.md "$stage/share/docs/inventory-v2.md"
install -m 0644 docs/api/inventory-v3.md "$stage/share/docs/inventory-v3.md"
install -m 0644 docs/api/control-v1.md "$stage/share/docs/control-v1.md"
install -m 0644 docs/api/diagnostics-v1.md "$stage/share/docs/diagnostics-v1.md"
install -m 0644 docs/api/hid-transition.md "$stage/share/docs/hid-transition.md"
install -m 0644 docs/api/atx-transition.md "$stage/share/docs/atx-transition.md"
install -m 0644 docs/api/media-transition.md "$stage/share/docs/media-transition.md"
install -m 0644 docs/api/application-update-v1.md "$stage/share/docs/application-update-v1.md"
install -m 0644 docs/api/application-activation-v1.md "$stage/share/docs/application-activation-v1.md"
install -m 0644 packaging/README.md "$stage/share/docs/packaging-boundary.md"

for legacy in hardened-logo.png sipeed.ico mockServiceWorker.js; do
  if find "$stage/share/web" -name "$legacy" -print -quit | rg -q .; then
    echo "legacy or development asset entered the bundle: $legacy" >&2
    exit 1
  fi
done
if rg -n -i 'NanoKVM|Hardened_NanoKVM|wiki\.sipeed\.com' "$stage/share/web"; then
  echo "legacy user-visible branding entered the bundle" >&2
  exit 1
fi
if rg -l '10\.0\.|/home/w0w|/mnt/d|BEGIN [A-Z ]*PRIVATE KEY' "$stage"; then
  echo "private or host-specific content entered the bundle" >&2
  exit 1
fi

server_sha=$(sha256sum "$stage/bin/bli-kvm-server" | cut -d ' ' -f 1)
broker_sha=$(sha256sum "$stage/libexec/bli-kvm-platform-broker" | cut -d ' ' -f 1)
video_proxy_sha=$(sha256sum "$stage/libexec/bli-kvm-video-proxy" | cut -d ' ' -f 1)
control_broker_sha=$(sha256sum "$stage/libexec/bli-kvm-control-broker" | cut -d ' ' -f 1)
hid_credential_gate_sha=$(sha256sum "$stage/libexec/bli-kvm-hid-credential-gate" | cut -d ' ' -f 1)
hid_isolated_gate_sha=$(sha256sum "$stage/libexec/bli-kvm-hid-isolated-gate" | cut -d ' ' -f 1)
atx_live_gate_sha=$(sha256sum "$stage/libexec/bli-kvm-atx-live-gate" | cut -d ' ' -f 1)
media_isolated_gate_sha=$(sha256sum "$stage/libexec/bli-kvm-media-isolated-gate" | cut -d ' ' -f 1)
update_supervisor_sha=$(sha256sum "$stage/libexec/bli-kvm-update-supervisor" | cut -d ' ' -f 1)
update_health_reporter_sha=$(sha256sum "$stage/libexec/bli-kvm-update-health-reporter" | cut -d ' ' -f 1)
cargo_lock_sha=$(sha256sum Cargo.lock | cut -d ' ' -f 1)
pnpm_lock_sha=$(sha256sum web/pnpm-lock.yaml | cut -d ' ' -f 1)
web_tree_sha=$(
  cd "$stage/share/web"
  find . -type f -print0 | sort -z | xargs -0 sha256sum | sha256sum | cut -d ' ' -f 1
)
server_size=$(wc -c < "$stage/bin/bli-kvm-server" | tr -d ' ')
broker_size=$(wc -c < "$stage/libexec/bli-kvm-platform-broker" | tr -d ' ')
video_proxy_size=$(wc -c < "$stage/libexec/bli-kvm-video-proxy" | tr -d ' ')
control_broker_size=$(wc -c < "$stage/libexec/bli-kvm-control-broker" | tr -d ' ')
hid_credential_gate_size=$(wc -c < "$stage/libexec/bli-kvm-hid-credential-gate" | tr -d ' ')
hid_isolated_gate_size=$(wc -c < "$stage/libexec/bli-kvm-hid-isolated-gate" | tr -d ' ')
atx_live_gate_size=$(wc -c < "$stage/libexec/bli-kvm-atx-live-gate" | tr -d ' ')
media_isolated_gate_size=$(wc -c < "$stage/libexec/bli-kvm-media-isolated-gate" | tr -d ' ')
update_supervisor_size=$(wc -c < "$stage/libexec/bli-kvm-update-supervisor" | tr -d ' ')
update_health_reporter_size=$(wc -c < "$stage/libexec/bli-kvm-update-health-reporter" | tr -d ' ')

source_state=uncommitted-development-checkpoint
source_commit=
if git diff --quiet \
  && git diff --cached --quiet \
  && [ -z "$(git ls-files --others --exclude-standard)" ]; then
  source_state=committed-development-checkpoint
  source_commit=$(git rev-parse HEAD)
fi

jq -n \
  --arg server_sha "$server_sha" \
  --arg broker_sha "$broker_sha" \
  --arg video_proxy_sha "$video_proxy_sha" \
  --arg control_broker_sha "$control_broker_sha" \
  --arg hid_credential_gate_sha "$hid_credential_gate_sha" \
  --arg hid_isolated_gate_sha "$hid_isolated_gate_sha" \
  --arg atx_live_gate_sha "$atx_live_gate_sha" \
  --arg media_isolated_gate_sha "$media_isolated_gate_sha" \
  --arg update_supervisor_sha "$update_supervisor_sha" \
  --arg update_health_reporter_sha "$update_health_reporter_sha" \
  --arg cargo_lock_sha "$cargo_lock_sha" \
  --arg pnpm_lock_sha "$pnpm_lock_sha" \
  --arg web_tree_sha "$web_tree_sha" \
  --arg source_state "$source_state" \
  --arg source_commit "$source_commit" \
  --argjson server_size "$server_size" \
  --argjson broker_size "$broker_size" \
  --argjson video_proxy_size "$video_proxy_size" \
  --argjson control_broker_size "$control_broker_size" \
  --argjson hid_credential_gate_size "$hid_credential_gate_size" \
  --argjson hid_isolated_gate_size "$hid_isolated_gate_size" \
  --argjson atx_live_gate_size "$atx_live_gate_size" \
  --argjson media_isolated_gate_size "$media_isolated_gate_size" \
  --argjson update_supervisor_size "$update_supervisor_size" \
  --argjson update_health_reporter_size "$update_health_reporter_size" \
  '{
    schemaVersion: 1,
    product: "Hardened BliKVM",
    version: "0.1.0-dev",
    target: "aarch64-unknown-linux-musl",
    sourceState: $source_state,
    sourceCommit: (if $source_commit == "" then null else $source_commit end),
    releaseAccepted: false,
    deviceAccepted: false,
    artifacts: {
      server: {path: "bin/bli-kvm-server", sha256: $server_sha, size: $server_size},
      broker: {path: "libexec/bli-kvm-platform-broker", sha256: $broker_sha, size: $broker_size},
      videoProxy: {path: "libexec/bli-kvm-video-proxy", sha256: $video_proxy_sha, size: $video_proxy_size},
      controlBroker: {path: "libexec/bli-kvm-control-broker", sha256: $control_broker_sha, size: $control_broker_size},
      hidCredentialGate: {path: "libexec/bli-kvm-hid-credential-gate", sha256: $hid_credential_gate_sha, size: $hid_credential_gate_size},
      hidIsolatedGate: {path: "libexec/bli-kvm-hid-isolated-gate", sha256: $hid_isolated_gate_sha, size: $hid_isolated_gate_size},
      atxLiveGate: {path: "libexec/bli-kvm-atx-live-gate", sha256: $atx_live_gate_sha, size: $atx_live_gate_size},
      mediaIsolatedGate: {path: "libexec/bli-kvm-media-isolated-gate", sha256: $media_isolated_gate_sha, size: $media_isolated_gate_size},
      updateSupervisor: {path: "libexec/bli-kvm-update-supervisor", sha256: $update_supervisor_sha, size: $update_supervisor_size},
      updateHealthReporter: {path: "libexec/bli-kvm-update-health-reporter", sha256: $update_health_reporter_sha, size: $update_health_reporter_size},
      web: {path: "share/web", treeSha256: $web_tree_sha}
    },
    locks: {cargo: $cargo_lock_sha, pnpm: $pnpm_lock_sha}
  }' > "$stage/MANIFEST.json"
chmod 0644 "$stage/MANIFEST.json"

(
  cd "$stage"
  find . -type f ! -name SHA256SUMS -print0 \
    | sort -z \
    | xargs -0 sha256sum > SHA256SUMS
  chmod 0644 SHA256SUMS
)

test "$(stat -c '%a' "$stage/bin/bli-kvm-server")" = 755
test "$(stat -c '%a' "$stage/libexec/bli-kvm-platform-broker")" = 755
test "$(stat -c '%a' "$stage/libexec/bli-kvm-video-proxy")" = 755
test "$(stat -c '%a' "$stage/libexec/bli-kvm-control-broker")" = 755
test "$(stat -c '%a' "$stage/libexec/bli-kvm-hid-credential-gate")" = 755
test "$(stat -c '%a' "$stage/libexec/bli-kvm-hid-isolated-gate")" = 755
test "$(stat -c '%a' "$stage/libexec/bli-kvm-atx-live-gate")" = 755
test "$(stat -c '%a' "$stage/libexec/bli-kvm-media-isolated-gate")" = 755
test "$(stat -c '%a' "$stage/libexec/bli-kvm-update-supervisor")" = 755
test "$(stat -c '%a' "$stage/libexec/bli-kvm-update-health-reporter")" = 755
test "$(stat -c '%a' "$stage/MANIFEST.json")" = 644
test "$(stat -c '%a' "$stage/SHA256SUMS")" = 644
test "$(stat -c '%a' "$stage/share/web")" = 755
test "$(stat -c '%a' "$stage/share/web/index.html")" = 644

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

tar -tzf "$archive" | sort
sha256sum "$archive"
