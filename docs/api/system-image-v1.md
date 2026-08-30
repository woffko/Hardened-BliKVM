# Signed system image v1

System images and application bundles are different formats, trust domains,
policies, and recovery transactions. A valid system signature can never be
verified by an `app-` key, and a valid application bundle can never authorize
a system component.

## Scope

`hardened-blikvm-system-v1` carries filesystem images for pre-provisioned
inactive A/B slots. It deliberately cannot describe or contain:

- a disk or device node;
- an MBR/GPT or partition offset;
- SPL, U-Boot, firmware, or another bootloader;
- persistent state, secrets, configuration, logs, or MSD contents;
- a shell command, installer, hook, delta, URL, or arbitrary path.

Recovery-media provisioning and bootloader/layout migration require a separate
future format and trust ceremony.

## Trust and target binding

The manifest is signed directly with Ed25519. System key IDs must begin with
`sys-`; application keys begin with `app-`. There is no unsigned fallback or
cross-domain key conversion.

Verification also requires a root-owned local policy containing:

- the exact accepted `layoutId`;
- the installed recovery implementation sequence;
- the installed boot-control implementation sequence;
- the exact rootfs inactive-slot capacity;
- whether a separate boot slot exists and its exact capacity.

Production policy values must come from a `bli-kvm-system-layout` validated
canonical profile; callers may not synthesize capacities from live free space.

The verifier only returns evidence. It has no device, mount, block, partition,
boot-selection, or reboot authority.

## Canonical manifest

The signed `manifest.json` is canonical compact JSON using this exact shape:

```json
{
  "format": "hardened-blikvm-system-v1",
  "product": "Hardened BliKVM",
  "target": "bli-h616-pikvm",
  "architecture": "aarch64",
  "keyId": "sys-example",
  "version": "1.0.0",
  "sequence": 1,
  "minimumRecoverySequence": 1,
  "minimumBootControlSequence": 1,
  "layoutId": "layout-h616-example-v1",
  "sourceCommit": "0123456789abcdef0123456789abcdef01234567",
  "components": [
    {
      "role": "rootfs",
      "path": "rootfs.img",
      "contentType": "ext4",
      "size": 2147483648,
      "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
    }
  ]
}
```

Unknown fields, non-canonical JSON, invalid identifiers, zero sequences,
unaccepted layout IDs, insufficient local recovery/boot-control sequences, and
component sizes that differ from the local layout policy are rejected.

Exactly one `rootfs`/`ext4` component is required. A `boot`/`vfat` component is
required only when the local layout policy declares a separate boot slot.
Component order and paths are fixed and lexicographically sorted:
`boot.img`, then `rootfs.img`.

## Deterministic archive

The artifact is an uncompressed deterministic tar with exactly these entries:

```text
manifest.json
manifest.sig
payload/
payload/boot.img       only for a layout with a separate boot slot
payload/rootfs.img
```

All entries use uid/gid/mtime zero. Directories use mode 0755 and files mode
0644. Links, devices, FIFOs, extensions that surface as entries, duplicate or
unsorted paths, alternate names, extra directories/files, non-zero trailing
data, and more than 64 KiB of tar zero padding are rejected.

The verifier authenticates the manifest before streaming multi-gigabyte
payloads, then checks exact sizes, SHA-256, and basic filesystem signatures.
The compile-time ceiling is 8 GiB for rootfs, 512 MiB for boot, and 10 GiB for
the complete archive. The layout policy may only reduce those bounds.

The standalone file entrypoint accepts only an absolute regular non-symlink
archive owned by its effective worker UID, with one link and no group/world
write bit. It reads through one `O_NOFOLLOW` descriptor and rejects any inode,
size, owner, mode, mtime, or ctime change observed across verification. A
future stager must still snapshot an uploaded source into a private inode and
write only from that same verified snapshot.

`tools/build_system_image_test_fixture.py` creates rootfs-only and split-boot
fixtures with an ephemeral Ed25519 key. The private key exists only in process
memory; output contains public metadata and signed minimal filesystem-signature
test bytes. These fixtures are deliberately not bootable images or accepted
layout profiles. The receipt reports whether the destination enforced requested
POSIX modes. A public-only fixture written to a mount that reports permissive
modes, such as the current temporary D: workspace, is untrusted between
sessions and must be regenerated or consumed only after its exact hashes are
revalidated from a trusted invocation.

## Required future transaction

Acceptance of this format does not implement installation. A future
recovery-owned writer must still:

1. prove the exact device and inactive layout profile without caller-provided
   paths or offsets;
2. persist an intent journal outside both system slots;
3. stream only to the inactive slot and read back the complete hash;
4. set a bounded one-shot boot attempt without raising the system floor;
5. confirm boot health before raising the floor;
6. automatically select the previous confirmed slot after timeout, boot-count
   exhaustion, verification failure, or interruption;
7. preserve state and MSD partitions and provide independently bootable
   recovery media.

The required journal/selector phases and crash reconciliation are specified in
`system-boot-control-v1.md`.
