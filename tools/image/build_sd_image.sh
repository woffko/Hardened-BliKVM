#!/bin/sh
# Build a sanitized SD card image and a RAW system payload from the running
# BliKVM test board, in the provisional corrective layout:
#
#   raw   sectors 1..2047   SPL/U-Boot copied from the board (MBR is new)
#   p1    12 GiB ext4       system root (armbian_root)
#   p2    14 GiB ext4       persistent data and update staging, mounted at /mnt
#   p3    remainder ext4    virtual media (MSD), empty
#
# Usage (from the repository root, credentials on stdin via project-memory-exec):
#   tools/image/build_sd_image.sh fetch WORK_DIR
#   tools/image/build_sd_image.sh assemble WORK_DIR OUTPUT_DIR
#
# fetch reads the root filesystem (one file system, excluding secrets and
# bulky caches) and the raw boot area over SSH without modifying the board.
# assemble runs locally under fakeroot, removes machine secrets, installs the
# first-boot provisioning unit and writes the images.
set -eu

ENDPOINT_HOST=${BLI_KVM_DEVICE_HOST:-10.0.77.118}
KNOWN_HOSTS=${BLI_KVM_KNOWN_HOSTS:-evidence/private/ssh-known-hosts}
IMAGE_BYTES=$((28 * 1024 * 1024 * 1024))
ROOT_START=8192
ROOT_SECTORS=$((12 * 1024 * 1024 * 2))
DATA_SECTORS=$((14 * 1024 * 1024 * 2))

action=${1:-}
work=${2:-}
case "$work" in /*) ;; *) echo "work directory must be absolute" >&2; exit 2 ;; esac

ssh_device() {
  # The SSH password arrives on fd 0 from project-memory-exec; keep it on fd 3.
  sshpass -d 3 ssh -o StrictHostKeyChecking=yes -o UserKnownHostsFile="$KNOWN_HOSTS" \
    "root@$ENDPOINT_HOST" "$@"
}

fetch() {
  mkdir -p "$work"
  exec 3<&0
  # Secrets and machine state never leave the board.
  # One SSH session (the password can be read only once): exactly 2047 raw
  # sectors after the MBR, then the root filesystem archive.
  ssh_device 'dd if=/dev/mmcblk0 bs=512 skip=1 count=2047 status=none &&
    tar --one-file-system --numeric-owner --xattrs --acls -C / -cpf - \
      --exclude=./root --exclude=./home/*/* --exclude=./var/cache/apt/archives \
      --exclude=./var/log/journal --exclude=./var/backups/hardened-blikvm-native \
      --exclude="./var/lib/hardened-blikvm-write-*" --exclude="./var/lib/hardened-blikvm-state-quarantine-*" \
      --exclude="./var/lib/hardened-blikvm-upload-stream-*" --exclude=./var/lib/hardened-blikvm-gadget-gate \
      --exclude=./var/lib/hardened-blikvm-owner-gate --exclude="./etc/ssh/ssh_host_*" \
      --exclude=./etc/NetworkManager/system-connections --exclude=./etc/kvmd/htpasswd \
      --exclude=./etc/kvmd/htpasswd.save --exclude=./etc/kvmd/totp.secret \
      --exclude=./etc/kvmd/ipmipasswd --exclude=./etc/kvmd/vncpasswd --exclude=./etc/kvmd/nginx/ssl \
      --exclude=./etc/kvmd/vnc/ssl --exclude=./etc/hardened-blikvm/kvmd-password \
      --exclude=./etc/hardened-blikvm/tls.key --exclude=./etc/hardened-blikvm/tls.crt \
      --exclude=./etc/ssl/private/ssl-cert-snakeoil.key --exclude=./etc/hardened-blikvm-gadget/serial \
      --exclude=./var/lib/hardened-blikvm/accounts --exclude=./var/lib/hardened-blikvm/sessions \
      --exclude="./home/*/.bash_history" .; status=$?; [ "$status" -le 1 ] || exit "$status"' \
    | { dd of="$work/boot-area.bin" bs=512 count=2047 iflag=fullblock status=none && cat > "$work/rootfs.tar"; }
  [ "$(stat -c %s "$work/boot-area.bin")" = $((2047 * 512)) ]
  # GNU tar exits 1 when a live file changed while it was read; the archive
  # must still list completely.
  tar -tf "$work/rootfs.tar" > /dev/null
  tar -xOf "$work/rootfs.tar" ./etc/os-release ./etc/armbian-release 2>/dev/null \
    | grep -E "^(VERSION|BOARD|PRETTY_NAME)=" > "$work/source-system.txt" || true
  sha256sum "$work/rootfs.tar" "$work/boot-area.bin" > "$work/fetch.sha256"
  # The raw boot area must carry the sunxi SPL magic at 8 KiB (offset 7680 here).
  dd if="$work/boot-area.bin" bs=1 skip=$((8192 - 512 + 4)) count=8 status=none | grep -q 'eGON.BT0'
  echo "fetched into $work"
}

