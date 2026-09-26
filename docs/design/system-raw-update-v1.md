# RAW system update v1 (plan phase E)

Status: implemented and host-tested; shipped in the update runtime
`0.1.0~dev26`. The boot-time step has not run on hardware: it takes effect
only in an initramfs built after the package is installed, and its device
test needs a spare card written from the new image (operator-gated).

## What the reference does

The reference offers "Allow raw system updates" (off by default): a signed
bundle with `boot.vfat` and `rootfs.sd` is written straight over the boot and
root partitions, synced, and the board reboots. It is meant for lab devices
with SD-card recovery and has no automatic rollback.

## Why ours differs

- The BliKVM card has no separate boot partition: `/boot` (kernel, device
  trees, `uInitrd`, `boot.scr`, `armbianEnv.txt`) lives on the root file
  system, which is mounted while the board runs. Writing it in place is not
  safe, so the write happens before the root is mounted, from the initramfs.
- Nearly all device state lives on the root file system: the web account,
  TLS keys, SSH host keys, machine id, NetworkManager connections, the
  administration switches, the application slots and exec store, the media
  catalog and Tailscale state. A raw root write would reset the device. The
  update therefore carries a fixed list of state paths across the write.
- The card layout of the new image (p1 root 12 GiB, p2 data 14 GiB at
  `/mnt`, p3 virtual media) has room to stage the payload and the backup on
  p2. The hand-built test board's layout (p3 data 5 GiB) does not, so the
  device test runs on a card written from the new image.
- Unlike the reference, the previous root is backed up and restored
  automatically when the new one fails; the operator can still roll back
  until confirming.

## Bundle

`hardened-blikvm-system-<version>-seq<N>.tar`, built by
`tools/build_system_raw_bundle.py` from the SD image builder's root image and
read by `bli-kvm-system-image-verifier`: `manifest.json` (compact JSON in the
verifier's field order), `manifest.sig` (Ed25519 over the manifest bytes
with the system key, separate from the application key), `payload/`, and one
component:

- `payload/rootfs.img.zst` (`ext4_zstd`): the root file system shrunk to its
  content (`resize2fs -M`, a multiple of 4 KiB) and compressed with zstd.
  The manifest records the compressed size and SHA-256 (checked while
  staging) and the uncompressed size and SHA-256 (checked while writing and
  again when read back from the partition).
- The manifest also carries the sequence, the source commit and the layout
  id of the card layout (`layout-bli-h616-sd-v1`). The image must fit the
  device's root partition.

The system trust root (`keyId`, raw `publicKey`, `layoutId`) ships as
`/usr/share/hardened-blikvm/system-update/system-trust.json`; the builder
refuses a signing key that does not match it. The private key stays outside
the repository.

## Flow

1. Switch: `Settings > Check for Updates > Allow raw system updates`
   (`/api/system-update/raw-enabled`), off by default, persisted by the board
   owner like the administration switches (`raw_system_update`). The server
   refuses the upload, install, confirm and rollback while it is off, and
   the root helper checks the switch file again. Switching it off discards
   an update that is staged but not written. The status and discarding work
   while it is off.
2. Upload: `POST /api/system-update/upload` streams the bundle
   (`application/octet-stream`, at most 8 GiB) to the root administration
   helper, which first checks the switch, the staging directory on the data
   partition (a separate file system mounted at `/mnt`) and the free space
   (the bundle, the staged payload and a backup the size of the root's used
   space), then receives it into `/mnt/.hardened-system-update/`.
3. Stage (helper): verify the signature, layout, sequence (above the last
   confirmed one) and the payload digest; copy the payload out of the
   archive and hash it again; archive the carried state into `state.tar`;
   keep the device's `/etc/fstab` and `/etc/shadow`; write the apply record
   (`apply.json`: digests, sizes, the root file-system UUID, generation)
   atomically with fsync. The upload is removed.
4. Install: `POST /api/system-update/install` with the operator's
   confirmation reboots through the owner's system-action profile.
