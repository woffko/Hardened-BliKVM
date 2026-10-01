#!/bin/sh
# Build a sanitized SD card image and a RAW system payload from the running
# BliKVM test board, in the provisional corrective layout:
#
#   raw   sectors 1..2047   SPL/U-Boot copied from the board (MBR is new)
#   p1    12 GiB ext4       system root (armbian_root)
#   p2    8 GiB ext4        persistent data and update staging, mounted at /mnt
#   p3    remainder ext4    virtual media (MSD), empty; the first boot grows it
#                           to the end of the card
#
# Usage (from the repository root, credentials on stdin via project-memory-exec):
#   tools/image/build_sd_image.sh fetch WORK_DIR
#   tools/image/build_sd_image.sh assemble WORK_DIR OUTPUT_DIR
#
# fetch reads the root filesystem (one file system, excluding secrets and
# bulky caches) and the raw boot area over SSH; on the board it only builds
# the image's initramfs in a temporary /var/tmp directory, never in /boot.
# assemble runs locally under fakeroot, removes machine secrets, installs the
# first-boot provisioning unit and writes the images.
set -eu

# The device address comes from the operator environment, never from Git.
ENDPOINT_HOST=${BLI_KVM_DEVICE_HOST:?set BLI_KVM_DEVICE_HOST to the board address}
# The release the image belongs to, shown on the About page.
IMAGE_RELEASE=${BLI_KVM_IMAGE_RELEASE:-development}
case "$IMAGE_RELEASE" in *[!A-Za-z0-9.+-]*|"") echo "BLI_KVM_IMAGE_RELEASE is invalid" >&2; exit 2 ;; esac
KNOWN_HOSTS=${BLI_KVM_KNOWN_HOSTS:-evidence/private/ssh-known-hosts}
IMAGE_BYTES=$((28 * 1024 * 1024 * 1024))
ROOT_START=8192
ROOT_SECTORS=$((12 * 1024 * 1024 * 2))
# Room for a RAW system update: the bundle, its payload and a root backup.
DATA_SECTORS=$((8 * 1024 * 1024 * 2))

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
  # sectors after the MBR, then the root filesystem archive. The image's
  # initramfs is built first into /var/tmp (never /boot), so it carries the
  # RAW system update step even while the board's own initramfs does not.
  ssh_device 'set -e; kernel=$(uname -r); boot_dir=/var/tmp/hardened-image-boot;
    rm -rf "$boot_dir"; mkdir -p "$boot_dir";
    mkinitramfs -o "$boot_dir/initrd.img-$kernel" "$kernel" >&2;
    mkimage -A arm64 -O linux -T ramdisk -C gzip -n uInitrd -d "$boot_dir/initrd.img-$kernel" "$boot_dir/uInitrd-$kernel" >/dev/null;
    set +e;
    dd if=/dev/mmcblk0 bs=512 skip=1 count=2047 status=none &&
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
      --exclude="./home/*/.bash_history" .; status=$?; rm -rf "$boot_dir"; [ "$status" -le 1 ] || exit "$status"' \
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
    staging=$1 tarball=$2 root_uuid=$3 data_uuid=$4 msd_uuid=$5 script_dir=$6 root_hash=$7 image_release=$8
    # ACLs (journal directories) and xattrs are not reproduced; systemd
    # recreates the journal ACLs on the device.
    tar --numeric-owner --no-acls --no-xattrs -C "$staging" -xpf "$tarball"
    # The initramfs built for the image replaces the board'"'"'s own one.
    for built in "$staging"/var/tmp/hardened-image-boot/initrd.img-* "$staging"/var/tmp/hardened-image-boot/uInitrd-*; do
      [ -f "$built" ] || { echo "the image initramfs is missing; fetch again" >&2; exit 1; }
      cat "$built" > "$staging/boot/${built##*/}"
    done
    rm -rf "$staging/var/tmp/hardened-image-boot"
    # Machine identity is regenerated on the first boot.
    : > "$staging/etc/machine-id"
    rm -f "$staging/var/lib/dbus/machine-id"
    find "$staging/var/log" -type f -exec truncate -s 0 {} +
    # The protected execution store and its selection ledger are kept: they
    # hold signed, verified slot copies (no secrets), and on the native
    # profile native boot starts every service from them.
    # Device state that identifies or authenticates this board: web account
    # and backups, Tailscale node keys, NetworkManager secret and leases, the
    # entropy seed and old deployment copies. First Setup recreates the account.
    rm -rf "$staging/var/lib/hardened-blikvm"/* "$staging/var/lib/tailscale"/* \
      "$staging/var/lib/NetworkManager/secret_key" "$staging"/var/lib/NetworkManager/*.lease \
      "$staging/var/lib/NetworkManager/seen-bssids" "$staging/var/lib/NetworkManager/timestamps" \
      "$staging/var/lib/systemd/random-seed" "$staging/var/lib/systemd/credential.secret" \
      "$staging"/var/lib/hardened-blikvm-gadget/deploy-* "$staging/var/lib/hardened-blikvm-gadget/gate"
    # NetworkManager rewrites resolv.conf when it connects; the source
    # board'"'"'s name servers and search domain describe its own network.
    if [ -f "$staging/etc/resolv.conf" ] && [ ! -L "$staging/etc/resolv.conf" ]; then
      printf "%s\n" "# Generated by NetworkManager" > "$staging/etc/resolv.conf"
    fi
    # The source board'"'"'s switches, confirmed firewall mode and journals do
    # not belong to a new board. Each service starts clean without its file:
    # every administration switch off, the firewall in Baseline, HID-only
    # off, empty media, ATX and HID journals, and the video settings at the
    # defaults of the capture unit.
    gadget_state="$staging/var/lib/hardened-blikvm-gadget"
    rm -f "$gadget_state/admin-features.json" "$gadget_state/firewall-mode.json" \
      "$gadget_state/journal.json" "$gadget_state/media-journal.json" \
      "$gadget_state/atx-journal.json" "$gadget_state/hid-ledger.json" \
      "$gadget_state/hid-only.env" "$staging/var/lib/hardened-blikvm-video/ustreamer.env"
    # The QUIC switch is the enablement of the gateway unit, not a file of
    # the board owner: a new card starts with the gateway disabled
    # (docs/design/quic-transport-v1.md).
    rm -f "$staging/etc/systemd/system/multi-user.target.wants/hardened-blikvm-quic.service"
    # Leftovers of earlier deployments on the source board; nothing refers to
    # them.
    rm -rf "$gadget_state"/kvmd-backup-* "$gadget_state"/unit-owner-* \
      "$gadget_state"/update-trust-*.json "$gadget_state"/web-pre-* \
      "$staging"/etc/hardened-blikvm/update-supervisor-*.json
    leftover=$(find "$gadget_state" "$staging/etc/hardened-blikvm" -maxdepth 1 \
      \( -name admin-features.json -o -name firewall-mode.json -o -name journal.json \
      -o -name media-journal.json -o -name atx-journal.json -o -name hid-ledger.json \
      -o -name hid-only.env -o -name "kvmd-backup-*" -o -name "unit-owner-*" \
      -o -name "update-trust-*.json" -o -name "web-pre-*" -o -name "update-supervisor-*.json" \) \
      -not -path "$staging/etc/hardened-blikvm/firewall-mode.json" -print)
    [ ! -e "$staging/var/lib/hardened-blikvm-video/ustreamer.env" ] || leftover="$leftover ustreamer.env"
    if [ -n "$leftover" ]; then
      echo "source board state remains: $leftover" >&2
      exit 1
    fi
    # State of the optional features belongs to the source board: the
    # assistant'"'"'s model key, gateway token and chats, stored scripts and the
    # Tailscale node state. They start empty on a new card.
    rm -rf "$staging/var/lib/private/hardened-blikvm-picoclaw" \
      "$staging/var/lib/hardened-blikvm-picoclaw"/* \
      "$staging/var/lib/hardened-blikvm-picoclaw-runtime"/* \
      "$staging/var/lib/hardened-blikvm-admin/scripts"/* \
      "$staging/var/lib/hardened-blikvm-tailscale"/*
    for state in var/lib/private var/lib/hardened-blikvm-picoclaw var/lib/hardened-blikvm-picoclaw-runtime \
      var/lib/hardened-blikvm-admin/scripts var/lib/hardened-blikvm-tailscale; do
      if [ -n "$(find "$staging/$state" -type f 2>/dev/null | head -n 1)" ]; then
        echo "feature state remains in $state" >&2
        exit 1
      fi
    done
    # A deny list only removes what is known: every directory under var/lib
    # must be one that was reviewed for a new card, so state a future feature
    # adds fails the build until somebody decides what to do with it.
    known_state=" alsa apt aptitude avahi-autoipd bluetooth chrony dbus dhcp dkms dpkg emacsen-common git hardened-blikvm hardened-blikvm-admin hardened-blikvm-exec hardened-blikvm-exec-ledger hardened-blikvm-gadget hardened-blikvm-picoclaw hardened-blikvm-picoclaw-runtime hardened-blikvm-slots hardened-blikvm-slots-v2 hardened-blikvm-system-actions hardened-blikvm-tailscale hardened-blikvm-update hardened-blikvm-update-v2 hardened-blikvm-upload hardened-blikvm-video kvmd logrotate man-db misc NetworkManager nfs nginx openvpn PackageKit pam php polkit-1 private python resolvconf selinux shells.state smartmontools sudo systemd tailscale ucf unattended-upgrades vim vnstat "
    unreviewed=
    for entry in "$staging"/var/lib/*; do
      [ -e "$entry" ] || [ -L "$entry" ] || continue
      case "$known_state" in
        *" ${entry##*/} "*) ;;
        *) unreviewed="$unreviewed ${entry##*/}" ;;
      esac
    done
    if [ -n "$unreviewed" ]; then
      echo "unreviewed state under var/lib:$unreviewed" >&2
      exit 1
    fi
    # The HID ledger lives on tmpfs, created by this entry; without it the
    # board owner cannot store it.
    [ -f "$staging/etc/tmpfiles.d/hardened-blikvm-hid-ledger.conf" ] || {
      echo "the HID ledger tmpfiles entry is missing on the source board" >&2
      exit 1
    }
    # Refuse to build an image that still carries a private key or known
    # per-device state anywhere outside documentation.
    if grep -rlI --exclude-dir=doc --exclude-dir=man "BEGIN [A-Z ]*PRIVATE KEY" \
        "$staging/etc" "$staging/var" "$staging/root" "$staging/home" 2>/dev/null | grep -q .; then
      echo "a private key remains in the root filesystem" >&2
      exit 1
    fi
    # Nor a Wi-Fi passphrase (NetworkManager keyfiles, wpa_supplicant).
    if grep -rlIE --exclude-dir=doc --exclude-dir=man "^[[:space:]]*(psk|wep-key[0-9]|password)=" \
        "$staging/etc/NetworkManager" "$staging/etc/wpa_supplicant" "$staging/var/lib/NetworkManager" 2>/dev/null | grep -q .; then
      echo "a Wi-Fi passphrase remains in the root filesystem" >&2
      exit 1
    fi
    for leaked in var/lib/tailscale/tailscaled.state var/lib/hardened-blikvm/account.json \
      etc/hardened-blikvm/tls.key etc/kvmd/htpasswd var/lib/NetworkManager/secret_key; do
      if [ -e "$staging/$leaked" ]; then
        echo "per-device secret remains: $leaked" >&2
        exit 1
      fi
    done
    # nginx must use the TLS pair the first boot generates; the board'"'"'s
    # own certificate (rendered into its nginx.conf) is excluded above.
    sed -i -e "s#/etc/kvmd/nginx/ssl/server.crt;#/etc/hardened-blikvm/tls.crt;#" \
      -e "s#/etc/kvmd/nginx/ssl/server.key;#/etc/hardened-blikvm/tls.key;#" \
      "$staging/etc/hardened-blikvm/nginx.conf"
    # The rendered nginx unit also starts only when that pair exists.
    for unit in "$staging"/etc/systemd/system/hardened-blikvm-nginx.service; do
      [ -f "$unit" ] || continue
      sed -i -e "s#^ConditionPathExists=/etc/kvmd/nginx/ssl/server.crt\$#ConditionPathExists=/etc/hardened-blikvm/tls.crt#" \
        -e "s#^ConditionPathExists=/etc/kvmd/nginx/ssl/server.key\$#ConditionPathExists=/etc/hardened-blikvm/tls.key#" "$unit"
    done
    grep -q "ssl_certificate /etc/hardened-blikvm/tls.crt;" "$staging/etc/hardened-blikvm/nginx.conf" &&
      grep -q "ssl_certificate_key /etc/hardened-blikvm/tls.key;" "$staging/etc/hardened-blikvm/nginx.conf" || {
      echo "nginx.conf does not use the first-boot TLS pair" >&2
      exit 1
    }
    install -d -m 0700 -o 0 -g 0 "$staging/root"
    install -d -m 0755 "$staging/var/log/journal"
    # The board'"'"'s connection profiles are excluded, but the directory is part
    # of the board owner'"'"'s sandbox (ReadWritePaths) and must exist.
    install -d -m 0700 -o 0 -g 0 "$staging/etc/NetworkManager/system-connections"
    # The image store starts empty on a new card, and so does its catalog.
    printf "%s\n" "{" "  \"schemaVersion\": 1," "  \"entries\": []" "}" \
      > "$staging/etc/hardened-blikvm-gadget/catalog.json"
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
      $2 == "/var/lib/hardened-blikvm-gadget/images" {
        $1 = "UUID=" msd
        # Mounted after tmpfiles, the tmpfiles rule for the image store
        # applies to the partition itself rather than the empty mount point.
        gsub(/,x-systemd\.requires=systemd-tmpfiles-setup\.service/, "", $4)
      }
      $2 == "var/lib/kvmd/msd" { next }
      { print }'"'"' "$staging/etc/fstab" > "$staging/etc/fstab.next"
    cat "$staging/etc/fstab.next" > "$staging/etc/fstab"
    rm -f "$staging/etc/fstab.next"
    install -m 0755 -o 0 -g 0 "$script_dir/hardened-blikvm-firstboot.sh" "$staging/usr/libexec/hardened-blikvm/hardened-blikvm-firstboot"
    install -m 0755 -o 0 -g 0 "$script_dir/hardened-blikvm-grow-image-store.sh" "$staging/usr/libexec/hardened-blikvm/hardened-blikvm-grow-image-store"
    install -m 0644 -o 0 -g 0 "$script_dir/hardened-blikvm-firstboot.service" "$staging/etc/systemd/system/hardened-blikvm-firstboot.service"
    ln -sf ../hardened-blikvm-firstboot.service "$staging/etc/systemd/system/multi-user.target.wants/hardened-blikvm-firstboot.service"
    : > "$staging/etc/hardened-blikvm/.firstboot-pending"
    # The About page shows this as the system image version.
    printf "Hardened BliKVM H616 SD image %s (%s)\n" "$image_release" "$(date -u +%Y-%m-%d)" > "$staging/etc/hardened-blikvm-image"
    chmod 0644 "$staging/etc/hardened-blikvm-image"
  ' sh "$staging" "$work/rootfs.tar" "$root_uuid" "$data_uuid" "$msd_uuid" "$script_dir" "$root_hash" "$IMAGE_RELEASE"

  # The image's initramfs must carry the RAW system update step.
  for initrd in "$staging"/boot/initrd.img-*; do
    zcat "$initrd" | cpio -t 2>/dev/null | grep -Eq '^(usr/)?sbin/bli-kvm-system-update$' || {
      echo "the image initramfs lacks the RAW system update step" >&2
      exit 1
    }
  done

  # Every path the units that start need must exist in the image, or be one
  # the first boot creates: a missing sandbox path stops a unit before it
  # runs (the board owner, and with it the whole application, on a card
  # without /etc/NetworkManager/system-connections), and a missing condition
  # path skips it silently (nginx without its certificate). Checked are the
  # rendered units in /etc/systemd/system, the packaged units that are
  # enabled, and the socket-activated services behind enabled sockets.
  created_at_first_boot="/etc/hardened-blikvm/tls.crt /etc/hardened-blikvm/tls.key /etc/hardened-blikvm-gadget/serial"
  # Default-off features whose files appear only once switched on.
  optional_until_enabled="/mnt/.hardened-system-update /mnt/.hardened-system-update/apply.json /var/lib/hardened-blikvm-picoclaw-runtime/picoclaw"
  enabled=$(find "$staging/etc/systemd/system" -path "*.wants/hardened-blikvm-*" -exec basename {} \; | sort -u)
  units=$(
    find "$staging/etc/systemd/system" -maxdepth 1 -type f -name "hardened-blikvm-*"
    find "$staging/etc/systemd/system" -mindepth 2 -maxdepth 2 -path "*/hardened-blikvm-*.d/*.conf" -type f
    for name in $enabled; do
      [ -f "$staging/lib/systemd/system/$name" ] && echo "$staging/lib/systemd/system/$name"
      case "$name" in
        *.socket)
          service="${name%.socket}"
          for candidate in "$service.service" "$service@.service"; do
            [ -f "$staging/lib/systemd/system/$candidate" ] && echo "$staging/lib/systemd/system/$candidate"
          done
          ;;
      esac
    done
  )
  missing=$(
    for unit in $units; do cat "$unit"; done |
      awk '/^(ReadWritePaths|ReadOnlyPaths|InaccessiblePaths|ExecPaths|NoExecPaths|BindPaths|BindReadOnlyPaths|ConditionPathExists|AssertPathExists|LoadCredential)=/ {
        n = split($0, words, /[= ]+/)
        for (i = 2; i <= n; i++) print words[i]
      }' | sed -e 's#^[^/]*:##' -e 's#:.*$##' | grep '^/' | grep -Ev '^/(run|dev|sys|proc|tmp)/' | grep -v '%' | sort -u |
      while read -r path; do
        case " $created_at_first_boot $optional_until_enabled " in *" $path "*) continue ;; esac
        [ -e "$staging$path" ] || [ -L "$staging$path" ] || echo "$path"
      done
  )
  if [ -n "$missing" ]; then
    echo "units need paths the image lacks:" >&2
    echo "$missing" >&2
    exit 1
  fi

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
  # The server stores uploads on this partition: its root carries the image
  # store's mode and group (tmpfiles: 0770 root:hardened-blikvm).
  store_gid=$(awk -F: '$1 == "hardened-blikvm" { print $3 }' "$staging/etc/group")
  case "$store_gid" in ''|*[!0-9]*) echo "the hardened-blikvm group is missing from the image" >&2; exit 1 ;; esac
  debugfs -w -R "set_inode_field / mode 040770" "$work/msd.ext4" >/dev/null 2>&1
  debugfs -w -R "set_inode_field / gid $store_gid" "$work/msd.ext4" >/dev/null 2>&1
  debugfs -R "stat /" "$work/msd.ext4" 2>/dev/null | grep -q "Mode:  0770" &&
    debugfs -R "stat /" "$work/msd.ext4" 2>/dev/null | grep -Eq "Group: +$store_gid( |\$)" || {
      echo "the image store's owner could not be set" >&2
      exit 1
    }
  e2fsck -fn "$work/msd.ext4" >/dev/null
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
Hardened BliKVM H616 SD image, release $IMAGE_RELEASE
source system: $(tr '\n' ' ' < "$work/source-system.txt")
layout: MBR; raw boot area sectors 1-2047 from the board; p1 root 12 GiB
        (UUID $root_uuid); p2 data 8 GiB at /mnt; p3 MSD remainder (about
        8 GiB), grown to the end of the card at the first boot; total 28 GiB
first boot: regenerates SSH host keys, machine id, TLS and snakeoil keys and the
            USB gadget serial; the web UI starts at First Setup, which sets the
            web password and root's password to the same value (later web
            password changes set root's too); until then root's password is
            1234 and an SSH login must change it; the blikvm account is locked.
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
