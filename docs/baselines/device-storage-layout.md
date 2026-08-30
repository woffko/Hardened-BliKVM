# H616 live storage and boot baseline

The designated test-only H616 device passed a bounded read-only storage
inventory. The probe changed no mount, filesystem, partition, service, boot
state, or physical output. It read only the MBR signature and boot/type byte of
each of four primary entries: ten raw block bytes total. KVMD and kvmd-nginx
remained active, and the already-`rw` root mode was unchanged. The sanitized
private receipt is mode 0600.

## Live disk geometry

The running root resolves to the only non-removable MMC disk. The disk has
61,071,360 512-byte sectors, or 31,268,536,320 bytes. Logical and physical
sector sizes are both 512 bytes. The MBR signature is `55aa`.

| MBR entry | Start bytes | Size bytes | End bytes | Type/filesystem | Sanitized live role |
| ---: | ---: | ---: | ---: | --- | --- |
| 1 | 4,194,304 | 5,448,400,896 | 5,452,595,200 | `83` / ext4, `armbian_root` | root and persistent logs |
| 2 | 1,048,576 | 3,145,728 | 4,194,304 | `83` / ext4 | unmounted, role unproven |
| 3 | 5,452,595,200 | 12,752,781,312 | 18,205,376,512 | `83` / ext4 | data |
| 4 | 18,205,376,512 | 10,476,322,816 | 28,681,699,328 | `83` / ext4, `MSD` | virtual-media storage |

All four boot flags are zero. Entry 2 physically precedes entry 1; MBR table
order is not physical start order. The final 2,586,836,992 bytes are
unpartitioned.

The unpartitioned tail is 2,861,563,904 bytes smaller than the current root
partition. It cannot hold a second equal-capacity root slot. Partition 2 is
only 3 MiB. Therefore the current layout has no safe unused target for a
same-size A/B system update.

## Correlation with the field image

All four live partition sizes match the previously inspected field-image MBR.
The live fourth partition ends at byte 28,681,699,328, while the supplied raw
capture ends at byte 27,815,575,552. The difference is exactly the previously
reported 866,123,776-byte truncation. The live device confirms the declared
geometry; it does not restore the missing capture bytes or make that archive a
valid recovery image.

## Boot evidence

- the kernel root selector and `armbianEnv.txt` use UUID indirection;
- `/boot/boot.cmd` and `/boot/boot.scr` exist on the running root filesystem;
- no extlinux configuration, kernel command-line slot hint, or separate
  firmware config was observed;
- `fw_printenv` and `fw_setenv` are absent;
- no redundant root, boot-attempt selector, confirmed-slot marker, or recovery
  selector is exposed by this evidence.

The probe did not read the boot script contents, raw SPL/U-Boot area, UUIDs,
serials, or unmounted partition contents. A later separately bounded probe
classified the standard sunxi magic locations, boot-script features, and p2
metadata in `device-boot-chain.md`.

## Consequence

An in-place A/B conversion would have to repartition or repurpose existing data
and cannot be an application-style update. It requires independently bootable
recovery media, a complete backup/restore design, exact SPL/U-Boot evidence,
offline filesystem handling, and power-cut tests. No current partition number
or free range is accepted as an update target.
