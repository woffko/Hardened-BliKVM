# H616 system-layout evidence

This baseline separates observed facts from a future recovery/A/B design. It
does not authorize a device write.

## Verified field-image facts

- The captured disk uses DOS/MBR and declares four partitions.
- Partition 1 contains the Armbian root filesystem.
- Partition 2 is a small ext4 filesystem whose production role is not yet
  established.
- Partition 3 is a large ext4 filesystem.
- Partition 4 is labelled `MSD`.
- The raw capture ends 866,123,776 bytes before partition 4 ends, so neither
  its tail nor the complete disk layout is available for recovery validation.
- The root filesystem requires unreplayed journal recovery and the image is
  older than the live test-device userspace.

## Pinned upstream conflicts

At `blikvm/blikvm@3e34c04a1dfa84636050585d12d5e585098258ea`, the first-boot
script unmounts `/dev/mmcblk0p3`, resizes partition 3 to 100 percent of the
device, runs `e2fsck` and `resize2fs`, and remounts it. That behavior is
incompatible with a later fourth partition.

At `woffko/Bli-PiKVM@6966a969f46f80172c5a1d1e0aa48be812625ac2`, the documented
PiKVM procedure asks the operator to create `/dev/mmcblk0p3` as the MSD
filesystem. This conflicts with both the vendor resize assumption and the
four-partition field image.

Neither pinned tree defines a signed full-system artifact, redundant root
slots, atomic boot selection, boot-attempt counter, confirmed boot marker, or
automatic system rollback. The vendor updater replaces application files and
may request a reboot; it is not a system A/B protocol.

## Consequence

No existing partition number, raw byte offset, partition-table entry, or
bootloader location is accepted as a Hardened-BliKVM write target. The
truncated field image cannot be repaired by guessing its missing tail.

The first system-image format therefore contains only signed filesystem-slot
payloads. It cannot contain or select a disk, device node, partition table,
bootloader, state partition, or MSD partition. A separately reviewed recovery
layout profile must later bind a layout ID to exact inactive-slot capacities
and boot-control semantics before any writer can exist.

The later bounded live inventory in `device-storage-layout.md` confirms the
same four partition sizes and roles: root/logs, an unmounted 3 MiB filesystem,
data, and MSD. It also proves that the complete device has only 2,586,836,992
unpartitioned bytes, which is smaller than the 5,448,400,896-byte root. The
current disk therefore has no unused same-size A/B root target.

## Missing evidence

- a complete redistribution-cleared recovery image;
- the exact H616 ROM/SPL/U-Boot boot chain and bootloader storage locations;
- a proven redundant-root partition map with preserved state and MSD roles;
- boot-attempt, health-confirmation, rollback-counter, and recovery-card
  behavior across interrupted writes and repeated reboot;
- acceptance on at least two devices and two recovery cards.
