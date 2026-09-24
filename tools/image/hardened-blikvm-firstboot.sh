#!/bin/sh
# Generate the per-device secrets that the image deliberately omits, then
# clear the pending marker. Every step is idempotent so an interrupted first
# boot simply repeats it.
set -eu

ssh-keygen -A

if [ ! -s /etc/hardened-blikvm/tls.key ] || [ ! -s /etc/hardened-blikvm/tls.crt ]; then
  umask 077
  openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 3650 \
    -subj "/CN=blikvm" -keyout /etc/hardened-blikvm/tls.key.next \
    -out /etc/hardened-blikvm/tls.crt.next
  chmod 0644 /etc/hardened-blikvm/tls.crt.next
  mv /etc/hardened-blikvm/tls.key.next /etc/hardened-blikvm/tls.key
  mv /etc/hardened-blikvm/tls.crt.next /etc/hardened-blikvm/tls.crt
fi

if [ ! -s /etc/ssl/private/ssl-cert-snakeoil.key ] && command -v make-ssl-cert >/dev/null; then
  make-ssl-cert generate-default-snakeoil --force-overwrite
fi

if [ ! -s /etc/hardened-blikvm-gadget/serial ]; then
  umask 022
  od -An -N8 -tx1 /dev/urandom | tr -d ' \n' | tr 'a-f' 'A-F' > /etc/hardened-blikvm-gadget/serial.next
  echo >> /etc/hardened-blikvm-gadget/serial.next
  mv /etc/hardened-blikvm-gadget/serial.next /etc/hardened-blikvm-gadget/serial
fi

if [ ! -s /etc/machine-id ]; then
  systemd-machine-id-setup
fi

sync
rm -f /etc/hardened-blikvm/.firstboot-pending
