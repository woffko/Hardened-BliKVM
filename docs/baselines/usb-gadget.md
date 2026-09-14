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

## Next gates

- capture the HID report descriptors and function parameters read-only from the
  device and pin them with provenance;
- a privileged service and systemd unit that own the configfs root;
- bind/unbind compatibility on the H616 UDC and host re-enumeration;
- switching media/HID transition engines to this gadget executor.
