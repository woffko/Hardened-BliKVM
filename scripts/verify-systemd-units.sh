#!/bin/sh
set -eu

project_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
# Every unit of packaging/systemd is verified and every service has its
# exposure checked: the lists come from the directory, so a unit added there
# cannot pass unchecked (security review of 2026-10-18, LOW-07).
units=$(cd "$project_root/packaging/systemd" && ls -1 -- *.service *.socket *.timer)
services=$(cd "$project_root/packaging/systemd" && ls -1 -- *.service native/*.service)
# The exposure a service may have (systemd-analyze security, tenths); 20
# unless named here with its reason.
exposure_limit() {
  case "$1" in
    # Root boundaries with required capabilities: public TLS termination and
    # the native execution helper (systemd control, DAC and ptrace identity
    # reads).
    hardened-blikvm-update-exec@.service | hardened-blikvm-nginx.service) echo 22 ;;
    # One remount, which must happen in the host's mount namespace: no file
    # system sandbox.
    hardened-blikvm-msd-writable.service) echo 37 ;;
    # The operator's root session (the web terminal), a root shell by design.
    hardened-blikvm-admin@.service) echo 94 ;;
    *) echo 20 ;;
  esac
}
if [ "${1:-}" = --list ]; then
  for unit in $units; do printf 'verify %s\n' "$unit"; done
  for service in $services; do
    printf 'exposure %s %s\n' "$service" "$(exposure_limit "${service##*/}")"
  done
  exit 0
fi
test_root=$(mktemp -d /tmp/hardened-blikvm-systemd.XXXXXX)
cleanup() {
  case "$test_root" in
    /tmp/hardened-blikvm-systemd.*) rm -rf -- "$test_root" ;;
    *) echo "refusing unsafe systemd test cleanup" >&2 ;;
  esac
}
trap cleanup EXIT HUP INT TERM

install -d -m 0755 \
  "$test_root/bin" \
  "$test_root/etc" \
  "$test_root/usr/bin" \
  "$test_root/usr/sbin" \
  "$test_root/usr/lib/systemd/system" \
  "$test_root/usr/lib/sysusers.d" \
  "$test_root/usr/lib/tmpfiles.d" \
  "$test_root/usr/libexec/hardened-blikvm" \
  "$test_root/usr/share/hardened-blikvm/web"
