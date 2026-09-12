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

Fresh metadata refresh completed successfully without APT warnings. The reviewed
same-release plan contains 214 upgrades, 143,835,594 download bytes and 758,784
bytes of net installed growth. Root still has 803,225,600 bytes available; data
has 11,386,343,424 bytes after metadata. Netplan and selinux-policy-default remain
kept back; kernel, DTB, bootloader and Armbian packages are excluded from this step.
`tools/download_system_upgrade.py` verifies that APT's command-line simulation
matches the reviewed versions before downloading, then checks the complete
archive set and unchanged dpkg/APT state. It does not install anything.

Download preparation initially hit its 60-second simulation limit before any
archives were downloaded. Read-only inspection proved installed versions were
unchanged. With a 300-second simulation bound and identical prepared inputs,
all 214 archives downloaded and verified successfully (143,835,594 bytes).

The installation runner uses an independent on-device systemd unit and a durable
private result. It rechecks exact versions, hashes, simulation, package state and
600 MiB root headroom before installation. A temporary owned `policy-rc.d` blocks
only stock Nginx and NetworkManager actions; normal libc/systemd handling remains
enabled. Needrestart is list-only, existing conffiles are preferred, and no reboot
is requested. Cleanup runs in the worker's `finally` with ExecStopPost as fallback.
Installation completed successfully: all 214 packages match the reviewed versions,
with no package additions/removals, changed holds, or changed automatic/manual
flags. The dpkg audit is clean and the checked SSH/custom Nginx configurations
are unchanged. The temporary service policy was removed. A fresh authenticated
SSH connection and HTTPS root response (HTTP 200; availability check only, not
TLS identity or authenticated GUI acceptance) succeeded.

SSH, NetworkManager, KVMD, the custom Nginx, application server, broker and video
proxy are active. Stock Nginx and kvmd-nginx remain inactive/disabled. Kernel is
still `5.19.4-sunxi64`; no reboot was performed and no reboot-required marker was
present. This is a Jammy package update, not an Ubuntu release or BSP upgrade.

Verified package versions now include Nginx `1.18.0-6ubuntu14.20`, systemd
`249.11-0ubuntu3.22`, libc6 `2.35-0ubuntu3.15`, OpenSSH server
`1:8.9p1-3ubuntu0.17`, and NetworkManager `1.36.6-0ubuntu2.5`.

APT recreated its two derived binary caches during maintenance; the final scoped
cleanup removed 161,427,456 bytes with no `.deb` files on root and unchanged dpkg
status. Final measured available space: root **760,246,272 bytes**, `/mnt`
**11,241,553,920 bytes**. Downloaded packages and isolated metadata remain on
`/mnt`; future APT use can recreate derived caches. Running services may still
have old libraries mapped until an appropriate restart. The installed
Logout/upload gate remains unaccepted and has not been rerun after this update.
