#!/bin/sh
# Give the image store, the card's last partition, the rest of the card.
# The SD image has a fixed 28 GiB layout, so a larger card would otherwise
# leave its remainder unused. Run by the first boot; a repeat finds nothing
# to do.
#
# Usage: hardened-blikvm-grow-image-store
#        hardened-blikvm-grow-image-store DISK NUMBER   (the partition table
#            only, for an image file in the tests)
set -eu

STORE=/var/lib/hardened-blikvm-gadget/images
# Less room than this after the partition is not worth a table write.
MIN_GROWTH_SECTORS=$((64 * 2048))

if [ "$#" -eq 2 ]; then
  disk=$1 number=$2 partition=
else
  source=$(awk -v store="$STORE" '$1 !~ /^#/ && $2 == store { print $1; exit }' /etc/fstab)
  if [ -z "$source" ]; then
    echo "the image store is not in /etc/fstab" >&2
    exit 0
  fi
  partition=$(findfs "$source")
  name=${partition##*/}
  number=$(cat "/sys/class/block/$name/partition")
  disk=/dev/$(basename "$(readlink -f "/sys/class/block/$name/..")")
fi

# The sector right after the partition.
end=$(sfdisk -d "$disk" | awk -v number="$number" '
  {
    line = $0
    gsub(/[ \t]/, "", line)
    split(line, head, ":")
    if (head[1] !~ ("[^0-9]" number "$")) next
    count = split(head[2], fields, ",")
    for (i = 1; i <= count; i++) {
      if (fields[i] ~ /^start=/) start = substr(fields[i], 7)
      if (fields[i] ~ /^size=/) size = substr(fields[i], 6)
    }
    printf "%.0f\n", start + size
  }')
if [ -z "$end" ]; then
  echo "partition $number of $disk was not found" >&2
  exit 1
fi
# The free area that follows it (sfdisk aligns its start to 1 MiB).
room=$(sfdisk -F "$disk" | awk -v end="$end" '
  $1 ~ /^[0-9]+$/ && $1 >= end && $1 < end + 2048 { printf "%.0f\n", $3 }')
if [ -z "$room" ] || [ "$room" -lt "$MIN_GROWTH_SECTORS" ]; then
  echo "the image store already takes the rest of the card"
  exit 0
fi

echo ",+" | sfdisk --no-reread --no-tell-kernel -q -N "$number" "$disk"
[ -n "$partition" ] || exit 0
partx -u -n "$number" "$disk"
if mountpoint -q "$STORE"; then
  resize2fs "$partition"
else
  # The first boot runs after the store's mount job, so nothing mounts it now.
  # e2fsck exits 1 when it corrected something.
  e2fsck -fp "$partition" || [ "$?" -le 1 ]
  resize2fs "$partition"
fi
echo "the image store now takes the rest of the card"
