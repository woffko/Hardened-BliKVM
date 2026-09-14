# USB gadget state machine

Status: host-only, no device access. Phase 3 groundwork for removing the
vendor `enable-gadget.sh` / `kvmd-msd.sh` scripts.

`crates/bli-kvm-usb-gadget` builds the configfs composite gadget from typed,
validated values:

- descriptors `idVendor`/`idProduct`/`bcdUSB`/`bcdDevice`;
- unique device strings (`manufacturer`, `product`, `serialnumber`,
  `configuration`); the legacy vendor serial `CAFEBABE` is rejected;
- a list of functions:
  - mass storage LUN (`file`, `ro`, `cdrom`, `removable`);
  - HID functions `hid.usb0/1/2` with `protocol`, `subclass`,
    `report_length` and a caller-provided `report_desc`;
- the per-function configuration symlink and, last, the UDC bind.

HID report descriptors are caller-provided on purpose: they are protocol facts
that must be captured read-only from the device, never guessed. The current test
stand has no active gadget (configfs is empty and KVMD is inactive), so the
device capture is an explicit later gate.

The operation order is a pure plan (`bind_steps`/`remove_steps`) so ordering
and idempotency are asserted directly: apply binds the UDC last and is a no-op
when re-applied, remove clears the UDC first and removes the tree last, and a
failed apply rolls back. Tests run against a temporary directory; the real
configfs root is only used by the future, physically gated device slice.

## Read-only state

`observe(root)` returns a mutation-free snapshot of the current gadget:
presence, the bound UDC (or `None` when unbound), the sorted function list, and
the mass-storage LUN state. This is the read path that media eligibility and
ownership-handoff verification will use, and it is covered by tests over a
temporary root.

## Runtime primitives

`set_lun_file`, `bind` and `unbind` provide the incremental operations a media
or HID executor needs after the gadget exists (select an image, attach, detach)
without rebuilding the whole gadget. Inputs are validated the same way as the
full spec, and the operations are covered by tests over a temporary root.

## HID report primitive

`write_report(hidg_dir, function, report)` writes one bounded report to the
function's character device (`hid.usb0/1/2` -> `hidg0/1/2`), rejecting unknown
functions, empty reports and reports over 64 bytes. It performs no semantic
translation; turning lease events into raw reports is a separate, sourced
slice. Covered by tests over a temporary device directory.

## Consumers

The media transition engine has a feature-gated `MediaExecutor::Gadget`
(`gadget-media-profile`) that selects the LUN file, binds the UDC on connect and
unbinds it on disconnect, and reports drive state from `observe`.

## Owner unit

`packaging/systemd/hardened-blikvm-gadget-owner.service` runs the control
broker in the gadget media profile as the privileged configfs owner (root), with
`ProtectKernelTunables=no` and `ReadWritePaths=/sys/kernel/config/usb_gadget`
so configfs stays writable while every other hardening flag stays on. It is
non-installable scaffolding until the physical gate; the sandbox-analysis score
is 1.3. The tmpfiles entry creates `/var/lib/hardened-blikvm-gadget`.

## Next gates

- a feature-gated `HidExecutor::Gadget` writing report batches to the HID
  character devices;
- capture the HID report descriptors and function parameters read-only from the
  device and pin them with provenance;
- a privileged service and systemd unit that own the configfs root;
- bind/unbind compatibility on the H616 UDC and host re-enumeration.