install -m 0755 /bin/true "$test_root/bin/true"
# The board server waits for the owner socket with a bounded shell loop.
install -m 0755 /bin/sh "$test_root/bin/sh"
install -m 0755 /bin/true "$test_root/usr/libexec/hardened-blikvm/bli-kvm-platform-broker"
install -m 0755 /bin/true "$test_root/usr/libexec/hardened-blikvm/bli-kvm-control-broker"
install -m 0755 /bin/true "$test_root/usr/libexec/hardened-blikvm/bli-kvm-hid-credential-gate"
install -m 0755 /bin/true "$test_root/usr/libexec/hardened-blikvm/bli-kvm-video-proxy"
install -m 0755 /bin/true "$test_root/usr/libexec/hardened-blikvm/bli-kvm-server"
install -m 0755 /bin/true "$test_root/usr/libexec/hardened-blikvm/bli-kvm-update-supervisor"
install -m 0755 /bin/true "$test_root/usr/libexec/hardened-blikvm/bli-kvm-update-health-reporter"
install -m 0755 /bin/true "$test_root/usr/libexec/hardened-blikvm/bli-kvm-diagnostic-collector"
install -m 0755 /bin/true "$test_root/usr/libexec/hardened-blikvm/bli-kvm-diagnostic-maintenance"
install -m 0755 /bin/true "$test_root/usr/libexec/hardened-blikvm/bli-kvm-diagnostic-persistence-gate"
install -m 0755 /bin/true "$test_root/usr/libexec/hardened-blikvm/bli-kvm-system-action-maintenance"
install -m 0755 /bin/true "$test_root/usr/libexec/hardened-blikvm/bli-kvm-boardd"
install -m 0755 /bin/true "$test_root/usr/libexec/hardened-blikvm/bli-kvm-update-exec-helper"
install -m 0755 /bin/true "$test_root/usr/libexec/hardened-blikvm/bli-kvm-admin-helper"
install -m 0755 /bin/true "$test_root/usr/libexec/hardened-blikvm/bli-kvm-system-update"
install -m 0755 /bin/true "$test_root/usr/libexec/hardened-blikvm/bli-kvm-quic-gateway"
install -m 0755 /bin/true "$test_root/usr/libexec/hardened-blikvm/bli-kvm-ve-load"
install -m 0755 /bin/true "$test_root/usr/libexec/hardened-blikvm/bli-kvm-video-encoder"
# The PicoClaw runtime is downloaded at run time, not packaged.
install -d -m 0755 "$test_root/var/lib/hardened-blikvm-picoclaw-runtime"
install -m 0755 /bin/true "$test_root/var/lib/hardened-blikvm-picoclaw-runtime/picoclaw"
install -m 0755 /bin/true "$test_root/usr/bin/ustreamer"
install -m 0755 /bin/true "$test_root/usr/bin/janus"
install -m 0755 /bin/true "$test_root/bin/mount"
install -m 0755 /bin/true "$test_root/usr/bin/systemctl"
install -m 0755 /bin/true "$test_root/usr/bin/install"
install -m 0755 /bin/true "$test_root/usr/sbin/nginx"
install -m 0644 \
  "$project_root"/packaging/systemd/*.service \
  "$project_root"/packaging/systemd/*.socket \
  "$project_root"/packaging/systemd/*.timer \
  "$test_root/usr/lib/systemd/system/"
install -m 0644 \
  "$project_root/packaging/sysusers.d/hardened-blikvm.conf" \
  "$project_root/packaging/sysusers.d/hardened-blikvm-diagnostics.conf" \
  "$project_root/packaging/sysusers.d/hardened-blikvm-quic.conf" \
  "$project_root/packaging/sysusers.d/hardened-blikvm-ve.conf" \
  "$test_root/usr/lib/sysusers.d/"
install -m 0644 \
  "$project_root/packaging/tmpfiles.d/hardened-blikvm-update.conf" \
  "$project_root/packaging/tmpfiles.d/hardened-blikvm-hid.conf" \
  "$project_root/packaging/tmpfiles.d/hardened-blikvm-media.conf" \
  "$project_root/packaging/tmpfiles.d/hardened-blikvm-gadget.conf" \
  "$project_root/packaging/tmpfiles.d/hardened-blikvm-system-actions.conf" \
  "$project_root/packaging/tmpfiles.d/hardened-blikvm-diagnostics.conf" \
  "$test_root/usr/lib/tmpfiles.d/"
: > "$test_root/usr/share/hardened-blikvm/web/index.html"
printf '%s\n' \
  '[Service]' \
  'Type=simple' \
  'ExecStart=/bin/true' \
  > "$test_root/usr/lib/systemd/system/kvmd.service"
printf '%s\n' \
  '[Service]' \
  'Type=simple' \
  'ExecStart=/bin/true' \
  > "$test_root/usr/lib/systemd/system/kvmd-nginx.service"
printf '%s\n' 'g kvmd -' > "$test_root/usr/lib/sysusers.d/kvmd-test.conf"
for target in sysinit.target basic.target shutdown.target multi-user.target sockets.target timers.target; do
  printf '%s\n' '[Unit]' "Description=Synthetic $target" \
    > "$test_root/usr/lib/systemd/system/$target"
done

systemd-sysusers --root="$test_root"
systemd-tmpfiles --cat-config --root="$test_root" >/dev/null
install -d -m 0755 \
  "$test_root/var/lib/hardened-blikvm-slots/current/bin" \
  "$test_root/var/lib/hardened-blikvm-slots/current/share/web"
install -m 0755 /bin/true \
  "$test_root/var/lib/hardened-blikvm-slots/current/bin/bli-kvm-server"
# The HID gadget's unit runs the native slot's owner binary.
install -d -m 0755 "$test_root/var/lib/hardened-blikvm-exec/current/libexec"
install -m 0755 /bin/true \
  "$test_root/var/lib/hardened-blikvm-exec/current/libexec/bli-kvm-control-broker"
: > "$test_root/var/lib/hardened-blikvm-slots/current/share/web/index.html"
# shellcheck disable=SC2086 # the unit names are words
systemd-analyze verify --root="$test_root" $units

# Native profile: the rendered replacements override the managed base units
# and must verify on their own, with no legacy recover/confirm dependency.
native_root="$test_root/native"
install -d -m 0755 "$native_root"
# Copy every entry but the native root itself; copying the whole tree into
# its own subdirectory aborts when readdir happens to return it first.
find "$test_root" -mindepth 1 -maxdepth 1 ! -name native \
  -exec cp -a {} "$native_root/" \; 2>/dev/null
install -m 0644 "$project_root"/packaging/systemd/native/*.service \
  "$native_root/usr/lib/systemd/system/"
install -d -m 0755 \
  "$native_root/var/lib/hardened-blikvm-exec/current/bin" \
  "$native_root/var/lib/hardened-blikvm-exec/current/libexec" \
  "$native_root/var/lib/hardened-blikvm-exec/current/share/web"
for binary in bin/bli-kvm-server libexec/bli-kvm-control-broker \
  libexec/bli-kvm-video-proxy libexec/bli-kvm-boardd; do
  install -m 0755 /bin/true "$native_root/var/lib/hardened-blikvm-exec/current/$binary"
done
: > "$native_root/var/lib/hardened-blikvm-exec/current/share/web/index.html"
# Every native replacement, with the base units the native profile starts
# beside them.
native_units=$(cd "$project_root/packaging/systemd/native" && ls -1 -- *.service)
# shellcheck disable=SC2086 # the unit names are words
systemd-analyze verify --root="$native_root" $native_units \
  hardened-blikvm-update-activation.socket \
  hardened-blikvm-update-exec.socket \
  hardened-blikvm-update-exec@.service \
  hardened-blikvm-update-native-boot.service \
  hardened-blikvm-hid-gadget.service
for unit in "$project_root"/packaging/systemd/native/*.service; do
  if grep -Eq 'update-(recover|confirm)|health-reporter|^\[Install\]' "$unit"; then
    echo "native unit keeps legacy update authority or install links: $unit" >&2
    exit 1
  fi
done
for service in $services; do
  if ! systemd-analyze security --offline=yes --no-pager \
    --threshold="$(exposure_limit "${service##*/}")" \
    "$project_root/packaging/systemd/$service" >/dev/null; then
    echo "$service is exposed beyond its limit" >&2
    exit 1
  fi
done
