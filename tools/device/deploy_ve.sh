#!/bin/sh
# Switch the encoder driver of the video engine on or off on the device.
# Runs as root on a board that has the update-runtime package installed
# (0.1.0~dev36 or later), which carries the driver for the board's kernel,
# its loader and the loader's unit, all inert until this script.
#
# Usage: deploy_ve.sh install|remove|status
#
# install  the group of the driver's device, the rule that gives the device
#          to it, the line that keeps the decoder driver from being loaded,
#          and the loader's unit enabled and started. The H.264 encoder is
#          restarted, so that it tries the device.
# remove   all of that undone: the encoder back on x264, the driver
#          unloaded, the decoder driver back where it was.
# status   what is there, changing nothing.
#
# Nothing of the picture depends on the driver: an encoder that cannot open
# the device uses x264 and says so (docs/design/h616-ve-encoder-driver-v1.md).
set -eu

action=${1:-}
case "$action" in install|remove|status) ;; *) echo "usage: $0 install|remove|status" >&2; exit 2 ;; esac
[ "$(id -u)" = 0 ] || { echo "must run as root" >&2; exit 2; }

examples=/usr/share/doc/hardened-blikvm-update-runtime/examples
unit=hardened-blikvm-ve.service
encoder=hardened-blikvm-video-encoder-h264.service
module=hardened_blikvm_ve_enc
node=/dev/hardened-blikvm-ve-enc
rule=/etc/udev/rules.d/99-hardened-blikvm-ve.rules
modprobe_file=/etc/modprobe.d/hardened-blikvm-ve.conf

engine() {
  for device in /sys/bus/platform/devices/*; do
    if grep -qs 'allwinner,sun50i-h616-video-engine' "$device/of_node/compatible"; then
      echo "$device"
      return 0
    fi
  done
  return 1
}

status() {
  device=$(engine) || device=
  holder=none
  if [ -n "$device" ] && [ -e "$device/driver" ]; then
    holder=$(basename "$(readlink "$device/driver")")
  fi
  echo "kernel: $(uname -r)"
  echo "driver for this kernel: $([ -f "/usr/lib/hardened-blikvm/modules/$(uname -r)/hardened-blikvm-ve-enc.ko" ] && echo present || echo absent)"
  echo "the engine's driver: $holder"
  echo "module loaded: $([ -d "/sys/module/$module" ] && echo yes || echo no)"
  echo "device: $([ -c "$node" ] && stat -c '%U:%G %a' "$node" || echo none)"
  echo "unit: $(systemctl is-enabled "$unit" 2>/dev/null || true), $(systemctl is-active "$unit" 2>/dev/null || true)"
  echo "encoder: $(systemctl is-active "$encoder" 2>/dev/null || true)"
}

restart_encoder() {
  if systemctl is-active --quiet "$encoder"; then
    systemctl restart "$encoder"
  fi
}

install_ve() {
  for file in "$examples/hardened-blikvm-ve-sysusers.conf" "$examples/99-hardened-blikvm-ve.rules" \
    "$examples/hardened-blikvm-ve-modprobe.conf" "/lib/systemd/system/$unit" \
    /usr/libexec/hardened-blikvm/bli-kvm-ve-load; do
    [ -f "$file" ] || { echo "$file is missing: install the update-runtime package first" >&2; exit 1; }
  done
  # The group is made from the package's example, as the QUIC gateway's
  # user is; deploy_vp8.sh made it already where the encoder is installed.
  systemd-sysusers "$examples/hardened-blikvm-ve-sysusers.conf"
  install -m 0644 -o root -g root "$examples/99-hardened-blikvm-ve.rules" "$rule"
  udevadm control --reload
  install -m 0644 -o root -g root "$examples/hardened-blikvm-ve-modprobe.conf" "$modprobe_file"
  systemctl daemon-reload
  systemctl enable "$unit"
  systemctl restart "$unit"
  restart_encoder
  status
}

remove_ve() {
  systemctl disable "$unit" 2>/dev/null || true
  # The encoder holds the device while it runs: it is stopped for the
  # length of the unload and started again without the device.
  was_active=no
  if systemctl is-active --quiet "$encoder"; then
    was_active=yes
    systemctl stop "$encoder"
  fi
  systemctl stop "$unit" 2>/dev/null || true
  if [ -d "/sys/module/$module" ]; then
    rmmod "$module"
  fi
  rm -f "$rule" "$modprobe_file"
  udevadm control --reload
  # The decoder driver takes the engine again, as on a board without this.
  modprobe sunxi_cedrus 2>/dev/null || true
  device=$(engine) || device=
  if [ -n "$device" ] && [ ! -e "$device/driver" ] && [ -d /sys/bus/platform/drivers/cedrus ]; then
    basename "$device" > /sys/bus/platform/drivers/cedrus/bind || true
  fi
  # The group stays: the H.264 encoder's unit names it.
  systemctl daemon-reload
  if [ "$was_active" = yes ]; then
    systemctl start "$encoder"
  fi
  status
}

case "$action" in
  install) install_ve ;;
  remove) remove_ve ;;
  status) status ;;
esac
