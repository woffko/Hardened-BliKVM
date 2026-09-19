# System update (A/B boot control) design v1

Status: design only, host-side. No device mutation, no code in this document
has run on the board. The reference offers a "system" update beside the
application update; this is the reviewed shape for ours.

## Why it is separate from the application update

The application update replaces the served application (server, board daemon,
web tree, units) inside the retained slots, and the board keeps running while
the candidate is verified. A system update replaces the **operating system
image**, so it must survive a reboot and must be able to fall back on its own if
the new image does not boot. That is a different risk class and needs its own
selector, not a flag on the application path.

## Shape

- Two system slots on the storage device, `system-a` and `system-b`, plus a
  `system-current` selector, mirroring the application slots that already exist
  in `bli-kvm-system-action-store` and the retained update path.
- A **storage adapter** owns the slots: it can list them, verify an image
  against a signed manifest, write an image into the inactive slot, and switch
  the selector. It is the only component that touches the storage device, which
  keeps the "one owner per line" rule.
- A **selector** owns the boot decision: the bootloader reads it, the adapter
  writes it. The selector has a *confirmed* and a *candidate* state; the first
  boot after a switch is a candidate boot, and it only becomes confirmed once
  the board reports health (units active, HTTPS answering, health endpoint at
  `200`) and the operator confirms.
- A **watchdog** reverts the selector to the previous slot when the candidate
  does not confirm inside a bounded window, so a broken image cannot brick the
  board into an unreachable state. This is the part the device gate must prove,
  and it is why the gate needs a reboot.
- Signing reuses the application update key and manifest format: the image
  manifest carries the sequence, the minimum system sequence, the source
  commits and the image digest, and the update is refused when the board's
  minimum sequence is higher.

## Boundaries

- An image write happens only into the inactive slot; the active slot is never
  written, so the running system is never modified in place.
- A switch happens only with an operator confirmation in the session; the
  watchdog revert is automatic and needs no confirmation because it restores
  the last confirmed state.
- The interface page stays hidden until the adapter, the selector and the
  watchdog all have host tests and a device gate, exactly like the application
  update page waited for its mechanism.

## Host tests before any device work

1. Slot listing and selection over a temporary root: an image lands only in the
   inactive slot, and the selector switches only to a verified image.
2. Sequence rules: an image below the minimum system sequence is refused, and a
   candidate that never confirms is reverted by the watchdog within its window.
3. Manifest parsing and signature verification reuse the application update
   verifier, with the same tests.

## Device facts captured read-only (2026-09-19)

- The board has a **single root partition**, `/dev/mmcblk0p1` (5.0 GB), mounted
  at `/`, with **496 MB free (91% used)**. The other visible partitions are the
  boot area and the 6.8 GB MSD partition used for media, so **there is no room
  for a second full system image**.
- There is **no u-boot environment tooling**: neither `fw_printenv`/`fw_setenv`
  nor `/etc/fw_env.config` exists, so a selector cannot be written through the
  bootloader environment today.
- The boot chain is the Armbian one: `/boot/Image`, the device trees,
  `uInitrd`, `boot.cmd`/`boot.scr` and `armbianEnv.txt`, which pins the root by
  UUID (`rootdev=UUID=a990fd36-...`).

## Conclusion

On this board the A/B system update cannot be built honestly: there is no room
for a second image, no reachable selector and no environment tooling, and
repartitioning the only storage device is not a reversible operation. The
interface's system-update entry therefore stays **hidden with this reason**
recorded, and the only supported way to change the operating system image
remains an operator-driven reflash of the card. Building a software-only
selector on top of `armbianEnv.txt`/`boot.scr` was considered and rejected: a
mistake there leaves the board unbootable with no watchdog to fall back on,
which is exactly the failure mode this track exists to avoid.
