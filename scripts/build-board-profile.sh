#!/bin/sh
# Build the board-owner test bundle (test builds only).
#
# This bundle contains the feature-gated server and the board-owner control
# broker built with the Rust gadget (media + HID) and the reviewed native ATX
# GPIO executor, plus the board-owner unit, its tmpfiles entry and an ATX
# timing template. It is never a release artifact: shipped builds keep the
# gadget and GPIO profiles compiled out, and the ATX timings are supplied by
# the operator at the physical gate.
set -eu
umask 022

project_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
target_dir=${1:-/mnt/d/debug/blikvm/hardened-blikvm/target-aarch64-board}
output_dir=${2:-/mnt/d/debug/blikvm/hardened-blikvm/bundles}
pnpm_store=${BLI_KVM_PNPM_STORE:-/mnt/d/debug/blikvm/hardened-blikvm/pnpm-store}
bundle_name=hardened-blikvm-board-profile-aarch64
cargo_features=restricted-control-surface,gadget-media-profile,gadget-hid-profile,chardev-atx-profile,system-action-profile,network-write-profile

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
boardd_binary="$release_dir/bli-kvm-boardd"
video_proxy_binary="$release_dir/bli-kvm-video-proxy"
bench_gate_binary="$release_dir/bli-kvm-atx-gpio-bench-gate"

temp_root=$(mktemp -d /tmp/hardened-blikvm-board-profile.XXXXXX)
cleanup() {
  case "$temp_root" in
    /tmp/hardened-blikvm-board-profile.*) rm -rf -- "$temp_root" ;;
    *) echo "refusing unsafe temporary cleanup" >&2 ;;
  esac
}
trap cleanup EXIT HUP INT TERM

stage="$temp_root/$bundle_name"
install -d -m 0755 \
  "$stage/libexec" \
  "$stage/share/systemd" \
  "$stage/share/tmpfiles.d" \
  "$stage/share/udev" \
  "$stage/share/web"
install -m 0755 "$server_binary" "$stage/libexec/bli-kvm-server"
install -m 0755 "$broker_binary" "$stage/libexec/bli-kvm-control-broker"
install -m 0755 "$video_proxy_binary" "$stage/libexec/bli-kvm-video-proxy"
install -m 0755 "$boardd_binary" "$stage/libexec/bli-kvm-boardd"
install -m 0755 "$bench_gate_binary" "$stage/libexec/bli-kvm-atx-gpio-bench-gate"
cp -a web/dist/. "$stage/share/web/"
install -m 0644 \
  packaging/systemd/hardened-blikvm-board-owner.service \
  packaging/systemd/hardened-blikvm-board-server.service \
  packaging/systemd/hardened-blikvm-board-video-proxy.service \
  packaging/systemd/hardened-blikvm-ustreamer.service \
  packaging/systemd/hardened-blikvm-boardd.service \
  packaging/systemd/hardened-blikvm-msd-writable.service \
  "$stage/share/systemd/"
install -d -m 0755 "$stage/share/systemd/systemd-journald.service.d"
install -m 0644 \
  packaging/systemd/systemd-journald.service.d/99-hardened-blikvm-persistent-journal.conf \
  "$stage/share/systemd/systemd-journald.service.d/"
install -m 0644 \
  packaging/udev/99-hardened-blikvm-video.rules \
  "$stage/share/udev/"
install -m 0644 \
  packaging/tmpfiles.d/hardened-blikvm-gadget.conf \
  "$stage/share/tmpfiles.d/"
install -m 0644 docs/api/control-v2.md "$stage/share/control-v2.md"
install -m 0644 docs/api/hid-transition.md "$stage/share/hid-transition.md"
install -m 0644 docs/api/atx-transition.md "$stage/share/atx-transition.md"
install -m 0644 docs/design/gui-parity-plan.md "$stage/share/gui-parity-plan.md"
install -m 0644 docs/design/phase3-5-device-gate-runbook.md \
  "$stage/share/device-gate-runbook.md"

