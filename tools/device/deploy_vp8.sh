#!/bin/sh
# Install or remove the VP8 WebRTC video path on the device. Runs as root
# from a staging directory containing:
#   bli-kvm-video-encoder                  aarch64 binary (feature vp8)
#   janus/*.jcfg                           packaging/janus
#   hardened-blikvm-janus.service          packaging/systemd
#   hardened-blikvm-video-encoder.service  packaging/systemd
#   hardened-blikvm-video-encoder-h264.service  packaging/systemd (x264)
#   60-hardened-blikvm-stream.conf         packaging/sysctl (MJPEG latency)
#   hardened-blikvm-ustreamer.service      packaging/systemd (default rate)
#   licenses/*                             license texts of the static libraries
#                                          linked into the encoder (libvpx,
#                                          x264, libjpeg-turbo; docs/licensing.md)
# Usage: deploy_vp8.sh install|remove STAGING_DIR
#
# The server advertises VP8 only when BLI_KVM_JANUS_WS is set, which this
# script adds as a drop-in for the serving units together with the encoder's
# demand socket (the encoder works only while a viewer is connected); the
# server restarts once.
set -eu

action=${1:-}
staging=${2:-}
case "$action" in install|remove) ;; *) echo "usage: $0 install|remove DIR" >&2; exit 2 ;; esac
[ "$(id -u)" = 0 ] || { echo "must run as root" >&2; exit 2; }
units=/etc/systemd/system
doc=/usr/share/doc/hardened-blikvm-video-encoder
serving="hardened-blikvm-server-current hardened-blikvm-server-candidate"

install_vp8() {
  case "$staging" in /*) ;; *) echo "staging directory must be absolute" >&2; exit 2 ;; esac
  command -v janus >/dev/null || { echo "the distribution janus package is required" >&2; exit 1; }
  install -m 0755 -o root -g root "$staging/bli-kvm-video-encoder" /usr/libexec/hardened-blikvm/bli-kvm-video-encoder
  install -d -m 0755 -o root -g root "$doc"
  for file in "$staging"/licenses/*; do
    if [ -f "$file" ]; then
      install -m 0644 -o root -g root "$file" "$doc/"
    fi
  done
  # Its own world-readable directory: /etc/hardened-blikvm is root-only and
  # the dynamic Janus user could not read its configuration there.
  rm -rf /etc/hardened-blikvm/janus
  install -d -m 0755 -o root -g root /etc/hardened-blikvm-janus
  for file in "$staging"/janus/*.jcfg; do
    install -m 0644 -o root -g root "$file" /etc/hardened-blikvm-janus/
  done
  install -m 0644 "$staging/hardened-blikvm-janus.service" "$units/"
  install -m 0644 "$staging/hardened-blikvm-video-encoder.service" "$units/"
  install -m 0644 "$staging/hardened-blikvm-video-encoder-h264.service" "$units/"
  install -m 0644 "$staging/hardened-blikvm-ustreamer.service" "$units/"
  install -m 0644 -o root -g root "$staging/60-hardened-blikvm-stream.conf" /etc/sysctl.d/
  sysctl -q -p /etc/sysctl.d/60-hardened-blikvm-stream.conf
  for unit in $serving; do
    install -d -m 0755 "$units/$unit.service.d"
    printf '%s\n' '[Service]' 'Environment=BLI_KVM_JANUS_WS=127.0.0.1:8188' \
      'Environment=BLI_KVM_WEBRTC_DEMAND_SOCKET=/run/hardened-blikvm-vp8/demand.sock' \
      'Environment=BLI_KVM_WEBRTC_H264_DEMAND_SOCKET=/run/hardened-blikvm-h264/demand.sock' \
      > "$units/$unit.service.d/60-webrtc.conf"
  done
  systemctl daemon-reload
  systemctl enable hardened-blikvm-janus.service
  # Janus reads its mountpoints only at start.
  systemctl restart hardened-blikvm-janus.service
  systemctl restart hardened-blikvm-ustreamer.service
  systemctl enable hardened-blikvm-video-encoder.service hardened-blikvm-video-encoder-h264.service
  systemctl restart hardened-blikvm-video-encoder.service hardened-blikvm-video-encoder-h264.service
  for unit in $serving; do
    if systemctl is-active --quiet "$unit"; then
      systemctl restart "$unit"
    fi
  done
  sleep 3
  for unit in hardened-blikvm-janus hardened-blikvm-video-encoder hardened-blikvm-video-encoder-h264 $serving; do
    printf '%s %s\n' "$unit" "$(systemctl is-active "$unit" || true)"
  done
}

remove_vp8() {
  systemctl disable --now hardened-blikvm-video-encoder.service hardened-blikvm-video-encoder-h264.service \
    hardened-blikvm-janus.service 2>/dev/null || true
  rm -f "$units/hardened-blikvm-video-encoder.service" "$units/hardened-blikvm-video-encoder-h264.service" \
    "$units/hardened-blikvm-janus.service"
  for unit in $serving; do
    rm -f "$units/$unit.service.d/60-webrtc.conf"
  done
  rm -rf /etc/hardened-blikvm-janus
  # Configuration from an earlier layout, unreadable for the dynamic user.
  rm -rf /etc/hardened-blikvm/janus
  rm -f /usr/libexec/hardened-blikvm/bli-kvm-video-encoder
  rm -rf "$doc"
  systemctl daemon-reload
  for unit in $serving; do
    if systemctl is-active --quiet "$unit"; then
      systemctl restart "$unit"
    fi
  done
}

case "$action" in
  install) install_vp8 ;;
  remove) remove_vp8 ;;
esac
