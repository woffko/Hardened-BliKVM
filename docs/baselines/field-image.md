# Sanitized field-image baseline

This baseline was collected read-only from the user-supplied BliKVM field
image. The archive and raw image remain outside Git. No journal replay, mount,
credential read, source execution, or image mutation was performed.

## Image integrity and layout

- The 7z container passed its built-in CRC extraction check.
- It contains one 27,815,575,552-byte DOS/MBR disk image.
- Partition 1 is a 5,448,400,896-byte ext4 Armbian root filesystem.
- Partition 2 is a 3,145,728-byte ext4 filesystem.
- Partition 3 is a 12,752,781,312-byte partition containing a
  12,750,684,160-byte ext4 filesystem.
- Partition 4 declares a 10,476,322,816-byte ext4 filesystem labelled `MSD`.
- Partition 4 extends 866,123,776 bytes beyond the captured image. The field
  image is therefore truncated relative to its partition table and must not be
  treated as a complete recovery or release artifact.

The root filesystem reports `needs_recovery` even though the superblock state
is `clean`, consistent with an image captured from a running or incompletely
unmounted system. Analysis uses a byte-exact partition copy and read-only
`debugfs`; the journal is intentionally not replayed.

## Platform and service topology

- Userspace: Armbian 24.2.1 / Ubuntu 22.04.4 LTS.
- Image build metadata: `mangopimcore`, `sun50iw9`, arm64, Armbian build
  revision marked dirty and versioned 22.08.2.
- Kernel: 5.19.4-sunxi64.
- KVMD: 3.291, configured through a PiKVM v2 `hdmiusb` / rpi4 compatibility
  identity rather than a native H616 platform profile.
- KVMD runs as the unprivileged `kvmd` service user and exposes an authenticated
  HTTP API on `/run/kvmd/kvmd.sock` with mode 0660.
- uStreamer uses `/run/kvmd/ustreamer.sock` and a shared JPEG sink.
- Nginx is the existing HTTP entrypoint.
- BliKVM-specific main, video, HID, and GPIO initialization services execute
  vendor shell scripts as root. These are migration targets, not a production
  trust boundary.

## Secret and provenance boundary

The filesystem contains live credential databases, backup credential files,
SSH material, shell/editor history, device-local configuration, and an older
vendor source checkout. Their contents were not read or extracted. None may be
copied into source, evidence bundles, build context, delegated prompts, or a
release image.

The private checkout and bundled release archive are used only as existence and
runtime-topology evidence. Code migration uses the separately pinned public
upstreams in `upstream.md`, preserving their licenses and exact revisions.

## Adapter consequence

The network-facing Rust server must not hold a KVMD password. During the
transition, a dedicated local broker will:

1. read a dedicated KVMD service credential through a systemd credential file;
2. connect only to the KVMD Unix socket;
3. expose a smaller permission-restricted Unix socket to `bli-kvm-server`;
4. accept only typed, allowlisted operations;
5. separate read-only inventory from mutating HID, ATX, and media commands;
6. remove one vendor root-shell path at a time after equivalent hardware tests.

This bridge is temporary. Direct Rust hardware adapters replace it only after
their physical and recovery gates pass.