install -m 0644 packaging/boardd.json.example "$stage/share/boardd.json.example"
cat > "$stage/share/system-action.env.example" <<'EOF'
# Reviewed system actions. The profile is compiled into the board build; it
# stays refused unless these are set, and the two commands are executed with no
# arguments, so use absolute paths to reviewed wrappers.
# BLI_KVM_SYSTEM_ACTION_PROFILE=1
# BLI_KVM_SYSTEM_ACTION_ARMED=1
# BLI_KVM_SYSTEM_ACTION_REBOOT_COMMAND=/usr/local/sbin/hardened-blikvm-reboot
# BLI_KVM_SYSTEM_ACTION_SHUTDOWN_COMMAND=/usr/local/sbin/hardened-blikvm-poweroff
EOF

cat > "$stage/share/atx.env.example" <<'EOF'
# Reviewed ATX pulse timings (milliseconds) and optional reconciliation path.
# Fill in the values confirmed on the board, then install as
# /etc/hardened-blikvm-gadget/atx.env
# BLI_KVM_ATX_POWER_SHORT_MS=
# BLI_KVM_ATX_POWER_LONG_MS=
# BLI_KVM_ATX_RESET_MS=
# BLI_KVM_ATX_RECONCILIATIONS=/etc/hardened-blikvm-gadget/atx-reconciliations.txt

# Server-side image upload (restricted surface only, POST /api/download/file):
# point the server at the
# same directory and catalog the board owner serves. Without this the upload
# endpoint answers 503.
# BLI_KVM_MEDIA_DIRECTORY=/var/lib/hardened-blikvm-gadget/images
# BLI_KVM_MEDIA_CATALOG=/etc/hardened-blikvm-gadget/catalog.json
# BLI_KVM_MEDIA_CONTROLS=1
EOF
chmod 0644 "$stage/share/atx.env.example"

server_sha=$(sha256sum "$stage/libexec/bli-kvm-server" | cut -d ' ' -f 1)
server_size=$(wc -c < "$stage/libexec/bli-kvm-server" | tr -d ' ')
broker_sha=$(sha256sum "$stage/libexec/bli-kvm-control-broker" | cut -d ' ' -f 1)
bench_gate_sha=$(sha256sum "$stage/libexec/bli-kvm-atx-gpio-bench-gate" | cut -d ' ' -f 1)
broker_size=$(wc -c < "$stage/libexec/bli-kvm-control-broker" | tr -d ' ')
bench_gate_size=$(wc -c < "$stage/libexec/bli-kvm-atx-gpio-bench-gate" | tr -d ' ')
web_index_sha=$(sha256sum "$stage/share/web/index.html" | cut -d ' ' -f 1)

source_state=uncommitted-development-checkpoint
if git diff --quiet \
  && git diff --cached --quiet \
  && [ -z "$(git ls-files --others --exclude-standard)" ]; then
  source_state=committed-development-checkpoint
fi

jq -n \
  --arg server_sha "$server_sha" \
  --arg broker_sha "$broker_sha" \
  --arg bench_gate_sha "$bench_gate_sha" \
  --arg source_state "$source_state" \
  --arg source_commit "$(git rev-parse HEAD)" \
  --arg cargo_features "$cargo_features" \
  --arg web_index_sha "$web_index_sha" \
  --argjson server_size "$server_size" \
  --argjson broker_size "$broker_size" \
  --argjson bench_gate_size "$bench_gate_size" \
  '{
    schemaVersion: 1,
    product: "Hardened BliKVM",
    profile: "board-owner-test",
    sourceState: $source_state,
    sourceCommit: $source_commit,
    cargoFeatures: ($cargo_features | split(",")),
    releaseAccepted: false,
    deviceAccepted: false,
    artifacts: {
      server: {path: "libexec/bli-kvm-server", sha256: $server_sha, size: $server_size},
      controlBroker: {path: "libexec/bli-kvm-control-broker", sha256: $broker_sha, size: $broker_size},
      atxGpioBenchGate: {path: "libexec/bli-kvm-atx-gpio-bench-gate", sha256: $bench_gate_sha, size: $bench_gate_size},
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
