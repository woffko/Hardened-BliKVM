# Virtual media image store v1

How images reach the board, how the catalog stays true to the store, and what
the operator is told when an attach fails. Found and fixed in the audit of
2026-09-27 (`docs/baselines/audit-20260927.md`, H1, H2, M2, M3).

## Store

- Images live in `/var/lib/hardened-blikvm-gadget/images`, the MSD partition
  (p3) of the SD image layout, mounted `nosuid,nodev,noexec`.
- The server (`hardened-blikvm-server`, group `hardened-blikvm`) writes
  uploads there, and so does the remote-download helper (same user); the
  board owner (root) attaches them to the USB gadget. The directory is
  `0770 root:hardened-blikvm`.
- The image builder gives the partition's root that mode and group when it
  creates p3 (checked after writing), and mounts it after
  `systemd-tmpfiles-setup.service` rather than before, so the tmpfiles rule for
  the directory also corrects a partition's root on a card that has one.
- An upload that cannot be stored is answered "the image store on the board is
  not writable" (409); the server log names the file and the error.

## Catalog

- `/etc/hardened-blikvm-gadget/catalog.json` lists the images the dialog
  offers, with an opaque ID each; the owner attaches only catalog entries.
- An image arrives in the catalog only through the Download menu (an upload,
  or a URL when the remote download is switched on). A file copied into the
  store by other means is not offered.
- When the catalog is read (the image dialog, the storage list) and before a
  mount, entries whose file is gone are dropped and logged
  (`media: dropped the catalog entry ...`). The image builder writes an empty
  catalog: a new card's store is empty.

## Attach and replace

- The owner spaces media commands; the dialog waits out that pause and sends
  a command again instead of reporting `cooldown_active`.
- A CD-ROM needs an image of at least 300 blocks of 2048 bytes (600 KiB): the
  kernel's mass-storage function refuses a shorter one (`ETOOSMALL`), so the
  gadget refuses it first. The owner answers `image_too_small`, and
  `image_missing` for a catalog entry without its file; the dialog explains
  both in English and Russian. Other failures stay `backend_rejected` with
  the detail in the owner's log.
- An upload or a remote download whose name is the attached image is refused
  (409) until the drive is detached, so the target never keeps reading an old
  file that the catalog no longer describes.

## Verification

`tools/playwright/media_gui_device_test.mjs` uploads a small ISO from the
Download menu, mounts it as a CD-ROM, checks it through the API, unmounts and
deletes it, on desktop and mobile.
