# H616 live boot-chain baseline

Two bounded read-only probes classified the live sunxi boot path and the small
unmounted partition. They changed no mount, journal, filesystem, service, boot
state, or physical output. KVMD and kvmd-nginx remained active and rootfs
remained `rw`. Private receipts are mode 0600 and contain no UUID, serial,
environment value, boot-script content, endpoint, or credential.

## Raw boot location

The [official U-Boot sunxi documentation](https://docs.u-boot.org/en/v2025.07/board/allwinner/sunxi.html)
states that Allwinner BootROM checks sector 16 (8 KiB) and, on newer SoCs
including ARM64 parts, sector 256 (128 KiB). Its standard user-area write ranges
end at sector 2048.

The probe read only the eight-byte `eGON.BT0` magic field at each standard
location:

- sector 16 / 8 KiB: present;
- sector 256 / 128 KiB: absent.

The small p2 filesystem starts at sector 2048, exactly after that standard raw
window. This identifies one live SPL/U-Boot-style image at the primary user-area
location, but it does not prove its complete extent, version, contents, or
whether a non-standard backup exists elsewhere.

No `mmcblk0boot0` or `mmcblk0boot1` block device is exposed by the running
kernel. The `mmc` utility exists, but `fw_printenv` and `fw_setenv` do not.

## Root-hosted boot scripts

The root filesystem contains both source and compiled Armbian boot scripts:

```text
/boot/boot.cmd  3,130 bytes  c07fb7907c31fcbbc9ffcadcefb865dc0dabbf879a4df5b488a4865bb17ed204
/boot/boot.scr  3,202 bytes  2d06ecf04bf791980c29f0a8714735b0653ddfebc82123955a9c29f3756b0df4
```

Both expose `part uuid` and `booti` strings. Neither exposes `bootcount`,
`bootlimit`, `altbootcmd`, `upgrade_available`, `saveenv`, or `bootz`. The
probe also found no `ext4load` or `fatload` string; this does not exclude a
generic U-Boot `load` command. `armbianEnv.txt` contains rootdev, rootfstype,
and overlays keys. Their values were not retained.

This evidence shows no existing bounded boot-attempt, A/B selector, confirmed
slot, or automatic rollback protocol.

## Small unmounted partition

Partition 2 was accessed with read-only `debugfs`, without journal replay,
directory listing, or file-content reads. It is a clean 3 MiB ext4 filesystem:

- 768 blocks of 4,096 bytes;
- 734 free blocks;
- 768 inodes, 757 free;
- no ext4 journal.

Of eight fixed boot-related paths, only `/lost+found` exists. `boot.scr`,
`boot.cmd`, `armbianEnv.txt`, `Image`, `uInitrd`, `dtb`, and `extlinux` were
absent. This allowlist result is not a complete content inventory and does not
authorize repartitioning, formatting, or reusing p2.

## Consequence

The current system loads a root-hosted script after a single detected primary
raw SPL/U-Boot image and provides no proven redundant selector storage. A safe
A/B conversion therefore requires recovery-media provisioning of a new layout
and boot-control mechanism; it cannot be enabled by merely writing another
rootfs into the existing free tail or by treating p2 as an assumed selector
partition.
