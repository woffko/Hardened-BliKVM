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
  "$test_root/usr/lib/systemd/system" \
  "$test_root/usr/lib/sysusers.d" \
  "$test_root/usr/libexec/hardened-blikvm" \
  "$test_root/usr/share/hardened-blikvm/web"
install -m 0755 /bin/true "$test_root/bin/true"
install -m 0755 /bin/true "$test_root/usr/libexec/hardened-blikvm/bli-kvm-platform-broker"
install -m 0755 /bin/true "$test_root/usr/libexec/hardened-blikvm/bli-kvm-control-broker"
install -m 0755 /bin/true "$test_root/usr/libexec/hardened-blikvm/bli-kvm-video-proxy"
install -m 0755 /bin/true "$test_root/usr/libexec/hardened-blikvm/bli-kvm-server"
install -m 0644 "$project_root"/packaging/systemd/*.service "$test_root/usr/lib/systemd/system/"
install -m 0644 \
  "$project_root/packaging/sysusers.d/hardened-blikvm.conf" \
  "$test_root/usr/lib/sysusers.d/"
: > "$test_root/usr/share/hardened-blikvm/web/index.html"
printf '%s\n' \
  '[Service]' \
  'Type=simple' \
  'ExecStart=/bin/true' \
  > "$test_root/usr/lib/systemd/system/kvmd.service"
printf '%s\n' 'g kvmd -' > "$test_root/usr/lib/sysusers.d/kvmd-test.conf"
for target in sysinit.target basic.target shutdown.target multi-user.target; do
  printf '%s\n' '[Unit]' "Description=Synthetic $target" \
    > "$test_root/usr/lib/systemd/system/$target"
done

systemd-sysusers --root="$test_root"
systemd-analyze verify --root="$test_root" \
  hardened-blikvm-platform-broker.service \
  hardened-blikvm-control-broker.service \
  hardened-blikvm-video-proxy.service \
  hardened-blikvm-server.service
systemd-analyze security --offline=yes --threshold=20 --no-pager \
  "$project_root/packaging/systemd/hardened-blikvm-platform-broker.service" \
  "$project_root/packaging/systemd/hardened-blikvm-control-broker.service" \
  "$project_root/packaging/systemd/hardened-blikvm-video-proxy.service" \
  "$project_root/packaging/systemd/hardened-blikvm-server.service" \
  >/dev/null
