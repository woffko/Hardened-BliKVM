# USB gadget MSD state machine

Status: host-only, no device access. Phase 3 groundwork for removing the
vendor `enable-gadget.sh` / `kvmd-msd.sh` scripts.

`crates/bli-kvm-usb-gadget` builds the configfs mass-storage gadget from typed,
validated values:

- descriptors `idVendor`/`idProduct`/`bcdUSB`/`bcdDevice` (generic and
  driver-safe `1d6b:0104` in tests);
- unique device strings (`manufacturer`, `product`, `serialnumber`,
  `configuration`); the legacy vendor serial `CAFEBABE` is rejected;
- the mass-storage LUN (`file`, `ro`, `cdrom`, `removable`);
- the configuration symlink and, last, the UDC bind.

The operation order is a pure plan (`bind_steps`/`remove_steps`) so ordering
and idempotency are asserted directly: apply binds the UDC last and is a no-op
when re-applied, remove clears the UDC first and removes the tree last, and a
failed apply rolls back. Tests run against a temporary directory; the real
configfs root is only used by the future, physically gated device slice.

## Not in this slice

- HID functions and their report descriptors (a later slice; the descriptors
  must be sourced from the device, not guessed).
- The privileged service and systemd unit that own the configfs root.
- Bind/unbind compatibility on the H616 UDC and host re-enumeration.
