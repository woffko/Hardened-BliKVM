# Board storage under our own owner (E1)

Date: 2026-09-16. Plan phase E1 of `docs/design/gui-parity-plan.md`. Receipt:
`evidence/private/board-storage-e1-20260916-a.json`, gate run
`evidence/private/storage-acceptance-20260916-a.json`.

## Host side (committed, 43 routes)

The server now serves the reference storage surface on the same session, CSRF
and media-capability gating as the existing virtual-media route:
`GET /api/storage/image`, `GET /api/storage/image/mounted`,
`GET /api/storage/cdrom`, `POST /api/storage/image/mount` (empty file unmounts),
`POST /api/storage/image/delete` and `POST /api/storage/usb/reconnect`.

The control protocol gained a media observe request whose response carries the
image name, attachment, cdrom and writable flags, a mount command that selects
and attaches in one recorded operation (so the reference single-call mount does
not trip the two-second cooldown), and a reconnect command. The catalog gained
`find_by_name` and an atomic `remove_image`; delete refuses the mounted image
and only touches a bare name inside the media directory.

## Device side

- The MSD partition `mmcblk0p4` is mounted by our own fstab entry at
  `/var/lib/hardened-blikvm-gadget/images`; `/var/lib/kvmd/msd` is unmounted and
  left empty. The image keeps its path, so the gadget needs no rewriting.
- The media paths are provisioned for the unprivileged server user: the images
  directory is group-writable, and the catalog directory and file are owned by
  the server user with modes the catalog validator accepts (it rejects group- or
  world-writable paths).
- Both are shipped in `packaging/tmpfiles.d/hardened-blikvm-gadget.conf`, so the
  layout survives reboots and re-installs.
- The device runs the matching binaries (server, control broker and video
  proxy), verified by comparing the installed and running hashes.

## Gate status

Passing on the live surface: image list, mounted image, cdrom flag, a fresh
`mount`, and `eject` (the state then reports the image as unmounted).

Open: remounting or reconnecting after a state change answers
`backend_rejected`. The kernel refuses LUN attribute writes while a backing file
is open (`lun.0/ro` returns EBUSY), and `reconnect` binds a controller that a
preceding eject already unbound. A fresh attach works, and the board is left
attached and streaming. The upload and reboot halves of the E1 gate are not run:
upload needs a reviewed writable-path check, reboot needs explicit operator
confirmation.
