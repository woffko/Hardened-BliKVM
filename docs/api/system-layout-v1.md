# Recovery system layout profile v1

A layout profile is signed recovery configuration, not a request parameter and
not an installer script. It contains sector geometry and fixed semantic roles,
never a Linux device path, UUID, mountpoint, command, URL, or arbitrary offset
supplied by an application client.

No H616 profile is accepted yet. A structurally valid profile becomes usable
only after its exact canonical hash, recovery image, bootloader build, and
physical failure matrix are separately approved.

## Recovery-only provisioning

The live four-partition MBR cannot be converted in place:

- its 2,586,836,992-byte free tail is smaller than the
  5,448,400,896-byte root;
- data and MSD occupy the remaining large regions;
- current boot scripts expose no A/B attempt/rollback protocol;
- the only detected sunxi SPL/U-Boot magic is in the raw 8 KiB location;
- the 3 MiB p2 filesystem is not an accepted selector or recovery partition.

V1 therefore describes a disk built or restored from independently bootable
recovery media. The running system never repartitions itself.

## Profile shape

Canonical JSON contains:

- schema version 1, product target, and a `layout-` ID;
- partition-table kind (`mbr` or `gpt`), exactly 512-byte sectors, and total
  disk sectors;
- physically sorted raw reserved regions;
- physically sorted partitions with fixed number, role, filesystem, start,
  length, and read-only-default policy;
- boot-control policy with redundant journal/selector copy counts and one to
  three candidate attempts;
- `external_media_required` recovery strategy.

Allowed partition roles are `root_a`, `root_b`, `state`, `boot_a`, `boot_b`,
`data`, and `msd`. Allowed raw roles are `bootloader_primary` and
`bootloader_backup`. No role authorizes a bootloader update; it only prevents
filesystem allocation over a separately accepted raw range.

## Geometry invariants

- partition numbers are unique and within the selected table limit;
- every partition is 1 MiB aligned, non-empty, in bounds, and non-overlapping;
- raw regions are ordered, non-empty, in bounds, and cannot overlap a
  partition or each other;
- one primary bootloader region must begin at sector 16 or 256 and end no later
  than sector 2048; GPT requires sector 256 so the primary table is preserved;
  an MBR profile may describe a non-overlapping backup at the other standard
  location;
- exactly one equal-capacity ext4 `root_a` and `root_b` pair is required, each
  at least 512 MiB and read-only by default;
- exactly one writable ext4 `state` partition of at least 64 MiB is required;
- `boot_a` and `boot_b` are either both absent or equal-capacity read-only vfat
  partitions of at least 16 MiB;
- `data` and `msd` are optional unique writable ext4 partitions;
- MBR profiles contain at most four partitions; GPT profiles reserve first and
  last 1 MiB from filesystem allocation.

The state partition holds distinct redundant journal and selector records; it
does not merge their authority. Both copy counts must be at least two. A
profile cannot claim physical anti-rollback merely by storing a software floor.

## Validation boundary

`bli-kvm-system-layout` parses only canonical bounded JSON, validates the above
invariants, and returns capacities for the system-image policy. It has no block
I/O, mount, partition-table, bootloader, selector, reboot, or recovery command.

The validator intentionally rejects the current live layout as an A/B profile.
It also rejects a synthetically valid profile if any range overlaps, root sizes
differ, boot slots are unpaired, copy counts or attempts are unsafe, or a raw
bootloader range uses an unproven location.
