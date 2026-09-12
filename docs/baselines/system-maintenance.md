# System maintenance checkpoint — 2026-09-12

The user requested source synchronization to GitHub and continuation on the
existing SD card, explicitly without a full-device backup/clone or a new card.
That choice does not authorize publishing device secrets. No image clone has
been made. Restoring stock firmware later will not restore private settings
from this repository; a fresh deployment/configuration will be needed.

## Verified device baseline

- Ubuntu 22.04/Jammy userspace, Armbian board-support package 22.08.2,
  `mangopimcore`/sunxi64, running kernel 5.19.4.
- Kernel image and DTB packages 22.08.2 are held; U-Boot package is also 22.08.2.
- The kernel reports a 31,268,536,320-byte SD device. Root partition is
  5,448,400,896 bytes; `/mnt` is a separate 12,752,781,312-byte partition.
  Virtual media uses another partition and must not be treated as free space.
- Last read-only storage audit: root available 595,423,232 bytes; `/mnt`
  available 11,737,841,664 bytes. These are observations, not reservations.
- APT lists are older than 200 days. Cached simulation with zero upgrades is
  not evidence that the installation is current.
- GUI application-write admission is off. Selected/confirmed application is
  sequence 8, floor 8, generation 40. No new sequence was activated.
- The approved upload-only Nginx streaming configuration was applied and
  reload-verified, but the subsequent installed Logout/upload gate still
  returned HTTP 502. It remains unaccepted; see
  [the detailed checkpoint](persistent-write-rehearsal.md).

## Current scope and order

1. Publish the source/history, tests, tools and documentation, excluding
   `evidence/private`, credentials and generated private configuration.
2. Relocate cached `.deb` files to the existing `/mnt` partition with byte-hash
   verification before removing source copies. Remove only the two named
   regenerable APT binary caches; do not clone the system or archive those caches.
3. Measure resulting space and obtain a current package plan before installation.
   Keep kernel/DTB holds and explicitly review bootloader/BSP changes.
4. Treat same-release package updates and a userspace release migration as
   separate operations. Preserve application slots, virtual-media data and
   private runtime settings; do not run blind autoremove or repartitioning.
5. Re-establish the system/runtime baseline before resuming final BLi-KVM gates.

Source history was synchronized to the private canonical GitHub repository at
`dd9e2b8`; the remote commit was verified against local HEAD. A full-history
credential scan produced one manually verified false positive: a parser field
literal named `journal_redacted_b64`, not a credential.

Cache relocation completed with all 179 `.deb` files hash-verified on `/mnt`.
The two regenerable binary caches were removed, freeing 207,810,560 bytes.
Root available space became 803,225,600 bytes; `/mnt` available space became
11,691,372,544 bytes. The dpkg status hash was unchanged.

Package installation has not been executed. `tools/prepare_system_upgrade_plan.py`
refreshes metadata in a fresh directory on `/mnt`, disables update hooks, keeps
the configured repository/signature policy, and calculates a no-removal upgrade
without installing. Existing holds and kernel/bootloader/Armbian packages are
protected in the in-memory plan. Net installed-size growth is not a peak disk
usage estimate; unpacking and maintainer scripts require additional headroom.
Detailed machine receipts and metadata diagnostics remain private.