assemble() {
  output=${3:-}
  case "$output" in /*) ;; *) echo "output directory must be absolute" >&2; exit 2 ;; esac
  [ -s "$work/rootfs.tar" ] && [ -s "$work/boot-area.bin" ] || { echo "run fetch first" >&2; exit 1; }
  sha256sum -c --quiet "$work/fetch.sha256"
  mkdir -p "$output"
  root_uuid=${BLI_KVM_ROOT_UUID:-$(cat /proc/sys/kernel/random/uuid)}
  data_uuid=$(cat /proc/sys/kernel/random/uuid)
  msd_uuid=$(cat /proc/sys/kernel/random/uuid)
  staging="$work/rootfs"
  rm -rf "$staging" "$work/fakeroot.state"
  mkdir -p "$staging"
  script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
  # A fresh random salt per build; the password itself is the documented default.
  root_hash=$(openssl passwd -6 1234)
  fakeroot -s "$work/fakeroot.state" -- sh -eu -c '
    staging=$1 tarball=$2 root_uuid=$3 data_uuid=$4 msd_uuid=$5 script_dir=$6 root_hash=$7
    tar --numeric-owner --xattrs --acls -C "$staging" -xpf "$tarball"
    # Machine identity is regenerated on the first boot.
    : > "$staging/etc/machine-id"
    rm -f "$staging/var/lib/dbus/machine-id"
    find "$staging/var/log" -type f -exec truncate -s 0 {} +
    rm -rf "$staging/var/lib/hardened-blikvm-exec"/* "$staging/var/lib/hardened-blikvm-exec-ledger"/*
    install -d -m 0700 -o 0 -g 0 "$staging/root"
    install -d -m 0755 "$staging/var/log/journal"
    # Default credentials follow the Armbian convention: root/1234, expired, so
    # the first SSH login must set a new password. The blikvm user is locked.
    awk -F: -v OFS=: -v hash="$root_hash" '"'"'$1 == "root" { $2 = hash; $3 = 0 }
      $1 == "blikvm" { $2 = "!" } { print }'"'"' "$staging/etc/shadow" > "$staging/etc/shadow.next"
    cat "$staging/etc/shadow.next" > "$staging/etc/shadow"
    rm -f "$staging/etc/shadow.next"
    # New filesystem identities.
    sed -i -E "s/^rootdev=.*/rootdev=UUID=$root_uuid/" "$staging/boot/armbianEnv.txt"
    awk -v root="$root_uuid" -v data="$data_uuid" -v msd="$msd_uuid" '"'"'
      $2 == "/" { $1 = "UUID=" root }
      $2 == "/mnt" { $1 = "UUID=" data }
      $2 == "/var/lib/hardened-blikvm-gadget/images" { $1 = "UUID=" msd }
      $2 == "var/lib/kvmd/msd" { next }
      { print }'"'"' "$staging/etc/fstab" > "$staging/etc/fstab.next"
    cat "$staging/etc/fstab.next" > "$staging/etc/fstab"
    rm -f "$staging/etc/fstab.next"
    install -m 0755 -o 0 -g 0 "$script_dir/hardened-blikvm-firstboot.sh" "$staging/usr/libexec/hardened-blikvm/hardened-blikvm-firstboot"
    install -m 0644 -o 0 -g 0 "$script_dir/hardened-blikvm-firstboot.service" "$staging/etc/systemd/system/hardened-blikvm-firstboot.service"
    ln -sf ../hardened-blikvm-firstboot.service "$staging/etc/systemd/system/multi-user.target.wants/hardened-blikvm-firstboot.service"
    : > "$staging/etc/hardened-blikvm/.firstboot-pending"
  ' sh "$staging" "$work/rootfs.tar" "$root_uuid" "$data_uuid" "$msd_uuid" "$script_dir" "$root_hash"

  root_img="$work/root.ext4"
  rm -f "$root_img" "$work/data.ext4" "$work/msd.ext4"
  fakeroot -i "$work/fakeroot.state" -- mke2fs -q -t ext4 -L armbian_root -U "$root_uuid" \
    -E root_owner=0:0,lazy_itable_init=0 -d "$staging" "$root_img" $((ROOT_SECTORS / 2))k
  data_seed="$work/data-seed"
  rm -rf "$data_seed"; mkdir -p "$data_seed/hardened-blikvm-logs/journal" "$data_seed/upload" "$data_seed/tmp"
  fakeroot -- mke2fs -q -t ext4 -U "$data_uuid" -E root_owner=0:0 -d "$data_seed" "$work/data.ext4" $((DATA_SECTORS / 2))k
  msd_start=$((ROOT_START + ROOT_SECTORS + DATA_SECTORS))
  msd_sectors=$((IMAGE_BYTES / 512 - msd_start))
  fakeroot -- mke2fs -q -t ext4 -L MSD -U "$msd_uuid" -E root_owner=0:0 "$work/msd.ext4" $((msd_sectors / 2))k
  e2fsck -fn "$root_img" >/dev/null

  image="$output/hardened-blikvm-h616-sd.img"
  rm -f "$image"
  truncate -s "$IMAGE_BYTES" "$image"
  printf 'label: dos\nunit: sectors\n%s\n%s\n%s\n' \
    "start=$ROOT_START, size=$ROOT_SECTORS, type=83" \
    "start=$((ROOT_START + ROOT_SECTORS)), size=$DATA_SECTORS, type=83" \
    "start=$msd_start, size=$msd_sectors, type=83" | sfdisk -q "$image"
  dd if="$work/boot-area.bin" of="$image" bs=512 seek=1 conv=notrunc status=none
  dd if="$root_img" of="$image" bs=1M seek=$((ROOT_START / 2048)) conv=notrunc,sparse status=none
  dd if="$work/data.ext4" of="$image" bs=1M seek=$(((ROOT_START + ROOT_SECTORS) / 2048)) conv=notrunc,sparse status=none
  dd if="$work/msd.ext4" of="$image" bs=1M seek=$((msd_start / 2048)) conv=notrunc,sparse status=none

  # RAW system payload: the root filesystem image for p1 of this layout.
  raw="$output/hardened-blikvm-h616-system-root.ext4"
  cp --sparse=always "$root_img" "$raw"
  zstd -q -T0 -19 -f "$image" -o "$image.zst"
  zstd -q -T0 -19 -f "$raw" -o "$raw.zst"
  (
    cd "$output"
    sha256sum "$(basename "$image")" "$(basename "$image").zst" "$(basename "$raw")" "$(basename "$raw").zst" > SHA256SUMS
  )
  cat > "$output/IMAGE-INFO.txt" <<INFO
Hardened BliKVM H616 SD image (development, not release-accepted)
source system: $(tr '\n' ' ' < "$work/source-system.txt")
layout: MBR; raw boot area sectors 1-2047 from the board; p1 root 12 GiB
        (UUID $root_uuid); p2 data 14 GiB at /mnt; p3 MSD remainder; total 28 GiB
first boot: regenerates SSH host keys, machine id, TLS and snakeoil keys and the
            USB gadget serial; root password is 1234 and must be changed at the
            first SSH login; the blikvm account is locked; the web UI starts at
            First Setup.
raw payload: hardened-blikvm-h616-system-root.ext4 is the p1 filesystem of this
             layout (starts at sector $ROOT_START, $ROOT_SECTORS sectors).
write:       zstd -dc hardened-blikvm-h616-sd.img.zst | sudo dd of=/dev/sdX bs=4M conv=fsync status=progress
INFO
  ls -la "$output"
}

case "$action" in
  fetch) fetch ;;
  assemble) assemble "$@" ;;
  *) echo "usage: $0 fetch WORK_DIR | assemble WORK_DIR OUTPUT_DIR" >&2; exit 2 ;;
esac