5. Apply (`local-premount`, before the root is mounted): the step reads the
   `/mnt` line of the root's `/etc/fstab` and looks for the apply record on
   that partition, both mounted read-only; only when a record exists is the
   data partition checked (`e2fsck -p`) and mounted read-write. For a staged
   update it checks the payload digest and that the image fits,
   copies the whole root partition to `root.backup.zst` and proves the backup
   by decompressing and hashing it, marks the record `writing`, writes the
   decompressed payload while hashing it, reads the partition back and
   compares, checks (`e2fsck -f -p`), grows (`resize2fs`) and relabels
   (`tune2fs -U` with the old UUID) the file system, then mounts the new root
   to point `boot/armbianEnv.txt` and the root, data and media lines of
   `etc/fstab` at this device, merge the password hashes and extract the
   carried state. It marks the record `written` and reboots so the boot
   loader reads the new kernel and configuration. Any failure before the
   write leaves the old root untouched (`failed`); a failure during or after
   the write, or power lost while `writing`, restores the backup the same
   verified way (`rolled_back`).
6. First boots: every boot of the new root counts an attempt in the
   initramfs; `hardened-blikvm-system-update-booted.service` marks the record
   `booted` once `https://127.0.0.1/api/health` answers. A new root that is
   not `booted` within two boots is replaced by the backup on the third.
7. The operator confirms (`POST /api/system-update/confirm`): the backup,
   payload and carried files are removed and the sequence becomes the new
   floor. Or rolls back (`POST /api/system-update/rollback`, confirmed): the
   record becomes `rollback_requested` and the board reboots into the
   restore.

Writing, reading back and backing up a 12 GiB root on the card takes up to
about 30 minutes; the web page says so and asks to keep power connected.

## Carried state

`/etc/hardened-blikvm`, `/etc/hardened-blikvm-gadget`, the SSH host keys
(DSA, ECDSA, Ed25519 and RSA: the image's first-boot provisioning creates any
that is missing, so a key left out is silently replaced),
`/etc/machine-id`, `/etc/hostname`, `/etc/hosts`,
`/etc/NetworkManager/system-connections`, `/root/.ssh`, the
`/var/lib/hardened-blikvm*` state (accounts, slots, exec store, switches,
system actions, video, administration scripts, PicoClaw runtime and state)
and `/var/lib/tailscale`. The state is restored with owners, modes, ACLs and
extended attributes before the new root first boots, so systemd never sees
an empty machine id and the web server never offers first-run setup.

`/etc/shadow` is merged per account: every account the new image has keeps
the device's hash and aging fields when the device has that account, so the
image's default root password never replaces the operator's; accounts only
the device had are dropped with their `passwd` entries. The image's
first-boot provisioning keeps existing keys, so it is harmless afterwards.

Nothing else is carried: the new image defines the system.

## Components

- `crates/bli-kvm-system-raw-update`: the stage, boot, booted, confirm,
  rollback and cancel steps behind a `Tools` trait, and the
  `bli-kvm-system-update` binary (`boot`, `booted`, `status`).
- `crates/bli-kvm-admin-helper`: the upload, stage, confirm, rollback,
  cancel and status requests.
- `crates/bli-kvm-server/src/system_update.rs`: the API above.
- `packaging/system-update/`: the trust root, the initramfs hook (copies the
  binary, zstd, e2fsprogs and GNU tar; a missing tool leaves the step off)
  and the `local-premount` script (continues the normal boot on any error).
  Nothing in the initramfs is board-specific, so an initramfs built on the
  test board also serves a card with the new layout.
- `web/src/pages/desktop/menu/settings/update/bli-system.tsx`.

## Host tests

- `cargo test -p bli-kvm-system-raw-update`: stage, write, boot attempts,
  confirmation and the sequence floor; the backup restoring a corrupted
  write, a failed file-system step, an interrupted write, a changed payload,
  too many boot attempts and an operator rollback byte for byte; old
  sequences, wrong layouts and cancellation; fstab, `armbianEnv.txt` and
  shadow merging; the real zstd, e2fsprogs and tar on image files (write,
  grow, relabel, restore the backup, carry state with modes); and the
  builder's bundle staged and written by the real verifier, with a tampered
  payload refused.
- `tools/test_build_system_raw_bundle.py`: the archive layout, canonical
  manifest, signature, shrink, and the refusal of a key that does not match
  the trust root.
- `crates/bli-kvm-server`: the upload length rules.

## Device test (operator-gated)

On a spare card written from the new image: switch the feature on, upload
and install a bundle from the same build, check that the board comes back
with the same account, TLS certificate, SSH host keys, root password,
application slot and switches, and confirm; then install a bundle whose
payload is corrupted after signing (refused at upload), and one that never
reports itself booted (for example with the web server masked) and check
that the third boot restores the previous root. The test board keeps its
current initramfs until then.
