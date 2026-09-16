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

Passing on the live surface (receipt
`evidence/private/storage-acceptance-20260916-a.json`): image list, mounted
image, cdrom flag, `mount`, `usb/reconnect`, `eject` (the state then reports the
image as unmounted) and a re-attach.

Four device defects the gate exposed, all fixed and committed:

1. A zero-byte configfs write never reaches the kernel store handler, so the
   gadget unbind and the LUN file release were silent no-ops. They now write a
   single newline, and `unbind` reads the UDC state first because the kernel
   answers an empty write to an empty UDC with ENODEV.
2. A LUN that still holds a backing file refuses `ro`, `cdrom` and `file`
   writes, and a config that still links a function refuses to drop it. The
   image swap now unbinds, unlinks the config, releases the file, drops the
   function and rebuilds it (`replace_mass_storage`).
3. The board certificate was a self-signed leaf with no subject alternative
   name, so no client could verify it. It is regenerated with CN and SAN for the
   board name and address; `curl --cacert <copy> https://10.0.77.118/` answers
   200, and the operator tools gained `--ca-file` and `--tls-server-name` so
   they verify instead of disabling TLS.
4. The nginx unit's `ProtectSystem=strict` keeps the compiled-in
   `/var/lib/nginx` temp paths read-only, which broke the config test once the
   ownership changed; every temp buffer now lives in the unit runtime
   directory.

Open: the media image upload (`POST /api/download/file`) aborts the request
(nginx logs `writev() failed (32: Broken pipe) while sending request to
upstream`; a direct POST answers 503) and writes neither a file nor a catalog
entry. The reboot half of the gate needs explicit operator confirmation.
