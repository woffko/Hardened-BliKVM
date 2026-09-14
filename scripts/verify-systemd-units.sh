#!/bin/sh
set -eu

project_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
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
: > "$test_root/var/lib/hardened-blikvm-slots/current/share/web/index.html"
systemd-analyze verify --root="$test_root" \
  hardened-blikvm-platform-broker.service \
  hardened-blikvm-control-broker.service \
  hardened-blikvm-hid-transition.service \
  hardened-blikvm-media-transition.service \
  hardened-blikvm-gadget-owner.service \
  hardened-blikvm-board-owner.service \
  hardened-blikvm-hid-credential.service \
  hardened-blikvm-video-proxy.service \
  hardened-blikvm-server.service \
  hardened-blikvm-nginx.service \
  hardened-blikvm-update-watchdog.service \
  hardened-blikvm-update-recover.service \
  hardened-blikvm-update-status.socket \
  hardened-blikvm-update-status@.service \
  hardened-blikvm-update-upload.socket \
  hardened-blikvm-update-upload@.service \
  hardened-blikvm-update-activation.socket \
  hardened-blikvm-update-activation@.service \
  hardened-blikvm-update-watchdog.timer \
  hardened-blikvm-update-confirm.socket \
  hardened-blikvm-update-confirm@.service \
  hardened-blikvm-update-activate@.service \
  hardened-blikvm-update-apply-candidate.service \
  hardened-blikvm-update-apply-confirmed.service \
  hardened-blikvm-server-current.service \
  hardened-blikvm-server-candidate.service \
  hardened-blikvm-diagnostic-collector.service \
  hardened-blikvm-diagnostic-store-init.service \
  hardened-blikvm-diagnostic-store-inspect.service \
  hardened-blikvm-diagnostic-persistence-gate.service \
  hardened-blikvm-system-action-store-init.service \
  hardened-blikvm-system-action-store-inspect.service
systemd-analyze security --offline=yes --threshold=20 --no-pager \
  "$project_root/packaging/systemd/hardened-blikvm-platform-broker.service" \
  "$project_root/packaging/systemd/hardened-blikvm-control-broker.service" \
  "$project_root/packaging/systemd/hardened-blikvm-hid-transition.service" \
  "$project_root/packaging/systemd/hardened-blikvm-media-transition.service" \
  "$project_root/packaging/systemd/hardened-blikvm-gadget-owner.service" \
  "$project_root/packaging/systemd/hardened-blikvm-board-owner.service" \
  "$project_root/packaging/systemd/hardened-blikvm-hid-credential.service" \
  "$project_root/packaging/systemd/hardened-blikvm-video-proxy.service" \
  "$project_root/packaging/systemd/hardened-blikvm-server.service" \
  "$project_root/packaging/systemd/hardened-blikvm-update-watchdog.service" \
  "$project_root/packaging/systemd/hardened-blikvm-update-recover.service" \
  "$project_root/packaging/systemd/hardened-blikvm-update-status@.service" \
  "$project_root/packaging/systemd/hardened-blikvm-update-upload@.service" \
  "$project_root/packaging/systemd/hardened-blikvm-update-activation@.service" \
  "$project_root/packaging/systemd/hardened-blikvm-update-confirm@.service" \
  "$project_root/packaging/systemd/hardened-blikvm-update-activate@.service" \
  "$project_root/packaging/systemd/hardened-blikvm-update-apply-candidate.service" \
  "$project_root/packaging/systemd/hardened-blikvm-update-apply-confirmed.service" \
  "$project_root/packaging/systemd/hardened-blikvm-server-current.service" \
  "$project_root/packaging/systemd/hardened-blikvm-server-candidate.service" \
  "$project_root/packaging/systemd/hardened-blikvm-diagnostic-collector.service" \
  "$project_root/packaging/systemd/hardened-blikvm-diagnostic-store-init.service" \
  "$project_root/packaging/systemd/hardened-blikvm-diagnostic-store-inspect.service" \
  "$project_root/packaging/systemd/hardened-blikvm-diagnostic-persistence-gate.service" \
  "$project_root/packaging/systemd/hardened-blikvm-system-action-store-init.service" \
  "$project_root/packaging/systemd/hardened-blikvm-system-action-store-inspect.service" \
  >/dev/null
systemd-analyze security --offline=yes --threshold=22 --no-pager \
  "$project_root/packaging/systemd/hardened-blikvm-nginx.service" \
  >/dev/null
