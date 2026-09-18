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
   board name and address; `curl --cacert <copy> https://<board-address>/` answers
   200, and the operator tools gained `--ca-file` and `--tls-server-name` so
   they verify instead of disabling TLS.
4. The nginx unit's `ProtectSystem=strict` keeps the compiled-in
   `/var/lib/nginx` temp paths read-only, which broke the config test once the
   ownership changed; every temp buffer now lives in the unit runtime
   directory.

The image upload passes too: uploading a 1 MiB image answers 200, the new image
mounts, ejects and deletes cleanly (file and catalog) and the reviewed
`kubuntu.img` re-attaches (`evidence/private/storage-upload-20260916-b.json`,
`evidence/private/storage-upload-cycle-20260916-a.json`). Two more defects were
fixed on the way:

5. The board-server unit's `ProtectSystem=strict` left the media paths
   read-only, so every upload failed with 503 and the client's remaining body
   tore the connection (nginx logged `writev() failed (32: Broken pipe)`). The
   unit now grants `ReadWritePaths` for the images directory and the catalog
   directory.
6. The media upload route was merged outside the session, CSRF and origin
   middleware, so it accepted unauthenticated requests. It now sits behind the
   same protected layer as the rest of the API, with a test that a rejected
   upload body is never consumed.

The reboot half of the gate still needs explicit operator confirmation: it
verifies that the fstab mount of the MSD partition and the tmpfiles permissions
survive a reboot.
